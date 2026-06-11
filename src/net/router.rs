//! Control/execution routing layer for the Vocal Calculator.
//!
//! The [`Router`] wraps the calculator engine, audio subsystem, and UI window,
//! dispatching actions to the local engine or a remote node depending on the
//! current [`ExecutionTarget`]. It also handles inbound remote actions and
//! broadcasts state snapshots to all connected controllers.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use tokio::sync::mpsc;

use crate::audio::VocalAudio;
use crate::core::action::CalcAction;
use crate::core::calculator::{CalcResult, Calculator};
use crate::net::protocol::*;
use crate::net::CalculatorWindow;

// ---------------------------------------------------------------------------
// Routing types
// ---------------------------------------------------------------------------

/// Where to execute a calculator action.
#[derive(Debug, Clone)]
pub enum ExecutionTarget {
    /// Execute on this node's local calculator engine.
    Local,
    /// Forward the action to a remote node for execution.
    Remote(NodeId),
}

/// Configuration that controls how the router dispatches actions.
#[derive(Debug, Clone)]
pub struct RoutingConfig {
    pub execution_target: ExecutionTarget,
    pub allow_remote_control: bool,
    pub conflict_policy: ConflictPolicy,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            execution_target: ExecutionTarget::Local,
            allow_remote_control: true,
            conflict_policy: ConflictPolicy::Interleaved,
        }
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
    audio: Rc<RefCell<Option<VocalAudio>>>,
    window: CalculatorWindow,
    local_node_id: NodeId,
    config: RoutingConfig,
    /// Set of connected remote peer node IDs.
    connected_peers: HashSet<NodeId>,
    /// Channel to the networking runtime for sending messages to specific peers.
    outgoing_tx: Option<mpsc::UnboundedSender<(NodeId, NetworkMessage)>>,
    /// Monotonically increasing sequence counter for outbound envelopes.
    local_seq: u64,
    /// Tokio runtime handle for driving async operations from the sync UI thread.
    runtime_handle: Option<tokio::runtime::Handle>,
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
    /// The router will execute all actions on the local calculator until
    /// reconfigured via [`set_execution_target`](Self::set_execution_target).
    pub fn new(
        calculator: Rc<RefCell<Calculator>>,
        audio: Rc<RefCell<Option<VocalAudio>>>,
        window: CalculatorWindow,
    ) -> Self {
        let inner = RouterInner {
            calculator,
            audio,
            window,
            local_node_id: NodeId::new_v4(),
            config: RoutingConfig::default(),
            connected_peers: HashSet::new(),
            outgoing_tx: None,
            local_seq: 0,
            runtime_handle: None,
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

    /// Set the execution target for calculator actions (local or remote).
    pub fn set_execution_target(&self, target: ExecutionTarget) {
        self.inner.borrow_mut().config.execution_target = target;
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

    // ---- Remote session management ---------------------------------------

    /// Register a remote node as connected.
    pub fn add_remote_session(&self, node_id: NodeId) {
        self.inner.borrow_mut().connected_peers.insert(node_id);
    }

    /// Remove a remote node from the connected set.
    pub fn remove_remote_session(&self, node_id: &NodeId) {
        self.inner.borrow_mut().connected_peers.remove(node_id);
    }

    /// Returns `true` if a session is registered for the given node.
    pub fn has_remote_session(&self, node_id: &NodeId) -> bool {
        self.inner.borrow().connected_peers.contains(node_id)
    }

    // ---- Dispatch (UI entry point) ---------------------------------------

    /// Dispatch a calculator action, routing according to the current
    /// [`ExecutionTarget`].
    pub fn dispatch(&self, action: CalcAction) {
        let target = self.inner.borrow().config.execution_target.clone();

        match target {
            ExecutionTarget::Local => {
                self.execute_local(action);
            }
            ExecutionTarget::Remote(node_id) => {
                // Speculative local echo: apply the action to the local
                // calculator and UI immediately so the user sees instant
                // feedback. The remote executor's authoritative StateUpdate
                // will overwrite this if there is a disagreement.
                self.apply_speculative(action);

                // Build and send the ActionEnvelope to the remote executor.
                let envelope = self.build_envelope(action);
                self.send_to_remote(node_id, envelope);
            }
        }
    }

    // ---- Remote action handling (network entry points) --------------------

    /// Handle an [`ActionEnvelope`] received from a remote controller.
    ///
    /// The networking layer should call this when an action arrives on a
    /// subscribed session.
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

        // -- Conflict policy check ------------------------------------------
        {
            let inner = self.inner.borrow();
            match inner.config.conflict_policy {
                ConflictPolicy::Exclusive => {
                    log::trace!(
                        "Exclusive policy: accepting action from {}",
                        envelope.source_id,
                    );
                }
                ConflictPolicy::Interleaved => {
                    // All actions accepted, applied in arrival order.
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

        // -- Broadcast state to all connected controllers ------------------
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
    pub fn handle_network_message(&self, msg: NetworkMessage) {
        match msg {
            NetworkMessage::Action(envelope) => {
                self.handle_remote_action(envelope);
            }
            NetworkMessage::StateUpdate(snapshot) => {
                // Authoritative state from the executing node -- apply to UI.
                let inner = self.inner.borrow();
                inner
                    .window
                    .set_display_text(snapshot.display.clone().into());
                inner
                    .window
                    .set_history_text(snapshot.history.clone().into());
                inner
                    .window
                    .set_memory_indicator(snapshot.memory_indicator.clone().into());
                inner.window.set_error_state(snapshot.is_error);
            }
            NetworkMessage::Ping => {
                // Ping/Pong is now handled by the session task directly.
                log::trace!("Received Ping in Router (should have been handled by session)");
            }
            NetworkMessage::Pong => {
                // Pong is handled by the session task's heartbeat tracker.
                log::trace!("Received Pong in Router (should have been handled by session)");
            }
            NetworkMessage::ControlRequest
            | NetworkMessage::ControlGrant(_)
            | NetworkMessage::ControlRelease => {
                log::debug!("Control arbitration message ignored: {:?}", msg);
            }
            other => {
                log::debug!("Unhandled network message: {:?}", other);
            }
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
    fn apply_result(&self, result: &CalcResult) {
        let inner = self.inner.borrow();
        inner
            .window
            .set_display_text(result.display.clone().into());
        inner
            .window
            .set_history_text(result.history.clone().into());
        inner
            .window
            .set_memory_indicator(result.memory_indicator.clone().into());
        inner.window.set_error_state(result.is_error);
        if let Some(ref mut audio) = *inner.audio.borrow_mut() {
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

    /// Broadcast a state snapshot to every connected remote session.
    fn broadcast_state(&self, snapshot: &StateSnapshot) {
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
