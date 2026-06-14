//! Slint callback registration.
//!
//! Each `register_*` function wires a group of UI callbacks to the
//! Router / state objects created in `App::run()`.

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::app::config::AppConfig;
use crate::audio::VocalAudio;
use crate::core::action::CalcAction;
use crate::core::token::BinaryOp;
use crate::net::protocol::NodeId;
use crate::net::{NetworkManager, NetworkState, Router};
use crate::net::CalculatorWindow;
use slint::ComponentHandle;

// ---------------------------------------------------------------------------
// Calculator action callbacks (15 buttons -> Router::dispatch)
// ---------------------------------------------------------------------------

pub fn register_calculator_callbacks(window: &CalculatorWindow, router: &Router) {
    {
        let router = router.clone();
        window.on_digit_pressed(move |d| {
            if (0..=9).contains(&d) {
                router.dispatch(CalcAction::Digit(d as u8));
            }
        });
    }
    {
        let router = router.clone();
        window.on_decimal_point(move || {
            router.dispatch(CalcAction::DecimalPoint);
        });
    }
    {
        let router = router.clone();
        window.on_operator_pressed(move |op: slint::SharedString| {
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
        window.on_equals(move || {
            router.dispatch(CalcAction::Equals);
        });
    }
    {
        let router = router.clone();
        window.on_percent(move || {
            router.dispatch(CalcAction::Percent);
        });
    }
    {
        let router = router.clone();
        window.on_mu(move || {
            router.dispatch(CalcAction::Mu);
        });
    }
    {
        let router = router.clone();
        window.on_square_root(move || {
            router.dispatch(CalcAction::SquareRoot);
        });
    }
    {
        let router = router.clone();
        window.on_backspace(move || {
            router.dispatch(CalcAction::Backspace);
        });
    }
    {
        let router = router.clone();
        window.on_clear_input(move || {
            router.dispatch(CalcAction::Clear);
        });
    }
    {
        let router = router.clone();
        window.on_all_clear(move || {
            router.dispatch(CalcAction::AllClear);
        });
    }
    {
        let router = router.clone();
        window.on_plus_minus(move || {
            router.dispatch(CalcAction::PlusMinus);
        });
    }
    {
        let router = router.clone();
        window.on_memory_recall(move || {
            router.dispatch(CalcAction::MemoryRecall);
        });
    }
    {
        let router = router.clone();
        window.on_memory_add(move || {
            router.dispatch(CalcAction::MemoryAdd);
        });
    }
    {
        let router = router.clone();
        window.on_memory_subtract(move || {
            router.dispatch(CalcAction::MemorySubtract);
        });
    }
    {
        let router = router.clone();
        window.on_memory_clear(move || {
            router.dispatch(CalcAction::MemoryClear);
        });
    }
}

// ---------------------------------------------------------------------------
// Keyboard action callbacks (FocusScope -> Router::dispatch + button flash)
// ---------------------------------------------------------------------------

/// Parse a keyboard action string into a [`CalcAction`].
///
/// Expected formats:
/// - `"digit:0"` through `"digit:9"`
/// - `"operator:add"`, `"operator:subtract"`, `"operator:multiply"`, `"operator:divide"`
/// - `"equals"`, `"decimal-point"`, `"backspace"`, `"all-clear"`, `"clear"`
/// - `"percent"`, `"sqrt"`, `"mu"`, `"memory-recall"`, `"plus-minus"`
/// - `"memory-add"`, `"memory-subtract"`, `"memory-clear"`
fn parse_action(action: &str) -> Option<CalcAction> {
    match action {
        // digit:N
        s if s.starts_with("digit:") => {
            let d = s.strip_prefix("digit:")?.parse::<u8>().ok()?;
            if d <= 9 { Some(CalcAction::Digit(d)) } else { None }
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
            log::trace!("Unknown keyboard action: {:?}", action);
            None
        }
    }
}

/// Register the `keyboard-action` callback on the window.
///
/// The FocusScope in Slint fires this callback with an action string
/// (e.g. `"digit:5"`, `"operator:add"`, `"equals"`). This function
/// parses the string into a [`CalcAction`] and dispatches it through the
/// [`Router`].
///
/// Visual feedback (button highlight) is handled entirely on the Slint
/// side via `keyboard-pressed` / `last-keyboard-action` properties and
/// the `keyboard-active` binding on each `SkeuBtn`.
pub fn register_keyboard_callbacks(window: &CalculatorWindow, router: &Router) {
    {
        let router = router.clone();
        window.on_keyboard_action(move |action: slint::SharedString| {
            if let Some(calc_action) = parse_action(action.as_str()) {
                router.dispatch(calc_action);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Network UI callbacks (settings panel, connect/disconnect, scan)
// ---------------------------------------------------------------------------

pub fn register_network_callbacks(
    window: &CalculatorWindow,
    router: &Router,
    net_state: &Arc<Mutex<NetworkState>>,
    net_manager: &Option<Rc<RefCell<NetworkManager>>>,
    scan_timer: &Rc<RefCell<Option<slint::Timer>>>,
    matrix_node_ids: &Rc<RefCell<Vec<NodeId>>>,
) {
    // Open / close the network settings panel.
    {
        let w = window.clone_strong();
        window.on_show_network_settings(move || {
            w.set_network_panel_visible(true);
        });
    }
    {
        let w = window.clone_strong();
        window.on_close_network_settings(move || {
            w.set_network_panel_visible(false);
        });
    }

    // Connect to a peer: parse node_id from string, look up peer in the
    // PeerTable, set Router execution target, and initiate TCP connection
    // via NetworkManager using the peer's session port.
    // Sends ControlRequest to ask the peer for permission to control them.
    {
        let router_c = router.clone();
        let net_state_c = net_state.clone();
        let nm_c = net_manager.clone();
        let w_c = window.clone_strong();
        window.on_connect_to_peer(move |nid_str: slint::SharedString| {
            let node_id: NodeId = match nid_str.as_str().parse() {
                Ok(id) => id,
                Err(e) => {
                    log::warn!("connect_to_peer: invalid node_id '{}': {}", nid_str, e);
                    return;
                }
            };
            let addr = {
                let state = net_state_c.lock().unwrap_or_else(|e| e.into_inner());
                match state.peers.get_peer(&node_id) {
                    Some(peer) => SocketAddr::new(peer.address.ip(), peer.tcp_port),
                    None => {
                        log::warn!("connect_to_peer: node {} not found in peers", node_id);
                        return;
                    }
                }
            };
            log::info!("Connecting to peer {} at {}", node_id, addr);

            let my_id = router_c.local_node_id();

            // If we were controlling someone else, release them first.
            for old_target in router_c.my_control_targets() {
                if old_target != my_id && old_target != node_id {
                    router_c.send_release_to(old_target);
                    router_c.set_route(my_id, old_target, false);
                }
            }

            // Set route to the new peer (broadcasts RoutingDelta).
            router_c.set_route(my_id, node_id, true);
            // Clear any previous pending request before setting a new one (C1).
            router_c.clear_pending_control_request();
            // Mark that we are waiting for authorization.
            router_c.set_pending_control_request(node_id);
            w_c.set_executing_remotely(false);
            w_c.set_network_status("等待授权...".into());

            // Initiate TCP connection. The poll timer will send
            // ControlRequest once the session is established.
            if let Some(ref nm) = nm_c {
                nm.borrow().connect_to_peer(addr, Some(node_id));
            }
        });
    }

    // Disconnect from the current remote peer: send RouteRevoke to
    // the peer, revoke the route in the matrix, and update UI.
    {
        let router_d = router.clone();
        let w_d = window.clone_strong();
        window.on_disconnect_peer(move |nid_str: slint::SharedString| {
            let node_id: NodeId = match nid_str.as_str().parse() {
                Ok(id) => id,
                Err(e) => {
                    log::warn!("disconnect_peer: invalid node_id '{}': {}", nid_str, e);
                    return;
                }
            };
            log::info!("Disconnecting from peer {}", node_id);
            let my_id = router_d.local_node_id();
            router_d.send_release_to(node_id);
            router_d.set_route(my_id, node_id, false);
            router_d.clear_pending_control_request();
            w_d.set_connected_peer_index(-1);
            w_d.set_executing_remotely(false);
            w_d.set_network_status("已启用".into());
        });
    }

    // Scan for LAN peers
    {
        let nm_s = net_manager.clone();
        let w_s = window.clone_strong();
        let scan_timer_s = scan_timer.clone();
        window.on_scan_peers(move || {
            if let Some(ref nm) = nm_s {
                nm.borrow().trigger_scan();
                w_s.set_scanning(true);
                let w_t = w_s.clone_strong();
                let scan_timer_c = scan_timer_s.clone();
                let timer = slint::Timer::default();
                timer.start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::from_secs(5),
                    move || {
                        w_t.set_scanning(false);
                        *scan_timer_c.borrow_mut() = None;
                    },
                );
                *scan_timer_s.borrow_mut() = Some(timer);
            }
        });
    }

    // Toggle "allow remote control" on/off.
    // When toggled off, sends ControlRelease to any peer currently controlling us.
    {
        let router_t = router.clone();
        let w_t = window.clone_strong();
        window.on_toggle_remote_control(move || {
            let current = router_t.config().allow_remote_control;
            let new_value = !current;
            router_t.set_allow_remote_control(new_value);
            w_t.set_allow_remote_control(new_value);
            log::info!("Allow remote control: {}", new_value);

            // If disabling, revoke inbound routes from all remote controllers.
            if !new_value {
                let my_id = router_t.local_node_id();
                let controllers: Vec<_> = router_t
                    .my_controllers()
                    .into_iter()
                    .filter(|id| *id != my_id)
                    .collect();
                for controller_id in controllers {
                    router_t.send_route_revoke_directed(controller_id, my_id);
                    // Use revoke_remote_route to bypass the ownership check
                    // in set_route (which rejects controller != my_id).
                    router_t.revoke_remote_route(controller_id, my_id);
                }
            }
        });
    }

    // Route toggled in the matrix UI: user clicked a cell in their own row.
    // Translate matrix grid coordinates (row, col) back to NodeIds and
    // update the routing matrix via the Router.
    //
    // When the user enables a route to a remote peer, this also initiates
    // a TCP connection (if one doesn't already exist) so that the routing
    // matrix toggle actually results in remote control, not just a local
    // matrix update with no backing session.
    //
    // Uses the shared `matrix_node_ids` list (updated by the poll timer
    // alongside the matrix cells) to guarantee the same ordering as the
    // last render.  Rebuilding the list here would race with peer
    // connect/disconnect events between render and click, shifting indices
    // and mapping the click to the wrong node (Bug 9).
    {
        let router_r = router.clone();
        let shared_ids = matrix_node_ids.clone();
        let net_state_r = net_state.clone();
        let nm_r = net_manager.clone();
        let w_r = window.clone_strong();
        window.on_route_toggled(move |row: i32, col: i32, value: bool| {
            let node_ids = shared_ids.borrow();
            let my_id = router_r.local_node_id();
            if let (Some(&controller), Some(&executor)) = (node_ids.get(row as usize), node_ids.get(col as usize))
                && controller == my_id
                && controller != executor
            {
                // If disabling a route, just update the matrix.
                // Also clear any pending control request so the poll timer
                // doesn't keep showing "等待授权..." (C1 fix).
                if !value {
                    router_r.set_route(controller, executor, false);
                    router_r.clear_pending_control_request();
                    return;
                }

                // Enabling a route to a remote peer: check if session exists.
                // Use the NetworkManager's actual session table (not the
                // Router's connected_peers which is updated on a 50ms timer).
                let has_session = nm_r.as_ref().map(|nm| {
                    nm.borrow().active_session_ids().contains(&executor)
                }).unwrap_or(false);

                if has_session {
                    // Session already exists — just set the route.
                    router_r.set_route(controller, executor, true);
                } else {
                    // No session: look up the peer address first, BEFORE
                    // modifying the routing matrix (Finding 4: avoid phantom
                    // routes if lookup fails).
                    let addr = {
                        let state = net_state_r.lock()
                            .unwrap_or_else(|e| e.into_inner());
                        state.peers.get_peer(&executor).map(|p| {
                            std::net::SocketAddr::new(p.address.ip(), p.tcp_port)
                        })
                    };
                    if let Some(addr) = addr {
                        log::info!(
                            "Route toggle: initiating connection to {} at {}",
                            executor, addr,
                        );
                        // Clean up any existing route to a different peer.
                        for old_target in router_r.my_control_targets() {
                            if old_target != my_id && old_target != executor {
                                router_r.send_release_to(old_target);
                                router_r.set_route(my_id, old_target, false);
                            }
                        }
                        // Now set the route and pending request.
                        router_r.set_route(controller, executor, true);
                        router_r.clear_pending_control_request();
                        router_r.set_pending_control_request(executor);
                        w_r.set_executing_remotely(false);
                        w_r.set_network_status("等待授权...".into());
                        if let Some(ref nm) = nm_r {
                            nm.borrow().connect_to_peer(addr, Some(executor));
                        }
                    } else {
                        log::warn!(
                            "Route toggle: peer {} not found in discovery table, cannot connect",
                            executor,
                        );
                        // Do NOT set the route — avoids phantom route.
                    }
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Settings callbacks
// ---------------------------------------------------------------------------

pub fn register_settings_callbacks(
    window: &CalculatorWindow,
    app_config: &AppConfig,
    net_manager: &Option<Rc<RefCell<NetworkManager>>>,
) {
    // Show / close settings panel
    {
        let w = window.clone_strong();
        let initial_name: slint::SharedString = app_config.network.display_name.clone().into();
        window.on_show_settings(move || {
            // Load current display name into the settings fields each time
            // the panel is opened so edits from config are reflected.
            let current = w.get_settings_display_name();
            if current.is_empty() {
                w.set_settings_display_name(initial_name.clone());
            }
            w.set_settings_save_status("".into());
            w.set_settings_panel_visible(true);
        });
    }
    {
        let w = window.clone_strong();
        window.on_close_settings(move || {
            w.set_settings_panel_visible(false);
        });
    }

    // Save display name: persist to config.toml and broadcast to peers.
    {
        let w = window.clone_strong();
        let nm = net_manager.clone();
        window.on_save_display_name(move |name: slint::SharedString| {
            let name_str = name.to_string().trim().to_string();
            if name_str.is_empty() {
                w.set_settings_save_status("名称不能为空".into());
                return;
            }
            let mut cfg = AppConfig::load();
            cfg.network.display_name = name_str.clone();
            match cfg.save() {
                Ok(()) => {
                    // Broadcast name change to all connected peers.
                    if let Some(ref nm) = nm {
                        nm.borrow_mut().update_display_name(name_str);
                    }
                    w.set_settings_save_status("已保存".into());
                    log::info!("Display name saved: {}", cfg.network.display_name);
                }
                Err(e) => {
                    w.set_settings_save_status(format!("保存失败: {}", e).into());
                    log::error!("Failed to save config: {}", e);
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Audio / theme callbacks
// ---------------------------------------------------------------------------

pub fn register_audio_callbacks(
    window: &CalculatorWindow,
    audio_ref: &Rc<RefCell<Option<VocalAudio>>>,
) {
    // Switch audio mode
    {
        let a = audio_ref.clone();
        let w = window.clone_strong();
        window.on_switch_audio_mode(move || {
            if let Some(ref mut audio) = *a.borrow_mut() {
                audio.cycle_mode();
                w.set_mode_indicator(audio.mode().name().into());
            }
        });
    }

    // Show / close about dialog
    {
        let w = window.clone_strong();
        window.on_show_about(move || {
            w.set_about_visible(true);
        });
    }
    {
        let w = window.clone_strong();
        window.on_close_about(move || {
            w.set_about_visible(false);
        });
    }

    // Toggle theme
    {
        let w = window.clone_strong();
        window.on_toggle_theme(move || {
            let current = w.get_dark_mode();
            w.set_dark_mode(!current);
        });
    }

    // Volume changed (slider 0.0..1.0 -> dB-scaled volume)
    {
        let a = audio_ref.clone();
        window.on_volume_changed(move |v| {
            if let Some(ref mut audio) = *a.borrow_mut() {
                audio.set_volume(v as f64);
            }
        });
    }

    // Toggle mute (the poll timer in bridge.rs reads audio-muted and
    // syncs it to the Router to actually suppress playback).
    {
        let w = window.clone_strong();
        window.on_toggle_mute(move || {
            let current = w.get_audio_muted();
            w.set_audio_muted(!current);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Valid action strings: digits
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Valid action strings: operator (primary + shorthand)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_operator_add() {
        assert_eq!(parse_action("operator:add"), Some(CalcAction::Operator(BinaryOp::Add)));
        assert_eq!(parse_action("add"), Some(CalcAction::Operator(BinaryOp::Add)));
    }

    #[test]
    fn parse_operator_subtract() {
        assert_eq!(parse_action("operator:subtract"), Some(CalcAction::Operator(BinaryOp::Subtract)));
        assert_eq!(parse_action("subtract"), Some(CalcAction::Operator(BinaryOp::Subtract)));
    }

    #[test]
    fn parse_operator_multiply() {
        assert_eq!(parse_action("operator:multiply"), Some(CalcAction::Operator(BinaryOp::Multiply)));
        assert_eq!(parse_action("multiply"), Some(CalcAction::Operator(BinaryOp::Multiply)));
    }

    #[test]
    fn parse_operator_divide() {
        assert_eq!(parse_action("operator:divide"), Some(CalcAction::Operator(BinaryOp::Divide)));
        assert_eq!(parse_action("divide"), Some(CalcAction::Operator(BinaryOp::Divide)));
    }

    // -----------------------------------------------------------------------
    // Valid action strings: direct actions
    // -----------------------------------------------------------------------

    #[test]
    fn parse_equals() {
        assert_eq!(parse_action("equals"), Some(CalcAction::Equals));
    }

    #[test]
    fn parse_decimal_point() {
        assert_eq!(parse_action("decimal-point"), Some(CalcAction::DecimalPoint));
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
        assert_eq!(parse_action("memory-recall"), Some(CalcAction::MemoryRecall));
    }

    #[test]
    fn parse_memory_add() {
        assert_eq!(parse_action("memory-add"), Some(CalcAction::MemoryAdd));
    }

    #[test]
    fn parse_memory_subtract() {
        assert_eq!(parse_action("memory-subtract"), Some(CalcAction::MemorySubtract));
    }

    #[test]
    fn parse_memory_clear() {
        assert_eq!(parse_action("memory-clear"), Some(CalcAction::MemoryClear));
    }

    // -----------------------------------------------------------------------
    // Boundary cases: out-of-range digit, malformed digit:, empty
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Unknown strings
    // -----------------------------------------------------------------------

    #[test]
    fn unknown_string_returns_none() {
        assert_eq!(parse_action("foobar"), None);
        assert_eq!(parse_action("equals!"), None);
        assert_eq!(parse_action(" clear"), None);
        assert_eq!(parse_action("operator:power"), None);
    }

    // -----------------------------------------------------------------------
    // Case sensitivity: parse_action expects lowercase only
    // -----------------------------------------------------------------------

    #[test]
    fn case_sensitive_uppercase_returns_none() {
        assert_eq!(parse_action("Digit:5"), None);
        assert_eq!(parse_action("DIGIT:5"), None);
        assert_eq!(parse_action("Equals"), None);
        assert_eq!(parse_action("EQUALS"), None);
        assert_eq!(parse_action("Decimal-Point"), None);
        assert_eq!(parse_action("Operator:Add"), None);
        assert_eq!(parse_action("ALL-CLEAR"), None);
    }
}
