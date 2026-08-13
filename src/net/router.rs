//! Calculator-first remote execution routing.
//!
//! A controller may select at most one remote executor. On the receiving
//! side, the persisted `allow_remote_control` switch is the only permission
//! boundary: an authenticated, active session may submit validated calculator
//! actions while the switch is enabled, and is rejected immediately while it
//! is disabled. There is deliberately no per-device trust state, approval
//! prompt, grant ledger, or distributed routing matrix.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::action::CalcAction;
use crate::core::calculator::{CalcResult, Calculator};
use crate::net::protocol::{ActionEnvelope, NetworkMessage, NodeId, StateSnapshot};
use crate::traits::{AudioPlayer, DisplayUpdater};

/// Maximum number of authenticated peers represented in product-level state.
/// The transport applies its own limits as well; this is a second boundary so
/// discovery/session churn cannot grow Router maps without bound.
const MAX_TRACKED_REMOTE_PEERS: usize = 64;
/// Calculator buttons are tiny discrete actions. This permits bursts far above
/// normal human input while bounding a compromised peer's work amplification.
const MAX_REMOTE_ACTIONS_PER_SECOND: u32 = 120;
const MAX_ACTION_AGE: Duration = Duration::from_secs(5 * 60);
const MAX_ACTION_FUTURE_SKEW: Duration = Duration::from_secs(30);
const MAX_DISPLAY_BYTES: usize = 64;
const MAX_HISTORY_BYTES: usize = 256;
const MAX_MEMORY_INDICATOR_BYTES: usize = 8;

#[derive(Debug, Clone, Copy)]
struct RateWindow {
    started: Instant,
    count: u32,
}

#[derive(Debug, Clone, Default)]
pub struct RoutingConfig {
    /// The single persisted permission boundary for inbound remote actions.
    pub allow_remote_control: bool,
}

pub struct Router {
    inner: Rc<RefCell<RouterInner>>,
}

struct RouterInner {
    calculator: Rc<RefCell<Calculator>>,
    audio: Option<Box<dyn AudioPlayer>>,
    display: Box<dyn DisplayUpdater>,
    local_node_id: NodeId,
    config: RoutingConfig,
    connected_peers: HashSet<NodeId>,
    peer_public_keys: HashMap<NodeId, [u8; 32]>,
    outgoing_tx: Option<mpsc::Sender<(NodeId, NetworkMessage)>>,
    runtime_handle: Option<tokio::runtime::Handle>,
    /// The controller's one selected executor. It may be selected before its
    /// TCP connection completes; dispatch safely falls back to local until the
    /// authenticated session is active.
    active_executor: Option<NodeId>,
    /// Controllers that have submitted at least one accepted action on their
    /// current session. This is display state, not an authorization list.
    active_controllers: HashSet<NodeId>,
    /// Per authenticated session replay boundary.
    last_remote_seq: HashMap<NodeId, u64>,
    remote_rate_windows: HashMap<NodeId, RateWindow>,
    local_seq: u64,
    last_state_update_seq: u64,
    audio_muted: bool,
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
    pub fn new(
        calculator: Rc<RefCell<Calculator>>,
        audio: Option<Box<dyn AudioPlayer>>,
        display: Box<dyn DisplayUpdater>,
    ) -> Self {
        Self {
            inner: Rc::new(RefCell::new(RouterInner {
                calculator,
                audio,
                display,
                local_node_id: NodeId::new_v4(),
                config: RoutingConfig::default(),
                connected_peers: HashSet::new(),
                peer_public_keys: HashMap::new(),
                outgoing_tx: None,
                runtime_handle: None,
                active_executor: None,
                active_controllers: HashSet::new(),
                last_remote_seq: HashMap::new(),
                remote_rate_windows: HashMap::new(),
                local_seq: 0,
                last_state_update_seq: 0,
                audio_muted: false,
                last_connection_error: None,
            })),
        }
    }

    pub fn set_runtime_handle(&self, handle: tokio::runtime::Handle) {
        self.inner.borrow_mut().runtime_handle = Some(handle);
    }

    pub fn set_outgoing_tx(&self, tx: mpsc::Sender<(NodeId, NetworkMessage)>) {
        self.inner.borrow_mut().outgoing_tx = Some(tx);
    }

    pub fn config(&self) -> RoutingConfig {
        self.inner.borrow().config.clone()
    }

    pub fn local_node_id(&self) -> NodeId {
        self.inner.borrow().local_node_id
    }

    pub fn set_local_node_id(&self, id: NodeId) {
        self.inner.borrow_mut().local_node_id = id;
    }

    /// Enable or disable inbound remote actions immediately.
    ///
    /// Disabling also clears transient "controlled remotely" display state,
    /// but deliberately retains the last accepted sequence for each live
    /// session so toggling the switch cannot make an old action replayable.
    pub fn set_allow_remote_control(&self, allow: bool) {
        let mut inner = self.inner.borrow_mut();
        inner.config.allow_remote_control = allow;
        if !allow {
            inner.active_controllers.clear();
            inner.remote_rate_windows.clear();
        }
    }

    pub fn set_remote_public_key(&self, node_id: NodeId, public_key: [u8; 32]) {
        let mut inner = self.inner.borrow_mut();
        if inner.connected_peers.contains(&node_id) {
            inner.peer_public_keys.insert(node_id, public_key);
        }
    }

    pub fn remote_public_key(&self, node_id: &NodeId) -> Option<[u8; 32]> {
        self.inner.borrow().peer_public_keys.get(node_id).copied()
    }

    /// Add an authenticated session to product-level state.
    pub fn add_remote_session(&self, node_id: NodeId) -> bool {
        let mut inner = self.inner.borrow_mut();
        if !inner.connected_peers.contains(&node_id)
            && inner.connected_peers.len() >= MAX_TRACKED_REMOTE_PEERS
        {
            log::warn!(
                "Ignoring peer {}: product peer limit {} reached",
                node_id,
                MAX_TRACKED_REMOTE_PEERS
            );
            return false;
        }
        inner.connected_peers.insert(node_id);
        // A new authenticated session is a new replay epoch. Session identity
        // is still bound to the Ed25519 key by the handshake layer.
        inner.last_remote_seq.remove(&node_id);
        inner.remote_rate_windows.remove(&node_id);
        true
    }

    pub fn remove_remote_session(&self, node_id: &NodeId) {
        let mut inner = self.inner.borrow_mut();
        inner.connected_peers.remove(node_id);
        inner.peer_public_keys.remove(node_id);
        inner.active_controllers.remove(node_id);
        inner.last_remote_seq.remove(node_id);
        inner.remote_rate_windows.remove(node_id);
        if inner.active_executor == Some(*node_id) {
            inner.active_executor = None;
            inner.last_state_update_seq = 0;
        }
    }

    pub fn cleanup_peer_disconnect(&self, node_id: &NodeId) {
        self.remove_remote_session(node_id);
    }

    pub fn has_remote_session(&self, node_id: &NodeId) -> bool {
        self.inner.borrow().connected_peers.contains(node_id)
    }

    pub fn set_connected_peers(&self, peers: HashSet<NodeId>) {
        let mut bounded: HashSet<NodeId> =
            peers.into_iter().take(MAX_TRACKED_REMOTE_PEERS).collect();
        let mut inner = self.inner.borrow_mut();
        if inner
            .active_executor
            .is_some_and(|target| !bounded.contains(&target))
        {
            inner.active_executor = None;
            inner.last_state_update_seq = 0;
        }
        inner.active_controllers.retain(|id| bounded.contains(id));
        inner.peer_public_keys.retain(|id, _| bounded.contains(id));
        inner.last_remote_seq.retain(|id, _| bounded.contains(id));
        inner
            .remote_rate_windows
            .retain(|id, _| bounded.contains(id));
        inner.connected_peers.clear();
        inner.connected_peers.extend(bounded.drain());
    }

    /// Select exactly one executor. Selection itself has no authorization
    /// exchange; an existing authenticated session makes it active instantly.
    pub fn select_remote_executor(&self, node_id: NodeId) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.active_executor = Some(node_id);
        inner.last_state_update_seq = 0;
        inner.connected_peers.contains(&node_id)
    }

    pub fn clear_remote_executor(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.active_executor = None;
        inner.last_state_update_seq = 0;
    }

    pub fn clear_remote_executor_if(&self, node_id: NodeId) {
        if self.inner.borrow().active_executor == Some(node_id) {
            self.clear_remote_executor();
        }
    }

    pub fn active_remote_executor(&self) -> Option<NodeId> {
        self.inner.borrow().active_executor
    }

    pub fn is_executing_remotely(&self) -> bool {
        let inner = self.inner.borrow();
        inner
            .active_executor
            .is_some_and(|id| inner.connected_peers.contains(&id))
    }

    pub fn active_remote_controllers(&self) -> Vec<NodeId> {
        self.inner
            .borrow()
            .active_controllers
            .iter()
            .copied()
            .collect()
    }

    pub fn set_audio_muted(&self, muted: bool) {
        self.inner.borrow_mut().audio_muted = muted;
    }

    pub fn is_audio_muted(&self) -> bool {
        self.inner.borrow().audio_muted
    }

    /// Routing mute is a presentation concern: controller audio is suppressed
    /// while one connected remote executor is active.
    pub fn is_muted(&self) -> bool {
        self.is_executing_remotely()
    }

    pub fn dispatch(&self, action: CalcAction) {
        if !Self::valid_action(action) {
            log::warn!("Rejected invalid local calculator action: {:?}", action);
            return;
        }
        let remote_target = {
            let inner = self.inner.borrow();
            inner
                .active_executor
                .filter(|id| inner.connected_peers.contains(id))
        };
        if let Some(target) = remote_target {
            let envelope = self.build_envelope(action);
            if self.send_message_to(target, NetworkMessage::Action(envelope)) {
                self.apply_speculative(action);
            } else {
                // Dropping a calculator button would be user-visible. On
                // bounded-channel overload, stop using the saturated remote
                // target and execute this action locally exactly once.
                self.clear_remote_executor_if(target);
                self.execute_local(action);
            }
        } else {
            self.execute_local(action);
        }
    }

    pub fn handle_network_message(&self, sender_id: NodeId, msg: NetworkMessage) {
        match msg {
            NetworkMessage::Action(envelope) => self.handle_remote_action(sender_id, envelope),
            NetworkMessage::StateUpdate(snapshot) => self.handle_remote_state(sender_id, snapshot),
            NetworkMessage::Ping | NetworkMessage::Pong => {
                log::trace!("Heartbeat message reached Router after session handling")
            }
            NetworkMessage::PeerNameUpdate { .. } => {}
            NetworkMessage::ConnectionFailed { target_node_id, .. } => {
                // `handle_network_message` is exclusively a remote ingress
                // API. ConnectionFailed is local-only and has no valid remote
                // source/target combination, even if the sender owns an
                // authenticated session.
                log::warn!(
                    "Rejected local-only ConnectionFailed from {sender_id} for {target_node_id:?}"
                );
            }
            // These messages retain their v5 wire discriminants for compatibility
            // with already-built peers. The calculator-first product model does
            // not act on them and never emits them.
            NetworkMessage::RoutingDelta { .. }
            | NetworkMessage::RoutingSync { .. }
            | NetworkMessage::RoutingRowRequest { .. }
            | NetworkMessage::RoutingRowAnnounce { .. }
            | NetworkMessage::RouteRevoke { .. }
            | NetworkMessage::RouteRequest { .. }
            | NetworkMessage::RouteGrant { .. }
            | NetworkMessage::RouteDenied { .. }
            | NetworkMessage::RouteRelease { .. }
            | NetworkMessage::PairingRequest { .. }
            | NetworkMessage::PairingConfirm { .. }
            | NetworkMessage::PairingReject => {
                log::debug!("Ignored deprecated v5 authorization/routing message from {sender_id}")
            }
            NetworkMessage::Hello { .. }
            | NetworkMessage::HelloAck { .. }
            | NetworkMessage::Subscribe
            | NetworkMessage::Unsubscribe
            | NetworkMessage::AuthChallenge { .. }
            | NetworkMessage::AuthProof { .. } => {
                log::debug!("Ignored session-control message delivered to Router")
            }
        }
    }

    fn handle_remote_action(&self, sender_id: NodeId, envelope: ActionEnvelope) {
        let rejection = {
            let mut inner = self.inner.borrow_mut();
            if sender_id != envelope.source_id {
                Some("source_identity_mismatch")
            } else if !inner.connected_peers.contains(&sender_id) {
                Some("no_authenticated_session")
            } else if !inner.config.allow_remote_control {
                Some("remote_control_disabled")
            } else if !Self::valid_action(envelope.action) {
                Some("invalid_action_schema")
            } else if !Self::valid_action_timestamp(envelope.timestamp_ms) {
                Some("invalid_action_timestamp")
            } else if envelope.seq == 0
                || inner
                    .last_remote_seq
                    .get(&sender_id)
                    .is_some_and(|last| envelope.seq <= *last)
            {
                Some("replayed_or_stale_action")
            } else if !Self::consume_rate_budget(&mut inner, sender_id) {
                Some("remote_action_rate_limited")
            } else {
                inner.last_remote_seq.insert(sender_id, envelope.seq);
                inner.active_controllers.insert(sender_id);
                None
            }
        };
        if let Some(reason) = rejection {
            log::warn!(
                "Rejected remote action seq={} from {}: {}",
                envelope.seq,
                sender_id,
                reason
            );
            return;
        }

        let result = {
            let inner = self.inner.borrow();
            inner.calculator.borrow_mut().dispatch(envelope.action)
        };
        self.apply_result(&result);
        let snapshot = Self::build_state_snapshot(&result, envelope.seq);
        let _ = self.send_message_to(sender_id, NetworkMessage::StateUpdate(snapshot));
    }

    fn handle_remote_state(&self, sender_id: NodeId, snapshot: StateSnapshot) {
        let accepted = {
            let mut inner = self.inner.borrow_mut();
            if inner.active_executor != Some(sender_id)
                || !inner.connected_peers.contains(&sender_id)
                || !Self::valid_state_snapshot(&snapshot)
                || snapshot.last_seq_applied == 0
                || snapshot.last_seq_applied > inner.local_seq
                || snapshot.last_seq_applied <= inner.last_state_update_seq
            {
                false
            } else {
                inner.last_state_update_seq = snapshot.last_seq_applied;
                true
            }
        };
        if !accepted {
            log::warn!("Rejected invalid or stale StateUpdate from {}", sender_id);
            return;
        }

        let calculator = Rc::clone(&self.inner.borrow().calculator);
        calculator.borrow_mut().reset_from_snapshot(
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

    fn consume_rate_budget(inner: &mut RouterInner, sender_id: NodeId) -> bool {
        let now = Instant::now();
        let window = inner
            .remote_rate_windows
            .entry(sender_id)
            .or_insert(RateWindow {
                started: now,
                count: 0,
            });
        if now.duration_since(window.started) >= Duration::from_secs(1) {
            window.started = now;
            window.count = 0;
        }
        if window.count >= MAX_REMOTE_ACTIONS_PER_SECOND {
            return false;
        }
        window.count += 1;
        true
    }

    fn valid_action(action: CalcAction) -> bool {
        !matches!(action, CalcAction::Digit(digit) if digit > 9)
    }

    fn valid_action_timestamp(timestamp_ms: u64) -> bool {
        let now_ms = Self::timestamp_ms();
        if timestamp_ms > now_ms {
            timestamp_ms - now_ms <= MAX_ACTION_FUTURE_SKEW.as_millis() as u64
        } else {
            now_ms - timestamp_ms <= MAX_ACTION_AGE.as_millis() as u64
        }
    }

    fn valid_state_snapshot(snapshot: &StateSnapshot) -> bool {
        snapshot.display.len() <= MAX_DISPLAY_BYTES
            && snapshot.history.len() <= MAX_HISTORY_BYTES
            && snapshot.memory_indicator.len() <= MAX_MEMORY_INDICATOR_BYTES
            && !snapshot.display.chars().any(char::is_control)
            && !snapshot.history.chars().any(char::is_control)
            && !snapshot.memory_indicator.chars().any(char::is_control)
            && matches!(snapshot.memory_indicator.as_str(), "" | "M")
            && if snapshot.is_error {
                snapshot.display == "错误"
            } else {
                Decimal::from_str(&snapshot.display).is_ok()
            }
    }

    fn execute_local(&self, action: CalcAction) {
        let result = {
            let inner = self.inner.borrow();
            inner.calculator.borrow_mut().dispatch(action)
        };
        self.apply_result(&result);
    }

    fn apply_speculative(&self, action: CalcAction) {
        let result = {
            let inner = self.inner.borrow();
            inner.calculator.borrow_mut().dispatch(action)
        };
        self.apply_result(&result);
    }

    fn apply_result(&self, result: &CalcResult) {
        let mut inner = self.inner.borrow_mut();
        inner.display.update_display(&result.display);
        inner.display.update_history(&result.history);
        inner
            .display
            .update_memory_indicator(&result.memory_indicator);
        inner.display.set_error_state(result.is_error);
        if !inner.audio_muted
            && let Some(audio) = inner.audio.as_mut()
        {
            audio.play_events(&result.events);
        }
    }

    fn build_envelope(&self, action: CalcAction) -> ActionEnvelope {
        let mut inner = self.inner.borrow_mut();
        inner.local_seq = inner.local_seq.saturating_add(1);
        ActionEnvelope {
            seq: inner.local_seq,
            source_id: inner.local_node_id,
            timestamp_ms: Self::timestamp_ms(),
            action,
        }
    }

    fn build_state_snapshot(result: &CalcResult, seq: u64) -> StateSnapshot {
        StateSnapshot {
            display: result.display.clone(),
            history: result.history.clone(),
            memory_indicator: result.memory_indicator.clone(),
            is_error: result.is_error,
            last_seq_applied: seq,
        }
    }

    fn send_message_to(&self, node_id: NodeId, msg: NetworkMessage) -> bool {
        let tx = self.inner.borrow().outgoing_tx.clone();
        match tx {
            Some(tx) => match tx.try_send((node_id, msg)) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // Drop newest. `dispatch` falls back locally for Actions;
                    // response snapshots are replaceable by the next one.
                    log::warn!("Router outgoing queue is full; dropping newest message");
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    log::warn!("Router outgoing channel is closed");
                    false
                }
            },
            None => {
                log::trace!("No outgoing channel configured for node {}", node_id);
                false
            }
        }
    }

    pub fn take_connection_error(&self) -> Option<String> {
        self.inner.borrow_mut().last_connection_error.take()
    }

    fn timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioMode;
    use crate::core::token::VocalEvent;

    #[derive(Debug, Default)]
    struct RecordedCalls {
        displays: Vec<String>,
        histories: Vec<String>,
        memory_indicators: Vec<String>,
        error_states: Vec<bool>,
    }

    struct MockDisplayUpdater(Rc<RefCell<RecordedCalls>>);

    impl DisplayUpdater for MockDisplayUpdater {
        fn update_display(&self, text: &str) {
            self.0.borrow_mut().displays.push(text.to_string());
        }
        fn update_history(&self, text: &str) {
            self.0.borrow_mut().histories.push(text.to_string());
        }
        fn update_memory_indicator(&self, indicator: &str) {
            self.0
                .borrow_mut()
                .memory_indicators
                .push(indicator.to_string());
        }
        fn set_error_state(&self, is_error: bool) {
            self.0.borrow_mut().error_states.push(is_error);
        }
    }

    struct MockAudioPlayer;

    impl AudioPlayer for MockAudioPlayer {
        fn play_events(&mut self, _events: &[VocalEvent]) {}
        fn set_mode(&mut self, _mode: AudioMode) {}
        fn set_volume(&mut self, _slider: f64) {}
        fn mode(&self) -> AudioMode {
            AudioMode::Normal
        }
    }

    fn make_router() -> (Router, Rc<RefCell<RecordedCalls>>) {
        let calls = Rc::new(RefCell::new(RecordedCalls::default()));
        let router = Router::new(
            Rc::new(RefCell::new(Calculator::new())),
            Some(Box::new(MockAudioPlayer)),
            Box::new(MockDisplayUpdater(calls.clone())),
        );
        (router, calls)
    }

    fn make_router_with_channel() -> (
        Router,
        Rc<RefCell<RecordedCalls>>,
        mpsc::Receiver<(NodeId, NetworkMessage)>,
    ) {
        let (router, calls) = make_router();
        let (tx, rx) = mpsc::channel(crate::net::OUTGOING_MESSAGE_CAPACITY);
        router.set_outgoing_tx(tx);
        (router, calls, rx)
    }

    fn envelope(source_id: NodeId, seq: u64, action: CalcAction) -> ActionEnvelope {
        ActionEnvelope {
            seq,
            source_id,
            timestamp_ms: Router::timestamp_ms(),
            action,
        }
    }

    fn enable_peer(router: &Router, peer: NodeId) {
        assert!(router.add_remote_session(peer));
        router.set_allow_remote_control(true);
    }

    #[test]
    fn local_calculator_remains_default() {
        let (router, calls) = make_router();
        router.dispatch(CalcAction::Digit(5));
        assert_eq!(
            calls.borrow().displays.last().map(String::as_str),
            Some("5")
        );
    }

    #[test]
    fn one_selected_executor_replaces_the_previous_target() {
        let (router, _calls, mut rx) = make_router_with_channel();
        let first = NodeId::new_v4();
        let second = NodeId::new_v4();
        router.add_remote_session(first);
        router.add_remote_session(second);
        assert!(router.select_remote_executor(first));
        assert!(router.select_remote_executor(second));

        router.dispatch(CalcAction::Digit(3));
        let (target, message) = rx.try_recv().unwrap();
        assert_eq!(target, second);
        assert!(matches!(message, NetworkMessage::Action(_)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn selected_but_not_connected_executor_falls_back_to_local() {
        let (router, calls) = make_router();
        let peer = NodeId::new_v4();
        assert!(!router.select_remote_executor(peer));
        router.dispatch(CalcAction::Digit(7));
        assert_eq!(
            calls.borrow().displays.last().map(String::as_str),
            Some("7")
        );
        assert!(!router.is_executing_remotely());
    }

    #[test]
    fn remote_control_switch_is_the_only_inbound_permission_boundary() {
        let (router, calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        assert!(router.add_remote_session(peer));

        router.handle_network_message(
            peer,
            NetworkMessage::Action(envelope(peer, 1, CalcAction::Digit(4))),
        );
        assert!(calls.borrow().displays.is_empty());
        assert!(rx.try_recv().is_err());

        router.set_allow_remote_control(true);
        router.handle_network_message(
            peer,
            NetworkMessage::Action(envelope(peer, 1, CalcAction::Digit(4))),
        );
        assert_eq!(
            calls.borrow().displays.last().map(String::as_str),
            Some("4")
        );
        assert!(matches!(rx.try_recv(), Ok((id, NetworkMessage::StateUpdate(_))) if id == peer));
    }

    #[test]
    fn disabling_remote_control_is_immediate_and_clears_status() {
        let (router, calls) = make_router();
        let peer = NodeId::new_v4();
        enable_peer(&router, peer);
        router.handle_network_message(
            peer,
            NetworkMessage::Action(envelope(peer, 1, CalcAction::Digit(1))),
        );
        assert_eq!(router.active_remote_controllers(), vec![peer]);
        let display_count = calls.borrow().displays.len();

        router.set_allow_remote_control(false);
        router.handle_network_message(
            peer,
            NetworkMessage::Action(envelope(peer, 2, CalcAction::Digit(2))),
        );
        assert_eq!(calls.borrow().displays.len(), display_count);
        assert!(router.active_remote_controllers().is_empty());
    }

    #[test]
    fn spoofed_source_and_unconnected_sender_are_rejected() {
        let (router, calls) = make_router();
        let connected = NodeId::new_v4();
        let attacker = NodeId::new_v4();
        enable_peer(&router, connected);

        router.handle_network_message(
            connected,
            NetworkMessage::Action(envelope(attacker, 1, CalcAction::Digit(8))),
        );
        router.handle_network_message(
            attacker,
            NetworkMessage::Action(envelope(attacker, 1, CalcAction::Digit(9))),
        );
        assert!(calls.borrow().displays.is_empty());
    }

    #[test]
    fn invalid_digit_schema_is_rejected_without_panicking() {
        let (router, calls) = make_router();
        let peer = NodeId::new_v4();
        enable_peer(&router, peer);
        router.handle_network_message(
            peer,
            NetworkMessage::Action(envelope(peer, 1, CalcAction::Digit(255))),
        );
        assert!(calls.borrow().displays.is_empty());
    }

    #[test]
    fn stale_and_duplicate_actions_are_rejected_before_execution() {
        let (router, calls) = make_router();
        let peer = NodeId::new_v4();
        enable_peer(&router, peer);
        for seq in [2, 2, 1] {
            router.handle_network_message(
                peer,
                NetworkMessage::Action(envelope(peer, seq, CalcAction::Digit(seq as u8))),
            );
        }
        assert_eq!(calls.borrow().displays.len(), 1);
        assert_eq!(calls.borrow().displays[0], "2");
    }

    #[test]
    fn stale_and_far_future_action_timestamps_are_rejected() {
        let (router, calls) = make_router();
        let peer = NodeId::new_v4();
        enable_peer(&router, peer);
        let now = Router::timestamp_ms();
        let old = ActionEnvelope {
            timestamp_ms: now - MAX_ACTION_AGE.as_millis() as u64 - 1,
            ..envelope(peer, 1, CalcAction::Digit(1))
        };
        let future = ActionEnvelope {
            timestamp_ms: now + MAX_ACTION_FUTURE_SKEW.as_millis() as u64 + 1,
            ..envelope(peer, 2, CalcAction::Digit(2))
        };
        router.handle_network_message(peer, NetworkMessage::Action(old));
        router.handle_network_message(peer, NetworkMessage::Action(future));
        assert!(calls.borrow().displays.is_empty());
    }

    #[test]
    fn remote_action_rate_is_bounded_per_authenticated_peer() {
        let (router, calls) = make_router();
        let peer = NodeId::new_v4();
        enable_peer(&router, peer);
        for seq in 1..=MAX_REMOTE_ACTIONS_PER_SECOND as u64 + 1 {
            router.handle_network_message(
                peer,
                NetworkMessage::Action(envelope(peer, seq, CalcAction::Clear)),
            );
        }
        assert_eq!(
            calls.borrow().displays.len(),
            MAX_REMOTE_ACTIONS_PER_SECOND as usize
        );
    }

    #[test]
    fn state_update_requires_selected_executor_and_valid_bounded_schema() {
        let (router, calls, mut rx) = make_router_with_channel();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        router.select_remote_executor(peer);
        router.dispatch(CalcAction::Digit(6));
        let (_, NetworkMessage::Action(sent)) = rx.try_recv().unwrap() else {
            panic!("expected outbound action");
        };

        let oversized = StateSnapshot {
            display: "x".repeat(MAX_DISPLAY_BYTES + 1),
            history: String::new(),
            memory_indicator: String::new(),
            is_error: false,
            last_seq_applied: sent.seq,
        };
        router.handle_network_message(peer, NetworkMessage::StateUpdate(oversized));
        let invalid_schema = StateSnapshot {
            display: "not-a-number".to_string(),
            history: "bad\nstate".to_string(),
            memory_indicator: "admin".to_string(),
            is_error: false,
            last_seq_applied: sent.seq,
        };
        router.handle_network_message(peer, NetworkMessage::StateUpdate(invalid_schema));
        let before = calls.borrow().displays.len();

        let valid = StateSnapshot {
            display: "6".to_string(),
            history: String::new(),
            memory_indicator: String::new(),
            is_error: false,
            last_seq_applied: sent.seq,
        };
        router.handle_network_message(peer, NetworkMessage::StateUpdate(valid.clone()));
        router.handle_network_message(peer, NetworkMessage::StateUpdate(valid));
        assert_eq!(calls.borrow().displays.len(), before + 1);
    }

    #[test]
    fn deprecated_v5_authorization_messages_have_no_product_state_effect() {
        let (router, calls) = make_router();
        let peer = NodeId::new_v4();
        enable_peer(&router, peer);
        router.handle_network_message(
            peer,
            NetworkMessage::RouteRequest {
                request_id: 1,
                controller: peer,
                executor: router.local_node_id(),
            },
        );
        router.handle_network_message(peer, NetworkMessage::PairingReject);
        assert!(router.active_remote_controllers().is_empty());
        assert!(calls.borrow().displays.is_empty());
    }

    #[test]
    fn authenticated_peer_cannot_inject_local_connection_failure() {
        let (router, _calls) = make_router();
        let peer = NodeId::new_v4();
        enable_peer(&router, peer);
        assert!(router.select_remote_executor(peer));

        router.handle_network_message(
            peer,
            NetworkMessage::ConnectionFailed {
                addr: "127.0.0.1:42420".parse().unwrap(),
                reason: "attacker_reason".to_string(),
                target_node_id: Some(peer),
            },
        );

        assert_eq!(router.active_remote_executor(), Some(peer));
        assert!(router.take_connection_error().is_none());
    }

    #[test]
    fn saturated_outgoing_queue_falls_back_to_local_without_losing_action() {
        let (router, calls) = make_router();
        let peer = NodeId::new_v4();
        router.add_remote_session(peer);
        router.select_remote_executor(peer);
        let (sender, mut receiver) = mpsc::channel(1);
        sender.try_send((peer, NetworkMessage::Ping)).unwrap();
        router.set_outgoing_tx(sender);

        router.dispatch(CalcAction::Digit(7));

        assert_eq!(
            calls.borrow().displays.last().map(String::as_str),
            Some("7")
        );
        assert_eq!(router.active_remote_executor(), None);
        assert!(matches!(
            receiver.try_recv(),
            Ok((target, NetworkMessage::Ping)) if target == peer
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn peer_tracking_has_a_hard_capacity_boundary() {
        let (router, _calls) = make_router();
        for _ in 0..MAX_TRACKED_REMOTE_PEERS {
            assert!(router.add_remote_session(NodeId::new_v4()));
        }
        assert!(!router.add_remote_session(NodeId::new_v4()));
    }
}
