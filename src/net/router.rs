//! Control/execution routing layer for the Vocal Calculator.
//!
//! The [`Router`] wraps the calculator engine, audio subsystem, and UI window,
//! dispatching actions according to the [`RoutingMatrix`].  The matrix is the
//! sole routing authority -- there is no legacy `ExecutionTarget` config.
//! It also handles inbound remote actions and broadcasts state snapshots to
//! all connected controllers.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::app::identity::derive_node_id;
use crate::app::storage::DeviceTrust;
use crate::core::action::CalcAction;
use crate::core::calculator::{CalcResult, Calculator};
use crate::net::protocol::*;
use crate::traits::{AudioPlayer, DisplayUpdater};

// ---------------------------------------------------------------------------
// Routing types
// ---------------------------------------------------------------------------

const PAIRING_CODE_DOMAIN: &[u8] = b"vocal-calculator-pairing-code-v1";
const PAIRING_CONFIRM_DOMAIN: &[u8] = b"vocal-calculator-pairing-confirm-v1";
const ROUTING_ROW_DOMAIN: &[u8] = b"vocal-calculator-routing-row-v1";

/// Configuration that controls how the router dispatches actions.
///
/// Routing decisions are made solely by the [`RoutingMatrix`]; this struct
/// only holds ancillary flags (remote-control acceptance, conflict policy).
#[derive(Debug, Clone)]
pub struct RoutingConfig {
    pub allow_remote_control: bool,
    pub conflict_policy: ConflictPolicy,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            allow_remote_control: false,
            conflict_policy: ConflictPolicy::Interleaved,
        }
    }
}

// ---------------------------------------------------------------------------
// Routing matrix
// ---------------------------------------------------------------------------

/// Distributed routing matrix that tracks which node controls which executor.
///
/// Each cell `(controller, executor)` is a boolean: `true` means the
/// controller is allowed to send actions to the executor.  Each row is
/// owned by the controller node — only the row owner may modify its cells.
///
/// The diagonal `(A, A) = true` represents self-control (local execution).
pub struct RoutingMatrix {
    entries: HashMap<(NodeId, NodeId), bool>,
    row_versions: HashMap<NodeId, u64>,
    my_id: NodeId,
}

#[derive(Debug, Clone, Copy)]
struct PairedDeviceTrust {
    public_key: [u8; 32],
    trust_state: DeviceTrust,
}

#[derive(Debug, Clone)]
struct SignedRoutingRow {
    owner: NodeId,
    version: u64,
    cells: Vec<(NodeId, NodeId, bool)>,
    owner_public_key: [u8; 32],
    signature: Vec<u8>,
}

impl SignedRoutingRow {
    fn into_message(self) -> NetworkMessage {
        NetworkMessage::RoutingRowAnnounce {
            owner: self.owner,
            version: self.version,
            cells: self.cells,
            owner_public_key: self.owner_public_key,
            signature: self.signature,
        }
    }
}

#[derive(Debug, Clone)]
struct PendingRouteApproval {
    request_id: u64,
    controller: NodeId,
    executor: NodeId,
}

fn canonicalize_cells(cells: &mut [(NodeId, NodeId, bool)]) {
    cells.sort_by(|a, b| {
        a.0.as_bytes()
            .cmp(b.0.as_bytes())
            .then_with(|| a.1.as_bytes().cmp(b.1.as_bytes()))
            .then_with(|| a.2.cmp(&b.2))
    });
}

impl RoutingMatrix {
    /// Create an empty routing matrix for the given local node.
    pub fn new(my_id: NodeId) -> Self {
        Self {
            entries: HashMap::new(),
            row_versions: HashMap::new(),
            my_id,
        }
    }

    /// Register a peer by adding its diagonal (self-control) entry.
    pub fn add_peer(&mut self, node_id: NodeId) {
        self.entries.insert((node_id, node_id), true);
        self.row_versions.entry(node_id).or_insert(0);
    }

    /// Return the current version for a row owner.
    pub fn row_version(&self, owner: NodeId) -> Option<u64> {
        self.row_versions.get(&owner).copied()
    }

    /// Return a sorted snapshot of a single owner's row.
    pub fn row_cells(&self, owner: NodeId) -> Vec<(NodeId, NodeId, bool)> {
        let mut cells: Vec<_> = self
            .entries
            .iter()
            .filter(|((controller, _), _)| *controller == owner)
            .map(|((controller, executor), value)| (*controller, *executor, *value))
            .collect();
        canonicalize_cells(&mut cells);
        cells
    }

    /// Remove all matrix entries involving the given peer (both as
    /// controller and as executor).
    pub fn remove_peer(&mut self, node_id: &NodeId) {
        self.entries
            .retain(|(c, e), _| c != node_id && e != node_id);
        self.row_versions.remove(node_id);
    }

    /// Set a single route.  Only the local node's own row may be modified
    /// through this method; returns `false` if `controller != my_id`.
    pub fn set_route(&mut self, controller: NodeId, executor: NodeId, value: bool) -> bool {
        if controller != self.my_id {
            log::warn!(
                "RoutingMatrix::set_route rejected: controller {} is not self ({})",
                controller,
                self.my_id,
            );
            return false;
        }
        self.entries.insert((controller, executor), value);
        let version = self.row_versions.entry(controller).or_insert(0);
        *version += 1;
        true
    }

    /// Apply an incremental delta from a remote owner.
    ///
    /// Each cell in `cells` is verified to belong to the declared `owner`;
    /// mismatched cells are silently skipped.  Stale deltas (version <= the
    /// current version for that owner) are rejected to prevent out-of-order
    /// gossip delivery from overwriting newer state.
    pub fn apply_delta(&mut self, owner: NodeId, version: u64, cells: &[(NodeId, NodeId, bool)]) {
        let current_version = self.row_versions.get(&owner).copied().unwrap_or(0);
        if version <= current_version {
            log::debug!(
                "RoutingMatrix::apply_delta: ignoring stale delta from owner {} (v{} <= v{})",
                owner,
                version,
                current_version,
            );
            return;
        }
        for &(controller, executor, value) in cells {
            if controller != owner {
                log::warn!(
                    "RoutingMatrix::apply_delta: cell ({}, {}) owner mismatch (expected {})",
                    controller,
                    executor,
                    owner,
                );
                continue;
            }
            self.entries.insert((controller, executor), value);
            self.row_versions.insert(controller, version);
        }
    }

    /// Replace a row after its owner signature has been verified.
    ///
    /// Unknown version-0 rows are accepted so a peer can relay another
    /// owner's initial self-control row. Existing rows only move forward by
    /// version. The local row is never overwritten by an equal-version
    /// relayed row.
    pub fn apply_authorized_row(
        &mut self,
        owner: NodeId,
        version: u64,
        cells: &[(NodeId, NodeId, bool)],
    ) -> bool {
        let current = self.row_versions.get(&owner).copied();
        let is_newer = match current {
            Some(current) if owner == self.my_id => version > current,
            Some(current) => version > current,
            None => true,
        };
        if !is_newer {
            log::debug!(
                "RoutingMatrix::apply_authorized_row: ignoring stale row from owner {} (v{} <= {:?})",
                owner,
                version,
                current,
            );
            return false;
        }

        self.entries
            .retain(|(controller, _), _| *controller != owner);
        for &(controller, executor, value) in cells {
            if controller != owner {
                log::warn!(
                    "RoutingMatrix::apply_authorized_row: cell ({}, {}) owner mismatch (expected {})",
                    controller,
                    executor,
                    owner,
                );
                continue;
            }
            self.entries.insert((controller, executor), value);
        }
        self.row_versions.insert(owner, version);
        self.entries.entry((self.my_id, self.my_id)).or_insert(true);
        self.row_versions.entry(self.my_id).or_insert(0);
        true
    }

    /// Apply a routing snapshot from a remote peer, merging into the local
    /// state rather than replacing it wholesale.
    ///
    /// Previous behaviour cleared *all* entries and re-inserted only the
    /// sync payload plus the local row.  This destroyed entries belonging to
    /// other connected peers that the syncing peer did not know about (e.g.
    /// in a 3-node topology where C syncs to A but C has never heard of B --
    /// A's `(B,B)` diagonal and any `(A,B)` routes would be silently lost).
    ///
    /// New behaviour: only rows whose controller appears in the sync are
    /// replaced (cleared then re-populated).  Rows from other controllers
    /// are left untouched.  The local node's own row is never cleared by a
    /// sync -- it is managed exclusively through [`set_route`](Self::set_route).
    pub fn apply_sync(&mut self, entries: &[(NodeId, NodeId, bool, u64)]) {
        // Collect which controllers appear in the sync payload.
        let sync_controllers: HashSet<NodeId> = entries.iter().map(|(c, _, _, _)| *c).collect();

        // Clear existing rows for non-local controllers that appear in the
        // sync.  This ensures entries the sender intentionally removed are
        // also removed here, while preserving rows from controllers the
        // sender does not know about.
        for controller in &sync_controllers {
            if *controller != self.my_id {
                self.entries.retain(|(c, _), _| c != controller);
                self.row_versions.remove(controller);
            }
        }

        // Insert every entry from the sync.
        for &(controller, executor, value, version) in entries {
            if controller == self.my_id {
                // For the local node's row, only accept the sync entry if
                // its version is >= our current row version.  A lower
                // version means we have made local changes the sync sender
                // has not seen; accepting it would overwrite user intent.
                let current = self.row_versions.get(&self.my_id).copied().unwrap_or(0);
                if version >= current {
                    self.entries.insert((controller, executor), value);
                    self.row_versions.insert(controller, version);
                }
            } else {
                self.entries.insert((controller, executor), value);
                self.row_versions.insert(controller, version);
            }
        }

        // Guarantee the self-control diagonal is always present.
        self.entries.entry((self.my_id, self.my_id)).or_insert(true);
        self.row_versions.entry(self.my_id).or_insert(0);
    }

    /// Return all executors that this node controls (i.e. every `(my_id, X)`
    /// where the value is `true`).  Includes self if the diagonal is set.
    pub fn my_control_targets(&self) -> Vec<NodeId> {
        self.entries
            .iter()
            .filter(|((c, _), v)| *c == self.my_id && **v)
            .map(|((_, e), _)| *e)
            .collect()
    }

    /// Return `true` if this node controls at least one *non-self* executor
    /// (i.e. the node is "muted" because its input goes to a remote peer).
    pub fn is_muted(&self) -> bool {
        self.entries
            .iter()
            .any(|((c, e), v)| *c == self.my_id && *e != self.my_id && *v)
    }

    /// Return all controllers that control this node (i.e. every `(X, my_id)`
    /// where the value is `true`).
    pub fn my_controllers(&self) -> Vec<NodeId> {
        self.entries
            .iter()
            .filter(|((_, e), v)| *e == self.my_id && **v)
            .map(|((c, _), _)| *c)
            .collect()
    }

    /// Check whether a specific controller is allowed to control this node.
    pub fn is_controlled_by(&self, controller: &NodeId) -> bool {
        self.entries
            .get(&(*controller, self.my_id))
            .copied()
            .unwrap_or(false)
    }

    /// Return a snapshot of the full matrix for UI display.
    pub fn get_matrix(&self) -> HashMap<(NodeId, NodeId), bool> {
        self.entries.clone()
    }

    /// Return every node currently represented in the matrix.
    pub fn node_ids(&self) -> Vec<NodeId> {
        let mut node_ids: Vec<NodeId> = self.entries.keys().flat_map(|(c, e)| [*c, *e]).collect();
        node_ids.sort();
        node_ids.dedup();
        node_ids
    }

    /// Return a row-major boolean snapshot for a caller-provided node order.
    pub fn cells_for_order(&self, node_ids: &[NodeId]) -> Vec<bool> {
        let n = node_ids.len();
        let mut cells = Vec::with_capacity(n * n);
        for controller in node_ids {
            for executor in node_ids {
                cells.push(
                    self.entries
                        .get(&(*controller, *executor))
                        .copied()
                        .unwrap_or(false),
                );
            }
        }
        cells
    }

    /// Return all entries with their row versions, suitable for building a
    /// [`NetworkMessage::RoutingSync`] message.
    pub fn sync_entries(&self) -> Vec<(NodeId, NodeId, bool, u64)> {
        self.entries
            .iter()
            .map(|((c, e), v)| {
                let version = self.row_versions.get(c).copied().unwrap_or(0);
                (*c, *e, *v, version)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Central dispatch layer with control/execution routing.
///
/// Wraps the calculator engine, audio subsystem, and UI window behind a
/// cheaply-clonable `Rc` handle so each callback closure can hold its own
/// copy without lifetime issues.
pub struct Router {
    inner: Rc<RefCell<RouterInner>>,
}

struct RouterInner {
    calculator: Rc<RefCell<Calculator>>,
    audio: Option<Box<dyn AudioPlayer>>,
    display: Box<dyn DisplayUpdater>,
    local_node_id: NodeId,
    config: RoutingConfig,
    /// Distributed routing matrix — the source of truth for who controls whom.
    routing_matrix: RoutingMatrix,
    /// Set of connected remote peer node IDs.
    connected_peers: HashSet<NodeId>,
    /// Channel to the networking runtime for sending messages to specific peers.
    outgoing_tx: Option<mpsc::UnboundedSender<(NodeId, NetworkMessage)>>,
    /// Monotonically increasing sequence counter for outbound envelopes.
    local_seq: u64,
    /// Tokio runtime handle for driving async operations from the sync UI thread.
    runtime_handle: Option<tokio::runtime::Handle>,
    /// Peer we sent a ControlRequest to, waiting for grant.
    pending_control_request: Option<NodeId>,
    /// Request id for the current pending route request.
    pending_route_request_id: Option<u64>,
    /// Inbound route requests waiting for explicit user approval.
    pending_route_approvals: HashMap<NodeId, PendingRouteApproval>,
    /// Verified remote public keys learned from the TCP handshake.
    peer_public_keys: HashMap<NodeId, [u8; 32]>,
    /// Local public key advertised during the TCP handshake.
    local_public_key: Option<[u8; 32]>,
    /// Local signing key used for pairing confirmations and signed row announce.
    local_signing_key: Option<SigningKey>,
    /// Paired-device trust policy keyed by node id and bound to public key.
    paired_devices: HashMap<NodeId, PairedDeviceTrust>,
    /// Inbound pairing requests waiting for the same user approval as a
    /// RouteRequest.
    pending_pairings: HashMap<NodeId, [u8; 32]>,
    /// Latest owner-signed routing rows that can be relayed safely.
    signed_rows: HashMap<NodeId, SignedRoutingRow>,
    /// When true, local audio playback is suppressed in apply_result().
    /// Set by the UI (user toggle) or automatically when controlling a
    /// remote executor (routing mute).
    audio_muted: bool,
    /// Highest `last_seq_applied` from any accepted StateUpdate.
    /// Used to reject duplicate or stale StateUpdates when multiple
    /// remote targets are active (Bug 11).
    last_state_update_seq: u64,
    /// Last connection failure reason. Set by `handle_network_message`
    /// when a `ConnectionFailed` message arrives. Cleared by the poll
    /// timer after displaying the error to the user.
    last_connection_error: Option<String>,
    /// When [`ConflictPolicy::Exclusive`] is active, only this remote
    /// controller may send inbound actions to this node.
    exclusive_controller: Option<NodeId>,
}

impl Clone for Router {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl Router {
    /// Create a new Router in local-only mode.
    ///
    /// The routing matrix is initialised with a self-control diagonal so
    /// all actions execute locally.  Use [`set_route`](Self::set_route) to
    /// add remote executors.
    pub fn new(
        calculator: Rc<RefCell<Calculator>>,
        audio: Option<Box<dyn AudioPlayer>>,
        display: Box<dyn DisplayUpdater>,
    ) -> Self {
        let local_node_id = NodeId::new_v4();
        let mut routing_matrix = RoutingMatrix::new(local_node_id);
        // Self-control diagonal: this node always controls itself by default.
        routing_matrix.add_peer(local_node_id);

        let inner = RouterInner {
            calculator,
            audio,
            display,
            local_node_id,
            config: RoutingConfig::default(),
            routing_matrix,
            connected_peers: HashSet::new(),
            outgoing_tx: None,
            local_seq: 0,
            runtime_handle: None,
            pending_control_request: None,
            pending_route_request_id: None,
            pending_route_approvals: HashMap::new(),
            peer_public_keys: HashMap::new(),
            local_public_key: None,
            local_signing_key: None,
            paired_devices: HashMap::new(),
            pending_pairings: HashMap::new(),
            signed_rows: HashMap::new(),
            audio_muted: false,
            last_state_update_seq: 0,
            last_connection_error: None,
            exclusive_controller: None,
        };
        Self {
            inner: Rc::new(RefCell::new(inner)),
        }
    }

    fn note_controller_route_enabled(inner: &mut RouterInner, controller: NodeId) {
        if inner.config.conflict_policy == ConflictPolicy::Exclusive {
            inner.exclusive_controller = Some(controller);
        }
    }

    fn note_controller_route_disabled(inner: &mut RouterInner, controller: NodeId) {
        if inner.exclusive_controller == Some(controller) {
            inner.exclusive_controller = None;
        }
    }

    fn exclusive_allows_controller(inner: &RouterInner, controller: NodeId) -> bool {
        if inner.config.conflict_policy != ConflictPolicy::Exclusive {
            return true;
        }
        let my_id = inner.local_node_id;
        let remote_controllers: Vec<NodeId> = inner
            .routing_matrix
            .my_controllers()
            .into_iter()
            .filter(|id| *id != my_id)
            .collect();
        if remote_controllers.is_empty() {
            return true;
        }
        let allowed = inner
            .exclusive_controller
            .filter(|id| remote_controllers.contains(id))
            .or_else(|| remote_controllers.first().copied());
        allowed == Some(controller)
    }

    // ---- Configuration ---------------------------------------------------

    /// Attach a tokio runtime handle so the router can perform async operations
    /// from a synchronous UI callback context.
    pub fn set_runtime_handle(&self, handle: tokio::runtime::Handle) {
        self.inner.borrow_mut().runtime_handle = Some(handle);
    }

    /// Set the outgoing message channel that routes messages to the networking runtime.
    pub fn set_outgoing_tx(&self, tx: mpsc::UnboundedSender<(NodeId, NetworkMessage)>) {
        self.inner.borrow_mut().outgoing_tx = Some(tx);
    }

    /// Store this device's network identity for pairing and signed row announce.
    pub fn set_local_identity(&self, public_key: [u8; 32], signing_key: SigningKey) {
        let mut inner = self.inner.borrow_mut();
        inner.local_public_key = Some(public_key);
        inner.local_signing_key = Some(signing_key);
    }

    /// Enable or disable acceptance of remote control actions.
    ///
    /// This only allows paired/trusted peers to enter the route authorization
    /// flow; it does not grant control to arbitrary connected peers.
    pub fn set_allow_remote_control(&self, allow: bool) {
        self.inner.borrow_mut().config.allow_remote_control = allow;
    }

    /// Replace the paired-device trust table used for route authorization.
    pub fn set_paired_devices<I>(&self, devices: I)
    where
        I: IntoIterator<Item = (NodeId, [u8; 32], DeviceTrust)>,
    {
        let mut inner = self.inner.borrow_mut();
        inner.paired_devices = devices
            .into_iter()
            .map(|(node_id, public_key, trust_state)| {
                (
                    node_id,
                    PairedDeviceTrust {
                        public_key,
                        trust_state,
                    },
                )
            })
            .collect();
    }

    /// Insert or update one paired-device trust record in the router cache.
    pub fn upsert_paired_device(
        &self,
        node_id: NodeId,
        public_key: [u8; 32],
        trust_state: DeviceTrust,
    ) {
        self.inner.borrow_mut().paired_devices.insert(
            node_id,
            PairedDeviceTrust {
                public_key,
                trust_state,
            },
        );
    }

    /// Return the trust state currently cached for a peer.
    pub fn peer_trust_state(&self, node_id: &NodeId) -> Option<DeviceTrust> {
        self.inner
            .borrow()
            .paired_devices
            .get(node_id)
            .map(|trust| trust.trust_state)
    }

    /// Return the verified session public key for a connected peer.
    pub fn remote_public_key(&self, node_id: &NodeId) -> Option<[u8; 32]> {
        self.inner.borrow().peer_public_keys.get(node_id).copied()
    }

    /// Return peers with pending pairing requests.
    pub fn pending_pairing_devices(&self) -> HashSet<NodeId> {
        self.inner
            .borrow()
            .pending_pairings
            .keys()
            .copied()
            .collect()
    }

    /// Hash shown by `PairingRequest`; currently binds the two session keys.
    pub fn pairing_code_hash(
        requester_public_key: [u8; 32],
        accepter_public_key: [u8; 32],
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(PAIRING_CODE_DOMAIN);
        hasher.update(requester_public_key);
        hasher.update(accepter_public_key);
        hasher.finalize().into()
    }

    /// Canonical payload signed by the pairing accepter.
    pub fn pairing_confirm_payload(
        signer_public_key: [u8; 32],
        peer_public_key: [u8; 32],
    ) -> Vec<u8> {
        let mut payload = Vec::with_capacity(
            PAIRING_CONFIRM_DOMAIN.len() + signer_public_key.len() + peer_public_key.len(),
        );
        payload.extend_from_slice(PAIRING_CONFIRM_DOMAIN);
        payload.extend_from_slice(&signer_public_key);
        payload.extend_from_slice(&peer_public_key);
        payload
    }

    /// Verify a `PairingConfirm` signature from `signer_public_key`.
    pub fn verify_pairing_confirm_signature(
        signer_public_key: [u8; 32],
        peer_public_key: [u8; 32],
        signature: &[u8],
    ) -> bool {
        let Ok(verifying_key) = VerifyingKey::from_bytes(&signer_public_key) else {
            return false;
        };
        let Ok(signature) = Signature::from_slice(signature) else {
            return false;
        };
        let payload = Self::pairing_confirm_payload(signer_public_key, peer_public_key);
        verifying_key.verify(&payload, &signature).is_ok()
    }

    /// Canonical payload signed by a routing-row owner.
    pub fn routing_row_signature_payload(
        owner: NodeId,
        version: u64,
        cells: &[(NodeId, NodeId, bool)],
    ) -> Vec<u8> {
        let mut sorted_cells = cells.to_vec();
        canonicalize_cells(&mut sorted_cells);
        let mut payload =
            Vec::with_capacity(ROUTING_ROW_DOMAIN.len() + 16 + 8 + sorted_cells.len() * 33);
        payload.extend_from_slice(ROUTING_ROW_DOMAIN);
        payload.extend_from_slice(owner.as_bytes());
        payload.extend_from_slice(&version.to_le_bytes());
        for (controller, executor, value) in sorted_cells {
            payload.extend_from_slice(controller.as_bytes());
            payload.extend_from_slice(executor.as_bytes());
            payload.push(u8::from(value));
        }
        payload
    }

    /// Set the conflict resolution policy for concurrent actions.
    pub fn set_conflict_policy(&self, policy: ConflictPolicy) {
        self.inner.borrow_mut().config.conflict_policy = policy;
    }

    /// Return a clone of the current routing configuration.
    pub fn config(&self) -> RoutingConfig {
        self.inner.borrow().config.clone()
    }

    /// Return this node's unique identifier.
    pub fn local_node_id(&self) -> NodeId {
        self.inner.borrow().local_node_id
    }

    /// Override this node's unique identifier.
    ///
    /// This must be called after the NetworkManager is created to ensure
    /// the Router and NetworkManager share the same NodeId.  Without
    /// this, routing matrix synchronization between devices will fail
    /// because the Router's owner IDs won't match the session sender IDs.
    pub fn set_local_node_id(&self, id: NodeId) {
        let mut inner = self.inner.borrow_mut();
        inner.local_node_id = id;
        inner.routing_matrix = RoutingMatrix::new(id);
        inner.routing_matrix.add_peer(id);
    }

    /// Set a pending control request target and send RouteRequest if a
    /// session already exists. If not, add_remote_session will send it later.
    pub fn set_pending_control_request(&self, node_id: NodeId) {
        let (my_id, request_id, has_session) = {
            let mut inner = self.inner.borrow_mut();
            let request_id = Self::timestamp_ms();
            inner.pending_control_request = Some(node_id);
            inner.pending_route_request_id = Some(request_id);
            (
                inner.local_node_id,
                request_id,
                inner.connected_peers.contains(&node_id),
            )
        };
        if has_session {
            self.send_pairing_request_if_needed(node_id);
            self.send_message_to(
                node_id,
                &NetworkMessage::RouteRequest {
                    request_id,
                    controller: my_id,
                    executor: node_id,
                },
            );
        }
    }

    /// Return the peer we are waiting for a ControlGrant from, if any.
    pub fn pending_control_request(&self) -> Option<NodeId> {
        self.inner.borrow().pending_control_request
    }

    /// Clear the pending control request (e.g. on disconnect or timeout).
    pub fn clear_pending_control_request(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.pending_control_request = None;
        inner.pending_route_request_id = None;
    }

    /// Return whether we are currently waiting for a ControlGrant.
    pub fn is_awaiting_grant(&self) -> bool {
        self.inner.borrow().pending_control_request.is_some()
    }

    /// Send a RouteRevoke to the given peer, notifying them that this node
    /// is revoking any routes involving it.
    ///
    /// This is intentionally a no-op.  Every calling path follows
    /// `send_route_revoke` with `set_route`, which commits the version
    /// bump and broadcasts a `RoutingDelta` to all connected peers
    /// (including the target).  Sending a separate `RouteRevoke` with an
    /// uncommitted `version + 1` was fragile -- if any caller omitted
    /// the paired `set_route` the version would be permanently behind,
    /// and two messages at the same version created unnecessary processing.
    pub fn send_route_revoke(&self, _node_id: NodeId) {
        // No-op: the RoutingDelta from the subsequent set_route() handles
        // notification to all connected peers.
    }

    /// Send a RouteRevoke with explicit from/to direction.
    ///
    /// Unlike [`send_route_revoke`] which always uses the local node as
    /// `from`, this method allows the caller to specify both fields.
    /// The message is sent to the `from` node (the row owner) so it can
    /// revoke its own route via `set_route`.
    pub fn send_route_revoke_directed(&self, from: NodeId, to: NodeId) {
        // Version 0: the receiver's `handle_route_revoke` delegates to
        // `set_route` for self-owned rows (`from == my_id`), which bumps
        // the version independently.  The message version is only used
        // for foreign-row revocations, and we must not fabricate a version
        // for a row we do not own.
        self.send_message_to(
            from,
            &NetworkMessage::RouteRevoke {
                from,
                to,
                version: 0,
            },
        );
    }

    // ---- Remote session management ---------------------------------------

    /// Register a remote node as connected and add its diagonal to the
    /// routing matrix (self-control for the new peer).
    pub fn add_remote_session(&self, node_id: NodeId) {
        let pending_request = {
            let mut inner = self.inner.borrow_mut();
            inner.connected_peers.insert(node_id);
            inner.routing_matrix.add_peer(node_id);
            if inner.pending_control_request == Some(node_id) {
                inner
                    .pending_route_request_id
                    .map(|request_id| (inner.local_node_id, request_id))
            } else {
                None
            }
        };
        if let Some((my_id, request_id)) = pending_request {
            self.send_pairing_request_if_needed(node_id);
            self.send_message_to(
                node_id,
                &NetworkMessage::RouteRequest {
                    request_id,
                    controller: my_id,
                    executor: node_id,
                },
            );
        }
    }

    /// Store the verified public key for a live remote session.
    pub fn set_remote_public_key(&self, node_id: NodeId, public_key: [u8; 32]) {
        self.inner
            .borrow_mut()
            .peer_public_keys
            .insert(node_id, public_key);
    }

    /// Remove a remote node from the connected set and purge all its
    /// routing matrix entries.
    pub fn remove_remote_session(&self, node_id: &NodeId) {
        let mut inner = self.inner.borrow_mut();
        inner.connected_peers.remove(node_id);
        inner.peer_public_keys.remove(node_id);
        inner.pending_pairings.remove(node_id);
        inner.signed_rows.remove(node_id);
        inner.routing_matrix.remove_peer(node_id);
        if inner.pending_control_request.as_ref() == Some(node_id) {
            inner.pending_control_request = None;
            inner.pending_route_request_id = None;
        }
    }

    /// Clean up all routing state when a peer disconnects.
    ///
    /// Revokes any outbound route where this node controls the departing
    /// peer (broadcasts a `RoutingDelta` to remaining peers), then
    /// removes the peer from the matrix and connected set.
    ///
    /// Inbound routes (departing peer controlling us) are removed locally
    /// by [`RoutingMatrix::remove_peer`].  We cannot broadcast a valid
    /// `RoutingDelta` for a row we don't own; other peers will detect the
    /// departure through their own session cleanup and purge the stale
    /// entries.
    pub fn cleanup_peer_disconnect(&self, node_id: &NodeId) {
        let my_id = self.inner.borrow().local_node_id;

        // If we were controlling this peer, revoke the route first
        // (this broadcasts a RoutingDelta to remaining peers).
        let was_controlling = {
            let inner = self.inner.borrow();
            inner
                .routing_matrix
                .entries
                .get(&(my_id, *node_id))
                .copied()
                .unwrap_or(false)
        };

        // Check if the departing peer was controlling us (inbound route).
        let was_controlled = {
            let inner = self.inner.borrow();
            inner
                .routing_matrix
                .entries
                .get(&(*node_id, my_id))
                .copied()
                .unwrap_or(false)
        };

        // Remove the departing peer from connected_peers BEFORE revoking
        // the route, so the RoutingDelta broadcast does not send to a peer
        // that is about to disconnect.
        {
            let mut inner = self.inner.borrow_mut();
            inner.connected_peers.remove(node_id);
            inner.peer_public_keys.remove(node_id);
            inner.pending_pairings.remove(node_id);
            inner.signed_rows.remove(node_id);
        }

        if was_controlling {
            self.set_route(my_id, *node_id, false);
        }

        if was_controlled {
            log::info!(
                "Departing peer {} was controlling us; inbound route removed locally",
                node_id,
            );
        }

        // Remove peer from matrix (cleans up both inbound and outbound
        // route entries locally).
        let mut inner = self.inner.borrow_mut();
        inner.routing_matrix.remove_peer(node_id);
        inner.pending_route_approvals.remove(node_id);
        if inner.pending_control_request.as_ref() == Some(node_id) {
            inner.pending_control_request = None;
            inner.pending_route_request_id = None;
        }
    }

    /// Returns `true` if a session is registered for the given node.
    pub fn has_remote_session(&self, node_id: &NodeId) -> bool {
        self.inner.borrow().connected_peers.contains(node_id)
    }

    /// Replace the entire connected-peer set.
    ///
    /// Called from the poll timer to synchronize the Router's broadcast
    /// target list with the networking runtime's active session set.
    /// Without this, [`broadcast_state`](Self::broadcast_state) would
    /// always see an empty set and never send state snapshots.
    pub fn set_connected_peers(&self, peers: HashSet<NodeId>) {
        self.inner.borrow_mut().connected_peers = peers;
    }

    /// Return controllers whose inbound route requests await user approval.
    pub fn pending_route_approval_controllers(&self) -> HashSet<NodeId> {
        self.inner
            .borrow()
            .pending_route_approvals
            .keys()
            .copied()
            .collect()
    }

    /// Approve or deny a pending inbound route request.
    pub fn respond_to_pending_route_request(&self, controller: NodeId, approve: bool) {
        let request = self
            .inner
            .borrow_mut()
            .pending_route_approvals
            .remove(&controller);
        let Some(request) = request else {
            log::debug!("No pending route approval for {}", controller);
            return;
        };

        if approve {
            self.grant_route_request(request.request_id, request.controller, request.executor);
        } else {
            self.deny_route_request(
                request.request_id,
                request.controller,
                request.executor,
                "user_denied",
            );
        }
    }

    /// Deny and clear all pending inbound route requests.
    pub fn deny_all_pending_route_requests(&self, reason: &str) {
        let requests: Vec<_> = self
            .inner
            .borrow_mut()
            .pending_route_approvals
            .drain()
            .map(|(_, request)| request)
            .collect();
        for request in requests {
            self.deny_route_request(
                request.request_id,
                request.controller,
                request.executor,
                reason,
            );
        }
    }

    // ---- Dispatch (UI entry point) ---------------------------------------

    /// Dispatch a calculator action, routing via the matrix.
    ///
    /// The routing matrix is the sole authority:
    ///   - If I control myself (diagonal entry), execute locally.
    ///   - For every other executor I control, send an `ActionEnvelope`.
    ///   - If the matrix has no entries for this node at all, fall back
    ///     to local execution as a safety net.
    ///
    /// While a `ControlGrant` is pending the action is always executed
    /// locally so the user sees immediate feedback.
    pub fn dispatch(&self, action: CalcAction) {
        let (is_pending, targets, my_id) = {
            let inner = self.inner.borrow();
            (
                inner.pending_control_request.is_some(),
                inner.routing_matrix.my_control_targets(),
                inner.local_node_id,
            )
        };

        // While awaiting a ControlGrant, fall back to local execution.
        if is_pending {
            self.execute_local(action);
            return;
        }

        // --- Matrix-based routing (sole authority) -----------------------
        if targets.is_empty() {
            // No matrix entries for this node -- default to local execution.
            self.execute_local(action);
            return;
        }

        // Separate self (diagonal) from remote targets.
        let remote_targets: Vec<NodeId> =
            targets.iter().copied().filter(|id| *id != my_id).collect();

        if remote_targets.is_empty() {
            // The ONLY target is self -- execute locally.
            self.execute_local(action);
        } else {
            // Send to ALL remote targets; do NOT also execute locally.
            // The remote executor(s) will broadcast authoritative state
            // back, avoiding double-dispatch of the same action.
            // Speculative local echo gives the user instant feedback.
            self.apply_speculative(action);
            let envelope = self.build_envelope(action);
            for target in remote_targets {
                self.send_to_remote(target, envelope.clone());
            }
        }
    }

    // ---- Remote action handling (network entry points) --------------------

    /// Handle an [`ActionEnvelope`] received from a remote controller.
    ///
    /// The networking layer should call this when an action arrives on a
    /// subscribed session.  Authorization is checked against the routing
    /// matrix: the sender must have an active route `(sender, self)`.
    pub fn handle_remote_action(&self, envelope: ActionEnvelope) {
        // -- Gate: remote control must be allowed ---------------------------
        {
            let inner = self.inner.borrow();
            if !inner.config.allow_remote_control {
                log::warn!(
                    "Rejected remote action seq={} from {}: remote control disabled",
                    envelope.seq,
                    envelope.source_id,
                );
                return;
            }
        }

        // -- Gate: sender must be an authorised controller in the matrix ----
        {
            let inner = self.inner.borrow();
            if !inner.routing_matrix.is_controlled_by(&envelope.source_id) {
                log::warn!(
                    "Rejected action from {}: not in routing matrix as controller",
                    envelope.source_id,
                );
                return;
            }
        }

        // -- Conflict policy check ------------------------------------------
        {
            let inner = self.inner.borrow();
            if !Self::exclusive_allows_controller(&inner, envelope.source_id) {
                log::warn!(
                    "Rejected action from {}: exclusive policy grants control to {:?}",
                    envelope.source_id,
                    inner.exclusive_controller,
                );
                return;
            }
        }

        // -- Execute on the local calculator --------------------------------
        let result = {
            let inner = self.inner.borrow();
            inner.calculator.borrow_mut().dispatch(envelope.action)
        };
        self.apply_result(&result);

        // -- Advance sequence counter ---------------------------------------
        {
            let mut inner = self.inner.borrow_mut();
            if envelope.seq > inner.local_seq {
                inner.local_seq = envelope.seq;
            }
        }

        // -- Broadcast state to all controllers that control me ------------
        let snapshot = {
            let inner = self.inner.borrow();
            Self::build_state_snapshot(&result, inner.local_seq)
        };
        self.broadcast_state(&snapshot);
    }

    /// Handle any [`NetworkMessage`] received on a remote session.
    ///
    /// This is the generic entry point for the networking layer; the router
    /// dispatches to the appropriate handler based on message type.
    ///
    /// `sender_id` is the node that sent this message (carried through the
    /// command channel from the session task).
    pub fn handle_network_message(&self, sender_id: NodeId, msg: NetworkMessage) {
        match msg {
            NetworkMessage::Action(envelope) => {
                if sender_id != envelope.source_id {
                    log::warn!(
                        "Rejected Action: sender {} does not match envelope source {}",
                        sender_id,
                        envelope.source_id,
                    );
                    return;
                }
                self.handle_remote_action(envelope);
            }
            NetworkMessage::StateUpdate(snapshot) => {
                {
                    let inner = self.inner.borrow();
                    let my_id = inner.local_node_id;
                    let sender_is_active_remote_target = sender_id != my_id
                        && inner.connected_peers.contains(&sender_id)
                        && inner
                            .routing_matrix
                            .entries
                            .get(&(my_id, sender_id))
                            .copied()
                            .unwrap_or(false)
                        && inner.pending_control_request != Some(sender_id);
                    if !sender_is_active_remote_target {
                        log::warn!(
                            "Rejected StateUpdate from {}: not an active remote execution target",
                            sender_id,
                        );
                        return;
                    }
                }

                // Authoritative state from the executing node -- reset the
                // local calculator so its internal state matches the remote,
                // then push the display values to the UI.
                //
                // Reject stale or duplicate StateUpdates (e.g. when multiple
                // remote targets process the same action and each sends back
                // a StateUpdate at the same seq).
                {
                    let mut inner = self.inner.borrow_mut();
                    if snapshot.last_seq_applied <= inner.last_state_update_seq {
                        log::debug!(
                            "Ignoring stale StateUpdate (seq {} <= {})",
                            snapshot.last_seq_applied,
                            inner.last_state_update_seq,
                        );
                        return;
                    }
                    inner.last_state_update_seq = snapshot.last_seq_applied;
                }
                let calc = {
                    let inner = self.inner.borrow();
                    Rc::clone(&inner.calculator)
                };
                calc.borrow_mut().reset_from_snapshot(
                    &snapshot.display,
                    &snapshot.history,
                    &snapshot.memory_indicator,
                    snapshot.is_error,
                );
                let inner = self.inner.borrow();
                inner.display.update_display(&snapshot.display);
                inner.display.update_history(&snapshot.history);
                inner
                    .display
                    .update_memory_indicator(&snapshot.memory_indicator);
                inner.display.set_error_state(snapshot.is_error);
            }
            NetworkMessage::Ping => {
                // Ping/Pong is now handled by the session task directly.
                log::trace!("Received Ping in Router (should have been handled by session)");
            }
            NetworkMessage::Pong => {
                // Pong is handled by the session task's heartbeat tracker.
                log::trace!("Received Pong in Router (should have been handled by session)");
            }
            NetworkMessage::RouteRevoke { from, to, version } => {
                let my_id = self.inner.borrow().local_node_id;
                let allowed = sender_id == from || (from == my_id && sender_id == to);
                if !allowed {
                    log::warn!(
                        "RouteRevoke rejected: sender={} from={} to={} my_id={}",
                        sender_id,
                        from,
                        to,
                        my_id,
                    );
                    return;
                }
                self.handle_route_revoke(from, to, version);
            }
            NetworkMessage::RouteRequest {
                request_id,
                controller,
                executor,
            } => {
                self.handle_route_request(sender_id, request_id, controller, executor);
            }
            NetworkMessage::RouteGrant {
                request_id,
                controller,
                executor,
            } => {
                self.handle_route_grant(sender_id, request_id, controller, executor);
            }
            NetworkMessage::RouteDenied {
                request_id,
                controller,
                executor,
                reason,
            } => {
                self.handle_route_denied(sender_id, request_id, controller, executor, reason);
            }
            NetworkMessage::RouteRelease {
                controller,
                executor,
            } => {
                self.handle_route_release(sender_id, controller, executor);
            }
            NetworkMessage::RoutingDelta {
                owner,
                version,
                cells,
            } => {
                // Authorization: the sender must be the row owner.
                if sender_id != owner {
                    log::warn!(
                        "RoutingDelta rejected: sender {} is not row owner {}",
                        sender_id,
                        owner,
                    );
                    return;
                }
                let mut inner = self.inner.borrow_mut();
                log::debug!(
                    "RoutingDelta from owner {} (v{}, {} cells)",
                    owner,
                    version,
                    cells.len(),
                );
                let my_id = inner.local_node_id;
                inner.routing_matrix.apply_delta(owner, version, &cells);
                for &(controller, executor, value) in &cells {
                    if executor != my_id {
                        continue;
                    }
                    if value {
                        Self::note_controller_route_enabled(&mut inner, controller);
                    } else {
                        Self::note_controller_route_disabled(&mut inner, controller);
                    }
                }
            }
            NetworkMessage::RoutingRowRequest { owner } => {
                self.handle_routing_row_request(sender_id, owner);
            }
            NetworkMessage::RoutingRowAnnounce {
                owner,
                version,
                cells,
                owner_public_key,
                signature,
            } => {
                self.handle_routing_row_announce(
                    sender_id,
                    owner,
                    version,
                    cells,
                    owner_public_key,
                    signature,
                );
            }
            NetworkMessage::RoutingSync { entries } => {
                let mut inner = self.inner.borrow_mut();
                let filtered_entries: Vec<_> = entries
                    .into_iter()
                    .filter(|(controller, _, _, _)| *controller == sender_id)
                    .collect();
                if filtered_entries.is_empty() {
                    log::warn!(
                        "RoutingSync from {} rejected: no entries owned by sender",
                        sender_id,
                    );
                    return;
                }
                log::debug!(
                    "RoutingSync from {} accepted {} sender-owned entries",
                    sender_id,
                    filtered_entries.len()
                );
                inner.routing_matrix.apply_sync(&filtered_entries);
            }
            NetworkMessage::ConnectionFailed {
                addr,
                reason,
                target_node_id,
            } => {
                // Connection failure from the connect task. Revert the
                // pending route if it still matches the target, and store
                // the error for UI display.
                log::warn!(
                    "Connection failed to {} ({:?}): {}",
                    addr,
                    target_node_id,
                    reason
                );

                // Compute revert details inside the borrow, then broadcast
                // after dropping it (broadcast_routing_delta also borrows inner).
                let revert_info = {
                    let mut inner = self.inner.borrow_mut();
                    let my_id = inner.local_node_id;
                    let mut revert = None;

                    // Only revert if the pending request still targets this peer.
                    if let Some(pending_peer) = inner.pending_control_request {
                        let should_revert = match target_node_id {
                            Some(tid) => tid == pending_peer,
                            None => true, // Unknown target — revert any pending.
                        };
                        if should_revert {
                            inner.pending_control_request = None;
                            inner.routing_matrix.set_route(my_id, pending_peer, false);
                            let version = inner
                                .routing_matrix
                                .row_versions
                                .get(&my_id)
                                .copied()
                                .unwrap_or(0);
                            revert = Some((my_id, pending_peer, version));
                            log::info!(
                                "Reverted route to {} after connection failure",
                                pending_peer,
                            );
                        }
                    }
                    // Store the failure reason so the poll timer can display it.
                    inner.last_connection_error = Some(reason);
                    revert
                };

                // Broadcast the route revert to connected peers so they
                // don't have stale routing state.
                if let Some((my_id, peer, version)) = revert_info {
                    self.broadcast_routing_delta(my_id, version, &[(my_id, peer, false)]);
                    self.broadcast_signed_local_row_announce();
                }
            }
            NetworkMessage::AuthChallenge { .. } | NetworkMessage::AuthProof { .. } => {
                log::trace!(
                    "Route auth proof message received; session handshake already verified"
                );
            }
            NetworkMessage::PairingRequest {
                public_key,
                pairing_code_hash,
            } => {
                self.handle_pairing_request(sender_id, public_key, pairing_code_hash);
            }
            NetworkMessage::PairingConfirm { signature } => {
                self.handle_pairing_confirm(sender_id, signature);
            }
            NetworkMessage::PairingReject => {
                self.handle_pairing_reject(sender_id);
            }
            other => {
                log::debug!("Unhandled network message: {:?}", other);
            }
        }
    }

    // ---- Pairing handlers -----------------------------------------------

    /// Send a pairing confirmation signed by the local identity.
    pub fn send_pairing_confirm(&self, node_id: NodeId, signature: Vec<u8>) {
        self.send_message_to(node_id, &NetworkMessage::PairingConfirm { signature });
        self.inner.borrow_mut().pending_pairings.remove(&node_id);
    }

    /// Send a pairing rejection to a peer.
    pub fn send_pairing_reject(&self, node_id: NodeId) {
        self.send_message_to(node_id, &NetworkMessage::PairingReject);
        self.inner.borrow_mut().pending_pairings.remove(&node_id);
    }

    /// Clear an in-memory pending pairing marker.
    pub fn clear_pending_pairing(&self, node_id: &NodeId) {
        self.inner.borrow_mut().pending_pairings.remove(node_id);
    }

    fn send_pairing_request_if_needed(&self, node_id: NodeId) {
        let msg = {
            let inner = self.inner.borrow();
            if inner.paired_devices.contains_key(&node_id) {
                return;
            }
            let Some(local_public_key) = inner.local_public_key else {
                return;
            };
            let Some(remote_public_key) = inner.peer_public_keys.get(&node_id).copied() else {
                return;
            };
            NetworkMessage::PairingRequest {
                public_key: local_public_key,
                pairing_code_hash: Self::pairing_code_hash(local_public_key, remote_public_key),
            }
        };
        self.send_message_to(node_id, &msg);
    }

    fn handle_pairing_request(
        &self,
        sender_id: NodeId,
        public_key: [u8; 32],
        pairing_code_hash: [u8; 32],
    ) {
        let should_reject = {
            let mut inner = self.inner.borrow_mut();
            let Some(session_public_key) = inner.peer_public_keys.get(&sender_id).copied() else {
                log::warn!(
                    "PairingRequest rejected: missing verified key for {}",
                    sender_id
                );
                return;
            };
            if session_public_key != public_key {
                log::warn!(
                    "PairingRequest rejected: sender {} public key does not match verified session",
                    sender_id
                );
                true
            } else if let Some(local_public_key) = inner.local_public_key {
                let expected = Self::pairing_code_hash(public_key, local_public_key);
                if expected != pairing_code_hash {
                    log::warn!(
                        "PairingRequest rejected: code hash mismatch for {}",
                        sender_id
                    );
                    true
                } else {
                    inner.pending_pairings.insert(sender_id, public_key);
                    false
                }
            } else {
                inner.pending_pairings.insert(sender_id, public_key);
                false
            }
        };

        if should_reject {
            self.send_pairing_reject(sender_id);
        }
    }

    fn handle_pairing_confirm(&self, sender_id: NodeId, signature: Vec<u8>) {
        let valid = {
            let inner = self.inner.borrow();
            let Some(sender_public_key) = inner.peer_public_keys.get(&sender_id).copied() else {
                log::warn!(
                    "PairingConfirm rejected: missing verified key for {}",
                    sender_id
                );
                return;
            };
            let Some(local_public_key) = inner.local_public_key else {
                log::warn!("PairingConfirm rejected: missing local public key");
                return;
            };
            Self::verify_pairing_confirm_signature(sender_public_key, local_public_key, &signature)
        };

        if valid {
            self.inner.borrow_mut().pending_pairings.remove(&sender_id);
            log::info!("PairingConfirm accepted from {}", sender_id);
        } else {
            self.inner.borrow_mut().last_connection_error =
                Some("pairing_confirm_invalid".to_string());
            log::warn!(
                "PairingConfirm rejected: invalid signature from {}",
                sender_id
            );
        }
    }

    fn handle_pairing_reject(&self, sender_id: NodeId) {
        let revert = {
            let mut inner = self.inner.borrow_mut();
            inner.pending_pairings.remove(&sender_id);
            inner.last_connection_error = Some("pairing_rejected".to_string());
            if inner.pending_control_request == Some(sender_id) {
                let my_id = inner.local_node_id;
                inner.pending_control_request = None;
                inner.pending_route_request_id = None;
                inner.routing_matrix.set_route(my_id, sender_id, false);
                let version = inner.routing_matrix.row_version(my_id).unwrap_or(0);
                Some((my_id, sender_id, version))
            } else {
                None
            }
        };

        if let Some((my_id, peer, version)) = revert {
            self.broadcast_routing_delta(my_id, version, &[(my_id, peer, false)]);
            self.broadcast_signed_local_row_announce();
        }
    }

    // ---- Signed routing row handlers -------------------------------------

    /// Send every currently known owner-signed row to a peer.
    pub fn send_signed_rows_to(&self, node_id: NodeId) {
        let rows = {
            let inner = self.inner.borrow();
            let mut rows: Vec<_> = inner.signed_rows.values().cloned().collect();
            if let Some(local_row) = Self::build_signed_local_row(&inner) {
                rows.push(local_row);
            }
            rows
        };
        for row in rows {
            self.send_message_to(node_id, &row.into_message());
        }
    }

    /// Ask a peer for an owner-signed row.
    pub fn send_routing_row_request_to(&self, node_id: NodeId, owner: NodeId) {
        self.send_message_to(node_id, &NetworkMessage::RoutingRowRequest { owner });
    }

    fn handle_routing_row_request(&self, sender_id: NodeId, owner: NodeId) {
        let row = {
            let inner = self.inner.borrow();
            if owner == inner.local_node_id {
                Self::build_signed_local_row(&inner)
            } else {
                inner.signed_rows.get(&owner).cloned()
            }
        };
        if let Some(row) = row {
            self.send_message_to(sender_id, &row.into_message());
        }
    }

    fn handle_routing_row_announce(
        &self,
        sender_id: NodeId,
        owner: NodeId,
        version: u64,
        mut cells: Vec<(NodeId, NodeId, bool)>,
        owner_public_key: [u8; 32],
        signature: Vec<u8>,
    ) {
        canonicalize_cells(&mut cells);
        if !Self::verify_routing_row_signature(owner, version, &cells, owner_public_key, &signature)
        {
            log::warn!(
                "RoutingRowAnnounce rejected: invalid owner signature (sender={}, owner={})",
                sender_id,
                owner
            );
            return;
        }

        let row = SignedRoutingRow {
            owner,
            version,
            cells: cells.clone(),
            owner_public_key,
            signature,
        };

        let should_forward = {
            let mut inner = self.inner.borrow_mut();
            let cached_version = inner.signed_rows.get(&owner).map(|row| row.version);
            let applied = inner
                .routing_matrix
                .apply_authorized_row(owner, version, &cells);
            let cache_updated =
                owner != inner.local_node_id && cached_version.map(|v| version > v).unwrap_or(true);
            if cache_updated {
                inner.signed_rows.insert(owner, row.clone());
            }
            applied || cache_updated
        };

        if should_forward {
            self.forward_signed_row(row, Some(sender_id));
        }
    }

    /// Verify a signed routing row against the owner's public key.
    pub fn verify_routing_row_signature(
        owner: NodeId,
        version: u64,
        cells: &[(NodeId, NodeId, bool)],
        owner_public_key: [u8; 32],
        signature: &[u8],
    ) -> bool {
        if owner_public_key == [0u8; 32]
            || cells.iter().any(|(controller, _, _)| *controller != owner)
        {
            return false;
        }
        let Ok(verifying_key) = VerifyingKey::from_bytes(&owner_public_key) else {
            return false;
        };
        if derive_node_id(&verifying_key) != owner {
            return false;
        }
        let Ok(signature) = Signature::from_slice(signature) else {
            return false;
        };
        let payload = Self::routing_row_signature_payload(owner, version, cells);
        verifying_key.verify(&payload, &signature).is_ok()
    }

    fn build_signed_local_row(inner: &RouterInner) -> Option<SignedRoutingRow> {
        let owner = inner.local_node_id;
        let owner_public_key = inner.local_public_key?;
        let signing_key = inner.local_signing_key.as_ref()?;
        let version = inner.routing_matrix.row_version(owner).unwrap_or(0);
        let cells = inner.routing_matrix.row_cells(owner);
        let payload = Self::routing_row_signature_payload(owner, version, &cells);
        let signature = signing_key.sign(&payload).to_bytes().to_vec();
        Some(SignedRoutingRow {
            owner,
            version,
            cells,
            owner_public_key,
            signature,
        })
    }

    fn broadcast_signed_local_row_announce(&self) {
        let (row, peers, tx) = {
            let inner = self.inner.borrow();
            (
                Self::build_signed_local_row(&inner),
                inner.connected_peers.iter().copied().collect::<Vec<_>>(),
                inner.outgoing_tx.clone(),
            )
        };
        let (Some(row), Some(tx)) = (row, tx) else {
            return;
        };
        let msg = row.into_message();
        for peer in peers {
            if tx.send((peer, msg.clone())).is_err() {
                break;
            }
        }
    }

    fn forward_signed_row(&self, row: SignedRoutingRow, except: Option<NodeId>) {
        let (peers, tx) = {
            let inner = self.inner.borrow();
            (
                inner
                    .connected_peers
                    .iter()
                    .copied()
                    .filter(|peer| Some(*peer) != except)
                    .collect::<Vec<_>>(),
                inner.outgoing_tx.clone(),
            )
        };
        let Some(tx) = tx else {
            return;
        };
        let msg = row.into_message();
        for peer in peers {
            if tx.send((peer, msg.clone())).is_err() {
                break;
            }
        }
    }

    // ---- Route authorization handlers ------------------------------------

    fn handle_route_request(
        &self,
        sender_id: NodeId,
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
    ) {
        let my_id = self.inner.borrow().local_node_id;
        if sender_id != controller || executor != my_id {
            log::warn!(
                "RouteRequest rejected: sender={} controller={} executor={} my_id={}",
                sender_id,
                controller,
                executor,
                my_id,
            );
            return;
        }

        if !self.inner.borrow().config.allow_remote_control {
            self.deny_route_request(request_id, controller, executor, "remote_control_disabled");
            return;
        }

        let mut unpaired_session_key = None;
        let trust_decision = {
            let inner = self.inner.borrow();
            let Some(session_pubkey) = inner.peer_public_keys.get(&controller).copied() else {
                log::warn!(
                    "RouteRequest rejected: missing verified public key for {}",
                    controller
                );
                self.deny_route_request(request_id, controller, executor, "missing_public_key");
                return;
            };
            match inner.paired_devices.get(&controller).copied() {
                Some(pairing) if pairing.public_key != session_pubkey => {
                    log::warn!(
                        "RouteRequest rejected: paired public key mismatch for {}",
                        controller
                    );
                    self.deny_route_request(
                        request_id,
                        controller,
                        executor,
                        "paired_key_mismatch",
                    );
                    return;
                }
                Some(pairing) => pairing.trust_state,
                None => {
                    log::info!(
                        "RouteRequest from unpaired verified device {}; asking user",
                        controller
                    );
                    unpaired_session_key = Some(session_pubkey);
                    DeviceTrust::AskEachTime
                }
            }
        };
        if let Some(public_key) = unpaired_session_key {
            self.inner
                .borrow_mut()
                .pending_pairings
                .insert(controller, public_key);
        }

        match trust_decision {
            DeviceTrust::Trusted => {
                self.grant_route_request(request_id, controller, executor);
                log::info!(
                    "RouteRequest auto-granted for trusted device: {} -> {}",
                    controller,
                    executor
                );
            }
            DeviceTrust::AskEachTime => {
                self.inner.borrow_mut().pending_route_approvals.insert(
                    controller,
                    PendingRouteApproval {
                        request_id,
                        controller,
                        executor,
                    },
                );
                log::info!(
                    "RouteRequest pending user approval: {} -> {}",
                    controller,
                    executor
                );
            }
            DeviceTrust::Blocked => {
                self.deny_route_request(request_id, controller, executor, "device_blocked");
            }
        }
    }

    fn grant_route_request(&self, request_id: u64, controller: NodeId, executor: NodeId) {
        {
            let mut inner = self.inner.borrow_mut();
            let version = inner
                .routing_matrix
                .row_versions
                .get(&controller)
                .copied()
                .unwrap_or(0)
                + 1;
            inner
                .routing_matrix
                .apply_delta(controller, version, &[(controller, executor, true)]);
            Self::note_controller_route_enabled(&mut inner, controller);
        }

        self.send_message_to(
            controller,
            &NetworkMessage::RouteGrant {
                request_id,
                controller,
                executor,
            },
        );
    }

    fn deny_route_request(
        &self,
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
        reason: &str,
    ) {
        self.send_message_to(
            controller,
            &NetworkMessage::RouteDenied {
                request_id,
                controller,
                executor,
                reason: reason.to_string(),
            },
        );
    }

    fn handle_route_grant(
        &self,
        sender_id: NodeId,
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
    ) {
        let grant = {
            let mut inner = self.inner.borrow_mut();
            let my_id = inner.local_node_id;
            if controller != my_id || sender_id != executor {
                log::warn!(
                    "RouteGrant rejected: sender={} controller={} executor={} my_id={}",
                    sender_id,
                    controller,
                    executor,
                    my_id,
                );
                return;
            }
            let Some(expected_id) = inner.pending_route_request_id else {
                log::debug!(
                    "Ignoring RouteGrant from {}: no pending request id",
                    sender_id
                );
                return;
            };
            if expected_id != request_id || inner.pending_control_request != Some(executor) {
                log::debug!("Ignoring stale RouteGrant from {}", sender_id);
                return;
            }

            inner.pending_control_request = None;
            inner.pending_route_request_id = None;
            inner.routing_matrix.set_route(my_id, executor, true);
            let version = inner
                .routing_matrix
                .row_versions
                .get(&my_id)
                .copied()
                .unwrap_or(0);
            (my_id, version)
        };

        self.broadcast_routing_delta(grant.0, grant.1, &[(controller, executor, true)]);
        self.broadcast_signed_local_row_announce();
        log::info!("RouteGrant accepted: {} -> {}", controller, executor);
    }

    fn handle_route_denied(
        &self,
        sender_id: NodeId,
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
        reason: String,
    ) {
        let revert = {
            let mut inner = self.inner.borrow_mut();
            let my_id = inner.local_node_id;
            if controller != my_id || sender_id != executor {
                log::warn!(
                    "RouteDenied rejected: sender={} controller={} executor={} my_id={}",
                    sender_id,
                    controller,
                    executor,
                    my_id,
                );
                return;
            }
            let Some(expected_id) = inner.pending_route_request_id else {
                log::debug!(
                    "Ignoring RouteDenied from {}: no pending request id",
                    sender_id
                );
                return;
            };
            if expected_id != request_id || inner.pending_control_request != Some(executor) {
                log::debug!("Ignoring stale RouteDenied from {}", sender_id);
                return;
            }

            inner.pending_control_request = None;
            inner.pending_route_request_id = None;
            inner.routing_matrix.set_route(my_id, executor, false);
            inner.last_connection_error = Some(reason);
            let version = inner
                .routing_matrix
                .row_versions
                .get(&my_id)
                .copied()
                .unwrap_or(0);
            (my_id, version)
        };

        self.broadcast_routing_delta(revert.0, revert.1, &[(controller, executor, false)]);
        self.broadcast_signed_local_row_announce();
        log::info!("RouteDenied applied: {} -/-> {}", controller, executor);
    }

    fn handle_route_release(&self, sender_id: NodeId, controller: NodeId, executor: NodeId) {
        if sender_id != controller && sender_id != executor {
            log::warn!(
                "RouteRelease rejected: sender={} controller={} executor={}",
                sender_id,
                controller,
                executor,
            );
            return;
        }

        let my_id = self.inner.borrow().local_node_id;
        self.inner
            .borrow_mut()
            .pending_route_approvals
            .remove(&controller);
        if controller == my_id {
            self.set_route(controller, executor, false);
            if self.pending_control_request() == Some(executor) {
                self.clear_pending_control_request();
            }
            return;
        }

        let version = {
            let mut inner = self.inner.borrow_mut();
            let version = inner
                .routing_matrix
                .row_versions
                .get(&controller)
                .copied()
                .unwrap_or(0)
                + 1;
            inner
                .routing_matrix
                .apply_delta(controller, version, &[(controller, executor, false)]);
            if executor == my_id {
                Self::note_controller_route_disabled(&mut inner, controller);
            }
            version
        };
        self.broadcast_routing_delta(controller, version, &[(controller, executor, false)]);
        log::info!("RouteRelease applied: {} -/-> {}", controller, executor);
    }

    // ---- Route revocation handler ----------------------------------------

    /// Handle a `RouteRevoke` from a remote peer.
    ///
    /// When the revoke targets the local node's own row (`from == my_id`),
    /// delegates to [`set_route`](Self::set_route) which enforces ownership
    /// and broadcasts a `RoutingDelta`.
    ///
    /// When the revoke targets a remote peer's row (`from != my_id`), uses
    /// [`apply_routing_delta`](Self::apply_routing_delta) to bypass the
    /// ownership check, then broadcasts the delta to other peers.
    /// The `version` carried in the message is used directly so that all
    /// receivers converge on the same version without independent computation.
    fn handle_route_revoke(&self, from: NodeId, to: NodeId, version: u64) {
        log::info!("RouteRevoke from {} -> {} (v{})", from, to, version);
        let my_id = self.inner.borrow().local_node_id;
        if from == my_id {
            // Own row: set_route handles version bump and broadcast.
            self.set_route(from, to, false);
            // If the message carries a higher version (fabricated by a
            // remote peer), advance our version to match so that our
            // subsequent deltas are not rejected as stale.
            if version > 0 {
                let mut inner = self.inner.borrow_mut();
                let entry = inner.routing_matrix.row_versions.entry(from).or_insert(0);
                *entry = (*entry).max(version);
            }
        } else {
            // Remote row: use the version from the message, apply locally,
            // and broadcast to other connected peers.
            self.apply_routing_delta(from, version, &[(from, to, false)]);
            self.broadcast_routing_delta(from, version, &[(from, to, false)]);
        }
    }

    // ---- Internal helpers ------------------------------------------------

    /// Execute an action on the local calculator and apply the result to UI
    /// and audio. Broadcasts the new state to all connected remote sessions.
    fn execute_local(&self, action: CalcAction) {
        let result = {
            let inner = self.inner.borrow();
            inner.calculator.borrow_mut().dispatch(action)
        };
        self.apply_result(&result);

        // Broadcast to all connected controllers.
        let snapshot = {
            let inner = self.inner.borrow();
            Self::build_state_snapshot(&result, inner.local_seq)
        };
        self.broadcast_state(&snapshot);
    }

    /// Speculatively apply an action locally when the real execution target
    /// is remote. Provides instant UI feedback; the authoritative state from
    /// the remote node will overwrite this if needed.
    fn apply_speculative(&self, action: CalcAction) {
        let result = {
            let inner = self.inner.borrow();
            inner.calculator.borrow_mut().dispatch(action)
        };
        self.apply_result(&result);
    }

    /// Apply a [`CalcResult`] to the UI widgets and audio subsystem.
    ///
    /// Audio playback is skipped when `audio_muted` is `true` (either the
    /// user toggled mute manually, or the routing matrix indicates this
    /// node is controlling a remote executor).
    fn apply_result(&self, result: &CalcResult) {
        let mut inner = self.inner.borrow_mut();
        inner.display.update_display(&result.display);
        inner.display.update_history(&result.history);
        inner
            .display
            .update_memory_indicator(&result.memory_indicator);
        inner.display.set_error_state(result.is_error);
        if !inner.audio_muted
            && let Some(ref mut audio) = inner.audio
        {
            audio.play_events(&result.events);
        }
    }

    /// Construct an [`ActionEnvelope`] for outbound transmission, incrementing
    /// the local sequence counter.
    fn build_envelope(&self, action: CalcAction) -> ActionEnvelope {
        let mut inner = self.inner.borrow_mut();
        inner.local_seq += 1;
        ActionEnvelope {
            seq: inner.local_seq,
            source_id: inner.local_node_id,
            timestamp_ms: Self::timestamp_ms(),
            action,
        }
    }

    /// Build a [`StateSnapshot`] from a calculator result and sequence number.
    fn build_state_snapshot(result: &CalcResult, seq: u64) -> StateSnapshot {
        StateSnapshot {
            display: result.display.clone(),
            history: result.history.clone(),
            memory_indicator: result.memory_indicator.clone(),
            is_error: result.is_error,
            last_seq_applied: seq,
        }
    }

    /// Broadcast a state snapshot to every controller that controls this
    /// node (according to the routing matrix), filtered by active sessions.
    /// Self is excluded — local state is already up-to-date.
    fn broadcast_state(&self, snapshot: &StateSnapshot) {
        let peers: Vec<NodeId>;
        let tx: Option<mpsc::UnboundedSender<(NodeId, NetworkMessage)>>;
        {
            let inner = self.inner.borrow();
            let my_id = inner.local_node_id;
            // Matrix is the sole authority: send to all *remote* controllers
            // that have a route to us (exclude self — local state is already
            // current).  Only send to controllers that also have an active session.
            peers = inner
                .routing_matrix
                .my_controllers()
                .into_iter()
                .filter(|id| *id != my_id)
                .filter(|id| inner.connected_peers.contains(id))
                .collect();
            tx = inner.outgoing_tx.clone();
        }
        if peers.is_empty() {
            return;
        }
        let tx = match tx {
            Some(tx) => tx,
            None => {
                log::trace!("No outgoing channel configured; skipping broadcast");
                return;
            }
        };
        let msg = NetworkMessage::StateUpdate(snapshot.clone());
        for node_id in peers {
            if tx.send((node_id, msg.clone())).is_err() {
                log::trace!("Outgoing channel closed during broadcast");
                break;
            }
        }
    }

    /// Send an [`ActionEnvelope`] to a specific remote node.
    fn send_to_remote(&self, node_id: NodeId, envelope: ActionEnvelope) {
        let msg = NetworkMessage::Action(envelope);
        self.send_message_to(node_id, &msg);
    }

    /// Notify a peer that this node is disconnecting by revoking all
    /// routes involving both nodes.
    pub fn send_release_to(&self, node_id: NodeId) {
        let my_id = self.inner.borrow().local_node_id;
        self.send_message_to(
            node_id,
            &NetworkMessage::RouteRelease {
                controller: my_id,
                executor: node_id,
            },
        );
    }

    // ---- Routing matrix public API ----------------------------------------

    /// Set a route in the routing matrix and broadcast a `RoutingDelta` to
    /// all connected peers so they can apply the change.  Only the local
    /// node's own row may be modified; returns `false` if `controller` is
    /// not this node.
    pub fn set_route(&self, controller: NodeId, executor: NodeId, value: bool) -> bool {
        let (ok, version) = {
            let mut inner = self.inner.borrow_mut();
            let ok = inner.routing_matrix.set_route(controller, executor, value);
            let version = inner
                .routing_matrix
                .row_versions
                .get(&controller)
                .copied()
                .unwrap_or(0);
            (ok, version)
        };
        if ok {
            self.broadcast_routing_delta(controller, version, &[(controller, executor, value)]);
            self.broadcast_signed_local_row_announce();
        }
        ok
    }

    /// Revoke a remote-owned route, bypassing the ownership check in
    /// [`set_route`](Self::set_route).
    ///
    /// Used when the local node needs to clear an inbound route from a
    /// remote controller (e.g. when the user disables "allow remote control").
    /// Applies the delta **locally only** -- we do not own the remote row
    /// and must not fabricate a version for it.  Broadcasting a fabricated
    /// version would either be rejected as stale (if the real version is
    /// higher) or advance past the real version causing network-wide
    /// divergence.  The row owner propagates the authoritative revocation
    /// through its own `RoutingDelta` (triggered by the `RouteRevoke` we
    /// send via [`send_route_revoke_directed`]).
    pub fn revoke_remote_route(&self, controller: NodeId, executor: NodeId) {
        {
            let mut inner = self.inner.borrow_mut();
            inner
                .routing_matrix
                .entries
                .insert((controller, executor), false);
        }
    }

    /// Send a full routing matrix snapshot to a specific peer.
    ///
    /// Called when a new session is established so the peer can initialise
    /// its local matrix from our current state.
    pub fn send_routing_sync_to(&self, node_id: NodeId) {
        let (entries, tx) = {
            let inner = self.inner.borrow();
            (
                inner.routing_matrix.sync_entries(),
                inner.outgoing_tx.clone(),
            )
        };
        if let Some(tx) = tx {
            let msg = NetworkMessage::RoutingSync { entries };
            let _ = tx.send((node_id, msg));
        }
    }

    /// Apply a remote routing delta to the local matrix.
    pub fn apply_routing_delta(
        &self,
        owner: NodeId,
        version: u64,
        cells: &[(NodeId, NodeId, bool)],
    ) {
        let mut inner = self.inner.borrow_mut();
        let my_id = inner.local_node_id;
        inner.routing_matrix.apply_delta(owner, version, cells);
        for &(controller, executor, value) in cells {
            if executor != my_id {
                continue;
            }
            if value {
                Self::note_controller_route_enabled(&mut inner, controller);
            } else {
                Self::note_controller_route_disabled(&mut inner, controller);
            }
        }
    }

    /// Apply a full routing sync snapshot to the local matrix.
    pub fn apply_routing_sync(&self, entries: &[(NodeId, NodeId, bool, u64)]) {
        self.inner.borrow_mut().routing_matrix.apply_sync(entries);
    }

    /// Return all executors that this node controls (including self).
    pub fn my_control_targets(&self) -> Vec<NodeId> {
        self.inner.borrow().routing_matrix.my_control_targets()
    }

    /// Return `true` if this node controls at least one non-self executor.
    pub fn is_muted(&self) -> bool {
        self.inner.borrow().routing_matrix.is_muted()
    }

    /// Set whether local audio playback is suppressed.
    ///
    /// Called by the UI poll timer to reflect the combined mute state
    /// (routing mute + user toggle).
    pub fn set_audio_muted(&self, muted: bool) {
        self.inner.borrow_mut().audio_muted = muted;
    }

    /// Return whether local audio playback is currently suppressed.
    pub fn is_audio_muted(&self) -> bool {
        self.inner.borrow().audio_muted
    }

    /// Return all controllers that control this node.
    pub fn my_controllers(&self) -> Vec<NodeId> {
        self.inner.borrow().routing_matrix.my_controllers()
    }

    /// Return a snapshot of the full routing matrix for UI display.
    pub fn get_routing_matrix(&self) -> HashMap<(NodeId, NodeId), bool> {
        self.inner.borrow().routing_matrix.get_matrix()
    }

    /// Return every node represented in the routing matrix without cloning
    /// the full cell map.
    pub fn routing_node_ids(&self) -> Vec<NodeId> {
        self.inner.borrow().routing_matrix.node_ids()
    }

    /// Return a row-major snapshot for a caller-provided node order.
    pub fn routing_cells_for_order(&self, node_ids: &[NodeId]) -> Vec<bool> {
        self.inner.borrow().routing_matrix.cells_for_order(node_ids)
    }

    /// Take the last connection error, clearing it from the router.
    ///
    /// Returns `Some(reason)` if a `ConnectionFailed` message was received
    /// since the last call. The poll timer should call this each tick and
    /// display the error to the user.
    pub fn take_connection_error(&self) -> Option<String> {
        self.inner.borrow_mut().last_connection_error.take()
    }

    /// Broadcast a `RoutingDelta` to all connected peers.
    fn broadcast_routing_delta(
        &self,
        owner: NodeId,
        version: u64,
        cells: &[(NodeId, NodeId, bool)],
    ) {
        let peers: Vec<NodeId>;
        let tx: Option<mpsc::UnboundedSender<(NodeId, NetworkMessage)>>;
        {
            let inner = self.inner.borrow();
            peers = inner.connected_peers.iter().copied().collect();
            tx = inner.outgoing_tx.clone();
        }
        if peers.is_empty() {
            return;
        }
        let tx = match tx {
            Some(tx) => tx,
            None => return,
        };
        let msg = NetworkMessage::RoutingDelta {
            owner,
            version,
            cells: cells.to_vec(),
        };
        for node_id in peers {
            if tx.send((node_id, msg.clone())).is_err() {
                break;
            }
        }
    }

    /// Send a [`NetworkMessage`] to a specific remote node via the outgoing
    /// channel to the networking runtime.
    fn send_message_to(&self, node_id: NodeId, msg: &NetworkMessage) {
        let tx = {
            let inner = self.inner.borrow();
            inner.outgoing_tx.clone()
        };

        match tx {
            Some(tx) => {
                if tx.send((node_id, msg.clone())).is_err() {
                    log::warn!("Outgoing channel is closed");
                }
            }
            None => {
                log::warn!("No outgoing channel configured for node {}", node_id);
            }
        }
    }

    /// Wall-clock milliseconds since Unix epoch.
    fn timestamp_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::identity::DeviceIdentity;
    use crate::audio::AudioMode;
    use crate::core::action::CalcAction;
    use crate::core::token::{BinaryOp, VocalEvent};
    use std::cell::RefCell;
    use std::rc::Rc;

    // -----------------------------------------------------------------------
    // Mock DisplayUpdater
    // -----------------------------------------------------------------------

    /// Records every call made to the display updater so tests can assert on
    /// the exact sequence of UI updates.
    #[derive(Debug, Clone, Default)]
    struct RecordedCalls {
        pub displays: Vec<String>,
        pub histories: Vec<String>,
        pub memory_indicators: Vec<String>,
        pub error_states: Vec<bool>,
    }

    struct MockDisplayUpdater {
        calls: Rc<RefCell<RecordedCalls>>,
    }

    impl MockDisplayUpdater {
        fn new(calls: Rc<RefCell<RecordedCalls>>) -> Self {
            Self { calls }
        }
    }

    impl DisplayUpdater for MockDisplayUpdater {
        fn update_display(&self, text: &str) {
            self.calls.borrow_mut().displays.push(text.to_string());
        }
        fn update_history(&self, text: &str) {
            self.calls.borrow_mut().histories.push(text.to_string());
        }
        fn update_memory_indicator(&self, indicator: &str) {
            self.calls
                .borrow_mut()
                .memory_indicators
                .push(indicator.to_string());
        }
        fn set_error_state(&self, is_error: bool) {
            self.calls.borrow_mut().error_states.push(is_error);
        }
    }

    // -----------------------------------------------------------------------
    // Mock AudioPlayer
    // -----------------------------------------------------------------------

    struct MockAudioPlayer {
        pub played_events: Vec<Vec<VocalEvent>>,
    }

    impl MockAudioPlayer {
        fn new() -> Self {
            Self {
                played_events: Vec::new(),
            }
        }
    }

    impl AudioPlayer for MockAudioPlayer {
        fn play_events(&mut self, events: &[VocalEvent]) {
            self.played_events.push(events.to_vec());
        }
        fn set_mode(&mut self, _mode: AudioMode) {}
        fn set_volume(&mut self, _slider: f64) {}
        fn mode(&self) -> AudioMode {
            AudioMode::Normal
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a Router wired to mocks and return the shared call recorder.
    fn make_router() -> (Router, Rc<RefCell<RecordedCalls>>) {
        let calls = Rc::new(RefCell::new(RecordedCalls::default()));
        let display = MockDisplayUpdater::new(calls.clone());
        let audio = MockAudioPlayer::new();
        let calc = Rc::new(RefCell::new(Calculator::new()));
        let router = Router::new(calc, Some(Box::new(audio)), Box::new(display));
        (router, calls)
    }

    /// Build a Router with an outgoing message channel and return the receiver
    /// so tests can inspect what was sent over the wire.
    fn make_router_with_channel() -> (
        Router,
        Rc<RefCell<RecordedCalls>>,
        mpsc::UnboundedReceiver<(NodeId, NetworkMessage)>,
    ) {
        let calls = Rc::new(RefCell::new(RecordedCalls::default()));
        let display = MockDisplayUpdater::new(calls.clone());
        let audio = MockAudioPlayer::new();
        let calc = Rc::new(RefCell::new(Calculator::new()));
        let router = Router::new(calc, Some(Box::new(audio)), Box::new(display));
        let (tx, rx) = mpsc::unbounded_channel();
        router.set_outgoing_tx(tx);
        (router, calls, rx)
    }

    fn allow_state_updates_from(router: &Router, peer: NodeId) {
        router.add_remote_session(peer);
        let my_id = router.local_node_id();
        router.set_route(my_id, peer, true);
        router.clear_pending_control_request();
    }

    fn trust_peer(router: &Router, peer: NodeId, public_key: [u8; 32], trust: DeviceTrust) {
        router.set_remote_public_key(peer, public_key);
        router.set_paired_devices([(peer, public_key, trust)]);
    }

    fn set_router_identity(router: &Router, identity: &DeviceIdentity) {
        router.set_local_node_id(identity.node_id());
        router.set_local_identity(identity.public_key_bytes(), identity.signing_key());
    }

    fn signed_row_message(
        identity: &DeviceIdentity,
        version: u64,
        cells: Vec<(NodeId, NodeId, bool)>,
    ) -> NetworkMessage {
        let owner = identity.node_id();
        let payload = Router::routing_row_signature_payload(owner, version, &cells);
        NetworkMessage::RoutingRowAnnounce {
            owner,
            version,
            cells,
            owner_public_key: identity.public_key_bytes(),
            signature: identity.sign(&payload).to_bytes().to_vec(),
        }
    }

    // -----------------------------------------------------------------------
    // 1. Local dispatch
    // -----------------------------------------------------------------------

    #[test]
    fn local_dispatch_digit_updates_display() {
        let (router, calls) = make_router();
        router.dispatch(CalcAction::Digit(5));

        let c = calls.borrow();
        assert!(
            c.displays.iter().any(|d| d == "5"),
            "Expected display to contain '5', got {:?}",
            c.displays
        );
    }

    #[test]
    fn local_dispatch_sequence_of_actions() {
        let (router, calls) = make_router();
        // 3 + 4 = 7
        router.dispatch(CalcAction::Digit(3));
        router.dispatch(CalcAction::Operator(BinaryOp::Add));
        router.dispatch(CalcAction::Digit(4));
        router.dispatch(CalcAction::Equals);

        let c = calls.borrow();
        let last_display = c.displays.last().unwrap();
        assert_eq!(last_display, "7");
    }

    #[test]
    fn local_dispatch_updates_history_and_memory_indicator() {
        let (router, calls) = make_router();
        // 5 M+ -> memory indicator should become "M"
        router.dispatch(CalcAction::Digit(5));
        router.dispatch(CalcAction::MemoryAdd);

        let c = calls.borrow();
        assert!(
            c.memory_indicators.iter().any(|m| m == "M"),
            "Expected memory indicator 'M', got {:?}",
            c.memory_indicators
        );
    }

    #[test]
    fn local_dispatch_error_sets_error_state() {
        let (router, calls) = make_router();
        // 5 / 0 = -> error
        router.dispatch(CalcAction::Digit(5));
        router.dispatch(CalcAction::Operator(BinaryOp::Divide));
        router.dispatch(CalcAction::Digit(0));
        router.dispatch(CalcAction::Equals);

        let c = calls.borrow();
        assert!(
            c.error_states.iter().any(|&e| e),
            "Expected at least one error_state=true call"
        );
    }

    // -----------------------------------------------------------------------
    // 2. Remote dispatch
    // -----------------------------------------------------------------------

    #[test]
    fn remote_dispatch_speculative_update_and_envelope() {
        let (router, calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        let my_id = router.local_node_id();
        router.set_route(my_id, peer, true);
        // Drain the RoutingDelta broadcast from set_route.
        let _ = rx.try_recv();

        router.dispatch(CalcAction::Digit(7));

        // Speculative: local display should already show "7".
        let c = calls.borrow();
        assert!(
            c.displays.iter().any(|d| d == "7"),
            "Speculative local echo should update display to '7', got {:?}",
            c.displays
        );

        // Envelope should have been sent to the peer.
        let (target, msg) = rx.try_recv().expect("Expected an outgoing message");
        assert_eq!(target, peer);
        match msg {
            NetworkMessage::Action(envelope) => {
                assert_eq!(envelope.action, CalcAction::Digit(7));
                assert_eq!(envelope.seq, 1); // first envelope
            }
            other => panic!("Expected Action envelope, got {:?}", other),
        }
    }

    #[test]
    fn remote_dispatch_no_channel_does_not_panic() {
        let (router, _calls) = make_router();
        // No outgoing_tx configured; dispatch should not panic.
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        let my_id = router.local_node_id();
        router.set_route(my_id, peer, true);

        // Should log a warning but not crash.
        router.dispatch(CalcAction::Digit(1));
    }

    // -----------------------------------------------------------------------
    // 3. handle_remote_action with allow_remote_control = false
    // -----------------------------------------------------------------------

    #[test]
    fn handle_remote_action_rejected_when_disabled() {
        let (router, calls) = make_router();
        router.set_allow_remote_control(false);

        let envelope = ActionEnvelope {
            seq: 1,
            source_id: NodeId::new_v4(),
            timestamp_ms: 0,
            action: CalcAction::Digit(9),
        };
        router.handle_remote_action(envelope);

        // No display update should have occurred from the remote action.
        let c = calls.borrow();
        assert!(
            c.displays.is_empty(),
            "Rejected action should not update display, got {:?}",
            c.displays
        );
    }

    #[test]
    fn handle_remote_action_accepted_when_enabled() {
        let (router, calls) = make_router();
        router.set_allow_remote_control(true);

        // Establish a controller via the routing matrix.
        let controller = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.apply_routing_delta(controller, 1, &[(controller, my_id, true)]);

        let envelope = ActionEnvelope {
            seq: 1,
            source_id: controller,
            timestamp_ms: 0,
            action: CalcAction::Digit(4),
        };
        router.handle_remote_action(envelope);

        let c = calls.borrow();
        assert!(
            c.displays.iter().any(|d| d == "4"),
            "Accepted action should update display to '4', got {:?}",
            c.displays
        );
    }

    #[test]
    fn handle_remote_action_rejected_when_not_controller() {
        let (router, calls) = make_router();
        router.set_allow_remote_control(true);

        // Grant control to peer_a via the routing matrix.
        let peer_a = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.apply_routing_delta(peer_a, 1, &[(peer_a, my_id, true)]);

        // peer_b (not the controller) tries to send an action.
        let peer_b = NodeId::new_v4();
        let envelope = ActionEnvelope {
            seq: 1,
            source_id: peer_b,
            timestamp_ms: 0,
            action: CalcAction::Digit(7),
        };
        router.handle_remote_action(envelope);

        // Action should have been rejected -- no display update.
        let c = calls.borrow();
        assert!(
            c.displays.is_empty(),
            "Action from non-controller should be rejected, got {:?}",
            c.displays
        );
    }

    #[test]
    fn network_action_rejects_spoofed_source_id() {
        let (router, calls) = make_router();
        router.set_allow_remote_control(true);
        let controller = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.apply_routing_delta(controller, 1, &[(controller, my_id, true)]);

        let envelope = ActionEnvelope {
            seq: 1,
            source_id: controller,
            timestamp_ms: 0,
            action: CalcAction::Digit(8),
        };
        router.handle_network_message(NodeId::new_v4(), NetworkMessage::Action(envelope));

        let c = calls.borrow();
        assert!(
            c.displays.is_empty(),
            "Action with spoofed envelope source should be rejected"
        );
    }

    // -----------------------------------------------------------------------
    // 4. handle_network_message for StateUpdate
    // -----------------------------------------------------------------------

    #[test]
    fn handle_network_message_state_update() {
        let (router, calls) = make_router();
        let executor = NodeId::new_v4();
        allow_state_updates_from(&router, executor);
        let snapshot = StateSnapshot {
            display: "42".to_string(),
            history: "40 +".to_string(),
            memory_indicator: "M".to_string(),
            is_error: false,
            last_seq_applied: 10,
        };
        router.handle_network_message(executor, NetworkMessage::StateUpdate(snapshot));

        let c = calls.borrow();
        assert!(
            c.displays.iter().any(|d| d == "42"),
            "StateUpdate should push display '42', got {:?}",
            c.displays
        );
        assert!(
            c.histories.iter().any(|h| h == "40 +"),
            "StateUpdate should push history '40 +', got {:?}",
            c.histories
        );
        assert!(
            c.memory_indicators.iter().any(|m| m == "M"),
            "StateUpdate should push memory indicator 'M', got {:?}",
            c.memory_indicators
        );
        assert!(
            c.error_states.iter().any(|&e| !e),
            "StateUpdate should set error_state=false"
        );
    }

    #[test]
    fn handle_network_message_state_update_with_error() {
        let (router, calls) = make_router();
        let executor = NodeId::new_v4();
        allow_state_updates_from(&router, executor);
        let snapshot = StateSnapshot {
            display: "错误".to_string(),
            history: "不能除以零".to_string(),
            memory_indicator: String::new(),
            is_error: true,
            last_seq_applied: 5,
        };
        router.handle_network_message(executor, NetworkMessage::StateUpdate(snapshot));

        let c = calls.borrow();
        assert!(c.displays.iter().any(|d| d == "错误"));
        assert!(c.error_states.iter().any(|&e| e));
    }

    #[test]
    fn handle_network_message_state_update_resets_calculator_state() {
        // Use a shared calculator so we can inspect state through the router.
        let calc = Rc::new(RefCell::new(Calculator::new()));
        let calls = Rc::new(RefCell::new(RecordedCalls::default()));
        let display = MockDisplayUpdater::new(calls.clone());
        let audio = MockAudioPlayer::new();
        let router = Router::new(calc.clone(), Some(Box::new(audio)), Box::new(display));
        let executor = NodeId::new_v4();
        allow_state_updates_from(&router, executor);

        // Speculative: 9 + 3 = → local acc = 12
        router.dispatch(CalcAction::Digit(9));
        router.dispatch(CalcAction::Operator(BinaryOp::Add));
        router.dispatch(CalcAction::Digit(3));
        router.dispatch(CalcAction::Equals);

        // Authoritative StateUpdate from the remote: display is "99"
        // (the remote had a different starting state).
        let snapshot = StateSnapshot {
            display: "99".to_string(),
            history: "90 + 9 = ".to_string(),
            memory_indicator: String::new(),
            is_error: false,
            last_seq_applied: 5,
        };
        router.handle_network_message(executor, NetworkMessage::StateUpdate(snapshot));

        // After reset, calculator acc should be 99, not 12.
        // Dispatch "+ 1 =" → should produce 100 (99+1), not 13 (12+1).
        router.dispatch(CalcAction::Operator(BinaryOp::Add));
        router.dispatch(CalcAction::Digit(1));
        router.dispatch(CalcAction::Equals);

        let c = calls.borrow();
        let last_display = c.displays.last().unwrap();
        assert_eq!(
            last_display, "100",
            "After StateUpdate reset, calculator acc should be 99 (from snapshot), not 12 (speculative). Got last display: {}",
            last_display
        );
    }

    #[test]
    fn handle_network_message_state_update_resets_error_state() {
        let calc = Rc::new(RefCell::new(Calculator::new()));
        let calls = Rc::new(RefCell::new(RecordedCalls::default()));
        let display = MockDisplayUpdater::new(calls.clone());
        let audio = MockAudioPlayer::new();
        let router = Router::new(calc.clone(), Some(Box::new(audio)), Box::new(display));
        let executor = NodeId::new_v4();
        allow_state_updates_from(&router, executor);

        // Cause a divide-by-zero error locally.
        router.dispatch(CalcAction::Digit(5));
        router.dispatch(CalcAction::Operator(BinaryOp::Divide));
        router.dispatch(CalcAction::Digit(0));
        router.dispatch(CalcAction::Equals);

        // Calculator should be in error state.
        {
            let c = calc.borrow();
            // Verify by checking that dispatching a digit returns error display.
            drop(c);
        }

        // StateUpdate: remote says we're back to normal with display "0".
        let snapshot = StateSnapshot {
            display: "0".to_string(),
            history: String::new(),
            memory_indicator: String::new(),
            is_error: false,
            last_seq_applied: 6,
        };
        router.handle_network_message(executor, NetworkMessage::StateUpdate(snapshot));

        // After reset, calculator should NOT be in error state.
        // Dispatch "1 + 2 =" → should produce 3, not stay in error.
        router.dispatch(CalcAction::Digit(1));
        router.dispatch(CalcAction::Operator(BinaryOp::Add));
        router.dispatch(CalcAction::Digit(2));
        router.dispatch(CalcAction::Equals);

        let c = calls.borrow();
        let last_display = c.displays.last().unwrap();
        assert_eq!(
            last_display, "3",
            "After StateUpdate reset from error, calculator should work normally. Got: {}",
            last_display
        );
    }

    #[test]
    fn state_update_rejected_from_non_control_target() {
        let (router, calls) = make_router();
        let sender = NodeId::new_v4();
        router.add_remote_session(sender);
        let snapshot = StateSnapshot {
            display: "999".to_string(),
            history: String::new(),
            memory_indicator: String::new(),
            is_error: false,
            last_seq_applied: 10,
        };

        router.handle_network_message(sender, NetworkMessage::StateUpdate(snapshot));

        let c = calls.borrow();
        assert!(
            c.displays.is_empty(),
            "StateUpdate from non-target should be rejected"
        );
    }

    #[test]
    fn state_update_rejected_while_route_grant_pending() {
        let (router, calls) = make_router();
        let sender = NodeId::new_v4();
        router.add_remote_session(sender);
        let my_id = router.local_node_id();
        router.set_route(my_id, sender, true);
        router.set_pending_control_request(sender);
        let snapshot = StateSnapshot {
            display: "123".to_string(),
            history: String::new(),
            memory_indicator: String::new(),
            is_error: false,
            last_seq_applied: 10,
        };

        router.handle_network_message(sender, NetworkMessage::StateUpdate(snapshot));

        let c = calls.borrow();
        assert!(
            c.displays.is_empty(),
            "StateUpdate while grant is pending should be rejected"
        );
    }

    #[test]
    fn handle_network_message_ping_does_not_update_display() {
        let (router, calls) = make_router();
        router.handle_network_message(NodeId::new_v4(), NetworkMessage::Ping);

        let c = calls.borrow();
        assert!(
            c.displays.is_empty(),
            "Ping should not trigger display updates"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Sequence counter advancement
    // -----------------------------------------------------------------------

    #[test]
    fn sequence_counter_advances_on_remote_dispatch() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        let my_id = router.local_node_id();
        router.set_route(my_id, peer, true);
        // Drain the RoutingDelta broadcast from set_route.
        let _ = rx.try_recv();

        // Dispatch three actions; seq should be 1, 2, 3.
        router.dispatch(CalcAction::Digit(1));
        router.dispatch(CalcAction::Digit(2));
        router.dispatch(CalcAction::Digit(3));

        for expected_seq in 1..=3u64 {
            let (_, msg) = rx.try_recv().unwrap();
            match msg {
                NetworkMessage::Action(env) => {
                    assert_eq!(
                        env.seq, expected_seq,
                        "Expected seq {} but got {}",
                        expected_seq, env.seq
                    );
                }
                other => panic!("Expected Action, got {:?}", other),
            }
        }
    }

    #[test]
    fn handle_remote_action_advances_local_seq() {
        let (router, _calls, mut rx) = make_router_with_channel();
        router.set_allow_remote_control(true);
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);

        // Establish a controller via the routing matrix.
        let my_id = router.local_node_id();
        router.apply_routing_delta(peer, 1, &[(peer, my_id, true)]);

        // Remote action with seq=50 should advance local_seq.
        let envelope = ActionEnvelope {
            seq: 50,
            source_id: peer,
            timestamp_ms: 0,
            action: CalcAction::Digit(0),
        };
        router.handle_remote_action(envelope);

        // Now dispatch a local action that should broadcast with seq=50
        // (the snapshot uses the current local_seq).
        router.dispatch(CalcAction::Digit(8));

        // The broadcast from execute_local uses local_seq which is now 50.
        let (_, msg) = rx
            .try_recv()
            .expect("Expected broadcast from local dispatch");
        match msg {
            NetworkMessage::StateUpdate(snap) => {
                assert_eq!(snap.last_seq_applied, 50);
            }
            other => panic!("Expected StateUpdate, got {:?}", other),
        }
    }

    #[test]
    fn handle_remote_action_does_not_decrease_seq() {
        let (router, _calls, mut rx) = make_router_with_channel();
        router.set_allow_remote_control(true);
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);

        // Establish a controller via the routing matrix.
        let my_id = router.local_node_id();
        router.apply_routing_delta(peer, 1, &[(peer, my_id, true)]);

        // First remote action with seq=100.
        let env_high = ActionEnvelope {
            seq: 100,
            source_id: peer,
            timestamp_ms: 0,
            action: CalcAction::Digit(1),
        };
        router.handle_remote_action(env_high);

        // Second remote action with seq=5 (lower than current).
        let env_low = ActionEnvelope {
            seq: 5,
            source_id: peer,
            timestamp_ms: 0,
            action: CalcAction::Digit(2),
        };
        router.handle_remote_action(env_low);

        // local_seq should remain at 100.
        router.dispatch(CalcAction::Digit(9));
        let (_, msg) = rx.try_recv().unwrap();
        match msg {
            NetworkMessage::StateUpdate(snap) => {
                assert_eq!(snap.last_seq_applied, 100);
            }
            other => panic!("Expected StateUpdate, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Additional: broadcast reaches multiple peers
    // -----------------------------------------------------------------------

    #[test]
    fn local_dispatch_broadcasts_to_all_peers() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer_a = NodeId::new_v4();
        let peer_b = NodeId::new_v4();
        router.add_remote_session(peer_a);
        router.add_remote_session(peer_b);
        let my_id = router.local_node_id();
        // Peers must be controllers of us in the matrix for broadcast to reach them.
        router.apply_routing_delta(peer_a, 1, &[(peer_a, my_id, true)]);
        router.apply_routing_delta(peer_b, 1, &[(peer_b, my_id, true)]);

        router.dispatch(CalcAction::Digit(6));

        // Should receive two StateUpdate messages (one per peer).
        let mut targets = HashSet::new();
        for _ in 0..2 {
            let (target, msg) = rx.try_recv().expect("Expected outgoing broadcast");
            targets.insert(target);
            assert!(matches!(msg, NetworkMessage::StateUpdate(_)));
        }
        assert!(targets.contains(&peer_a));
        assert!(targets.contains(&peer_b));
    }

    #[test]
    fn remove_peer_stops_broadcasts() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        let my_id = router.local_node_id();
        // Peer must be a controller of us for broadcast to reach them.
        router.apply_routing_delta(peer, 1, &[(peer, my_id, true)]);
        router.dispatch(CalcAction::Digit(1));
        assert!(rx.try_recv().is_ok(), "Should have one broadcast");

        router.remove_remote_session(&peer);
        router.dispatch(CalcAction::Digit(2));
        assert!(
            rx.try_recv().is_err(),
            "Removed peer should not receive broadcasts"
        );
    }

    #[test]
    fn cleanup_peer_disconnect_does_not_send_delta_to_departing_peer() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let departing = NodeId::new_v4();
        let staying = NodeId::new_v4();
        router.add_remote_session(departing);
        router.add_remote_session(staying);
        let my_id = router.local_node_id();

        // We control the departing peer via a route.
        router.set_route(my_id, departing, true);
        // Drain ALL RoutingDelta broadcasts from set_route (sent to every
        // connected peer, i.e. both `departing` and `staying`).
        while rx.try_recv().is_ok() {}

        // cleanup_peer_disconnect should revoke the route (my_id, departing -> false)
        // but the RoutingDelta must NOT be sent to the departing peer.
        router.cleanup_peer_disconnect(&departing);

        // The only message in the channel should be addressed to `staying`, not `departing`.
        let mut targets = Vec::new();
        while let Ok((target, msg)) = rx.try_recv() {
            assert!(
                target != departing,
                "Departing peer must not receive any message during cleanup, got {:?}",
                msg,
            );
            targets.push(target);
        }
        assert!(
            targets.contains(&staying),
            "Staying peer should have received the RoutingDelta, but got targets {:?}",
            targets,
        );
    }

    // -----------------------------------------------------------------------
    // set_connected_peers (poll timer sync)
    // -----------------------------------------------------------------------

    #[test]
    fn set_connected_peers_enables_broadcasts() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();
        // Peer must be a controller of us in the matrix for broadcast to reach them.
        router.apply_routing_delta(peer, 1, &[(peer, my_id, true)]);

        // Before sync: dispatch should produce no broadcast (empty connected_peers).
        router.dispatch(CalcAction::Digit(1));
        assert!(
            rx.try_recv().is_err(),
            "Empty connected_peers should not broadcast"
        );

        // Sync a peer set (simulates what the poll timer does).
        let mut peers = HashSet::new();
        peers.insert(peer);
        router.set_connected_peers(peers);

        // After sync: dispatch should broadcast to the synced peer.
        router.dispatch(CalcAction::Digit(2));
        let (target, msg) = rx.try_recv().expect("Expected broadcast after sync");
        assert_eq!(target, peer);
        assert!(matches!(msg, NetworkMessage::StateUpdate(_)));
    }

    #[test]
    fn set_connected_peers_replaces_old_set() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer_a = NodeId::new_v4();
        let peer_b = NodeId::new_v4();
        let my_id = router.local_node_id();
        // Both peers must be controllers of us in the matrix.
        router.apply_routing_delta(peer_a, 1, &[(peer_a, my_id, true)]);
        router.apply_routing_delta(peer_b, 1, &[(peer_b, my_id, true)]);

        // Initial sync with peer_a.
        let mut peers = HashSet::new();
        peers.insert(peer_a);
        router.set_connected_peers(peers);

        // Re-sync with peer_b only (peer_a removed).
        let mut peers = HashSet::new();
        peers.insert(peer_b);
        router.set_connected_peers(peers);

        router.dispatch(CalcAction::Digit(3));

        // Only peer_b should receive the broadcast.
        let (target, msg) = rx.try_recv().expect("Expected broadcast to peer_b");
        assert_eq!(target, peer_b);
        assert!(matches!(msg, NetworkMessage::StateUpdate(_)));
        assert!(
            rx.try_recv().is_err(),
            "peer_a should not receive broadcast after being replaced"
        );
    }

    #[test]
    fn set_connected_peers_empty_clears_broadcasts() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();
        // Peer must be a controller of us in the matrix.
        router.apply_routing_delta(peer, 1, &[(peer, my_id, true)]);

        // Add a peer, verify broadcast works.
        let mut peers = HashSet::new();
        peers.insert(peer);
        router.set_connected_peers(peers);
        router.dispatch(CalcAction::Digit(1));
        assert!(rx.try_recv().is_ok());

        // Clear all peers.
        router.set_connected_peers(HashSet::new());
        router.dispatch(CalcAction::Digit(2));
        assert!(
            rx.try_recv().is_err(),
            "Empty set should stop all broadcasts"
        );
    }

    // -----------------------------------------------------------------------
    // Config accessors
    // -----------------------------------------------------------------------

    #[test]
    fn default_config_is_local() {
        let (router, _calls) = make_router();
        let cfg = router.config();
        assert!(!cfg.allow_remote_control);
    }

    #[test]
    fn source_id_matches_local_node() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        let my_id = router.local_node_id();
        router.set_route(my_id, peer, true);
        // Drain the RoutingDelta broadcast from set_route.
        let _ = rx.try_recv();

        router.dispatch(CalcAction::Digit(0));

        let (_, msg) = rx.try_recv().unwrap();
        match msg {
            NetworkMessage::Action(env) => {
                assert_eq!(env.source_id, router.local_node_id());
            }
            other => panic!("Expected Action, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 6. Routing matrix integration
    // -----------------------------------------------------------------------

    #[test]
    fn matrix_set_route_updates_targets() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer = NodeId::new_v4();

        // Before setting a route, only self-control exists.
        let targets = router.my_control_targets();
        assert_eq!(targets.len(), 1);
        assert!(targets.contains(&router.local_node_id()));

        // Set a route to control the peer.
        let my_id = router.local_node_id();
        assert!(router.set_route(my_id, peer, true));

        // Now the peer should be in our control targets.
        let targets = router.my_control_targets();
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&peer));
    }

    #[test]
    fn matrix_set_route_rejects_other_rows() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer_a = NodeId::new_v4();
        let peer_b = NodeId::new_v4();

        // Cannot set a route for another node's row.
        assert!(!router.set_route(peer_a, peer_b, true));
    }

    #[test]
    fn matrix_routing_delta_applies_remote_row() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Peer sends a delta: they control us.
        router.apply_routing_delta(peer, 1, &[(peer, my_id, true)]);

        // Peer should now be in our controllers list.
        let controllers = router.my_controllers();
        assert!(controllers.contains(&peer));
        assert!(
            router
                .get_routing_matrix()
                .get(&(peer, my_id))
                .copied()
                .unwrap_or(false)
        );
    }

    #[test]
    fn matrix_routing_delta_rejects_mismatched_owner() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer_a = NodeId::new_v4();
        let peer_b = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Delta claims owner=peer_a but cell controller=peer_b -- should be skipped.
        router.apply_routing_delta(peer_a, 1, &[(peer_b, my_id, true)]);

        // peer_b should NOT be in our controllers.
        let controllers = router.my_controllers();
        assert!(!controllers.contains(&peer_b));
    }

    #[test]
    fn matrix_is_muted_when_controlling_remote() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer = NodeId::new_v4();

        // Not muted initially (only self-control).
        assert!(!router.is_muted());

        // Set a route to control the remote peer.
        let my_id = router.local_node_id();
        router.set_route(my_id, peer, true);

        // Now muted because we control a non-self executor.
        assert!(router.is_muted());
    }

    #[test]
    fn audio_muted_default_is_false() {
        let (router, _calls) = make_router();
        assert!(!router.is_audio_muted());
    }

    #[test]
    fn audio_muted_set_and_get() {
        let (router, _calls) = make_router();
        router.set_audio_muted(true);
        assert!(router.is_audio_muted());
        router.set_audio_muted(false);
        assert!(!router.is_audio_muted());
    }

    #[test]
    fn matrix_route_revoke_removes_route() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Set up a route.
        router.set_route(my_id, peer, true);
        assert!(router.my_control_targets().contains(&peer));

        // Revoke the route.
        router.handle_network_message(
            peer,
            NetworkMessage::RouteRevoke {
                from: my_id,
                to: peer,
                version: 2,
            },
        );

        // Route should be gone.
        assert!(!router.my_control_targets().contains(&peer));
    }

    #[test]
    fn remote_peer_revokes_own_route() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let remote_peer = NodeId::new_v4();
        let other_peer = NodeId::new_v4();
        router.add_remote_session(remote_peer);
        router.add_remote_session(other_peer);
        let my_id = router.local_node_id();

        // Remote peer controls us via a routing delta.
        router.apply_routing_delta(remote_peer, 1, &[(remote_peer, my_id, true)]);
        assert!(router.my_controllers().contains(&remote_peer));

        // Drain any messages.
        while rx.try_recv().is_ok() {}

        // Remote peer revokes its own route to us.
        router.handle_network_message(
            remote_peer,
            NetworkMessage::RouteRevoke {
                from: remote_peer,
                to: my_id,
                version: 2,
            },
        );

        // The route should be removed -- we are no longer controlled by remote_peer.
        assert!(
            !router.my_controllers().contains(&remote_peer),
            "Remote peer's route should be revoked"
        );

        // A RoutingDelta should have been broadcast to other_peer.
        let mut found_delta = false;
        while let Ok((target, msg)) = rx.try_recv() {
            if target == other_peer
                && let NetworkMessage::RoutingDelta { owner, cells, .. } = msg
            {
                assert_eq!(owner, remote_peer);
                assert_eq!(cells, vec![(remote_peer, my_id, false)]);
                found_delta = true;
            }
        }
        assert!(
            found_delta,
            "Expected RoutingDelta broadcast to other_peer after remote revoke"
        );
    }

    #[test]
    fn route_revoke_rejects_spoofed_sender() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let attacker = NodeId::new_v4();
        let my_id = router.local_node_id();

        router.add_remote_session(peer);
        router.set_route(my_id, peer, true);
        assert!(router.my_control_targets().contains(&peer));

        router.handle_network_message(
            attacker,
            NetworkMessage::RouteRevoke {
                from: my_id,
                to: peer,
                version: 2,
            },
        );

        assert!(
            router.my_control_targets().contains(&peer),
            "Third-party RouteRevoke should not revoke local route"
        );
    }

    #[test]
    fn matrix_is_controlled_by_check() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Not controlled by peer initially.
        assert!(
            !router
                .get_routing_matrix()
                .get(&(peer, my_id))
                .copied()
                .unwrap_or(false)
        );

        // Apply delta: peer controls us.
        router.apply_routing_delta(peer, 1, &[(peer, my_id, true)]);

        // Now controlled by peer.
        assert!(
            router
                .get_routing_matrix()
                .get(&(peer, my_id))
                .copied()
                .unwrap_or(false)
        );
    }

    #[test]
    fn matrix_full_sync_replaces_state() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer_a = NodeId::new_v4();
        let peer_b = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Apply a full sync with two entries.
        router.apply_routing_sync(&[(peer_a, my_id, true, 1), (my_id, peer_b, true, 1)]);

        let matrix = router.get_routing_matrix();
        assert!(matrix.get(&(peer_a, my_id)).copied().unwrap_or(false));
        assert!(matrix.get(&(my_id, peer_b)).copied().unwrap_or(false));
        // Self-control diagonal must be preserved even after a full sync.
        assert!(
            matrix.get(&(my_id, my_id)).copied().unwrap_or(false),
            "Local self-control diagonal must survive apply_sync"
        );
    }

    #[test]
    fn apply_sync_preserves_entries_from_unknown_peers() {
        // Regression test: when peer C sends a RoutingSync that does NOT
        // include entries for peer B, apply_sync must NOT destroy B's
        // entries.  Previously apply_sync cleared ALL entries before
        // inserting the sync payload, which wiped B's diagonal and any
        // routes involving B.
        let (router, _calls, _rx) = make_router_with_channel();
        let peer_b = NodeId::new_v4();
        let peer_c = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Establish B's presence: diagonal + a route from us to B.
        router.add_remote_session(peer_b);
        router.set_route(my_id, peer_b, true);
        assert!(
            router
                .get_routing_matrix()
                .get(&(peer_b, peer_b))
                .copied()
                .unwrap_or(false)
        );
        assert!(
            router
                .get_routing_matrix()
                .get(&(my_id, peer_b))
                .copied()
                .unwrap_or(false)
        );

        // C connects and sends a RoutingSync that only knows about C itself.
        // This simulates the scenario where C has never heard of B.
        router.add_remote_session(peer_c);
        router.apply_routing_sync(&[(peer_c, peer_c, true, 0), (my_id, my_id, true, 0)]);

        let matrix = router.get_routing_matrix();

        // B's diagonal MUST survive.
        assert!(
            matrix.get(&(peer_b, peer_b)).copied().unwrap_or(false),
            "B's diagonal was wiped by apply_sync from C -- entries from unknown peers must be preserved"
        );
        // Our route to B MUST survive.
        assert!(
            matrix.get(&(my_id, peer_b)).copied().unwrap_or(false),
            "Route (my_id -> B) was wiped by apply_sync from C -- local routes must be preserved"
        );
        // C's diagonal should be present.
        assert!(
            matrix.get(&(peer_c, peer_c)).copied().unwrap_or(false),
            "C's diagonal should be present after sync"
        );
    }

    #[test]
    fn apply_sync_does_not_downgrade_local_routes() {
        // If a sync contains a stale entry for the local row (lower version),
        // the local row must NOT be overwritten.
        let (router, _calls, _rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Set a route (bumps local version to 1).
        router.set_route(my_id, peer, true);
        assert!(
            router
                .get_routing_matrix()
                .get(&(my_id, peer))
                .copied()
                .unwrap_or(false)
        );

        // A sync arrives claiming our row has no route at version 0.
        // The local version (1) is higher, so the sync must be ignored.
        router.apply_routing_sync(&[]);

        let matrix = router.get_routing_matrix();
        assert!(
            matrix.get(&(my_id, peer)).copied().unwrap_or(false),
            "Stale sync must not overwrite a newer local route"
        );
    }

    #[test]
    fn apply_sync_accepts_newer_entries_for_local_row() {
        // If a sync contains a NEWER version of the local row (e.g. after
        // a restart when a peer echoes our previous state back), the sync
        // entries should be accepted.
        let (router, _calls, _rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Simulate receiving our own row from a peer at a higher version
        // (e.g. peer is echoing back a state we sent before a restart).
        router.apply_routing_sync(&[(my_id, peer, true, 5), (peer, peer, true, 0)]);

        let matrix = router.get_routing_matrix();
        assert!(
            matrix.get(&(my_id, peer)).copied().unwrap_or(false),
            "Sync with higher version for local row should be accepted"
        );
    }

    #[test]
    fn routing_sync_rejects_third_party_rows() {
        let (router, _calls, _rx) = make_router_with_channel();
        let sender = NodeId::new_v4();
        let third_party = NodeId::new_v4();
        let my_id = router.local_node_id();

        router.handle_network_message(
            sender,
            NetworkMessage::RoutingSync {
                entries: vec![(third_party, my_id, true, 1)],
            },
        );

        let matrix = router.get_routing_matrix();
        assert!(
            !matrix.get(&(third_party, my_id)).copied().unwrap_or(false),
            "RoutingSync must not let a peer write third-party rows"
        );
    }

    #[test]
    fn signed_row_announce_converges_across_asymmetric_three_node_topology() {
        let (router_a, _calls_a, mut rx_a) = make_router_with_channel();
        let (router_c, _calls_c, _rx_c) = make_router_with_channel();
        let identity_a = DeviceIdentity::generate();
        let identity_b = DeviceIdentity::generate();
        let identity_c = DeviceIdentity::generate();
        set_router_identity(&router_a, &identity_a);
        set_router_identity(&router_c, &identity_c);

        let a = identity_a.node_id();
        let b = identity_b.node_id();
        let c = identity_c.node_id();
        router_a.add_remote_session(b);
        router_a.add_remote_session(c);
        router_c.add_remote_session(a);

        let b_cells = vec![(b, b, true), (b, c, true)];
        router_a.handle_network_message(b, signed_row_message(&identity_b, 1, b_cells.clone()));

        assert!(
            router_a
                .get_routing_matrix()
                .get(&(b, c))
                .copied()
                .unwrap_or(false),
            "A should apply B's owner-signed row"
        );

        let mut forwarded = None;
        while let Ok((target, msg)) = rx_a.try_recv() {
            if target == c
                && matches!(msg, NetworkMessage::RoutingRowAnnounce { owner, .. } if owner == b)
            {
                forwarded = Some(msg);
                break;
            }
        }
        let forwarded = forwarded.expect("A should relay B's signed row to C");

        router_c.handle_network_message(a, forwarded);
        assert!(
            router_c
                .get_routing_matrix()
                .get(&(b, c))
                .copied()
                .unwrap_or(false),
            "C should accept B's row relayed by A after verifying B's signature"
        );
    }

    #[test]
    fn signed_row_announce_rejects_forged_third_party_owner() {
        let (router, _calls, _rx) = make_router_with_channel();
        let attacker_identity = DeviceIdentity::generate();
        let victim_identity = DeviceIdentity::generate();
        let attacker = attacker_identity.node_id();
        let victim = victim_identity.node_id();
        let my_id = router.local_node_id();
        let cells = vec![(victim, my_id, true)];
        let payload = Router::routing_row_signature_payload(victim, 1, &cells);
        let forged_signature = attacker_identity.sign(&payload).to_bytes().to_vec();

        router.handle_network_message(
            attacker,
            NetworkMessage::RoutingRowAnnounce {
                owner: victim,
                version: 1,
                cells,
                owner_public_key: attacker_identity.public_key_bytes(),
                signature: forged_signature,
            },
        );

        assert!(
            !router
                .get_routing_matrix()
                .get(&(victim, my_id))
                .copied()
                .unwrap_or(false),
            "attacker must not be able to modify a third-party row"
        );
    }

    #[test]
    fn send_route_revoke_is_noop_routing_delta_handles_notification() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        let my_id = router.local_node_id();

        // send_route_revoke is intentionally a no-op; the subsequent
        // set_route() call handles the RoutingDelta broadcast.
        router.send_route_revoke(peer);
        assert!(
            rx.try_recv().is_err(),
            "send_route_revoke should not send any message"
        );

        // set_route broadcasts the RoutingDelta to all connected peers.
        router.set_route(my_id, peer, false);
        let (target, msg) = rx.try_recv().expect("Expected RoutingDelta from set_route");
        assert_eq!(target, peer);
        assert!(matches!(msg, NetworkMessage::RoutingDelta { .. }));
    }

    // -----------------------------------------------------------------------
    // 7. Dispatch falls back to local while awaiting grant
    // -----------------------------------------------------------------------

    #[test]
    fn route_request_denied_when_remote_control_disabled() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let controller = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.add_remote_session(controller);

        router.handle_network_message(
            controller,
            NetworkMessage::RouteRequest {
                request_id: 42,
                controller,
                executor: my_id,
            },
        );

        let (target, msg) = rx.try_recv().expect("expected RouteDenied");
        assert_eq!(target, controller);
        match msg {
            NetworkMessage::RouteDenied {
                request_id,
                controller: c,
                executor,
                reason,
            } => {
                assert_eq!(request_id, 42);
                assert_eq!(c, controller);
                assert_eq!(executor, my_id);
                assert_eq!(reason, "remote_control_disabled");
            }
            other => panic!("expected RouteDenied, got {:?}", other),
        }
        assert!(!router.my_controllers().contains(&controller));
    }

    #[test]
    fn route_request_granted_when_remote_control_allowed() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let controller = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.set_allow_remote_control(true);
        router.add_remote_session(controller);
        trust_peer(&router, controller, [7u8; 32], DeviceTrust::Trusted);

        router.handle_network_message(
            controller,
            NetworkMessage::RouteRequest {
                request_id: 7,
                controller,
                executor: my_id,
            },
        );

        let (target, msg) = rx.try_recv().expect("expected RouteGrant");
        assert_eq!(target, controller);
        assert!(matches!(
            msg,
            NetworkMessage::RouteGrant {
                request_id: 7,
                controller: c,
                executor
            } if c == controller && executor == my_id
        ));
        assert!(router.my_controllers().contains(&controller));
    }

    #[test]
    fn route_request_from_new_verified_device_waits_for_user_approval() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let controller = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.set_allow_remote_control(true);
        router.add_remote_session(controller);
        router.set_remote_public_key(controller, [9u8; 32]);

        router.handle_network_message(
            controller,
            NetworkMessage::RouteRequest {
                request_id: 8,
                controller,
                executor: my_id,
            },
        );

        assert!(
            rx.try_recv().is_err(),
            "new verified device should wait for user approval"
        );
        assert!(
            router
                .pending_route_approval_controllers()
                .contains(&controller)
        );
        assert!(router.pending_pairing_devices().contains(&controller));
        assert!(!router.my_controllers().contains(&controller));
    }

    #[test]
    fn route_request_pending_when_device_asks_each_time() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let controller = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.set_allow_remote_control(true);
        router.add_remote_session(controller);
        trust_peer(&router, controller, [10u8; 32], DeviceTrust::AskEachTime);

        router.handle_network_message(
            controller,
            NetworkMessage::RouteRequest {
                request_id: 9,
                controller,
                executor: my_id,
            },
        );

        assert!(
            rx.try_recv().is_err(),
            "AskEachTime should not auto-grant or auto-deny"
        );
        assert!(
            router
                .pending_route_approval_controllers()
                .contains(&controller)
        );

        router.respond_to_pending_route_request(controller, true);
        let (target, msg) = rx.try_recv().expect("expected RouteGrant after approval");
        assert_eq!(target, controller);
        assert!(matches!(
            msg,
            NetworkMessage::RouteGrant { request_id: 9, .. }
        ));
        assert!(router.my_controllers().contains(&controller));
    }

    #[test]
    fn route_request_denied_when_device_blocked() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let controller = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.set_allow_remote_control(true);
        router.add_remote_session(controller);
        trust_peer(&router, controller, [11u8; 32], DeviceTrust::Blocked);

        router.handle_network_message(
            controller,
            NetworkMessage::RouteRequest {
                request_id: 10,
                controller,
                executor: my_id,
            },
        );

        let (_, msg) = rx.try_recv().expect("expected RouteDenied");
        assert!(matches!(
            msg,
            NetworkMessage::RouteDenied { reason, .. } if reason == "device_blocked"
        ));
        assert!(!router.my_controllers().contains(&controller));
    }

    #[test]
    fn route_request_denied_when_paired_public_key_mismatches_session() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let controller = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.set_allow_remote_control(true);
        router.add_remote_session(controller);
        router.set_paired_devices([(controller, [12u8; 32], DeviceTrust::Trusted)]);
        router.set_remote_public_key(controller, [13u8; 32]);

        router.handle_network_message(
            controller,
            NetworkMessage::RouteRequest {
                request_id: 11,
                controller,
                executor: my_id,
            },
        );

        let (_, msg) = rx.try_recv().expect("expected RouteDenied");
        assert!(matches!(
            msg,
            NetworkMessage::RouteDenied { reason, .. } if reason == "paired_key_mismatch"
        ));
        assert!(!router.my_controllers().contains(&controller));
    }

    #[test]
    fn pairing_request_records_pending_pairing_for_verified_session_key() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let local_identity = DeviceIdentity::generate();
        let remote_identity = DeviceIdentity::generate();
        set_router_identity(&router, &local_identity);
        let remote = remote_identity.node_id();
        router.add_remote_session(remote);
        router.set_remote_public_key(remote, remote_identity.public_key_bytes());

        router.handle_network_message(
            remote,
            NetworkMessage::PairingRequest {
                public_key: remote_identity.public_key_bytes(),
                pairing_code_hash: Router::pairing_code_hash(
                    remote_identity.public_key_bytes(),
                    local_identity.public_key_bytes(),
                ),
            },
        );

        assert!(router.pending_pairing_devices().contains(&remote));
        assert!(
            rx.try_recv().is_err(),
            "valid PairingRequest should not be rejected"
        );
    }

    #[test]
    fn pairing_request_rejects_code_hash_mismatch() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let local_identity = DeviceIdentity::generate();
        let remote_identity = DeviceIdentity::generate();
        set_router_identity(&router, &local_identity);
        let remote = remote_identity.node_id();
        router.add_remote_session(remote);
        router.set_remote_public_key(remote, remote_identity.public_key_bytes());

        router.handle_network_message(
            remote,
            NetworkMessage::PairingRequest {
                public_key: remote_identity.public_key_bytes(),
                pairing_code_hash: [99u8; 32],
            },
        );

        let (target, msg) = rx.try_recv().expect("expected PairingReject");
        assert_eq!(target, remote);
        assert!(matches!(msg, NetworkMessage::PairingReject));
        assert!(!router.pending_pairing_devices().contains(&remote));
    }

    #[test]
    fn pairing_confirm_verifies_signature_and_clears_pending_pairing() {
        let (router, _calls, _rx) = make_router_with_channel();
        let local_identity = DeviceIdentity::generate();
        let remote_identity = DeviceIdentity::generate();
        set_router_identity(&router, &local_identity);
        let remote = remote_identity.node_id();
        router.add_remote_session(remote);
        router.set_remote_public_key(remote, remote_identity.public_key_bytes());

        router.handle_network_message(
            remote,
            NetworkMessage::PairingRequest {
                public_key: remote_identity.public_key_bytes(),
                pairing_code_hash: Router::pairing_code_hash(
                    remote_identity.public_key_bytes(),
                    local_identity.public_key_bytes(),
                ),
            },
        );
        assert!(router.pending_pairing_devices().contains(&remote));

        let payload = Router::pairing_confirm_payload(
            remote_identity.public_key_bytes(),
            local_identity.public_key_bytes(),
        );
        router.handle_network_message(
            remote,
            NetworkMessage::PairingConfirm {
                signature: remote_identity.sign(&payload).to_bytes().to_vec(),
            },
        );

        assert!(!router.pending_pairing_devices().contains(&remote));
    }

    #[test]
    fn route_grant_clears_pending_request() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.add_remote_session(peer);
        router.set_route(my_id, peer, true);
        router.set_pending_control_request(peer);

        let mut request_id = None;
        while let Ok((_, msg)) = rx.try_recv() {
            if let NetworkMessage::RouteRequest { request_id: id, .. } = msg {
                request_id = Some(id);
            }
        }
        let request_id = request_id.expect("RouteRequest should be sent");

        router.handle_network_message(
            peer,
            NetworkMessage::RouteGrant {
                request_id,
                controller: my_id,
                executor: peer,
            },
        );

        assert!(!router.is_awaiting_grant());
        assert!(router.my_control_targets().contains(&peer));
    }

    #[test]
    fn route_denied_clears_pending_and_reverts_route() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.add_remote_session(peer);
        router.set_route(my_id, peer, true);
        router.set_pending_control_request(peer);

        let mut request_id = None;
        while let Ok((_, msg)) = rx.try_recv() {
            if let NetworkMessage::RouteRequest { request_id: id, .. } = msg {
                request_id = Some(id);
            }
        }
        let request_id = request_id.expect("RouteRequest should be sent");

        router.handle_network_message(
            peer,
            NetworkMessage::RouteDenied {
                request_id,
                controller: my_id,
                executor: peer,
                reason: "blocked".to_string(),
            },
        );

        assert!(!router.is_awaiting_grant());
        assert!(!router.my_control_targets().contains(&peer));
        assert_eq!(router.take_connection_error().as_deref(), Some("blocked"));
    }

    #[test]
    fn dispatch_falls_back_to_local_when_awaiting_grant() {
        let (router, calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        // Simulate the connect callback: route to remote + pending.
        let my_id = router.local_node_id();
        router.set_route(my_id, peer, true);
        router.set_pending_control_request(peer);
        // Drain setup messages (RoutingDelta + RouteRequest). The dispatch
        // below must not send an Action while the grant is pending.
        while rx.try_recv().is_ok() {}

        // Dispatch should execute locally (not send Action envelope) while pending.
        router.dispatch(CalcAction::Digit(5));

        // Local display should update via execute_local.
        let c = calls.borrow();
        assert!(
            c.displays.iter().any(|d| d == "5"),
            "Expected local display update while awaiting grant, got {:?}",
            c.displays
        );

        // No Action envelope should have been sent while awaiting grant.
        // (No StateUpdate either -- no peer controls us in the matrix.)
        assert!(
            rx.try_recv().is_err(),
            "Should not have sent any messages while awaiting grant"
        );
    }

    #[test]
    fn dispatch_uses_remote_after_pending_cleared() {
        let (router, calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        // Set route to remote peer via the matrix.
        let my_id = router.local_node_id();
        router.set_route(my_id, peer, true);
        router.set_pending_control_request(peer);

        // First dispatch: should be local (pending).
        router.dispatch(CalcAction::Digit(3));
        rx.try_recv().unwrap(); // drain StateUpdate

        // Simulate route setup completing: clear pending.
        router.clear_pending_control_request();

        // Second dispatch: should be remote now (pending cleared, matrix has target).
        router.dispatch(CalcAction::Digit(7));

        // Speculative echo updates display.  Calculator accumulated: "3" + "7" = "37".
        let c = calls.borrow();
        assert!(
            c.displays.iter().any(|d| d == "37"),
            "Expected display '37' after sequential digits, got {:?}",
            c.displays
        );

        // Should see an Action envelope (not just StateUpdate).
        let mut found_action = false;
        while let Ok((_, msg)) = rx.try_recv() {
            if matches!(msg, NetworkMessage::Action(_)) {
                found_action = true;
            }
        }
        assert!(
            found_action,
            "Expected Action envelope after pending cleared"
        );
    }

    #[test]
    fn exclusive_policy_rejects_non_primary_controller() {
        let (router, calls) = make_router();
        router.set_allow_remote_control(true);
        router.set_conflict_policy(ConflictPolicy::Exclusive);
        let primary = NodeId::new_v4();
        let secondary = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.add_remote_session(primary);
        router.add_remote_session(secondary);
        router.apply_routing_delta(primary, 1, &[(primary, my_id, true)]);
        router.apply_routing_delta(secondary, 2, &[(secondary, my_id, true)]);

        router.handle_remote_action(ActionEnvelope {
            seq: 1,
            source_id: primary,
            timestamp_ms: 0,
            action: CalcAction::Digit(1),
        });

        {
            let c = calls.borrow();
            assert!(
                c.displays.is_empty(),
                "Exclusive policy should reject non-primary controller, got {:?}",
                c.displays
            );
        }

        router.handle_remote_action(ActionEnvelope {
            seq: 2,
            source_id: secondary,
            timestamp_ms: 0,
            action: CalcAction::Digit(2),
        });

        let c = calls.borrow();
        assert!(
            c.displays.iter().any(|d| d == "2"),
            "Exclusive policy should accept the designated controller"
        );
    }

    #[test]
    fn route_grant_ignores_stale_request_id() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();
        router.add_remote_session(peer);
        router.set_route(my_id, peer, true);
        router.set_pending_control_request(peer);

        let mut request_id = None;
        while let Ok((_, msg)) = rx.try_recv() {
            if let NetworkMessage::RouteRequest { request_id: id, .. } = msg {
                request_id = Some(id);
            }
        }
        let request_id = request_id.expect("RouteRequest should be sent");

        router.handle_network_message(
            peer,
            NetworkMessage::RouteGrant {
                request_id: request_id.wrapping_add(1),
                controller: my_id,
                executor: peer,
            },
        );

        assert!(
            router.is_awaiting_grant(),
            "Stale RouteGrant must not clear the pending grant"
        );
    }
}
