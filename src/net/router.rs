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

use tokio::sync::mpsc;

use crate::core::action::CalcAction;
use crate::core::calculator::{CalcResult, Calculator};
use crate::net::protocol::*;
use crate::traits::{AudioPlayer, DisplayUpdater};

// ---------------------------------------------------------------------------
// Routing types
// ---------------------------------------------------------------------------

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

    /// Remove all matrix entries involving the given peer (both as
    /// controller and as executor).
    pub fn remove_peer(&mut self, node_id: &NodeId) {
        self.entries.retain(|(c, e), _| c != node_id && e != node_id);
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
                owner, version, current_version,
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
        let sync_controllers: HashSet<NodeId> =
            entries.iter().map(|(c, _, _, _)| *c).collect();

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
            audio_muted: false,
            last_state_update_seq: 0,
            last_connection_error: None,
        };
        Self {
            inner: Rc::new(RefCell::new(inner)),
        }
    }

    // ---- Configuration ---------------------------------------------------

    /// Attach a tokio runtime handle so the router can perform async operations
    /// from the synchronous slint callback context.
    pub fn set_runtime_handle(&self, handle: tokio::runtime::Handle) {
        self.inner.borrow_mut().runtime_handle = Some(handle);
    }

    /// Set the outgoing message channel that routes messages to the networking runtime.
    pub fn set_outgoing_tx(&self, tx: mpsc::UnboundedSender<(NodeId, NetworkMessage)>) {
        self.inner.borrow_mut().outgoing_tx = Some(tx);
    }

    /// Enable or disable acceptance of remote control actions.
    pub fn set_allow_remote_control(&self, allow: bool) {
        self.inner.borrow_mut().config.allow_remote_control = allow;
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

    /// Set a pending control request target. The poll timer will attempt
    /// to send the ControlRequest message when the session is ready.
    pub fn set_pending_control_request(&self, node_id: NodeId) {
        self.inner.borrow_mut().pending_control_request = Some(node_id);
    }

    /// Return the peer we are waiting for a ControlGrant from, if any.
    pub fn pending_control_request(&self) -> Option<NodeId> {
        self.inner.borrow().pending_control_request
    }

    /// Clear the pending control request (e.g. on disconnect or timeout).
    pub fn clear_pending_control_request(&self) {
        self.inner.borrow_mut().pending_control_request = None;
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
            &NetworkMessage::RouteRevoke { from, to, version: 0 },
        );
    }

    // ---- Remote session management ---------------------------------------

    /// Register a remote node as connected and add its diagonal to the
    /// routing matrix (self-control for the new peer).
    pub fn add_remote_session(&self, node_id: NodeId) {
        let mut inner = self.inner.borrow_mut();
        inner.connected_peers.insert(node_id);
        inner.routing_matrix.add_peer(node_id);
    }

    /// Remove a remote node from the connected set and purge all its
    /// routing matrix entries.
    pub fn remove_remote_session(&self, node_id: &NodeId) {
        let mut inner = self.inner.borrow_mut();
        inner.connected_peers.remove(node_id);
        inner.routing_matrix.remove_peer(node_id);
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
        let remote_targets: Vec<NodeId> = targets
            .iter()
            .copied()
            .filter(|id| *id != my_id)
            .collect();

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
            match inner.config.conflict_policy {
                ConflictPolicy::Exclusive => {
                    log::trace!(
                        "Exclusive policy: accepting action from controller {}",
                        envelope.source_id,
                    );
                }
                ConflictPolicy::Interleaved => {
                    // All actions from authorised controllers accepted,
                    // applied in arrival order.
                }
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
                self.handle_remote_action(envelope);
            }
            NetworkMessage::StateUpdate(snapshot) => {
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
                inner.display.update_memory_indicator(&snapshot.memory_indicator);
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
                self.handle_route_revoke(from, to, version);
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
                inner.routing_matrix.apply_delta(owner, version, &cells);
            }
            NetworkMessage::RoutingSync { entries } => {
                let mut inner = self.inner.borrow_mut();
                log::debug!("RoutingSync from {} ({} entries)", sender_id, entries.len());
                inner.routing_matrix.apply_sync(&entries);
                // If we were waiting for this peer to accept our connection,
                // receiving its RoutingSync is proof that the session is live
                // and the peer has processed our state.
                if let Some(pending) = inner.pending_control_request
                    && sender_id == pending
                {
                    inner.pending_control_request = None;
                    log::info!(
                        "Cleared pending control request: RoutingSync from target {}",
                        pending,
                    );
                }
            }
            NetworkMessage::ConnectionFailed { addr, reason, target_node_id } => {
                // Connection failure from the connect task. Revert the
                // pending route if it still matches the target, and store
                // the error for UI display.
                log::warn!("Connection failed to {} ({:?}): {}", addr, target_node_id, reason);

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
                            inner.routing_matrix.set_route(
                                my_id,
                                pending_peer,
                                false,
                            );
                            let version = inner.routing_matrix.row_versions
                                .get(&my_id).copied().unwrap_or(0);
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
                }
            }
            other => {
                log::debug!("Unhandled network message: {:?}", other);
            }
        }
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
        inner.display.update_memory_indicator(&result.memory_indicator);
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
        self.send_route_revoke(node_id);
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
            let version = inner.routing_matrix.row_versions.get(&controller).copied().unwrap_or(0);
            (ok, version)
        };
        if ok {
            self.broadcast_routing_delta(controller, version, &[(controller, executor, value)]);
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
            inner.routing_matrix.entries.insert((controller, executor), false);
        }
    }

    /// Send a full routing matrix snapshot to a specific peer.
    ///
    /// Called when a new session is established so the peer can initialise
    /// its local matrix from our current state.
    pub fn send_routing_sync_to(&self, node_id: NodeId) {
        let (entries, tx) = {
            let inner = self.inner.borrow();
            (inner.routing_matrix.sync_entries(), inner.outgoing_tx.clone())
        };
        if let Some(tx) = tx {
            let msg = NetworkMessage::RoutingSync { entries };
            let _ = tx.send((node_id, msg));
        }
    }

    /// Apply a remote routing delta to the local matrix.
    pub fn apply_routing_delta(&self, owner: NodeId, version: u64, cells: &[(NodeId, NodeId, bool)]) {
        self.inner.borrow_mut().routing_matrix.apply_delta(owner, version, cells);
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
    #[derive(Debug, Clone)]
    struct RecordedCalls {
        pub displays: Vec<String>,
        pub histories: Vec<String>,
        pub memory_indicators: Vec<String>,
        pub error_states: Vec<bool>,
    }

    impl Default for RecordedCalls {
        fn default() -> Self {
            Self {
                displays: Vec::new(),
                histories: Vec::new(),
                memory_indicators: Vec::new(),
                error_states: Vec::new(),
            }
        }
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

    // -----------------------------------------------------------------------
    // 4. handle_network_message for StateUpdate
    // -----------------------------------------------------------------------

    #[test]
    fn handle_network_message_state_update() {
        let (router, calls) = make_router();
        let snapshot = StateSnapshot {
            display: "42".to_string(),
            history: "40 +".to_string(),
            memory_indicator: "M".to_string(),
            is_error: false,
            last_seq_applied: 10,
        };
        router.handle_network_message(NodeId::new_v4(), NetworkMessage::StateUpdate(snapshot));

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
        let snapshot = StateSnapshot {
            display: "错误".to_string(),
            history: "不能除以零".to_string(),
            memory_indicator: String::new(),
            is_error: true,
            last_seq_applied: 5,
        };
        router.handle_network_message(NodeId::new_v4(), NetworkMessage::StateUpdate(snapshot));

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
        router.handle_network_message(NodeId::new_v4(), NetworkMessage::StateUpdate(snapshot));

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
        router.handle_network_message(NodeId::new_v4(), NetworkMessage::StateUpdate(snapshot));

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
        let (_, msg) = rx.try_recv().expect("Expected broadcast from local dispatch");
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
        assert!(router.get_routing_matrix().get(&(peer, my_id)).copied().unwrap_or(false));
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
        router.handle_network_message(peer, NetworkMessage::RouteRevoke {
            from: my_id,
            to: peer,
            version: 2,
        });

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
            if target == other_peer {
                if let NetworkMessage::RoutingDelta { owner, cells, .. } = msg {
                    assert_eq!(owner, remote_peer);
                    assert_eq!(cells, vec![(remote_peer, my_id, false)]);
                    found_delta = true;
                }
            }
        }
        assert!(
            found_delta,
            "Expected RoutingDelta broadcast to other_peer after remote revoke"
        );
    }

    #[test]
    fn matrix_is_controlled_by_check() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Not controlled by peer initially.
        assert!(!router.get_routing_matrix().get(&(peer, my_id)).copied().unwrap_or(false));

        // Apply delta: peer controls us.
        router.apply_routing_delta(peer, 1, &[(peer, my_id, true)]);

        // Now controlled by peer.
        assert!(router.get_routing_matrix().get(&(peer, my_id)).copied().unwrap_or(false));
    }

    #[test]
    fn matrix_full_sync_replaces_state() {
        let (router, _calls, _rx) = make_router_with_channel();
        let peer_a = NodeId::new_v4();
        let peer_b = NodeId::new_v4();
        let my_id = router.local_node_id();

        // Apply a full sync with two entries.
        router.apply_routing_sync(&[
            (peer_a, my_id, true, 1),
            (my_id, peer_b, true, 1),
        ]);

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
        assert!(router.get_routing_matrix().get(&(peer_b, peer_b)).copied().unwrap_or(false));
        assert!(router.get_routing_matrix().get(&(my_id, peer_b)).copied().unwrap_or(false));

        // C connects and sends a RoutingSync that only knows about C itself.
        // This simulates the scenario where C has never heard of B.
        router.add_remote_session(peer_c);
        router.apply_routing_sync(&[
            (peer_c, peer_c, true, 0),
            (my_id, my_id, true, 0),
        ]);

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
        assert!(router.get_routing_matrix().get(&(my_id, peer)).copied().unwrap_or(false));

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
        router.apply_routing_sync(&[
            (my_id, peer, true, 5),
            (peer, peer, true, 0),
        ]);

        let matrix = router.get_routing_matrix();
        assert!(
            matrix.get(&(my_id, peer)).copied().unwrap_or(false),
            "Sync with higher version for local row should be accepted"
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
        assert!(rx.try_recv().is_err(), "send_route_revoke should not send any message");

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
    fn dispatch_falls_back_to_local_when_awaiting_grant() {
        let (router, calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        // Simulate the connect callback: route to remote + pending.
        let my_id = router.local_node_id();
        router.set_route(my_id, peer, true);
        router.set_pending_control_request(peer);
        // Drain the RoutingDelta broadcast from set_route.
        let _ = rx.try_recv();

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
        assert!(c.displays.iter().any(|d| d == "37"),
            "Expected display '37' after sequential digits, got {:?}", c.displays);

        // Should see an Action envelope (not just StateUpdate).
        let mut found_action = false;
        while let Ok((_, msg)) = rx.try_recv() {
            if matches!(msg, NetworkMessage::Action(_)) {
                found_action = true;
            }
        }
        assert!(found_action, "Expected Action envelope after pending cleared");
    }
}
