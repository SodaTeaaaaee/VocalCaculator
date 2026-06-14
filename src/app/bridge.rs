//! Bridge between UI events and the networking / calculator backend.
//!
//! Sets up the Slint timer that polls the NetworkManager and syncs the
//! peer list to the UI VecModel.  Also handles network startup and
//! configuration wiring.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::app::config;
use crate::core::calculator::Calculator;
use crate::net::protocol::{ConflictPolicy, NodeId};
use crate::net::{NetworkManager, NetworkState, PeerInfoSlint, Router};
use crate::net::CalculatorWindow;
use crate::audio::VocalAudio;
use crate::traits::DisplayUpdater;
use slint::{ComponentHandle, VecModel};

/// Create the Router (shared by all callbacks).
///
/// Audio is wrapped in [`SharedAudio`] so the Router and the audio
/// callbacks can both access the same `VocalAudio` through the shared
/// `Rc<RefCell<Option<...>>>`.
pub fn create_router(
    calc_ref: Rc<RefCell<Calculator>>,
    audio_ref: Rc<RefCell<Option<VocalAudio>>>,
    window: &CalculatorWindow,
) -> Router {
    let audio_player: Box<dyn crate::traits::AudioPlayer> =
        Box::new(SharedAudio(audio_ref));
    let display: Box<dyn crate::traits::DisplayUpdater> =
        Box::new(window.clone_strong());
    Router::new(calc_ref, Some(audio_player), display)
}

// ---------------------------------------------------------------------------
// SharedAudio -- bridges Rc<RefCell<Option<VocalAudio>>> to AudioPlayer
// ---------------------------------------------------------------------------

/// Wrapper that implements [`AudioPlayer`] by borrowing from a shared
/// `Rc<RefCell<Option<VocalAudio>>>`.  This lets the Router call audio
/// methods through the trait while the audio callbacks retain direct
/// access to the same shared cell.
struct SharedAudio(Rc<RefCell<Option<VocalAudio>>>);

impl crate::traits::AudioPlayer for SharedAudio {
    fn play_events(&mut self, events: &[crate::core::token::VocalEvent]) {
        if let Some(ref mut audio) = *self.0.borrow_mut() {
            audio.play_events(events);
        }
    }

    fn set_mode(&mut self, mode: crate::audio::AudioMode) {
        if let Some(ref mut audio) = *self.0.borrow_mut() {
            audio.set_mode(mode);
        }
    }

    fn set_volume(&mut self, slider: f64) {
        if let Some(ref mut audio) = *self.0.borrow_mut() {
            audio.set_volume(slider);
        }
    }

    fn mode(&self) -> crate::audio::AudioMode {
        self.0
            .borrow()
            .as_ref()
            .map(|a| a.mode())
            .unwrap_or(crate::audio::AudioMode::Normal)
    }
}

// ---------------------------------------------------------------------------
// DisplayUpdater impl for CalculatorWindow
// ---------------------------------------------------------------------------

impl DisplayUpdater for CalculatorWindow {
    fn update_display(&self, text: &str) {
        self.set_display_text(text.into());
    }

    fn update_history(&self, text: &str) {
        self.set_history_text(text.into());
    }

    fn update_memory_indicator(&self, indicator: &str) {
        self.set_memory_indicator(indicator.into());
    }

    fn set_error_state(&self, is_error: bool) {
        // Delegate to the Slint-generated inherent method.
        // Inherent methods take priority over trait methods in resolution,
        // so there is no infinite recursion here.
        CalculatorWindow::set_error_state(self, is_error);
    }
}

/// Shared state created during network initialization.
pub struct NetworkContext {
    pub net_state: Arc<Mutex<NetworkState>>,
    pub net_manager: Option<Rc<RefCell<NetworkManager>>>,
    pub peers_model: Rc<VecModel<PeerInfoSlint>>,
    /// Scan timer handle. Stored here so it stays alive for the
    /// SingleShot duration; dropping it would cancel the timer.
    pub scan_timer: Rc<RefCell<Option<slint::Timer>>>,
    /// Sorted node ID list used by the poll timer to render the routing
    /// matrix and by the `on_route_toggled` callback to map grid
    /// coordinates back to `NodeId` pairs.  Shared so the callback
    /// uses the exact same ordering as the last render (Bug 9 fix).
    pub matrix_node_ids: Rc<RefCell<Vec<NodeId>>>,
}

/// Initialize networking based on app config.  Returns the shared state
/// objects that callbacks and the timer will need.
pub fn init_networking(
    app_config: &config::AppConfig,
    router: &Router,
    window: &CalculatorWindow,
) -> NetworkContext {
    let net_state: Arc<Mutex<NetworkState>>;
    let net_manager: Option<Rc<RefCell<NetworkManager>>>;

    if app_config.network.enabled {
        let mut nm = NetworkManager::new(app_config.network.display_name.clone());
        // Synchronize Router and NetworkManager NodeIds so that routing
        // matrix owner IDs match session sender IDs.
        router.set_local_node_id(nm.local_node_id());
        let handle = nm.start();
        router.set_runtime_handle(handle.runtime_handle().clone());
        router.set_outgoing_tx(handle.outgoing_sender());
        net_state = nm.state();

        match app_config.network.conflict_policy.as_str() {
            "exclusive" => router.set_conflict_policy(ConflictPolicy::Exclusive),
            _ => router.set_conflict_policy(ConflictPolicy::Interleaved),
        }

        router.set_allow_remote_control(app_config.network.allow_remote_control);
        window.set_allow_remote_control(app_config.network.allow_remote_control);

        window.set_network_status("已启用".into());
        log::info!("Network enabled (name={})", app_config.network.display_name);

        net_manager = Some(Rc::new(RefCell::new(nm)));
    } else {
        net_state = Arc::new(Mutex::new(NetworkState::default()));
        window.set_network_status("".into());
        net_manager = None;
    }

    let peers_model = Rc::new(VecModel::<PeerInfoSlint>::default());
    window.set_peers(peers_model.clone().into());

    NetworkContext {
        net_state,
        net_manager,
        peers_model,
        scan_timer: Rc::new(RefCell::new(None)),
        matrix_node_ids: Rc::new(RefCell::new(Vec::new())),
    }
}

/// Start the Slint timer that polls the NetworkManager and syncs the
/// peer list to the UI VecModel (runs every 50 ms).
pub fn start_poll_timer(
    net_manager: &Option<Rc<RefCell<NetworkManager>>>,
    router: &Router,
    net_state: &Arc<Mutex<NetworkState>>,
    peers_model: &Rc<VecModel<PeerInfoSlint>>,
    window: &CalculatorWindow,
    matrix_node_ids: &Rc<RefCell<Vec<NodeId>>>,
) -> Option<slint::Timer> {
    let nm = match net_manager {
        Some(nm) => nm.clone(),
        None => return None,
    };

    let nm_timer = nm;
    let router_timer = router.clone();
    let net_state_timer = net_state.clone();
    let w_timer = window.clone_strong();
    let peers_timer = peers_model.clone();
    let matrix_node_ids_timer = matrix_node_ids.clone();
    let mut prev_sessions: HashSet<NodeId> = HashSet::new();
    // Counter for pending_control_request timeout (ticks at 50ms each).
    // If the pending request isn't resolved within ~10 seconds (200 ticks),
    // it is cleared and the route is reverted.
    // When the pending target changes, the counter resets so the new target
    // gets its full timeout budget (H3 fix).
    let mut pending_timeout_ticks: u32 = 0;
    let mut prev_pending_peer: Option<NodeId> = None;
    const PENDING_TIMEOUT_TICKS: u32 = 200; // 200 * 50ms = 10s

    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(50),
        move || {
            // Keep the Router's broadcast peer set in sync with the
            // networking runtime's active TCP sessions.  Without this,
            // broadcast_state() would always see an empty set and never
            // send state snapshots back to connected peers.
            {
                let nm_ref = nm_timer.borrow();
                router_timer.set_connected_peers(nm_ref.active_session_ids());
            }

            // --- Routing matrix lifecycle: detect session changes ------
            // Compare the current session set with the previous tick to
            // find newly connected and just-disconnected peers.
            let current_sessions: HashSet<NodeId> =
                nm_timer.borrow().active_session_ids();
            let new_peers: Vec<NodeId> =
                current_sessions.difference(&prev_sessions).copied().collect();
            let removed_peers: Vec<NodeId> =
                prev_sessions.difference(&current_sessions).copied().collect();

            // Register new peers in the routing matrix (adds their
            // self-control diagonal) and send them a full RoutingSync
            // so they can initialise their matrix from our current state.
            for peer_id in &new_peers {
                router_timer.add_remote_session(*peer_id);
                router_timer.send_routing_sync_to(*peer_id);
            }
            // Clean up routing state for peers that disconnected.
            for peer_id in &removed_peers {
                router_timer.cleanup_peer_disconnect(peer_id);
            }
            prev_sessions = current_sessions;

            // Drain incoming messages from the network runtime.
            {
                let mut nm = nm_timer.borrow_mut();
                let router_ref = router_timer.clone();
                let state_ref = net_state_timer.clone();
                nm.process_incoming(&move |sender_id, msg| {
                    // Intercept PeerNameUpdate: update the peer's display
                    // name in NetworkState so the UI picks it up on the
                    // next tick.  Still forward to Router (it will log and
                    // discard, which is harmless).
                    if let crate::net::protocol::NetworkMessage::PeerNameUpdate {
                        ref display_name,
                    } = msg
                    {
                        let mut state = state_ref.lock().unwrap_or_else(|e| e.into_inner());
                        state.peers.update_name(&sender_id, display_name);
                    }
                    router_ref.handle_network_message(sender_id, msg);
                });
            }

            // NOTE: pending_control_request is normally cleared when we
            // receive a RoutingSync from the target peer (inside
            // handle_network_message), which is proof that the session is
            // live and the peer has processed our RoutingDelta.
            //
            // However, if the TCP connection fails or the peer never
            // responds, the pending request would be stuck forever.
            // We add a timeout: if the pending request hasn't been cleared
            // within ~10 seconds, clear it and revert the route.
            {
                let is_pending = router_timer.is_awaiting_grant();
                let current_pending = router_timer.pending_control_request();
                // Reset timeout counter when the pending target changes (H3).
                if current_pending != prev_pending_peer {
                    pending_timeout_ticks = 0;
                    prev_pending_peer = current_pending;
                }
                if is_pending {
                    pending_timeout_ticks += 1;
                    if pending_timeout_ticks >= PENDING_TIMEOUT_TICKS {
                        log::warn!(
                            "Pending control request timed out after {} ticks; reverting",
                            pending_timeout_ticks,
                        );
                        // Revert the route to the pending peer.
                        if let Some(pending_peer) = router_timer.pending_control_request() {
                            let my_id = router_timer.local_node_id();
                            router_timer.set_route(my_id, pending_peer, false);
                        }
                        router_timer.clear_pending_control_request();
                        pending_timeout_ticks = 0;
                        w_timer.set_network_status("连接超时".into());
                        w_timer.set_executing_remotely(false);
                    }
                } else {
                    pending_timeout_ticks = 0;
                }
            }

            // --- Connection failure handling ----------------------------
            // Check if the Router received a ConnectionFailed message
            // from the connect task. If so, show the error immediately.
            if let Some(error_reason) = router_timer.take_connection_error() {
                log::warn!("Connection error from Router: {}", error_reason);
                let error_msg = match error_reason.as_str() {
                    "timeout" | "handshake_timeout" => "连接超时".to_string(),
                    "connection_refused" => "连接被拒绝".to_string(),
                    "connection_reset" => "连接中断".to_string(),
                    "host_unreachable" => "设备不可达".to_string(),
                    "network_unreachable" => "网络不可达".to_string(),
                    "permission_denied" => "访问被拒绝".to_string(),
                    other => format!("连接失败: {}", other),
                };
                w_timer.set_network_status(error_msg.into());
                w_timer.set_executing_remotely(false);
                pending_timeout_ticks = 0;
                prev_pending_peer = None;
            }

            // --- Matrix-based UI state ---------------------------------
            // Query the routing matrix to determine where this node's
            // actions are being executed and whether the local display
            // is muted (actions forwarded to a remote executor).
            let my_id = router_timer.local_node_id();
            let targets = router_timer.my_control_targets();
            let remote_targets: Vec<NodeId> =
                targets.into_iter().filter(|id| *id != my_id).collect();
            let is_muted = router_timer.is_muted();

            // Sync peer list to VecModel from NetworkState.
            let state = net_state_timer.lock().unwrap_or_else(|e| e.into_inner());

            let mut remote_peer_name: Option<String> = None;
            let mut connected_idx: i32 = -1;
            let mut slint_peers = Vec::new();

            for (i, (node_id, peer)) in state.peers.iter().enumerate() {
                let nid_str = node_id.to_string();
                let is_conn = remote_targets.contains(node_id);
                if is_conn {
                    connected_idx = i as i32;
                    remote_peer_name = Some(peer.display_name.clone());
                }
                slint_peers.push(PeerInfoSlint {
                    name: peer.display_name.clone().into(),
                    address: format!("{}:{}", peer.address.ip(), peer.tcp_port).into(),
                    is_connected: is_conn,
                    latency_ms: state.latency_ms.map(|v| v as i32).unwrap_or(-1),
                    index: i as i32,
                    node_id_string: nid_str.into(),
                });
            }

            let is_any_connected = state.is_connected;

            // Collect stale remote targets (in our matrix but absent
            // from the active session set) for cleanup.  Use the session
            // table, not the discovery table, because discovery entries
            // expire after 90s even when the TCP session is still alive.
            // Exclude the pending_control_request target — TCP connect is
            // async and the session won't exist yet on the first few ticks.
            let stale_targets: Vec<NodeId> = {
                let active_sessions = nm_timer.borrow().active_session_ids();
                let pending = router_timer.pending_control_request();
                remote_targets
                    .iter()
                    .filter(|t| !active_sessions.contains(t))
                    .filter(|t| pending.as_ref() != Some(*t))
                    .copied()
                    .collect()
            };

            drop(state);

            // Clean up routes to peers that vanished from discovery.
            for target in &stale_targets {
                router_timer.cleanup_peer_disconnect(target);
            }

            // Update VecModel.
            peers_timer.set_vec(slint_peers);
            w_timer.set_connected_peer_index(connected_idx);

            // Update network-status display based on routing matrix.
            if is_muted {
                if router_timer.is_awaiting_grant() {
                    w_timer.set_network_status("等待授权...".into());
                    w_timer.set_executing_remotely(false);
                } else {
                    let name = remote_peer_name.as_deref().unwrap_or("未知");
                    w_timer.set_network_status(format!("远程: {}", name).into());
                    w_timer.set_executing_remotely(true);
                }
            } else if is_any_connected {
                w_timer.set_network_status("已连接".into());
                w_timer.set_executing_remotely(false);
            } else {
                w_timer.set_network_status("已启用".into());
                w_timer.set_executing_remotely(false);
            }

            // Update remote-controlled indicator (are we being controlled?).
            let is_remote_controlled = router_timer
                .my_controllers()
                .iter()
                .any(|id| *id != my_id);
            w_timer.set_remote_controlled(is_remote_controlled);

            // --- Routing matrix UI sync -----------------------------------
            // Query the routing matrix and push the full grid to the Slint
            // UI so the NetworkPanel can render it.
            {
                let matrix = router_timer.get_routing_matrix();
                let state = net_state_timer.lock().unwrap_or_else(|e| e.into_inner());

                // Collect all unique node IDs from the matrix and sort them
                // deterministically (by display name, then by UUID string).
                let mut node_ids: Vec<NodeId> =
                    matrix.keys().flat_map(|(c, e)| vec![*c, *e]).collect();
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
                let my_id = router_timer.local_node_id();

                for (i, nid) in node_ids.iter().enumerate() {
                    if *nid == my_id {
                        my_idx = i as i32;
                    }
                    // Resolve display name: peer table entry > "本机" (self) > truncated UUID.
                    let display_name: slint::SharedString = if let Some(p) = state.peers.get_peer(nid) {
                        p.display_name.clone().into()
                    } else if *nid == my_id {
                        "本机".into()
                    } else {
                        // Truncate UUID to first 8 hex chars for readability.
                        let uuid_str = nid.to_string();
                        uuid_str[..8].to_string().into()
                    };
                    names.push(display_name);
                    for other in &node_ids {
                        cells.push(matrix.get(&(*nid, *other)).copied().unwrap_or(false));
                    }
                }

                drop(state);

                // Store the sorted node ID list so the on_route_toggled
                // callback can use the same ordering as the last render
                // (fixes Bug 9: coordinate mapping race condition).
                *matrix_node_ids_timer.borrow_mut() = node_ids.clone();

                w_timer.set_matrix_size(n as i32);
                w_timer.set_peer_names(std::rc::Rc::new(VecModel::from(names)).into());
                w_timer.set_matrix_cells(std::rc::Rc::new(VecModel::from(cells)).into());
                w_timer.set_my_index(my_idx);
            }

            // Sync mute state: effective mute = routing mute OR user toggle.
            let user_muted = w_timer.get_audio_muted();
            let routing_muted = is_muted;
            router_timer.set_audio_muted(user_muted || routing_muted);
        },
    );
    Some(timer)
}
