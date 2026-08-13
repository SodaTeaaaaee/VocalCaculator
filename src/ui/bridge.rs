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
use crate::net::protocol::{NetworkMessage, valid_display_name};
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
            let _ = startup_event_tx.try_send(UiEvent::NetworkStatusUpdate(
                "网络启动失败，本机计算器仍可正常使用".to_string(),
            ));
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

    let ctx = NetworkContext { net_state };

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
        // ---- Peer discovery ----
        UiEvent::PeerDiscovered(payload) => {
            let info = PeerDisplayInfo {
                name: Signal::new(payload.name),
                address: Signal::new(payload.address),
                is_connected: Signal::new(payload.is_connected),
                route_active: Signal::new(false),
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
                NET_CONTEXT.with(|cell| {
                    if let Some(ref net_ctx) = *cell.borrow() {
                        let state = net_ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(peer) = state.peers.get_peer(&node_id) {
                            router.set_remote_public_key(node_id, peer.public_key);
                        }
                    }
                });
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
                bincode::config::standard().with_limit::<4096>(),
            ) {
                Ok((msg, consumed)) if consumed == bytes.len() => {
                    if let Ok(sender_id) = sender_uuid.parse::<uuid::Uuid>() {
                        // Intercept PeerNameUpdate: update the peer's
                        // display name in NetworkState so the UI picks
                        // it up on the next sync.
                        if let NetworkMessage::PeerNameUpdate { ref display_name } = msg {
                            if valid_display_name(display_name) {
                                NET_CONTEXT.with(|cell| {
                                    if let Some(ref net_ctx) = *cell.borrow() {
                                        let mut state = net_ctx
                                            .net_state
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        state.peers.update_name(&sender_id, display_name);
                                    }
                                });
                            } else {
                                log::warn!("Rejected invalid peer display name from {sender_id}");
                            }
                        }
                        if let Some(router) = get_router() {
                            router.handle_network_message(sender_id, msg);
                        }
                    }
                }
                Ok((_msg, consumed)) => {
                    log::warn!(
                        "Rejected network message with trailing bytes: consumed {}, frame {}",
                        consumed,
                        bytes.len()
                    );
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
                "remote_control_disabled" => "对方未开启远程控制".to_string(),
                "fingerprint_mismatch" => "发现信息与连接密钥不一致".to_string(),
                other => format!("连接失败: {}", other),
            };
            if let Some(router) = get_router() {
                router.clear_remote_executor();
            }
            *ctx.net.status.write() = error_msg;
            *ctx.net.executing_remotely.write() = false;
            true
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

    let active_target = router.active_remote_executor();
    let active_sessions = with_nm(|nm| nm.active_session_ids()).unwrap_or_default();
    let user_muted = *ctx.audio.muted.read();
    router.set_audio_muted(user_muted || router.is_executing_remotely());

    NET_CONTEXT.with(|cell| {
        let borrow = cell.borrow();
        let net_ctx = match borrow.as_ref() {
            Some(c) => c.net_state.clone(),
            None => return,
        };
        let state = net_ctx
            .lock()
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());

        let mut active_peer_name: Option<String> = None;
        let mut connected_idx: i32 = -1;
        let mut new_peers = Vec::new();

        for (i, (node_id, peer)) in state.peers.iter().enumerate() {
            let nid_str: String = node_id.to_string();
            let route_active = active_target == Some(*node_id);
            let is_connected = active_sessions.contains(node_id);
            if route_active {
                connected_idx = i as i32;
                active_peer_name = Some(peer.display_name.clone());
            }
            new_peers.push(PeerDisplayInfo {
                name: Signal::new(peer.display_name.clone()),
                address: Signal::new(
                    peer.display_endpoint()
                        .map(|address| address.to_string())
                        .unwrap_or_default(),
                ),
                is_connected: Signal::new(is_connected),
                route_active: Signal::new(route_active),
                latency_ms: Signal::new(state.latency_ms.map(|v| v as i32).unwrap_or(-1)),
                index: Signal::new(i as i32),
                node_id_string: Signal::new(nid_str),
            });
        }

        *ctx.net.peers.write() = new_peers;
        *ctx.net.connected_peer_index.write() = connected_idx;

        let executing_remotely = router.is_executing_remotely();
        let active_controllers = router.active_remote_controllers();
        if executing_remotely {
            let name = active_peer_name.as_deref().unwrap_or("远程设备");
            *ctx.net.status.write() = format!("正在使用 {} 远程计算", name);
        } else if active_target.is_some() {
            *ctx.net.status.write() = "正在连接设备...".to_string();
        } else if !active_controllers.is_empty() {
            *ctx.net.status.write() = "正在接受远程控制".to_string();
        } else if state.is_connected {
            *ctx.net.status.write() = "已连接".to_string();
        } else {
            *ctx.net.status.write() = "已启用".to_string();
        }
        *ctx.net.executing_remotely.write() = executing_remotely;
        *ctx.net.remote_controlled.write() = !active_controllers.is_empty();
    });
}

// ---------------------------------------------------------------------------
// Network action handlers
// ---------------------------------------------------------------------------

/// Initiate a connection to a peer identified by its NodeId string.
///
/// The peer must have been discovered via LAN scan. Exactly one peer can be
/// selected as the remote calculator executor at a time.
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

    if target == router.local_node_id() {
        return;
    }

    if router.active_remote_executor() == Some(target) {
        sync_network_state(ctx);
        return;
    }

    router.select_remote_executor(target);
    let has_session = with_nm(|nm| nm.active_session_ids().contains(&target)).unwrap_or(false);
    if has_session {
        *ctx.net.status.write() = "已选择远程计算设备".to_string();
        *ctx.net.executing_remotely.write() = true;
        log::trace!(
            "Using existing session for peer {}; no TCP reconnect",
            target
        );
        return;
    }

    // Look up the stable service endpoint from NetworkState and connect via TCP.
    let peer_addr = NET_CONTEXT.with(|cell| {
        cell.borrow().as_ref().and_then(|net_ctx| {
            let state = net_ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .peers
                .get_peer(&target)
                .and_then(|p| p.service_endpoint)
        })
    });

    match peer_addr {
        Some(addr) => {
            log::info!("Connecting to peer {} at {}", target, addr);
            *ctx.net.status.write() = "正在连接设备...".to_string();
            *ctx.net.executing_remotely.write() = false;

            if with_nm(|nm| nm.connect_to_peer(addr, Some(target))).unwrap_or(false) {
                log::trace!("Connect command sent for {}", target);
            } else {
                router.clear_remote_executor_if(target);
                *ctx.net.status.write() = "无法启动连接".to_string();
            }
        }
        None => {
            router.clear_remote_executor_if(target);
            log::warn!("Peer {} not found in discovery table", target);
            *ctx.net.status.write() = "未找到设备".to_string();
        }
    }
}

/// Disconnect from a peer identified by its NodeId string.
///
/// Stops sending calculator actions to this peer. The authenticated TCP
/// session may remain available for later reuse.
pub fn handle_disconnect_peer(mut ctx: CalcContext, node_id_str: String) {
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

    if target == router.local_node_id() {
        return;
    }

    router.clear_remote_executor_if(target);
    *ctx.net.executing_remotely.write() = false;
    *ctx.net.status.write() = "已停止远程执行".to_string();
    log::info!("Stopped remote execution on peer {}", target);
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

/// Toggle the sole inbound remote-control permission boundary.
///
/// Authenticated sessions may submit valid calculator actions only while this
/// persisted switch is enabled. Turning it off takes effect immediately.
pub fn handle_toggle_remote_control(mut ctx: CalcContext) {
    let current = *ctx.net.allow_remote_control.read();
    let next = !current;
    *ctx.net.allow_remote_control.write() = next;
    let mut app_config = config::AppConfig::load();
    app_config.network.allow_remote_control = next;
    if let Err(e) = app_config.save() {
        log::error!("Failed to save remote-control config: {}", e);
    }

    if let Some(router) = get_router() {
        router.set_allow_remote_control(next);
    }

    sync_network_state(ctx.clone());
    *ctx.net.status.write() = if next {
        "远程控制已开启".to_string()
    } else {
        "远程控制已关闭".to_string()
    };

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
    if !valid_display_name(&trimmed) {
        *ctx.settings.save_status.write() = "名称需为 1-64 字节且不能含控制字符".to_string();
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
