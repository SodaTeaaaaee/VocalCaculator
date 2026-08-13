//! CalculatorUI -- root Dioxus component for the skeuomorphic calculator.
//!
//! Owns the 7-row button layout table and renders the full calculator
//! body including all sub-components: BrandLabel, PresenceBanner, StatusBar,
//! HistoryText, LcdDisplay, ButtonGrid.

use dioxus::prelude::*;

use super::about_dialog::AboutDialog;
use super::brand_label::BrandLabel;
use super::button_grid::ButtonGrid;
use super::display::LcdDisplay;
use super::history_text::HistoryText;
#[cfg(not(target_os = "android"))]
use super::icon::{Icon, IconName};
use super::keyboard::{
    KeyboardHandler, WorkbenchSurface, activate_nearby, activate_settings, use_workbench_surface,
};
use super::network_panel::NetworkPanel;
use super::presence_banner::PresenceBanner;
use super::settings_panel::SettingsPanel;
use super::status_bar::{MobileQuickActions, StatusBar};
use crate::app::network_mode::{self, NetworkMode};
use crate::components::workbench::Workbench;
use crate::net::protocol::{LAN_FIXED_PORT, NodeId};
use crate::net::view::{BindStatus, PeerViewModel};
use crate::ui::state::CalcContext;

// ---------------------------------------------------------------------------
// ButtonDef -- static descriptor for a single calculator button
// ---------------------------------------------------------------------------

/// Static descriptor for one button in the calculator grid.
///
/// Each field is `'static` so the entire layout table can live in a
/// compile-time constant.
#[derive(Debug, Clone, Copy)]
pub struct ButtonDef {
    /// Visible text label (e.g. "7", "AC", "MU").  `None` when the button
    /// uses an icon glyph instead.
    pub label: Option<&'static str>,
    /// Icon glyph (Nerd Font codepoint).  `None` when the button uses a
    /// text label instead.
    pub icon: Option<&'static str>,
    /// Visual category that maps to a CSS class suffix:
    /// "digit" | "op" | "func" | "clear" | "ci" | "bs" | "eq"
    pub btn_type: &'static str,
    /// Action identifier dispatched on click (matches keyboard-action values).
    pub action: &'static str,
    /// Number of grid columns this button spans (default 1).
    pub colspan: u8,
}

impl ButtonDef {
    const fn new(label: &'static str, btn_type: &'static str, action: &'static str) -> Self {
        Self {
            label: Some(label),
            icon: None,
            btn_type,
            action,
            colspan: 1,
        }
    }

    const fn icon(icon: &'static str, btn_type: &'static str, action: &'static str) -> Self {
        Self {
            label: None,
            icon: Some(icon),
            btn_type,
            action,
            colspan: 1,
        }
    }

    const fn with_colspan(mut self, colspan: u8) -> Self {
        self.colspan = colspan;
        self
    }
}

// ---------------------------------------------------------------------------
// BUTTON_ROWS -- 7-row static layout table
// ---------------------------------------------------------------------------

/// Static button layout for the calculator's 4-column grid.
///
/// Each inner slice is one row of the button grid.  The outer array has
/// exactly 7 entries (rows 0..6).
pub const BUTTON_ROWS: [&[ButtonDef]; 7] = [
    // Row 0: MC  MR  M-  M+
    &[
        ButtonDef::new("MC", "func", "memory-clear"),
        ButtonDef::new("MR", "func", "memory-recall"),
        ButtonDef::new("M-", "func", "memory-subtract"),
        ButtonDef::new("M+", "func", "memory-add"),
    ],
    // Row 1: %  sqrt  MU (colspan 2)
    &[
        ButtonDef::new("%", "func", "percent"),
        ButtonDef::new("\u{221A}", "func", "sqrt"),
        ButtonDef::new("MU", "func", "mu").with_colspan(2),
    ],
    // Row 2: AC  C  backspace  div
    &[
        ButtonDef::new("AC", "clear", "all-clear"),
        ButtonDef::new("C", "ci", "clear"),
        ButtonDef::icon("\u{232B}", "bs", "backspace"),
        ButtonDef::new("\u{00F7}", "op", "divide"),
    ],
    // Row 3: 7  8  9  mul
    &[
        ButtonDef::new("7", "digit", "digit:7"),
        ButtonDef::new("8", "digit", "digit:8"),
        ButtonDef::new("9", "digit", "digit:9"),
        ButtonDef::new("\u{00D7}", "op", "multiply"),
    ],
    // Row 4: 4  5  6  sub
    &[
        ButtonDef::new("4", "digit", "digit:4"),
        ButtonDef::new("5", "digit", "digit:5"),
        ButtonDef::new("6", "digit", "digit:6"),
        ButtonDef::new("-", "op", "subtract"),
    ],
    // Row 5: 1  2  3  add
    &[
        ButtonDef::new("1", "digit", "digit:1"),
        ButtonDef::new("2", "digit", "digit:2"),
        ButtonDef::new("3", "digit", "digit:3"),
        ButtonDef::new("+", "op", "add"),
    ],
    // Row 6: plus-minus  0  .  =
    &[
        ButtonDef::new("\u{00B1}", "func", "plus-minus"),
        ButtonDef::new("0", "digit", "digit:0"),
        ButtonDef::new(".", "digit", "decimal-point"),
        ButtonDef::new("=", "eq", "equals"),
    ],
];

// ---------------------------------------------------------------------------
// CalculatorUI -- root component
// ---------------------------------------------------------------------------

/// Event handlers for the calculator chrome. Display / net / audio / settings
/// are read from [`CalcContext`].
#[derive(Props, Clone, PartialEq)]
pub struct CalculatorUIProps {
    pub on_digit_pressed: EventHandler<u8>,
    pub on_decimal_point: EventHandler<()>,
    pub on_operator_pressed: EventHandler<String>,
    pub on_equals: EventHandler<()>,
    pub on_percent: EventHandler<()>,
    pub on_mu: EventHandler<()>,
    pub on_square_root: EventHandler<()>,
    pub on_backspace: EventHandler<()>,
    pub on_clear_input: EventHandler<()>,
    pub on_all_clear: EventHandler<()>,
    pub on_plus_minus: EventHandler<()>,
    pub on_memory_recall: EventHandler<()>,
    pub on_memory_add: EventHandler<()>,
    pub on_memory_subtract: EventHandler<()>,
    pub on_memory_clear: EventHandler<()>,
    pub on_switch_audio_mode: EventHandler<()>,
    pub on_toggle_mute: EventHandler<()>,
    pub on_volume_changed: EventHandler<f64>,
    pub on_toggle_theme: EventHandler<()>,
    pub on_save_display_name: EventHandler<String>,
    pub on_use_executor: EventHandler<NodeId>,
    pub on_stop_executor: EventHandler<NodeId>,
    pub on_scan_peers: EventHandler<()>,
    pub on_toggle_remote_control: EventHandler<()>,
    pub on_keyboard_action: EventHandler<String>,
    pub on_keyboard_pressed: EventHandler<bool>,
    pub on_last_action: EventHandler<String>,
}

/// Root calculator component.
///
/// Renders the full skeuomorphic calculator body with a 7-row CSS Grid
/// layout.  Delegates each visual section to a dedicated sub-component.
#[component]
pub fn CalculatorUI(props: CalculatorUIProps) -> Element {
    let ctx = use_context::<CalcContext>();
    let workbench_surface = use_workbench_surface();
    use_context_provider(|| WorkbenchSurface(workbench_surface));

    let mut split_left_px = use_signal(|| None::<i32>);
    let mut split_dragging = use_signal(|| false);
    let split_style = split_left_px()
        .map(|value| format!("--split-left: {value}px;"))
        .unwrap_or_else(|| "--split-left: 40vw;".to_string());

    let display_text = (*ctx.display.text.read()).clone();
    let history_text = (*ctx.display.history.read()).clone();
    let memory_indicator = (*ctx.display.memory_indicator.read()).clone();
    let error_state = *ctx.display.is_error.read();
    let mode_indicator = (*ctx.audio.mode_indicator.read()).clone();
    let audio_status = (*ctx.audio.audio_status.read()).clone();
    let audio_muted = *ctx.audio.muted.read();
    let audio_volume = *ctx.audio.volume.read();
    let dark_mode = *ctx.audio.dark_mode.read();
    let about_visible = *ctx.audio.about_visible.read();
    let network_status = (*ctx.net.status.read()).clone();
    let remote_controlled = *ctx.net.remote_controlled.read();
    let executing_remotely = *ctx.net.executing_remotely.read();
    let scanning = *ctx.net.scanning.read();
    let allow_remote_control = *ctx.net.allow_remote_control.read();
    let bind = (*ctx.net.bind.read()).clone();
    let fingerprint = (*ctx.net.local_fingerprint.read()).clone();
    let peers = (*ctx.net.peers.read()).clone();
    let controllers = (*ctx.net.controllers.read()).clone();
    let selected_executor = *ctx.net.selected_executor.read();
    let workbench_tab = *ctx.net.workbench_tab.read();
    let network_panel_visible = *ctx.net.panel_visible.read();
    let settings_panel_visible = *ctx.settings.panel_visible.read();
    let display_name = (*ctx.settings.display_name.read()).clone();
    let save_status = (*ctx.settings.save_status.read()).clone();
    let app_version = (*ctx.app_version.read()).clone();
    let keyboard_pressed = *ctx.keyboard_pressed.read();
    let last_keyboard_action = (*ctx.last_keyboard_action.read()).clone();

    let controller_names = controllers
        .iter()
        .map(|id| peer_display_name(&peers, *id))
        .collect::<Vec<_>>();
    let selected_executor_name = selected_executor.map(|id| peer_display_name(&peers, id));
    let port = listener_port(&bind);
    let network_mode_label = network_mode_label();
    let body_class = if executing_remotely {
        "calculator-body executing-remotely"
    } else if remote_controlled {
        "calculator-body remote-controlled"
    } else {
        "calculator-body"
    };

    rsx! {
        div {
            class: if split_dragging() { "calculator-window is-resizing" } else { "calculator-window" },
            style: "{split_style}",
            onmousemove: move |evt: MouseEvent| {
                if split_dragging() {
                    let x = evt.client_coordinates().x.round() as i32;
                    split_left_px.set(Some(x.clamp(280, 1400)));
                }
            },
            onmouseup: move |_| split_dragging.set(false),
            onmouseleave: move |_| split_dragging.set(false),

            AppChrome {}

            div { class: "calculator-stage",

                div {
                    class: body_class,

                    div { class: "calculator-body-inner",

                        BrandLabel {
                            dark_mode: dark_mode,
                            mode_indicator: mode_indicator.clone(),
                        }

                        div { class: "calculator-status-stack",
                            PresenceBanner {
                                bind: bind.clone(),
                                network_mode_label: network_mode_label.clone(),
                                status_text: network_status.clone(),
                                remote_controlled: remote_controlled,
                                executing_remotely: executing_remotely,
                                controller_names: controller_names.clone(),
                                selected_executor_name: selected_executor_name.clone(),
                                port: port,
                            }

                            StatusBar {
                            memory_indicator: memory_indicator.clone(),
                            audio_status: audio_status.clone(),
                            mode_indicator: mode_indicator.clone(),
                            network_status: network_status.clone(),
                            error_state: error_state,
                            remote_controlled: remote_controlled,
                            executing_remotely: executing_remotely,
                            on_show_network: {
                                let ctx = ctx.clone();
                                move |_| activate_nearby(&ctx, workbench_surface())
                            },
                            }
                        }

                        MobileQuickActions {
                            dark_mode: dark_mode,
                            network_status: network_status.clone(),
                            remote_controlled: remote_controlled,
                            executing_remotely: executing_remotely,
                            on_toggle_theme: props.on_toggle_theme,
                            on_switch_audio_mode: props.on_switch_audio_mode,
                            on_show_about: {
                                let mut ctx = ctx.clone();
                                move |_| {
                                    let already_open = *ctx.audio.about_visible.read();
                                    *ctx.audio.about_visible.write() = false;
                                    *ctx.settings.panel_visible.write() = false;
                                    *ctx.net.panel_visible.write() = false;
                                    if !already_open {
                                        *ctx.audio.about_visible.write() = true;
                                    }
                                }
                            },
                            on_show_network_settings: {
                                let ctx = ctx.clone();
                                move |_| activate_nearby(&ctx, workbench_surface())
                            },
                            on_show_settings: {
                                let ctx = ctx.clone();
                                move |_| activate_settings(&ctx, workbench_surface())
                            },
                        }

                        HistoryText {
                            history_text: history_text.clone(),
                            dark_mode: dark_mode,
                        }

                        LcdDisplay {
                            display_text: display_text.clone(),
                            error_state: error_state,
                            dark_mode: dark_mode,
                        }

                        ButtonGrid {
                            on_digit_pressed: props.on_digit_pressed,
                            on_decimal_point: props.on_decimal_point,
                            on_operator_pressed: props.on_operator_pressed,
                            on_equals: props.on_equals,
                            on_percent: props.on_percent,
                            on_mu: props.on_mu,
                            on_square_root: props.on_square_root,
                            on_backspace: props.on_backspace,
                            on_clear_input: props.on_clear_input,
                            on_all_clear: props.on_all_clear,
                            on_plus_minus: props.on_plus_minus,
                            on_memory_recall: props.on_memory_recall,
                            on_memory_add: props.on_memory_add,
                            on_memory_subtract: props.on_memory_subtract,
                            on_memory_clear: props.on_memory_clear,
                            keyboard_pressed: keyboard_pressed,
                            last_keyboard_action: last_keyboard_action.clone(),
                        }
                    }
                }
            }

            div {
                class: "split-handle",
                title: "调整计算器和面板宽度",
                role: "separator",
                tabindex: "0",
                onmousedown: move |evt: MouseEvent| {
                    evt.prevent_default();
                    split_dragging.set(true);
                    let x = evt.client_coordinates().x.round() as i32;
                    split_left_px.set(Some(x.clamp(280, 1400)));
                },
                onkeydown: move |evt: KeyboardEvent| {
                    let current = split_left_px().unwrap_or(520);
                    match evt.key() {
                        Key::ArrowLeft => {
                            evt.prevent_default();
                            split_left_px.set(Some((current - 24).clamp(280, 1400)));
                        }
                        Key::ArrowRight => {
                            evt.prevent_default();
                            split_left_px.set(Some((current + 24).clamp(280, 1400)));
                        }
                        _ => {}
                    }
                },
                span { class: "split-handle__grip" }
            }

            Workbench {
                active_tab: workbench_tab,
                on_tab_change: {
                    let mut ctx = ctx.clone();
                    move |tab| *ctx.net.workbench_tab.write() = tab
                },
                display_name: display_name.clone(),
                save_status: save_status.clone(),
                fingerprint: fingerprint.clone(),
                network_mode_label: network_mode_label.clone(),
                bind: bind.clone(),
                allow_remote_control: allow_remote_control,
                controller_names: controller_names.clone(),
                selected_executor_name: selected_executor_name.clone(),
                on_save_name: props.on_save_display_name,
                on_toggle_remote_control: move |_| props.on_toggle_remote_control.call(()),
                scanning: scanning,
                peers: peers.clone(),
                on_scan: move |_| props.on_scan_peers.call(()),
                on_use_executor: props.on_use_executor,
                on_stop_executor: props.on_stop_executor,
                audio_status: audio_status.clone(),
                audio_muted: audio_muted,
                audio_volume: audio_volume,
                mode_indicator: mode_indicator.clone(),
                dark_mode: dark_mode,
                app_version: app_version.clone(),
                on_switch_audio_mode: props.on_switch_audio_mode,
                on_toggle_mute: props.on_toggle_mute,
                on_volume_changed: props.on_volume_changed,
                on_toggle_theme: props.on_toggle_theme,
                on_show_about: {
                    let mut ctx = ctx.clone();
                    move |_| {
                        *ctx.settings.panel_visible.write() = false;
                        *ctx.net.panel_visible.write() = false;
                        *ctx.audio.about_visible.write() = true;
                    }
                },
            }

            if network_panel_visible {
                NetworkPanel {
                    visible: true,
                    display_name: display_name.clone(),
                    save_status: save_status.clone(),
                    fingerprint: fingerprint.clone(),
                    network_mode_label: network_mode_label.clone(),
                    bind: bind.clone(),
                    allow_remote_control: allow_remote_control,
                    controller_names: controller_names.clone(),
                    selected_executor_name: selected_executor_name.clone(),
                    scanning: scanning,
                    peers: peers.clone(),
                    onclose: {
                        let mut ctx = ctx.clone();
                        move |_| *ctx.net.panel_visible.write() = false
                    },
                    on_save_name: props.on_save_display_name,
                    ontoggle_remote_control: move |_| props.on_toggle_remote_control.call(()),
                    onscan: move |_| props.on_scan_peers.call(()),
                    onconnect: props.on_use_executor,
                    ondisconnect: props.on_stop_executor,
                }
            }

            if settings_panel_visible {
                SettingsPanel {
                    display_name: display_name.clone(),
                    save_status: save_status.clone(),
                    audio_status: audio_status.clone(),
                    audio_muted: audio_muted,
                    audio_volume: audio_volume,
                    mode_indicator: mode_indicator.clone(),
                    dark_mode: dark_mode,
                    app_version: app_version.clone(),
                    onclose: {
                        let mut ctx = ctx.clone();
                        move |_| *ctx.settings.panel_visible.write() = false
                    },
                    on_save_name: move |name| props.on_save_display_name.call(name),
                    on_switch_audio_mode: props.on_switch_audio_mode,
                    on_toggle_mute: props.on_toggle_mute,
                    on_volume_changed: props.on_volume_changed,
                    on_toggle_theme: props.on_toggle_theme,
                    on_show_about: {
                        let mut ctx = ctx.clone();
                        move |_| {
                            *ctx.settings.panel_visible.write() = false;
                            *ctx.net.panel_visible.write() = false;
                            *ctx.audio.about_visible.write() = true;
                        }
                    },
                }
            }

            if about_visible {
                AboutDialog {
                    app_version: app_version.clone(),
                    onclose: {
                        let mut ctx = ctx.clone();
                        move |_| *ctx.audio.about_visible.write() = false
                    },
                }
            }

            KeyboardHandler {
                on_keyboard_action: props.on_keyboard_action,
                on_switch_audio_mode: props.on_switch_audio_mode,
                on_toggle_mute: props.on_toggle_mute,
                on_toggle_theme: props.on_toggle_theme,
                on_scan_peers: props.on_scan_peers,
                on_keyboard_pressed: props.on_keyboard_pressed,
                on_last_action: props.on_last_action,
            }
        }
    }
}

#[component]
#[cfg(not(target_os = "android"))]
fn AppChrome() -> Element {
    let window = dioxus::desktop::use_window();
    let drag_window = window.clone();
    let close_window = window.clone();

    rsx! {
        header { class: "app-chrome", aria_label: "窗口控制栏",
            div {
                class: "app-chrome__drag-region",
                title: "拖动窗口",
                onmousedown: move |_| drag_window.drag(),
                span { class: "app-chrome__title", "VocalCalculator" }
            }

            button {
                class: "app-chrome__close",
                title: "关闭",
                aria_label: "关闭窗口",
                onclick: move |_| close_window.close(),
                Icon { name: IconName::X }
            }
        }
    }
}

#[component]
#[cfg(target_os = "android")]
fn AppChrome() -> Element {
    rsx! {}
}

fn network_mode_label() -> String {
    match network_mode::current() {
        NetworkMode::Lan => "局域网".to_string(),
        NetworkMode::Offline => "离线".to_string(),
        NetworkMode::LoopbackTest => "回环测试".to_string(),
    }
}

fn listener_port(bind: &BindStatus) -> u16 {
    match bind {
        BindStatus::Bound { addr } => addr.port(),
        BindStatus::BindFailed { port } => *port,
        BindStatus::Offline | BindStatus::Unavailable => LAN_FIXED_PORT,
    }
}

fn peer_display_name(peers: &[PeerViewModel], id: NodeId) -> String {
    peers
        .iter()
        .find(|peer| peer.node_id == id)
        .map(|peer| peer.display_name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| id.to_string())
}
