pub mod config;

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::audio::VocalAudio;
use crate::core::action::CalcAction;
use crate::core::calculator::Calculator;
use crate::core::token::BinaryOp;
use crate::net::protocol::ConflictPolicy;
use crate::net::{CalculatorWindow, ExecutionTarget, NetworkManager, NetworkState, PeerInfoSlint, Router};
use slint::{ComponentHandle, VecModel};

/// Central application controller.
pub struct App {
    calculator: Calculator,
    audio: Option<VocalAudio>,
    window: CalculatorWindow,
}

impl App {
    pub fn new() -> Result<Self, slint::PlatformError> {
        let window = CalculatorWindow::new()?;
        let (audio, status) = match VocalAudio::new() {
            Some(a) => {
                let n = a.sound_count();
                (Some(a), format!("音频正常 ({n} 个音效)"))
            }
            None => (None, "无音频设备".to_string()),
        };
        let calculator = Calculator::new();
        let app = Self {
            calculator,
            audio,
            window,
        };
        app.window.set_audio_status(status.into());

        // Detect system dark mode and apply
        let dark = crate::detect_system_dark_mode();
        app.window.set_dark_mode(dark);

        Ok(app)
    }

    pub fn run(mut self) -> Result<(), slint::PlatformError> {
        let w = self.window.clone_strong();
        let calc_ref = Rc::new(RefCell::new(self.calculator));
        let audio_ref = Rc::new(RefCell::new(self.audio.take()));

        // Build the Router that all callbacks will dispatch through
        let router = Router::new(calc_ref, audio_ref.clone(), w.clone_strong());

        w.set_display_text("0".into());
        w.set_mode_indicator("普通".into());
        if audio_ref.borrow().is_none() {
            w.set_audio_status("无音频设备".into());
        }

        // =====================================================================
        // Network startup
        // =====================================================================

        let app_config = config::AppConfig::load();

        // Shared network state for the Slint timer to poll.
        let net_state: Arc<Mutex<NetworkState>>;

        // The NetworkManager must outlive the event loop, but it can't be
        // stored inside a Slint callback closure (not `Send`). We use
        // Rc<RefCell<>> on the main thread.
        let net_manager: Option<Rc<RefCell<NetworkManager>>> = if app_config.network.enabled {
            let mut nm = NetworkManager::new(
                app_config.network.display_name.clone(),
            );

            // Start the networking runtime.
            let handle = nm.start();
            router.set_runtime_handle(handle.runtime_handle().clone());
            router.set_outgoing_tx(handle.outgoing_sender());
            net_state = nm.state();

            // Apply conflict policy from config.
            match app_config.network.conflict_policy.as_str() {
                "exclusive" => {
                    router.set_conflict_policy(ConflictPolicy::Exclusive);
                }
                _ => {
                    router.set_conflict_policy(ConflictPolicy::Interleaved);
                }
            }

            if app_config.network.allow_remote_control {
                router.set_allow_remote_control(true);
            }

            w.set_network_status("已启用".into());
            log::info!(
                "Network enabled (name={})",
                app_config.network.display_name,
            );

            Some(Rc::new(RefCell::new(nm)))
        } else {
            net_state = Arc::new(Mutex::new(NetworkState::default()));
            w.set_network_status("".into());
            None
        };

        // =====================================================================
        // Peer list model (VecModel + index-to-NodeId mapping)
        // =====================================================================

        let peers_model = Rc::new(VecModel::<PeerInfoSlint>::default());
        w.set_peers(peers_model.clone().into());

        // Maps VecModel index -> NodeId for callback lookups.
        let node_id_map: Rc<RefCell<Vec<crate::net::protocol::NodeId>>> =
            Rc::new(RefCell::new(Vec::new()));

        // =====================================================================
        // Slint timer: poll NetworkManager + sync peer list (500ms)
        // =====================================================================

        if let Some(ref nm) = net_manager {
            let nm_timer = nm.clone();
            let router_timer = router.clone();
            let net_state_timer = net_state.clone();
            let w_timer = w.clone_strong();
            let peers_timer = peers_model.clone();
            let node_map_timer = node_id_map.clone();

            let timer = slint::Timer::default();
            timer.start(
                slint::TimerMode::Repeated,
                std::time::Duration::from_millis(500),
                move || {
                    // Drain incoming messages from the network runtime.
                    {
                        let mut nm = nm_timer.borrow_mut();
                        let router_ref = router_timer.clone();
                        nm.process_incoming(&move |msg| {
                            router_ref.handle_network_message(msg);
                        });
                    }

                    // Sync peer list to VecModel from NetworkState.
                    let connected_target = router_timer.config().execution_target;
                    let state = net_state_timer.lock().unwrap();

                    let mut new_map = Vec::new();
                    let mut remote_peer_name: Option<String> = None;
                    let mut connected_idx: i32 = -1;
                    let mut slint_peers = Vec::new();

                    for (i, (node_id, peer)) in state.peers.iter().enumerate() {
                        new_map.push(*node_id);
                        let is_conn = match &connected_target {
                            ExecutionTarget::Remote(id) if *id == *node_id => {
                                connected_idx = i as i32;
                                remote_peer_name = Some(peer.display_name.clone());
                                true
                            }
                            _ => false,
                        };
                        slint_peers.push(PeerInfoSlint {
                            name: peer.display_name.clone().into(),
                            address: format!("{}:{}", peer.address.ip(), peer.tcp_port)
                                .into(),
                            is_connected: is_conn,
                            latency_ms: state.latency_ms.map(|v| v as i32).unwrap_or(-1),
                            index: i as i32,
                        });
                    }

                    let is_any_connected = state.is_connected;
                    drop(state);

                    // Update index-to-NodeId mapping and VecModel.
                    *node_map_timer.borrow_mut() = new_map;
                    peers_timer.set_vec(slint_peers);
                    w_timer.set_connected_peer_index(connected_idx);

                    // If the connected peer vanished from discovery, fall back
                    // to local execution.
                    if connected_idx == -1
                        && matches!(connected_target, ExecutionTarget::Remote(_))
                    {
                        router_timer.set_execution_target(ExecutionTarget::Local);
                        w_timer.set_executing_remotely(false);
                    }

                    // Update network-status display.
                    match &connected_target {
                        ExecutionTarget::Remote(_) => {
                            let name = remote_peer_name.as_deref().unwrap_or("未知");
                            w_timer
                                .set_network_status(format!("远程: {}", name).into());
                            w_timer.set_executing_remotely(true);
                        }
                        ExecutionTarget::Local => {
                            if is_any_connected {
                                w_timer.set_network_status("已连接".into());
                            } else {
                                w_timer.set_network_status("已启用".into());
                            }
                            w_timer.set_executing_remotely(false);
                        }
                    }
                },
            );
        }

        // =====================================================================
        // Network UI callbacks
        // =====================================================================

        // Open / close the network settings panel.
        {
            let w2 = w.clone_strong();
            w.on_show_network_settings(move || {
                w2.set_network_panel_visible(true);
            });
        }
        {
            let w2 = w.clone_strong();
            w.on_close_network_settings(move || {
                w2.set_network_panel_visible(false);
            });
        }

        // Connect to a peer: look up NodeId by index, set Router execution
        // target, initiate TCP connection via NetworkManager.
        {
            let router_c = router.clone();
            let net_state_c = net_state.clone();
            let node_map_c = node_id_map.clone();
            let nm_c = net_manager.clone();
            let w_c = w.clone_strong();
            w.on_connect_to_peer(move |idx| {
                let idx = idx as usize;
                let node_id = {
                    let map = node_map_c.borrow();
                    if idx >= map.len() {
                        log::warn!("connect_to_peer: index {} out of range", idx);
                        return;
                    }
                    map[idx]
                };
                let addr = {
                    let state = net_state_c.lock().unwrap();
                    match state.peers.get(&node_id) {
                        Some(peer) => SocketAddr::new(peer.address.ip(), peer.tcp_port),
                        None => {
                            log::warn!("connect_to_peer: node {} not found in peers", node_id);
                            return;
                        }
                    }
                };
                log::info!("Connecting to peer {} at {}", node_id, addr);
                router_c.set_execution_target(ExecutionTarget::Remote(node_id));
                w_c.set_connected_peer_index(idx as i32);
                w_c.set_executing_remotely(true);
                if let Some(ref nm) = nm_c {
                    nm.borrow().connect_to_peer(addr);
                }
            });
        }

        // Disconnect from the current remote peer: reset Router to local
        // execution and update UI.
        {
            let router_d = router.clone();
            let w_d = w.clone_strong();
            w.on_disconnect_peer(move |_idx| {
                log::info!("Disconnecting from remote peer");
                router_d.set_execution_target(ExecutionTarget::Local);
                w_d.set_connected_peer_index(-1);
                w_d.set_executing_remotely(false);
                w_d.set_network_status("已启用".into());
            });
        }

        // Scan for LAN peers
        {
            let nm_s = net_manager.clone();
            let w_s = w.clone_strong();
            w.on_scan_peers(move || {
                if let Some(ref nm) = nm_s {
                    nm.borrow().trigger_scan();
                    w_s.set_scanning(true);
                    let w_t = w_s.clone_strong();
                    let scan_timer_ref: Rc<RefCell<Option<slint::Timer>>> =
                        Rc::new(RefCell::new(None));
                    let scan_timer_ref2 = scan_timer_ref.clone();
                    let timer = slint::Timer::default();
                    timer.start(
                        slint::TimerMode::SingleShot,
                        std::time::Duration::from_secs(5),
                        move || {
                            w_t.set_scanning(false);
                            *scan_timer_ref2.borrow_mut() = None;
                        },
                    );
                    *scan_timer_ref.borrow_mut() = Some(timer);
                }
            });
        }

        // =====================================================================
        // Calculator action callbacks (via Router)
        // =====================================================================

        {
            let router = router.clone();
            w.on_digit_pressed(move |d| {
                router.dispatch(CalcAction::Digit(d as u8));
            });
        }

        {
            let router = router.clone();
            w.on_decimal_point(move || {
                router.dispatch(CalcAction::DecimalPoint);
            });
        }

        {
            let router = router.clone();
            w.on_operator_pressed(move |op: slint::SharedString| {
                let binary_op = match op.as_str() {
                    "+" => BinaryOp::Add,
                    "-" => BinaryOp::Subtract,
                    "*" => BinaryOp::Multiply,
                    "/" => BinaryOp::Divide,
                    _ => return,
                };
                router.dispatch(CalcAction::Operator(binary_op));
            });
        }

        {
            let router = router.clone();
            w.on_equals(move || {
                router.dispatch(CalcAction::Equals);
            });
        }

        {
            let router = router.clone();
            w.on_percent(move || {
                router.dispatch(CalcAction::Percent);
            });
        }

        {
            let router = router.clone();
            w.on_mu(move || {
                router.dispatch(CalcAction::Mu);
            });
        }

        {
            let router = router.clone();
            w.on_square_root(move || {
                router.dispatch(CalcAction::SquareRoot);
            });
        }

        {
            let router = router.clone();
            w.on_backspace(move || {
                router.dispatch(CalcAction::Backspace);
            });
        }

        {
            let router = router.clone();
            w.on_clear_input(move || {
                router.dispatch(CalcAction::Clear);
            });
        }

        {
            let router = router.clone();
            w.on_all_clear(move || {
                router.dispatch(CalcAction::AllClear);
            });
        }

        {
            let router = router.clone();
            w.on_plus_minus(move || {
                router.dispatch(CalcAction::PlusMinus);
            });
        }

        {
            let router = router.clone();
            w.on_memory_recall(move || {
                router.dispatch(CalcAction::MemoryRecall);
            });
        }

        {
            let router = router.clone();
            w.on_memory_add(move || {
                router.dispatch(CalcAction::MemoryAdd);
            });
        }

        {
            let router = router.clone();
            w.on_memory_subtract(move || {
                router.dispatch(CalcAction::MemorySubtract);
            });
        }

        {
            let router = router.clone();
            w.on_memory_clear(move || {
                router.dispatch(CalcAction::MemoryClear);
            });
        }

        // =====================================================================
        // Non-calculator callbacks
        // =====================================================================

        // Switch audio mode
        {
            let a = audio_ref.clone();
            let w2 = w.clone_strong();
            w.on_switch_audio_mode(move || {
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.cycle_mode();
                    w2.set_mode_indicator(audio.mode().name().into());
                }
            });
        }

        // Show about (placeholder)
        w.on_show_about(move || {});

        // Toggle theme
        {
            let w2 = w.clone_strong();
            w.on_toggle_theme(move || {
                let current = w2.get_dark_mode();
                w2.set_dark_mode(!current);
            });
        }

        // Volume changed (slider 0.0..1.0 -> dB-scaled volume)
        {
            let a = audio_ref.clone();
            w.on_volume_changed(move |v| {
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.set_volume(v as f64);
                }
            });
        }

        // =====================================================================
        // Run the Slint event loop (blocks until window closes)
        // =====================================================================

        let result = w.run();

        // =====================================================================
        // Graceful shutdown
        // =====================================================================

        if let Some(nm) = net_manager {
            nm.borrow_mut().shutdown();
        }

        result
    }
}
