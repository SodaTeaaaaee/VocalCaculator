//! Bridge between Dioxus UI events and the calculator / networking backend.
//!
//! Provides callback wiring functions that connect [`CalcContext`] signals
//! to the [`Router`] for action dispatch and to the [`NetworkManager`] for
//! LAN peer operations.  This is the Dioxus-UI equivalent of
//! the previous retained UI bridge.
//!
//! # Architecture
//!
//! * A thread-local [`Router`] instance dispatches calculator actions through
//!   the routing matrix.  [`create_router`] initialises it; the `handle_*`
//!   functions borrow it to dispatch actions.
//!
//! * A thread-local [`NetworkManager`] owns the async networking runtime.
//!   [`init_networking`] creates and starts it; [`sync_network_state`]
//!   reads its shared state and writes to [`NetUiState`] signals.
//!
//! * [`start_network_event_loop`] spawns a Dioxus coroutine that processes
//!   [`UiEvent`]s received through an unbounded channel.  Each event
//!   updates the corresponding [`CalcContext`] signal directly -- no
//!   polling loop is needed.  The networking thread sends events via
//!   [`UnboundedSender<UiEvent>`].  Pending-control-request timeouts are
//!   tracked with a spawned `tokio::time::sleep` task instead of a poll
//!   counter.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use dioxus::prelude::*;

use crate::app::config;
use crate::app::storage::Storage;
use crate::audio::VocalAudio;
use crate::core::action::CalcAction;
use crate::core::calculator::Calculator;
use crate::core::token::BinaryOp;
use crate::net::protocol::{ConflictPolicy, NetworkMessage, NodeId};
use crate::net::state::NetworkState;
use crate::net::{NetworkManager, Router};
use crate::ui::events::UiEvent;
use crate::ui::state::{CalcContext, PeerDisplayInfo};

// ---------------------------------------------------------------------------
// Thread-local Router + NetworkManager storage
// ---------------------------------------------------------------------------

thread_local! {
    static ROUTER: RefCell<Option<Router>> = const { RefCell::new(None) };
    static NET_MANAGER: RefCell<Option<NetworkManager>> = const { RefCell::new(None) };
}

/// Return a clone of the stored [`Router`], if one has been created.
fn get_router() -> Option<Router> {
    ROUTER.with(|r| r.borrow().clone())
}

/// Execute `f` with a mutable borrow of the thread-local [`NetworkManager`].
///
/// Returns `None` if networking has not been initialised.
fn with_nm<R>(f: impl FnOnce(&mut NetworkManager) -> R) -> Option<R> {
    NET_MANAGER.with(|cell| cell.borrow_mut().as_mut().map(f))
}

// ---------------------------------------------------------------------------
// parse_action -- copied from src/app/callbacks.rs
// ---------------------------------------------------------------------------

/// Parse a keyboard action string into a [`CalcAction`].
///
/// Expected formats:
/// - `"digit:0"` through `"digit:9"`
/// - `"operator:add"`, `"operator:subtract"`, `"operator:multiply"`, `"operator:divide"`
/// - `"equals"`, `"decimal-point"`, `"backspace"`, `"all-clear"`, `"clear"`
/// - `"percent"`, `"sqrt"`, `"mu"`, `"memory-recall"`, `"plus-minus"`
/// - `"memory-add"`, `"memory-subtract"`, `"memory-clear"`
pub fn parse_action(action: &str) -> Option<CalcAction> {
    match action {
        // digit:N
        s if s.starts_with("digit:") => {
            let d = s.strip_prefix("digit:")?.parse::<u8>().ok()?;
            if d <= 9 {
                Some(CalcAction::Digit(d))
            } else {
                None
            }
        }
        // operator:* (accept both "operator:add" and "add" formats)
        "operator:add" | "add" => Some(CalcAction::Operator(BinaryOp::Add)),
        "operator:subtract" | "subtract" => Some(CalcAction::Operator(BinaryOp::Subtract)),
        "operator:multiply" | "multiply" => Some(CalcAction::Operator(BinaryOp::Multiply)),
        "operator:divide" | "divide" => Some(CalcAction::Operator(BinaryOp::Divide)),
        // direct actions
        "equals" => Some(CalcAction::Equals),
        "decimal-point" => Some(CalcAction::DecimalPoint),
        "backspace" => Some(CalcAction::Backspace),
        "all-clear" => Some(CalcAction::AllClear),
        "clear" => Some(CalcAction::Clear),
        "percent" => Some(CalcAction::Percent),
        "sqrt" => Some(CalcAction::SquareRoot),
        "mu" => Some(CalcAction::Mu),
        "memory-recall" => Some(CalcAction::MemoryRecall),
        "memory-add" => Some(CalcAction::MemoryAdd),
        "memory-subtract" => Some(CalcAction::MemorySubtract),
        "memory-clear" => Some(CalcAction::MemoryClear),
        "plus-minus" => Some(CalcAction::PlusMinus),
        _ => {
            log::trace!("Unknown action: {:?}", action);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Handler functions -- dispatch CalcActions through the Router
// ---------------------------------------------------------------------------

/// Dispatch a digit press through the [`Router`].
///
/// Validates that `d` is in `0..=9` before dispatching.
pub fn handle_digit(_ctx: CalcContext, d: u8) {
    if d > 9 {
        return;
    }
    if let Some(router) = get_router() {
        router.dispatch(CalcAction::Digit(d));
    }
}

/// Parse an operator symbol string and dispatch through the [`Router`].
///
/// Accepts `"+", "-", "*", "/"` as well as the longer names
/// `"add", "subtract", "multiply", "divide"`.
pub fn handle_operator(_ctx: CalcContext, op: &str) {
    let binary_op = match op {
        "+" | "add" => BinaryOp::Add,
        "-" | "subtract" => BinaryOp::Subtract,
        "*" | "multiply" => BinaryOp::Multiply,
        "/" | "divide" => BinaryOp::Divide,
        _ => {
            log::trace!("Unknown operator: {:?}", op);
            return;
        }
    };
    if let Some(router) = get_router() {
        router.dispatch(CalcAction::Operator(binary_op));
    }
}

/// Parse an action string and dispatch the resulting [`CalcAction`]
/// through the [`Router`].
///
/// This is the primary entry point for keyboard and button events.
/// Action strings follow the same format as [`parse_action`].
pub fn handle_action(_ctx: CalcContext, action: &str) {
    if let Some(calc_action) = parse_action(action)
        && let Some(router) = get_router()
    {
        router.dispatch(calc_action);
    }
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// Toggle between light and dark theme.
///
/// Flips the `dark_mode` signal in [`CalcContext`] and sets the
/// `data-theme` attribute on the document root element so that CSS
/// custom properties switch accordingly.
pub fn toggle_theme(mut ctx: CalcContext) {
    let current = *ctx.audio.dark_mode.read();
    let next = !current;
    *ctx.audio.dark_mode.write() = next;

    let theme = if next { "dark" } else { "light" };
    // Desktop (webview): set data-theme on <html> via JS eval.
    dioxus::document::eval(&format!(
        r#"document.documentElement.setAttribute("data-theme", "{}")"#,
        theme
    ));
}

// ---------------------------------------------------------------------------
// SharedAudio -- bridges Rc<RefCell<Option<VocalAudio>>> to AudioPlayer
// ---------------------------------------------------------------------------

/// Wrapper that lets the Router and Dioxus callbacks share the same
/// `VocalAudio` instance without moving audio ownership into either side.
struct SharedAudio(Rc<RefCell<Option<VocalAudio>>>);

impl crate::traits::AudioPlayer for SharedAudio {
    fn play_events(&mut self, events: &[crate::core::token::VocalEvent]) {
        if let Some(audio) = self.0.borrow_mut().as_mut() {
            audio.play_events(events);
        }
    }

    fn set_mode(&mut self, mode: crate::audio::AudioMode) {
        if let Some(audio) = self.0.borrow_mut().as_mut() {
            audio.set_mode(mode);
        }
    }

    fn set_volume(&mut self, slider: f64) {
        if let Some(audio) = self.0.borrow_mut().as_mut() {
            audio.set_volume(slider);
        }
    }

    fn mode(&self) -> crate::audio::AudioMode {
        self.0
            .borrow()
            .as_ref()
            .map(|audio| audio.mode())
            .unwrap_or(crate::audio::AudioMode::Normal)
    }
}

/// Create a [`Router`] backed by Dioxus signals and the shared audio
/// subsystem, store it in the thread-local cell, and return a clone.
pub fn create_router(ctx: CalcContext, audio_ref: Rc<RefCell<Option<VocalAudio>>>) -> Router {
    let calc = Rc::new(RefCell::new(Calculator::new()));
    let audio_player: Option<Box<dyn crate::traits::AudioPlayer>> =
        Some(Box::new(SharedAudio(audio_ref)));
    let router = Router::new(calc, audio_player, Box::new(ctx));
    ROUTER.with(|r| *r.borrow_mut() = Some(router.clone()));
    router
}

/// Apply the user's mute preference to the router, preserving automatic
/// routing mute when this device is controlling a remote executor.
pub fn set_router_user_mute(user_muted: bool) {
    if let Some(router) = get_router() {
        router.set_audio_muted(user_muted || router.is_muted());
    }
}

// ---------------------------------------------------------------------------
// NetworkContext -- shared state for the Dioxus event loop
// ---------------------------------------------------------------------------

/// Shared state created during network initialisation.
///
/// Shared network state used by the Dioxus event bridge.
pub struct NetworkContext {
    pub net_state: Arc<Mutex<NetworkState>>,
    pub matrix_node_ids: Rc<RefCell<Vec<NodeId>>>,
}

thread_local! {
    static NET_CONTEXT: RefCell<Option<NetworkContext>> = const { RefCell::new(None) };
}

/// Initialise networking based on app config.
///
/// Creates a [`NetworkManager`], starts the async runtime, wires the
/// [`Router`] to the networking subsystem, and stores both in their
/// respective thread-local cells.  Returns `true` if networking was
/// successfully enabled.
///
/// `ui_event_tx` is the sender half of the [`UiEvent`] channel; the
/// networking runtime uses it to push state-change events to the UI.
///
/// `storage` provides the [`DeviceIdentity`] (node_id + public key)
/// and will be used for paired-device lookups.
///
/// This mirrors `src/app/bridge::init_networking` for the Dioxus UI.
pub fn init_networking(
    ui_event_tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
    storage: Arc<Storage>,
) -> bool {
    let app_config = storage.config();

    if !app_config.network.enabled {
        log::info!("Networking disabled in config");
        return false;
    }

    let router = match get_router() {
        Some(r) => r,
        None => {
            log::error!("init_networking: Router not yet created");
            return false;
        }
    };

    // Extract config values before moving storage into NetworkManager.
    let conflict_policy = app_config.network.conflict_policy.clone();
    let allow_remote_control = app_config.network.allow_remote_control;
    let display_name = app_config.network.display_name.clone();

    let mut nm = NetworkManager::new(storage, ui_event_tx);

    // Synchronise Router and NetworkManager NodeIds so that routing
    // matrix owner IDs match session sender IDs.
    router.set_local_node_id(nm.local_node_id());
    let handle = nm.start();
    router.set_runtime_handle(handle.runtime_handle().clone());
    router.set_outgoing_tx(handle.outgoing_sender());

    let net_state = nm.state();

    match conflict_policy.as_str() {
        "exclusive" => router.set_conflict_policy(ConflictPolicy::Exclusive),
        _ => router.set_conflict_policy(ConflictPolicy::Interleaved),
    }

    router.set_allow_remote_control(allow_remote_control);

    log::info!(
        "Network enabled (name={}, id={})",
        display_name,
        nm.local_node_id(),
    );

    let ctx = NetworkContext {
        net_state,
        matrix_node_ids: Rc::new(RefCell::new(Vec::new())),
    };

    NET_MANAGER.with(|cell| *cell.borrow_mut() = Some(nm));
    NET_CONTEXT.with(|cell| *cell.borrow_mut() = Some(ctx));

    true
}

// ---------------------------------------------------------------------------
// start_network_event_loop -- pure event-driven UiEvent consumer
// ---------------------------------------------------------------------------

/// Pending-control-request timeout duration.
const PENDING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Spawn an async task that consumes [`UiEvent`]s from `rx` and updates
/// [`CalcContext`] signals.  This is a pure event-driven replacement for
/// the old 50 ms poll timer.
///
/// The networking thread sends [`UiEvent`]s through the corresponding
/// [`UnboundedSender<UiEvent>`].  Each variant is matched and the
/// appropriate signal in [`CalcContext`] is updated.  No polling loop
/// is used.
///
/// Pending-control-request timeouts are tracked by spawning a
/// `tokio::time::sleep` task instead of a tick counter.
///
/// Can be called from a component body or a `use_hook` closure.
pub fn start_network_event_loop(
    mut ctx: CalcContext,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<UiEvent>,
) {
    spawn(async move {
        // Tracks whether a timeout task is currently sleeping for a
        // pending control request.  Avoids spawning duplicate tasks.
        let timeout_active = Rc::new(RefCell::new(false));
        // The peer we are currently waiting on (if any).  Used to
        // avoid re-spawning a timeout task when the same request is
        // still pending across consecutive events.
        let tracked_pending_peer: Rc<RefCell<Option<NodeId>>> = Rc::new(RefCell::new(None));

        loop {
            let event = match rx.recv().await {
                Some(ev) => ev,
                None => {
                    log::info!("UiEvent channel closed, stopping network event loop");
                    break;
                }
            };

            // Dispatch the event and determine whether a full state
            // sync is needed afterwards.
            let needs_sync = dispatch_event(&mut ctx, event);

            if needs_sync {
                sync_network_state(ctx.clone());
            }

            // Drain any pending incoming messages from the network
            // runtime that were not delivered as individual
            // UiEvent::NetworkMessage events.  This is a non-blocking
            // try_recv loop and ensures backward compatibility with
            // the current networking runtime which still enqueues
            // messages into the NetworkManager incoming channel.
            drain_incoming_messages(&ctx);

            // Spawn a timeout task for any pending control request
            // that does not already have one running.
            maybe_spawn_pending_timeout(&ctx, &timeout_active, &tracked_pending_peer);
        }
    });
}

// ---------------------------------------------------------------------------
// dispatch_event -- match a single UiEvent and update CalcContext signals
// ---------------------------------------------------------------------------

/// Process a single [`UiEvent`] and update the corresponding
/// [`CalcContext`] signals.
///
/// Returns `true` if the caller should call [`sync_network_state`]
/// after this event (i.e. the event affects state that the sync
/// function reads from the [`NetworkManager`] / [`Router`]).
fn dispatch_event(ctx: &mut CalcContext, event: UiEvent) -> bool {
    match event {
        // ---- Peer discovery ----
        UiEvent::PeerDiscovered(payload) => {
            let info = PeerDisplayInfo {
                name: Signal::new(payload.name),
                address: Signal::new(payload.address),
                is_connected: Signal::new(payload.is_connected),
                latency_ms: Signal::new(payload.latency_ms),
                index: Signal::new(payload.index),
                node_id_string: Signal::new(payload.node_id_string),
            };
            let mut peers = (*ctx.net.peers.read()).clone();
            if let Some(existing) = peers
                .iter_mut()
                .find(|p| *p.node_id_string.read() == *info.node_id_string.read())
            {
                *existing = info;
            } else {
                peers.push(info);
            }
            *ctx.net.peers.write() = peers;
            false
        }
        UiEvent::PeerLost(uuid) => {
            let mut peers = (*ctx.net.peers.read()).clone();
            peers.retain(|p| *p.node_id_string.read() != uuid);
            *ctx.net.peers.write() = peers;
            false
        }

        // ---- TCP sessions ----
        UiEvent::SessionEstablished(uuid) => {
            if let Ok(node_id) = uuid.parse::<uuid::Uuid>()
                && let Some(router) = get_router()
            {
                router.add_remote_session(node_id);
                router.send_routing_sync_to(node_id);
                // Keep the Router's broadcast peer set in sync.
                if let Some(()) = with_nm(|nm| {
                    router.set_connected_peers(nm.active_session_ids());
                }) {}
            }
            true
        }
        UiEvent::SessionLost(uuid) => {
            if let Ok(node_id) = uuid.parse::<uuid::Uuid>()
                && let Some(router) = get_router()
            {
                router.cleanup_peer_disconnect(&node_id);
                if let Some(()) = with_nm(|nm| {
                    router.set_connected_peers(nm.active_session_ids());
                }) {}
            }
            true
        }

        // ---- Messages ----
        UiEvent::NetworkMessage(sender_uuid, bytes) => {
            match bincode::serde::decode_from_slice::<NetworkMessage, _>(
                &bytes,
                bincode::config::standard(),
            ) {
                Ok((msg, _)) => {
                    if let Ok(sender_id) = sender_uuid.parse::<uuid::Uuid>() {
                        // Intercept PeerNameUpdate: update the peer's
                        // display name in NetworkState so the UI picks
                        // it up on the next sync.
                        if let NetworkMessage::PeerNameUpdate { ref display_name } = msg {
                            NET_CONTEXT.with(|cell| {
                                if let Some(ref net_ctx) = *cell.borrow() {
                                    let mut state =
                                        net_ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
                                    state.peers.update_name(&sender_id, display_name);
                                }
                            });
                        }
                        if let Some(router) = get_router() {
                            router.handle_network_message(sender_id, msg);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to deserialize UiEvent::NetworkMessage: {}", e);
                }
            }
            true
        }

        // ---- Errors ----
        UiEvent::ConnectionError(reason) => {
            let error_msg = match reason.as_str() {
                "timeout" | "handshake_timeout" => "连接超时".to_string(),
                "bind_failed" => "网络端口无法监听".to_string(),
                "connection_refused" => "连接被拒绝".to_string(),
                "connection_reset" => "连接中断".to_string(),
                "host_unreachable" => "设备不可达".to_string(),
                "network_unreachable" => "网络不可达".to_string(),
                "permission_denied" => "访问被拒绝".to_string(),
                other => format!("连接失败: {}", other),
            };
            let mut needs_sync = false;
            if let Some(router) = get_router()
                && let Some(pending_peer) = router.pending_control_request()
            {
                let my_id = router.local_node_id();
                router.set_route(my_id, pending_peer, false);
                router.clear_pending_control_request();
                needs_sync = true;
            }
            *ctx.net.status.write() = error_msg;
            *ctx.net.executing_remotely.write() = false;
            needs_sync
        }

        // ---- Latency ----
        UiEvent::LatencyUpdate(peer_uuid, latency_ms) => {
            let mut peers = (*ctx.net.peers.read()).clone();
            for peer in &mut peers {
                if *peer.node_id_string.read() == peer_uuid {
                    *peer.latency_ms.write() = latency_ms;
                    break;
                }
            }
            *ctx.net.peers.write() = peers;
            false
        }

        // ---- Routing matrix ----
        UiEvent::RoutingMatrixUpdate(data) => {
            *ctx.net.matrix_size.write() = data.size;
            *ctx.net.peer_names.write() = data.names;
            *ctx.net.matrix_cells.write() = data.cells;
            *ctx.net.my_index.write() = data.my_index;
            *ctx.net.matrix_node_ids.write() = data.node_ids;
            // Also persist into NET_CONTEXT so handle_route_toggled
            // uses the same ordering as the last render (Bug 9 fix).
            NET_CONTEXT.with(|cell| {
                if let Some(ref net_ctx) = *cell.borrow() {
                    let node_ids: Vec<NodeId> = ctx
                        .net
                        .matrix_node_ids
                        .read()
                        .iter()
                        .filter_map(|s| s.parse::<uuid::Uuid>().ok())
                        .collect();
                    *net_ctx.matrix_node_ids.borrow_mut() = node_ids;
                }
            });
            false
        }

        // ---- Remote control status ----
        UiEvent::RemoteControlStatus(remote_controlled, executing_remotely) => {
            *ctx.net.remote_controlled.write() = remote_controlled;
            *ctx.net.executing_remotely.write() = executing_remotely;
            false
        }

        // ---- Status text ----
        UiEvent::NetworkStatusUpdate(status) => {
            *ctx.net.status.write() = status;
            false
        }

        // ---- Pending request timeout ----
        UiEvent::PendingTimeout(peer_uuid) => {
            if let Ok(pending_peer) = peer_uuid.parse::<uuid::Uuid>()
                && let Some(router) = get_router()
            {
                let my_id = router.local_node_id();
                router.set_route(my_id, pending_peer, false);
                router.clear_pending_control_request();
            }
            *ctx.net.status.write() = "连接超时".to_string();
            *ctx.net.executing_remotely.write() = false;
            true
        }
    }
}

// ---------------------------------------------------------------------------
// drain_incoming_messages -- backward-compat bridge for NetworkManager
// ---------------------------------------------------------------------------

/// Drain pending incoming messages from the [`NetworkManager`] internal
/// channel and forward them to the [`Router`].
///
/// With the event-driven architecture the networking runtime sends
/// [`UiEvent::NetworkMessage`] events through the UiEvent channel
/// rather than buffering them internally.  This function is now a
/// no-op retained for backward compatibility with the event loop
/// call site.
fn drain_incoming_messages(_ctx: &CalcContext) {
    // No-op: messages arrive as UiEvent::NetworkMessage through the
    // event channel and are dispatched by dispatch_event().
}

// ---------------------------------------------------------------------------
// maybe_spawn_pending_timeout -- tokio::time::sleep replacement for tick counter
// ---------------------------------------------------------------------------

/// If a control-request is pending and no timeout task is currently
/// running, spawn one that sleeps for [`PENDING_TIMEOUT`] and then
/// resolves the request if it is still pending.
///
/// This replaces the old tick-counter approach (`pending_timeout_ticks`)
/// from the poll timer with a `tokio::time::sleep` based timeout.
fn maybe_spawn_pending_timeout(
    ctx: &CalcContext,
    timeout_active: &Rc<RefCell<bool>>,
    tracked_peer: &Rc<RefCell<Option<NodeId>>>,
) {
    let router = match get_router() {
        Some(r) => r,
        None => return,
    };

    let is_pending = router.is_awaiting_grant();
    let current_pending = router.pending_control_request();

    // If the pending target changed, allow re-spawning.
    if current_pending != *tracked_peer.borrow() {
        *tracked_peer.borrow_mut() = current_pending;
        *timeout_active.borrow_mut() = false;
    }

    if is_pending && !*timeout_active.borrow() {
        *timeout_active.borrow_mut() = true;

        let pending_peer = match current_pending {
            Some(p) => p,
            None => return,
        };

        let mut ctx_clone = ctx.clone();
        let active_flag = timeout_active.clone();
        let tracked = tracked_peer.clone();

        spawn(async move {
            tokio::time::sleep(PENDING_TIMEOUT).await;

            // Only act if the same request is still pending.
            if let Some(router) = get_router() {
                let still_pending = router.is_awaiting_grant()
                    && router.pending_control_request() == Some(pending_peer);

                if still_pending {
                    log::warn!(
                        "Pending control request to {} timed out; reverting",
                        pending_peer,
                    );
                    let my_id = router.local_node_id();
                    router.set_route(my_id, pending_peer, false);
                    router.clear_pending_control_request();
                    *ctx_clone.net.status.write() = "连接超时".to_string();
                    *ctx_clone.net.executing_remotely.write() = false;
                    sync_network_state(ctx_clone.clone());
                }
            }

            *active_flag.borrow_mut() = false;
            *tracked.borrow_mut() = None;
        });
    } else if !is_pending && *timeout_active.borrow() {
        // Request resolved before timeout; mark inactive so the
        // sleeping task will see is_pending == false and no-op.
        *timeout_active.borrow_mut() = false;
        *tracked_peer.borrow_mut() = None;
    }
}

// ---------------------------------------------------------------------------
// sync_network_state -- read from NetworkManager, write to NetUiState signals
// ---------------------------------------------------------------------------

/// Read the current network state from the [`NetworkManager`] and
/// [`Router`], then update all [`NetUiState`] signals.
///
/// Called after session and message events in the event loop, and
/// on-demand by handler functions after state-changing operations.
///
/// This mirrors the "Sync peer list" and "Routing matrix UI sync"
/// sections of `src/app/bridge::start_poll_timer`.
pub fn sync_network_state(mut ctx: CalcContext) {
    let router = match get_router() {
        Some(r) => r,
        None => return,
    };

    // --- Matrix-based UI state: determine remote execution target ---
    let my_id = router.local_node_id();
    let targets = router.my_control_targets();
    let remote_targets: Vec<NodeId> = targets.into_iter().filter(|id| *id != my_id).collect();
    let is_muted = router.is_muted();
    let user_muted = *ctx.audio.muted.read();
    router.set_audio_muted(user_muted || is_muted);

    // --- Sync peer list from NetworkState ---
    NET_CONTEXT.with(|cell| {
        let borrow = cell.borrow();
        let net_ctx = match borrow.as_ref() {
            Some(c) => c.net_state.clone(),
            None => return,
        };
        let state = net_ctx
            .lock()
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());

        let mut remote_peer_name: Option<String> = None;
        let mut connected_idx: i32 = -1;
        let mut new_peers = Vec::new();

        for (i, (node_id, peer)) in state.peers.iter().enumerate() {
            let nid_str: String = node_id.to_string();
            let is_conn = remote_targets.contains(node_id);
            if is_conn {
                connected_idx = i as i32;
                remote_peer_name = Some(peer.display_name.clone());
            }
            new_peers.push(PeerDisplayInfo {
                name: Signal::new(peer.display_name.clone()),
                address: Signal::new(format!("{}:{}", peer.address.ip(), peer.tcp_port)),
                is_connected: Signal::new(is_conn),
                latency_ms: Signal::new(state.latency_ms.map(|v| v as i32).unwrap_or(-1)),
                index: Signal::new(i as i32),
                node_id_string: Signal::new(nid_str),
            });
        }

        let is_any_connected = state.is_connected;

        // Clean up stale remote targets (in our matrix but absent from
        // active sessions).
        if let Some(stale_targets) = with_nm(|nm| {
            let active_sessions = nm.active_session_ids();
            let pending = router.pending_control_request();
            remote_targets
                .iter()
                .filter(|t| !active_sessions.contains(t))
                .filter(|t| pending.as_ref() != Some(*t))
                .copied()
                .collect::<Vec<NodeId>>()
        }) {
            for target in &stale_targets {
                router.cleanup_peer_disconnect(target);
            }
        }

        *ctx.net.peers.write() = new_peers;
        *ctx.net.connected_peer_index.write() = connected_idx;

        // Update network-status display based on routing matrix.
        if is_muted {
            if router.is_awaiting_grant() {
                *ctx.net.status.write() = "等待授权...".to_string();
                *ctx.net.executing_remotely.write() = false;
            } else {
                let name = remote_peer_name.as_deref().unwrap_or("未知");
                *ctx.net.status.write() = format!("远程: {}", name);
                *ctx.net.executing_remotely.write() = true;
            }
        } else if is_any_connected {
            *ctx.net.status.write() = "已连接".to_string();
            *ctx.net.executing_remotely.write() = false;
        } else {
            *ctx.net.status.write() = "已启用".to_string();
            *ctx.net.executing_remotely.write() = false;
        }

        // Update remote-controlled indicator (are we being controlled?).
        let is_remote_controlled = router.my_controllers().iter().any(|id| *id != my_id);
        *ctx.net.remote_controlled.write() = is_remote_controlled;

        // --- Routing matrix UI sync ---
        let matrix = router.get_routing_matrix();

        let mut node_ids: Vec<NodeId> = matrix.keys().flat_map(|(c, e)| vec![*c, *e]).collect();
        node_ids.sort_by(|a, b| {
            let a_name = state
                .peers
                .get_peer(a)
                .map(|p| p.display_name.clone())
                .unwrap_or_default();
            let b_name = state
                .peers
                .get_peer(b)
                .map(|p| p.display_name.clone())
                .unwrap_or_default();
            a_name
                .cmp(&b_name)
                .then_with(|| a.to_string().cmp(&b.to_string()))
        });
        node_ids.dedup();

        let n = node_ids.len();
        let mut names = Vec::with_capacity(n);
        let mut cells = Vec::with_capacity(n * n);
        let mut my_idx: i32 = -1;

        for (i, nid) in node_ids.iter().enumerate() {
            if *nid == my_id {
                my_idx = i as i32;
            }
            let display_name: String = if let Some(p) = state.peers.get_peer(nid) {
                p.display_name.clone()
            } else if *nid == my_id {
                "本机".to_string()
            } else {
                let uuid_str = nid.to_string();
                uuid_str[..8].to_string()
            };
            names.push(display_name);
            for other in &node_ids {
                cells.push(matrix.get(&(*nid, *other)).copied().unwrap_or(false));
            }
        }

        // Store sorted node IDs so handle_route_toggled uses the same
        // ordering as the last render (Bug 9 fix).
        NET_CONTEXT.with(|ctx_cell| {
            if let Some(ref net_ctx) = *ctx_cell.borrow() {
                *net_ctx.matrix_node_ids.borrow_mut() = node_ids.clone();
            }
        });

        *ctx.net.matrix_size.write() = n as i32;
        *ctx.net.peer_names.write() = names;
        *ctx.net.matrix_cells.write() = cells;
        *ctx.net.my_index.write() = my_idx;
    });
}

// ---------------------------------------------------------------------------
// Network action handlers
// ---------------------------------------------------------------------------

/// Initiate a connection to a peer identified by its NodeId string.
///
/// The peer must have been discovered via LAN scan.  If the peer is
/// already connected, its route is cleared (disconnect).
pub fn handle_connect_peer(mut ctx: CalcContext, node_id_str: String) {
    let target = match node_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => uuid,
        Err(e) => {
            log::warn!("Invalid node ID '{}': {}", node_id_str, e);
            *ctx.net.status.write() = "无效的节点ID".to_string();
            return;
        }
    };

    let router = match get_router() {
        Some(r) => r,
        None => return,
    };

    let my_id = router.local_node_id();
    if target == my_id {
        return;
    }

    // If already connected (has a route), clear the route (disconnect).
    let targets = router.my_control_targets();
    if targets.contains(&target) {
        sync_network_state(ctx);
        return;
    }

    // Look up peer address from NetworkState and connect via TCP.
    let peer_addr = NET_CONTEXT.with(|cell| {
        cell.borrow().as_ref().and_then(|net_ctx| {
            let state = net_ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .peers
                .get_peer(&target)
                .map(|p| std::net::SocketAddr::new(p.address.ip(), p.tcp_port))
        })
    });

    match peer_addr {
        Some(addr) => {
            log::info!("Connecting to peer {} at {}", target, addr);
            for old_target in router.my_control_targets() {
                if old_target != my_id && old_target != target {
                    router.send_release_to(old_target);
                    router.set_route(my_id, old_target, false);
                }
            }

            router.set_route(my_id, target, true);
            router.clear_pending_control_request();
            router.set_pending_control_request(target);
            *ctx.net.status.write() = "等待授权...".to_string();
            *ctx.net.executing_remotely.write() = false;

            if let Some(()) = with_nm(|nm| {
                nm.connect_to_peer(addr, Some(target));
            }) {
                log::trace!("Connect command sent for {}", target);
            }
        }
        None => {
            log::warn!("Peer {} not found in discovery table", target);
            *ctx.net.status.write() = "未找到设备".to_string();
        }
    }
}

/// Disconnect from a peer identified by its NodeId string.
///
/// Clears the routing matrix route for this peer, which stops sending
/// actions to it.  The TCP session remains alive (for potential
/// re-routing) but the control relationship is severed.
pub fn handle_disconnect_peer(ctx: CalcContext, node_id_str: String) {
    let target = match node_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => uuid,
        Err(e) => {
            log::warn!("Invalid node ID '{}': {}", node_id_str, e);
            return;
        }
    };

    let router = match get_router() {
        Some(r) => r,
        None => return,
    };

    let my_id = router.local_node_id();
    if target == my_id {
        return;
    }

    router.set_route(my_id, target, false);
    router.send_release_to(target);
    router.clear_pending_control_request();
    log::info!("Disconnected from peer {}", target);
    sync_network_state(ctx);
}

/// Trigger a LAN peer discovery scan.
///
/// Broadcasts Discover + Announce messages on the local network.
/// The scan is fire-and-forget; discovered peers will appear as
/// [`UiEvent::PeerDiscovered`] events.
pub fn handle_scan_peers(mut ctx: CalcContext) {
    *ctx.net.scanning.write() = true;

    if let Some(()) = with_nm(|nm| {
        nm.trigger_scan();
    }) {
        log::info!("LAN scan triggered");
    } else {
        log::warn!("Cannot scan: networking not initialised");
    }

    // Reset scanning indicator after a short delay.
    let mut scanning = ctx.net.scanning;
    spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        *scanning.write() = false;
    });
}

/// Toggle whether this node accepts remote control from other peers.
///
/// When enabled, other peers can route actions to this node's calculator.
/// When disabled, incoming remote actions are rejected.
pub fn handle_toggle_remote_control(mut ctx: CalcContext) {
    let current = *ctx.net.allow_remote_control.read();
    let next = !current;
    *ctx.net.allow_remote_control.write() = next;

    if let Some(router) = get_router() {
        router.set_allow_remote_control(next);
        if !next {
            let my_id = router.local_node_id();
            let controllers: Vec<_> = router
                .my_controllers()
                .into_iter()
                .filter(|id| *id != my_id)
                .collect();
            for controller_id in controllers {
                router.send_route_revoke_directed(controller_id, my_id);
                router.revoke_remote_route(controller_id, my_id);
            }
        }
    }

    sync_network_state(ctx);

    log::info!(
        "Remote control {}",
        if next { "enabled" } else { "disabled" }
    );
}

/// Save the display name to config and update the network manager.
///
/// The new name is persisted to `config.toml` and broadcast to all
/// connected peers via `PeerNameUpdate`.
pub fn handle_save_display_name(mut ctx: CalcContext, name: String) {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        *ctx.settings.save_status.write() = "名称不能为空".to_string();
        return;
    }

    *ctx.settings.display_name.write() = trimmed.clone();

    // Update NetworkManager and broadcast to peers.
    if let Some(()) = with_nm(|nm| {
        nm.update_display_name(trimmed.clone());
    }) {}

    // Persist to config file.
    let mut app_config = config::AppConfig::load();
    app_config.network.display_name = trimmed;
    match app_config.save() {
        Ok(()) => {
            *ctx.settings.save_status.write() = "已保存".to_string();
            log::info!("Display name saved to config");
        }
        Err(e) => {
            *ctx.settings.save_status.write() = "保存失败".to_string();
            log::error!("Failed to save config: {}", e);
        }
    }
}

/// Handle a routing matrix cell toggle (user clicks a cell in the
/// NetworkPanel grid).
///
/// `row` and `col` are indices into the sorted node-ID list maintained
/// in `matrix_node_ids`.  `value` is the desired route state (`true` =
/// grant control, `false` = revoke).
///
/// The row must correspond to this node (only the row owner can modify
/// their own routes).  Toggling control of another peer initiates the
/// ControlRequest handshake; revoking is immediate.
pub fn handle_route_toggled(mut ctx: CalcContext, row: i32, col: i32, value: bool) {
    let node_ids: Vec<NodeId> = NET_CONTEXT.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|c| c.matrix_node_ids.borrow().clone())
            .unwrap_or_default()
    });

    if node_ids.is_empty() {
        log::warn!("handle_route_toggled: matrix_node_ids is empty");
        return;
    }

    let row_idx = row as usize;
    let col_idx = col as usize;

    if row_idx >= node_ids.len() || col_idx >= node_ids.len() {
        log::warn!(
            "handle_route_toggled: index out of bounds (row={}, col={}, len={})",
            row,
            col,
            node_ids.len()
        );
        return;
    }

    let from_id = node_ids[row_idx];
    let to_id = node_ids[col_idx];

    let router = match get_router() {
        Some(r) => r,
        None => return,
    };

    let my_id = router.local_node_id();

    // Only the row owner can modify their own row.
    if from_id != my_id {
        log::warn!(
            "handle_route_toggled: row {} is not ours ({}), ignoring",
            row,
            my_id
        );
        return;
    }

    // Don't allow toggling the self-control diagonal.
    if to_id == my_id {
        log::trace!("handle_route_toggled: ignoring diagonal toggle");
        return;
    }

    if value {
        let has_session = with_nm(|nm| nm.active_session_ids().contains(&to_id)).unwrap_or(false);
        if has_session {
            let ok = router.set_route(from_id, to_id, true);
            if ok {
                log::info!("Route set: {} -> {}", from_id, to_id);
            } else {
                log::warn!("Route set failed: {} -> {}", from_id, to_id);
            }
        } else {
            let peer_addr = NET_CONTEXT.with(|cell| {
                cell.borrow().as_ref().and_then(|net_ctx| {
                    let state = net_ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
                    state
                        .peers
                        .get_peer(&to_id)
                        .map(|p| std::net::SocketAddr::new(p.address.ip(), p.tcp_port))
                })
            });
            if let Some(addr) = peer_addr {
                for old_target in router.my_control_targets() {
                    if old_target != my_id && old_target != to_id {
                        router.send_release_to(old_target);
                        router.set_route(my_id, old_target, false);
                    }
                }
                router.set_route(from_id, to_id, true);
                router.clear_pending_control_request();
                router.set_pending_control_request(to_id);
                *ctx.net.status.write() = "等待授权...".to_string();
                *ctx.net.executing_remotely.write() = false;
                if let Some(()) = with_nm(|nm| nm.connect_to_peer(addr, Some(to_id))) {}
            } else {
                log::warn!(
                    "Route toggle: peer {} not found in discovery table, cannot connect",
                    to_id,
                );
            }
        }
    } else {
        // Revoking control: clear the route.
        router.set_route(from_id, to_id, false);
        router.send_release_to(to_id);
        router.clear_pending_control_request();
        log::info!("Route revoked: {} -/-> {}", from_id, to_id);
    }

    sync_network_state(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // parse_action tests (mirrors src/app/callbacks.rs test suite)
    // -------------------------------------------------------------------

    #[test]
    fn parse_digit_0_through_9() {
        for d in 0..=9u8 {
            let input = format!("digit:{d}");
            assert_eq!(
                parse_action(&input),
                Some(CalcAction::Digit(d)),
                "failed for {input}"
            );
        }
    }

    #[test]
    fn parse_operator_add() {
        assert_eq!(
            parse_action("operator:add"),
            Some(CalcAction::Operator(BinaryOp::Add))
        );
        assert_eq!(
            parse_action("add"),
            Some(CalcAction::Operator(BinaryOp::Add))
        );
    }

    #[test]
    fn parse_operator_subtract() {
        assert_eq!(
            parse_action("operator:subtract"),
            Some(CalcAction::Operator(BinaryOp::Subtract))
        );
        assert_eq!(
            parse_action("subtract"),
            Some(CalcAction::Operator(BinaryOp::Subtract))
        );
    }

    #[test]
    fn parse_operator_multiply() {
        assert_eq!(
            parse_action("operator:multiply"),
            Some(CalcAction::Operator(BinaryOp::Multiply))
        );
        assert_eq!(
            parse_action("multiply"),
            Some(CalcAction::Operator(BinaryOp::Multiply))
        );
    }

    #[test]
    fn parse_operator_divide() {
        assert_eq!(
            parse_action("operator:divide"),
            Some(CalcAction::Operator(BinaryOp::Divide))
        );
        assert_eq!(
            parse_action("divide"),
            Some(CalcAction::Operator(BinaryOp::Divide))
        );
    }

    #[test]
    fn parse_equals() {
        assert_eq!(parse_action("equals"), Some(CalcAction::Equals));
    }

    #[test]
    fn parse_decimal_point() {
        assert_eq!(
            parse_action("decimal-point"),
            Some(CalcAction::DecimalPoint)
        );
    }

    #[test]
    fn parse_backspace() {
        assert_eq!(parse_action("backspace"), Some(CalcAction::Backspace));
    }

    #[test]
    fn parse_all_clear() {
        assert_eq!(parse_action("all-clear"), Some(CalcAction::AllClear));
    }

    #[test]
    fn parse_clear() {
        assert_eq!(parse_action("clear"), Some(CalcAction::Clear));
    }

    #[test]
    fn parse_percent() {
        assert_eq!(parse_action("percent"), Some(CalcAction::Percent));
    }

    #[test]
    fn parse_sqrt() {
        assert_eq!(parse_action("sqrt"), Some(CalcAction::SquareRoot));
    }

    #[test]
    fn parse_mu() {
        assert_eq!(parse_action("mu"), Some(CalcAction::Mu));
    }

    #[test]
    fn parse_plus_minus() {
        assert_eq!(parse_action("plus-minus"), Some(CalcAction::PlusMinus));
    }

    #[test]
    fn parse_memory_recall() {
        assert_eq!(
            parse_action("memory-recall"),
            Some(CalcAction::MemoryRecall)
        );
    }

    #[test]
    fn parse_memory_add() {
        assert_eq!(parse_action("memory-add"), Some(CalcAction::MemoryAdd));
    }

    #[test]
    fn parse_memory_subtract() {
        assert_eq!(
            parse_action("memory-subtract"),
            Some(CalcAction::MemorySubtract)
        );
    }

    #[test]
    fn parse_memory_clear() {
        assert_eq!(parse_action("memory-clear"), Some(CalcAction::MemoryClear));
    }

    // -------------------------------------------------------------------
    // Boundary / error cases
    // -------------------------------------------------------------------

    #[test]
    fn digit_out_of_range_returns_none() {
        assert_eq!(parse_action("digit:10"), None);
        assert_eq!(parse_action("digit:99"), None);
        assert_eq!(parse_action("digit:255"), None);
    }

    #[test]
    fn digit_missing_value_returns_none() {
        assert_eq!(parse_action("digit:"), None);
    }

    #[test]
    fn digit_non_numeric_returns_none() {
        assert_eq!(parse_action("digit:abc"), None);
        assert_eq!(parse_action("digit: "), None);
    }

    #[test]
    fn empty_string_returns_none() {
        assert_eq!(parse_action(""), None);
    }

    #[test]
    fn unknown_string_returns_none() {
        assert_eq!(parse_action("foobar"), None);
        assert_eq!(parse_action("equals!"), None);
        assert_eq!(parse_action(" clear"), None);
        assert_eq!(parse_action("operator:power"), None);
    }

    // -------------------------------------------------------------------
    // handle_operator symbol mapping
    // -------------------------------------------------------------------

    #[test]
    fn handle_operator_symbol_plus() {
        assert_eq!(
            parse_action("add"),
            Some(CalcAction::Operator(BinaryOp::Add))
        );
    }
}
