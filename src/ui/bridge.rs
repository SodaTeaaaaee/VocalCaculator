//! Bridge between Dioxus UI events and the calculator / networking backend.
//!
//! Provides callback wiring functions that connect [`CalcContext`] signals
//! to the [`Router`] for action dispatch and to the [`NetworkManager`] for
//! LAN peer operations.  This is the Dioxus-UI equivalent of
//! the previous retained UI bridge.
//!
//! # Architecture
//!
//! * A thread-local [`Router`] instance dispatches calculator actions locally
//!   or to one selected remote executor. [`create_router`] initialises it;
//!   the `handle_*` functions borrow it to dispatch actions.
//!
//! * A thread-local [`NetworkManager`] owns the async networking runtime.
//!   [`init_networking`] creates and starts it; [`sync_network_state`]
//!   reads its shared state and writes to [`NetUiState`] signals.
//!
//! * [`start_network_event_loop`] spawns a Dioxus coroutine that processes
//!   [`UiEvent`]s received through a bounded channel.  Each event
//!   updates the corresponding [`CalcContext`] signal directly. The
//!   networking thread uses a finite queue with explicit overload handling.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use dioxus::prelude::*;

use crate::app::config;
use crate::app::network_mode::{self, NetworkMode};
use crate::app::storage::Storage;
use crate::audio::VocalAudio;
use crate::core::action::CalcAction;
use crate::core::calculator::Calculator;
use crate::core::token::BinaryOp;
use crate::net::discovery::public_key_fingerprint;
use crate::net::protocol::{LAN_FIXED_PORT, NetworkMessage, NodeId, valid_display_name};
use crate::net::state::NetworkState;
use crate::net::view::{
    BindStatus, ConnectErrorKind, NetworkStatusKind, PeerPresence, PeerRole, PeerViewModel,
    ScanState,
};
use crate::net::{NetworkManager, Router};
use crate::ui::command::AppCommand;
use crate::ui::events::UiEvent;
use crate::ui::state::CalcContext;

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
    let mut app_config = config::AppConfig::load();
    app_config.dark_mode = next;
    if let Err(e) = app_config.save() {
        log::error!("Failed to save theme config: {}", e);
    }
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
    pub local_node_id: NodeId,
    pub local_fingerprint: String,
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
/// `storage` provides the authenticated local [`DeviceIdentity`].
///
/// This mirrors `src/app/bridge::init_networking` for the Dioxus UI.
pub fn init_networking(
    ui_event_tx: tokio::sync::mpsc::Sender<UiEvent>,
    storage: Arc<Storage>,
) -> bool {
    let app_config = storage.config();

    // The resolved NetworkMode already folds in the legacy
    // `network.enabled` flag (see `network_mode::resolve_network_mode`),
    // so this replaces the old `!app_config.network.enabled` check.
    // `mode` is threaded into `NetworkManager::start` below, which passes
    // it into the runtime: `Lan` binds the fixed session port and starts
    // discovery, `LoopbackTest` binds loopback-only and skips discovery.
    let mode = network_mode::current();
    if mode == NetworkMode::Offline {
        log::info!("Networking disabled (NetworkMode::Offline)");
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
    let allow_remote_control = app_config.network.allow_remote_control;
    let display_name = app_config.network.display_name.clone();

    let startup_event_tx = ui_event_tx.clone();
    let local_fingerprint = public_key_fingerprint(&storage.identity().public_key_bytes());
    let mut nm = NetworkManager::new(storage, ui_event_tx);

    // Synchronise Router and NetworkManager identity. The session handshake
    // proves possession of the corresponding Ed25519 key before a message can
    // reach the Router.
    router.set_local_node_id(nm.local_node_id());
    router.set_allow_remote_control(allow_remote_control);
    let handle = match nm.start(mode) {
        Ok(handle) => handle,
        Err(error) => {
            log::error!("Networking failed to start: {}", error);
            let _ = startup_event_tx.try_send(UiEvent::NetworkStatus {
                kind: NetworkStatusKind::Error,
                text: "网络启动失败，本机计算器仍可正常使用".to_string(),
            });
            return false;
        }
    };
    router.set_runtime_handle(handle.runtime_handle().clone());
    router.set_outgoing_tx(handle.outgoing_sender());

    let net_state = nm.state();

    log::info!(
        "Network enabled (name={}, id={})",
        display_name,
        nm.local_node_id(),
    );

    let local_node_id = nm.local_node_id();
    let ctx = NetworkContext {
        net_state,
        local_node_id,
        local_fingerprint,
    };

    NET_MANAGER.with(|cell| *cell.borrow_mut() = Some(nm));
    NET_CONTEXT.with(|cell| *cell.borrow_mut() = Some(ctx));

    true
}

// ---------------------------------------------------------------------------
// start_network_event_loop -- pure event-driven UiEvent consumer
// ---------------------------------------------------------------------------

/// Spawn an async task that consumes [`UiEvent`]s from `rx` and updates
/// [`CalcContext`] signals.  This is a pure event-driven replacement for
/// the old 50 ms poll timer.
///
/// The networking thread sends [`UiEvent`]s through the corresponding
/// a bounded sender. Each variant is matched and the
/// appropriate signal in [`CalcContext`] is updated.  No polling loop
/// is used.
///
/// Can be called from a component body or a `use_hook` closure.
pub fn start_network_event_loop(
    mut ctx: CalcContext,
    mut rx: tokio::sync::mpsc::Receiver<UiEvent>,
) {
    populate_local_identity(&mut ctx);

    spawn(async move {
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
        UiEvent::PeerUpsert(model) => {
            upsert_peer(ctx, with_routing_role(model));
            false
        }
        UiEvent::PeerLost { node_id } => {
            let mut peers = (*ctx.net.peers.read()).clone();
            peers.retain(|peer| peer.node_id != node_id);
            *ctx.net.peers.write() = peers;
            false
        }
        UiEvent::SessionEstablished {
            node_id,
            session_id,
        } => {
            if let Some(router) = get_router() {
                router.add_remote_session(node_id);
                NET_CONTEXT.with(|cell| {
                    if let Some(ref net_ctx) = *cell.borrow() {
                        let state = net_ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(peer) = state.peers.get_peer(&node_id) {
                            router.set_remote_public_key(node_id, peer.public_key);
                        }
                    }
                });
                if let Some(()) = with_nm(|nm| {
                    router.set_connected_peers(nm.active_session_ids());
                }) {}
            }
            mark_peer_session(ctx, node_id, Some(session_id), PeerPresence::Connected);
            true
        }
        UiEvent::SessionLost {
            node_id,
            session_id: _,
        } => {
            if let Some(router) = get_router() {
                router.cleanup_peer_disconnect(&node_id);
                if let Some(()) = with_nm(|nm| {
                    router.set_connected_peers(nm.active_session_ids());
                }) {}
            }
            mark_peer_session(ctx, node_id, None, PeerPresence::Nearby);
            true
        }
        UiEvent::InboundMessage { sender, message } => {
            if let NetworkMessage::PeerNameUpdate { ref display_name } = message {
                if valid_display_name(display_name) {
                    NET_CONTEXT.with(|cell| {
                        if let Some(ref net_ctx) = *cell.borrow() {
                            let mut state =
                                net_ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.peers.update_name(&sender, display_name);
                        }
                    });
                    update_peer(ctx, sender, |peer| {
                        peer.display_name = display_name.clone();
                    });
                } else {
                    log::warn!("Rejected invalid peer display name from {sender}");
                }
            }
            if let Some(router) = get_router() {
                router.handle_network_message(sender, message);
            }
            true
        }
        UiEvent::ConnectionError { target, kind } => {
            *ctx.net.status.write() = kind.to_zh().to_string();
            if let Some(node_id) = target {
                let selected = get_router()
                    .and_then(|router| router.active_remote_executor())
                    .or_else(|| *ctx.net.selected_executor.read());
                if selected == Some(node_id) {
                    if let Some(router) = get_router() {
                        router.clear_remote_executor_if(node_id);
                    }
                    *ctx.net.selected_executor.write() = None;
                    *ctx.net.executing_remotely.write() = false;
                }
                let presence = if kind == ConnectErrorKind::FingerprintMismatch {
                    PeerPresence::FingerprintMismatch
                } else {
                    PeerPresence::Unreachable
                };
                update_peer(ctx, node_id, |peer| {
                    peer.presence = presence;
                    if presence != PeerPresence::Connected {
                        peer.session_id = None;
                    }
                });
            }
            if let Some(router) = get_router() {
                let view = router.view();
                apply_routing_roles(ctx, &view.active_controllers, view.active_executor);
            }
            false
        }
        UiEvent::LatencyUpdate {
            node_id,
            latency_ms,
        } => {
            update_peer(ctx, node_id, |peer| {
                peer.latency_ms = latency_ms;
            });
            false
        }
        UiEvent::RemoteControl {
            controllers,
            executor,
        } => {
            *ctx.net.controllers.write() = controllers.clone();
            *ctx.net.selected_executor.write() = executor;
            *ctx.net.remote_controlled.write() = !controllers.is_empty();
            let executing = get_router()
                .map(|router| router.is_executing_remotely())
                .unwrap_or(executor.is_some());
            *ctx.net.executing_remotely.write() = executing;
            apply_routing_roles(ctx, &controllers, executor);
            false
        }
        UiEvent::NetworkStatus { kind, text } => {
            *ctx.net.status.write() = text;
            if kind == NetworkStatusKind::ListenerUnavailable {
                *ctx.net.bind.write() = BindStatus::BindFailed {
                    port: LAN_FIXED_PORT,
                };
            }
            false
        }
        UiEvent::ScanState(state) => {
            *ctx.net.scanning.write() = matches!(state, ScanState::InFlight);
            false
        }
        UiEvent::ListenerBound { addr } => {
            *ctx.net.bind.write() = BindStatus::Bound { addr };
            false
        }
        UiEvent::ListenerFailed { port } => {
            *ctx.net.bind.write() = BindStatus::BindFailed { port };
            false
        }
        UiEvent::BindStatus(status) => {
            *ctx.net.bind.write() = status;
            false
        }
    }
}

fn populate_local_identity(ctx: &mut CalcContext) {
    NET_CONTEXT.with(|cell| {
        if let Some(ref net_ctx) = *cell.borrow() {
            *ctx.net.local_node_id.write() = Some(net_ctx.local_node_id);
            *ctx.net.local_fingerprint.write() = net_ctx.local_fingerprint.clone();
        }
    });
    if ctx.net.local_node_id.read().is_none()
        && let Some(id) = with_nm(|nm| nm.local_node_id())
    {
        *ctx.net.local_node_id.write() = Some(id);
    }
}

fn with_routing_role(mut model: PeerViewModel) -> PeerViewModel {
    if let Some(router) = get_router() {
        let view = router.view();
        if view.active_executor == Some(model.node_id) {
            model.role = PeerRole::SelectedExecutor;
        } else if view.active_controllers.contains(&model.node_id) {
            model.role = PeerRole::ControllingUs;
        }
    }
    model
}

fn upsert_peer(ctx: &mut CalcContext, model: PeerViewModel) {
    let mut peers = (*ctx.net.peers.read()).clone();
    if let Some(existing) = peers.iter_mut().find(|peer| peer.node_id == model.node_id) {
        *existing = model;
    } else {
        peers.push(model);
    }
    *ctx.net.peers.write() = peers;
}

fn update_peer(ctx: &mut CalcContext, node_id: NodeId, update: impl FnOnce(&mut PeerViewModel)) {
    let mut peers = (*ctx.net.peers.read()).clone();
    if let Some(peer) = peers.iter_mut().find(|peer| peer.node_id == node_id) {
        update(peer);
        *ctx.net.peers.write() = peers;
    }
}

fn mark_peer_session(
    ctx: &mut CalcContext,
    node_id: NodeId,
    session_id: Option<NodeId>,
    presence: PeerPresence,
) {
    let mut peers = (*ctx.net.peers.read()).clone();
    if let Some(peer) = peers.iter_mut().find(|peer| peer.node_id == node_id) {
        peer.presence = presence;
        peer.session_id = session_id;
    } else if presence == PeerPresence::Connected {
        let (display_name, endpoint, fingerprint) = lookup_peer_identity(node_id);
        peers.push(PeerViewModel {
            node_id,
            display_name,
            endpoint,
            fingerprint,
            presence,
            role: PeerRole::Idle,
            latency_ms: None,
            session_id,
        });
    }
    *ctx.net.peers.write() = peers;
}

fn lookup_peer_identity(node_id: NodeId) -> (String, Option<std::net::SocketAddr>, Option<String>) {
    NET_CONTEXT
        .with(|cell| {
            cell.borrow().as_ref().and_then(|net_ctx| {
                let state = net_ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
                state.peers.get_peer(&node_id).map(|peer| {
                    (
                        peer.display_name.clone(),
                        peer.display_endpoint(),
                        peer.public_key_fingerprint.clone(),
                    )
                })
            })
        })
        .unwrap_or_else(|| (node_id.to_string(), None, None))
}

fn apply_routing_roles(ctx: &mut CalcContext, controllers: &[NodeId], executor: Option<NodeId>) {
    let mut peers = (*ctx.net.peers.read()).clone();
    for peer in &mut peers {
        if executor == Some(peer.node_id) {
            peer.role = PeerRole::SelectedExecutor;
        } else if controllers.contains(&peer.node_id) {
            peer.role = PeerRole::ControllingUs;
        } else {
            peer.role = PeerRole::Idle;
        }
    }
    *ctx.net.peers.write() = peers;
}

fn close_overlays(ctx: &mut CalcContext) {
    *ctx.audio.about_visible.write() = false;
    *ctx.settings.panel_visible.write() = false;
    *ctx.net.panel_visible.write() = false;
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
/// Residual sync refreshes product roles from [`Router::view`] and
/// active sessions. Presence is owned by [`UiEvent`]s.
pub fn sync_network_state(mut ctx: CalcContext) {
    populate_local_identity(&mut ctx);

    let router = match get_router() {
        Some(r) => r,
        None => return,
    };

    let view = router.view();
    let active_sessions = with_nm(|nm| nm.active_session_ids()).unwrap_or_default();
    let user_muted = *ctx.audio.muted.read();
    router.set_audio_muted(user_muted || router.is_executing_remotely());

    *ctx.net.controllers.write() = view.active_controllers.clone();
    *ctx.net.selected_executor.write() = view.active_executor;
    *ctx.net.remote_controlled.write() = !view.active_controllers.is_empty();
    *ctx.net.executing_remotely.write() = router.is_executing_remotely();

    let mut peers = (*ctx.net.peers.read()).clone();
    for peer in &mut peers {
        if view.active_executor == Some(peer.node_id) {
            peer.role = PeerRole::SelectedExecutor;
        } else if view.active_controllers.contains(&peer.node_id) {
            peer.role = PeerRole::ControllingUs;
        } else {
            peer.role = PeerRole::Idle;
        }
        if active_sessions.contains(&peer.node_id) {
            if peer.presence != PeerPresence::FingerprintMismatch {
                peer.presence = PeerPresence::Connected;
            }
        }
    }
    *ctx.net.peers.write() = peers;

    let executing_remotely = router.is_executing_remotely();
    if executing_remotely {
        let name = view
            .active_executor
            .and_then(|id| {
                (*ctx.net.peers.read())
                    .iter()
                    .find(|peer| peer.node_id == id)
                    .map(|peer| peer.display_name.clone())
            })
            .unwrap_or_else(|| "远程设备".to_string());
        *ctx.net.status.write() = format!("正在使用 {name} 远程计算");
    } else if view.active_executor.is_some() {
        *ctx.net.status.write() = "正在连接设备...".to_string();
    } else if !view.active_controllers.is_empty() {
        *ctx.net.status.write() = "正在接受远程控制".to_string();
    }
}

// ---------------------------------------------------------------------------
// Network action handlers
// ---------------------------------------------------------------------------

/// Initiate a connection to a peer identified by its NodeId string.
///
/// The peer must have been discovered via LAN scan. Exactly one peer can be
/// selected as the remote calculator executor at a time.
pub fn handle_connect_peer(mut ctx: CalcContext, node_id_str: String) {
    let target = match node_id_str.parse::<NodeId>() {
        Ok(uuid) => uuid,
        Err(e) => {
            log::warn!("Invalid node ID '{}': {}", node_id_str, e);
            *ctx.net.status.write() = "无效的节点ID".to_string();
            return;
        }
    };
    handle_use_as_executor(ctx, target);
}

/// Select `target` as the single remote executor and connect if needed.
pub fn handle_use_as_executor(mut ctx: CalcContext, target: NodeId) {
    let router = match get_router() {
        Some(r) => r,
        None => return,
    };

    if target == router.local_node_id() {
        return;
    }

    if router.active_remote_executor() == Some(target) {
        *ctx.net.selected_executor.write() = Some(target);
        sync_network_state(ctx);
        return;
    }

    router.select_remote_executor(target);
    *ctx.net.selected_executor.write() = Some(target);

    let has_session = with_nm(|nm| nm.active_session_ids().contains(&target)).unwrap_or(false);
    if has_session {
        *ctx.net.status.write() = "已选择远程计算设备".to_string();
        *ctx.net.executing_remotely.write() = true;
        update_peer(&mut ctx, target, |peer| {
            peer.role = PeerRole::SelectedExecutor;
            peer.presence = PeerPresence::Connected;
        });
        log::trace!("Using existing session for peer {target}; no TCP reconnect");
        sync_network_state(ctx);
        return;
    }

    let peer_addr = NET_CONTEXT.with(|cell| {
        cell.borrow().as_ref().and_then(|net_ctx| {
            let state = net_ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .peers
                .get_peer(&target)
                .and_then(|peer| peer.service_endpoint)
        })
    });

    match peer_addr {
        Some(addr) => {
            log::info!("Connecting to peer {} at {}", target, addr);
            *ctx.net.status.write() = "正在连接设备...".to_string();
            *ctx.net.executing_remotely.write() = false;
            update_peer(&mut ctx, target, |peer| {
                peer.role = PeerRole::SelectedExecutor;
                peer.presence = PeerPresence::Connecting;
            });

            if with_nm(|nm| nm.connect_to_peer(addr, Some(target))).unwrap_or(false) {
                log::trace!("Connect command sent for {target}");
            } else {
                router.clear_remote_executor_if(target);
                *ctx.net.selected_executor.write() = None;
                *ctx.net.status.write() = "无法启动连接".to_string();
            }
        }
        None => {
            router.clear_remote_executor_if(target);
            *ctx.net.selected_executor.write() = None;
            log::warn!("Peer {target} not found in discovery table");
            *ctx.net.status.write() = "未找到设备".to_string();
            update_peer(&mut ctx, target, |peer| {
                peer.presence = PeerPresence::Unreachable;
                peer.role = PeerRole::Idle;
            });
        }
    }
}

/// Disconnect from a peer identified by its NodeId string.
///
/// Stops sending calculator actions to this peer. The authenticated TCP
/// session may remain available for later reuse.
pub fn handle_disconnect_peer(mut ctx: CalcContext, node_id_str: String) {
    let target = match node_id_str.parse::<NodeId>() {
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

    if target == router.local_node_id() {
        return;
    }

    router.clear_remote_executor_if(target);
    *ctx.net.executing_remotely.write() = false;
    if *ctx.net.selected_executor.read() == Some(target) {
        *ctx.net.selected_executor.write() = None;
    }
    *ctx.net.status.write() = "已停止远程执行".to_string();
    log::info!("Stopped remote execution on peer {target}");
    sync_network_state(ctx);
}

/// Clear the selected remote executor without tearing down TCP sessions.
pub fn handle_stop_remote_execution(mut ctx: CalcContext) {
    if let Some(router) = get_router() {
        router.clear_remote_executor();
    }
    *ctx.net.selected_executor.write() = None;
    *ctx.net.executing_remotely.write() = false;
    *ctx.net.status.write() = "已停止远程执行".to_string();
    sync_network_state(ctx);
}

/// Trigger a LAN peer discovery scan.
///
/// Broadcasts Discover + Announce messages on the local network.
/// Scan progress is owned by [`UiEvent::ScanState`]; this does not sleep.
pub fn handle_scan_peers(mut ctx: CalcContext) {
    *ctx.net.scanning.write() = true;

    if let Some(()) = with_nm(|nm| {
        nm.trigger_scan();
    }) {
        log::info!("LAN scan triggered");
    } else {
        log::warn!("Cannot scan: networking not initialised");
        *ctx.net.scanning.write() = false;
    }
}

/// Toggle the sole inbound remote-control permission boundary.
///
/// Authenticated sessions may submit valid calculator actions only while this
/// persisted switch is enabled. Turning it off takes effect immediately.
pub fn handle_toggle_remote_control(ctx: CalcContext) {
    let current = *ctx.net.allow_remote_control.read();
    let next = !current;
    handle_set_allow_remote_control(ctx, next);
}

/// Persist and apply the inbound remote-control switch.
pub fn handle_set_allow_remote_control(mut ctx: CalcContext, allow: bool) {
    *ctx.net.allow_remote_control.write() = allow;
    let mut app_config = config::AppConfig::load();
    app_config.network.allow_remote_control = allow;
    if let Err(e) = app_config.save() {
        log::error!("Failed to save remote-control config: {e}");
    }

    if let Some(router) = get_router() {
        router.set_allow_remote_control(allow);
    }

    sync_network_state(ctx.clone());
    *ctx.net.status.write() = if allow {
        "远程控制已开启".to_string()
    } else {
        "远程控制已关闭".to_string()
    };

    log::info!(
        "Remote control {}",
        if allow { "enabled" } else { "disabled" }
    );
}

/// Save the display name to config and update the network manager.
///
/// The new name is persisted to `config.toml` and broadcast to all
/// connected peers via `PeerNameUpdate`.
pub fn handle_save_display_name(mut ctx: CalcContext, name: String) {
    let trimmed = name.trim().to_string();
    if !valid_display_name(&trimmed) {
        *ctx.settings.save_status.write() = "名称需为 1-64 字节且不能含控制字符".to_string();
        return;
    }

    *ctx.settings.display_name.write() = trimmed.clone();

    if let Some(()) = with_nm(|nm| {
        nm.update_display_name(trimmed.clone());
    }) {}

    let mut app_config = config::AppConfig::load();
    app_config.network.display_name = trimmed;
    match app_config.save() {
        Ok(()) => {
            *ctx.settings.save_status.write() = "已保存".to_string();
            log::info!("Display name saved to config");
        }
        Err(e) => {
            *ctx.settings.save_status.write() = "保存失败".to_string();
            log::error!("Failed to save config: {e}");
        }
    }
}

/// Route chrome / network intents. Calculator buttons stay [`CalcAction`] 1:1.
pub fn dispatch_command(mut ctx: CalcContext, command: AppCommand) {
    match command {
        AppCommand::Calc(action) => {
            if let Some(router) = get_router() {
                router.dispatch(action);
            }
        }
        AppCommand::SetDisplayName(name) => handle_save_display_name(ctx, name),
        AppCommand::SetAllowRemoteControl(allow) => handle_set_allow_remote_control(ctx, allow),
        AppCommand::ScanNearby => handle_scan_peers(ctx),
        AppCommand::UseAsExecutor(node_id) => handle_use_as_executor(ctx, node_id),
        AppCommand::StopRemoteExecution => handle_stop_remote_execution(ctx),
        AppCommand::CloseOverlays => close_overlays(&mut ctx),
        AppCommand::ShowNearbyOverlay => {
            close_overlays(&mut ctx);
            *ctx.net.panel_visible.write() = true;
        }
        AppCommand::ShowSettingsOverlay => {
            close_overlays(&mut ctx);
            *ctx.settings.panel_visible.write() = true;
        }
        AppCommand::ToggleTheme => toggle_theme(ctx),
        AppCommand::SetWorkbenchTab(tab) => {
            *ctx.net.workbench_tab.write() = tab;
        }
        AppCommand::ShowAbout => {
            close_overlays(&mut ctx);
            *ctx.audio.about_visible.write() = true;
        }
        AppCommand::CycleAudioMode => {
            let next = ctx.audio.mode.read().next();
            *ctx.audio.mode.write() = next;
            *ctx.audio.mode_indicator.write() = next.name().to_string();
        }
        AppCommand::ToggleMute => {
            let current = *ctx.audio.muted.read();
            let next = !current;
            *ctx.audio.muted.write() = next;
            set_router_user_mute(next);
        }
        AppCommand::SetVolume(volume) => {
            *ctx.audio.volume.write() = volume;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_display_name_schema_rejects_empty_oversized_and_control_text() {
        assert!(valid_display_name("Calculator 1"));
        assert!(!valid_display_name("   "));
        assert!(!valid_display_name(
            &"x".repeat(crate::net::protocol::MAX_DISPLAY_NAME_BYTES + 1)
        ));
        assert!(!valid_display_name("bad\nname"));
    }

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
