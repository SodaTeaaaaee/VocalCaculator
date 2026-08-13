//! CalculatorUI -- root Dioxus component for the skeuomorphic calculator.
//!
//! Owns the 7-row button layout table and renders the full calculator
//! body including all sub-components: BrandLabel, StatusBar,
//! HistoryText, LcdDisplay, ButtonGrid.

use dioxus::prelude::*;

use super::about_dialog::AboutDialog;
use super::brand_label::BrandLabel;
use super::button_grid::ButtonGrid;
use super::display::LcdDisplay;
use super::history_text::HistoryText;
#[cfg(not(target_os = "android"))]
use super::icon::{Icon, IconName};
use super::keyboard::KeyboardHandler;
use super::network_panel::{NetworkPanel, NetworkPanelContent, PeerDisplayInfo};
use super::settings_panel::{SettingsContent, SettingsPanel};
use super::status_bar::{MobileQuickActions, StatusBar};

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

/// Props for the CalculatorUI root component.
///
/// All display state is passed in as simple `String` / `bool` props.
/// Signal bridging (Phase 4) will replace these with reactive signals.
#[derive(Props, Clone, PartialEq)]
pub struct CalculatorUIProps {
    // -- Display data --
    pub display_text: String,
    pub history_text: String,
    pub memory_indicator: String,
    pub mode_indicator: String,
    pub error_state: bool,
    pub audio_status: String,
    pub audio_muted: bool,
    pub audio_volume: f64,
    pub dark_mode: bool,

    // -- Network status --
    pub network_status: String,
    pub remote_controlled: bool,
    pub executing_remotely: bool,

    // -- Overlay visibility --
    pub about_visible: bool,
    pub settings_panel_visible: bool,
    pub network_panel_visible: bool,
    pub scanning: bool,
    pub allow_remote_control: bool,

    // -- Remote calculator peers --
    pub peers: Vec<PeerDisplayInfo>,
    pub connected_peer_index: i32,

    // -- App metadata --
    pub app_version: String,

    // -- Settings --
    pub settings_display_name: String,
    pub settings_save_status: String,

    // -- Calculator event handlers --
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

    // -- Audio callbacks --
    pub on_switch_audio_mode: EventHandler<()>,
    pub on_toggle_mute: EventHandler<()>,
    pub on_volume_changed: EventHandler<f64>,

    // -- Theme --
    pub on_toggle_theme: EventHandler<()>,

    // -- About dialog --
    pub on_show_about: EventHandler<()>,
    pub on_close_about: EventHandler<()>,

    // -- Settings panel --
    pub on_show_settings: EventHandler<()>,
    pub on_close_settings: EventHandler<()>,
    pub on_save_display_name: EventHandler<String>,

    // -- Network panel --
    pub on_show_network_settings: EventHandler<()>,
    pub on_close_network_settings: EventHandler<()>,
    pub on_connect_to_peer: EventHandler<String>,
    pub on_disconnect_peer: EventHandler<String>,
    pub on_scan_peers: EventHandler<()>,
    pub on_toggle_remote_control: EventHandler<()>,

    // -- Keyboard --
    pub keyboard_pressed: bool,
    pub last_keyboard_action: String,
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
    let mut split_left_px = use_signal(|| None::<i32>);
    let mut split_dragging = use_signal(|| false);
    let mut workbench_tab = use_signal(|| WorkbenchTab::Overview);
    let split_style = split_left_px()
        .map(|value| format!("--split-left: {value}px;"))
        .unwrap_or_else(|| "--split-left: 40vw;".to_string());

    // Remote-controlled glow state (applied via CSS class)
    let body_class = if props.executing_remotely {
        "calculator-body executing-remotely"
    } else if props.remote_controlled {
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

                        // [0] Brand and active mode
                        BrandLabel {
                            dark_mode: props.dark_mode,
                            mode_indicator: props.mode_indicator.clone(),
                        }

                        // [1] Status bar
                        StatusBar {
                            memory_indicator: props.memory_indicator.clone(),
                            audio_status: props.audio_status.clone(),
                            mode_indicator: props.mode_indicator.clone(),
                            network_status: props.network_status.clone(),
                            error_state: props.error_state,
                            remote_controlled: props.remote_controlled,
                            executing_remotely: props.executing_remotely,
                        }

                        // [2] Mobile-only quick actions
                        MobileQuickActions {
                            dark_mode: props.dark_mode,
                            network_status: props.network_status.clone(),
                            remote_controlled: props.remote_controlled,
                            executing_remotely: props.executing_remotely,
                            on_toggle_theme: props.on_toggle_theme,
                            on_switch_audio_mode: props.on_switch_audio_mode,
                            on_show_about: props.on_show_about,
                            on_show_network_settings: props.on_show_network_settings,
                            on_show_settings: props.on_show_settings,
                        }

                        // [3] History text
                        HistoryText {
                            history_text: props.history_text.clone(),
                            dark_mode: props.dark_mode,
                        }

                        // [4] LCD display
                        LcdDisplay {
                            display_text: props.display_text.clone(),
                            error_state: props.error_state,
                            dark_mode: props.dark_mode,
                        }

                        // [5] Button grid (7 rows)
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
                            keyboard_pressed: props.keyboard_pressed,
                            last_keyboard_action: props.last_keyboard_action.clone(),
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
                active_tab: workbench_tab(),
                audio_status: props.audio_status.clone(),
                audio_muted: props.audio_muted,
                audio_volume: props.audio_volume,
                mode_indicator: props.mode_indicator.clone(),
                dark_mode: props.dark_mode,
                network_status: props.network_status.clone(),
                remote_controlled: props.remote_controlled,
                executing_remotely: props.executing_remotely,
                scanning: props.scanning,
                allow_remote_control: props.allow_remote_control,
                settings_display_name: props.settings_display_name.clone(),
                settings_save_status: props.settings_save_status.clone(),
                app_version: props.app_version.clone(),
                peers: props.peers.clone(),
                connected_peer_index: props.connected_peer_index,
                on_tab_change: move |tab| workbench_tab.set(tab),
                on_switch_audio_mode: props.on_switch_audio_mode,
                on_toggle_mute: props.on_toggle_mute,
                on_volume_changed: props.on_volume_changed,
                on_toggle_theme: props.on_toggle_theme,
                on_show_about: props.on_show_about,
                on_save_display_name: props.on_save_display_name,
                on_connect_to_peer: props.on_connect_to_peer,
                on_disconnect_peer: props.on_disconnect_peer,
                on_scan_peers: props.on_scan_peers,
                on_toggle_remote_control: props.on_toggle_remote_control,
            }

            if props.network_panel_visible {
                NetworkPanel {
                    visible: true,
                    network_status: props.network_status.clone(),
                    remote_controlled: props.remote_controlled,
                    executing_remotely: props.executing_remotely,
                    scanning: props.scanning,
                    allow_remote_control: props.allow_remote_control,
                    audio_muted: props.audio_muted,
                    peers: props.peers.clone(),
                    connected_peer_index: props.connected_peer_index,
                    onclose: move |_| props.on_close_network_settings.call(()),
                    onconnect: move |id| props.on_connect_to_peer.call(id),
                    ondisconnect: move |id| props.on_disconnect_peer.call(id),
                    onscan: move |_| props.on_scan_peers.call(()),
                    ontoggle_remote_control: move |_| props.on_toggle_remote_control.call(()),
                    ontoggle_mute: move |_| props.on_toggle_mute.call(()),
                }
            }

            if props.settings_panel_visible {
                SettingsPanel {
                    display_name: props.settings_display_name.clone(),
                    save_status: props.settings_save_status.clone(),
                    audio_status: props.audio_status.clone(),
                    audio_muted: props.audio_muted,
                    audio_volume: props.audio_volume,
                    mode_indicator: props.mode_indicator.clone(),
                    dark_mode: props.dark_mode,
                    app_version: props.app_version.clone(),
                    onclose: move |_| props.on_close_settings.call(()),
                    on_save_name: move |name| props.on_save_display_name.call(name),
                    on_switch_audio_mode: props.on_switch_audio_mode,
                    on_toggle_mute: props.on_toggle_mute,
                    on_volume_changed: props.on_volume_changed,
                    on_toggle_theme: props.on_toggle_theme,
                    on_show_about: props.on_show_about,
                }
            }

            if props.about_visible {
                AboutDialog {
                    app_version: props.app_version.clone(),
                    onclose: move |_| props.on_close_about.call(()),
                }
            }

            KeyboardHandler {
                network_panel_visible: props.network_panel_visible,
                settings_panel_visible: props.settings_panel_visible,
                about_visible: props.about_visible,
                on_keyboard_action: props.on_keyboard_action,
                on_close_about: props.on_close_about,
                on_close_settings: props.on_close_settings,
                on_close_network_settings: props.on_close_network_settings,
                on_switch_audio_mode: props.on_switch_audio_mode,
                on_toggle_mute: props.on_toggle_mute,
                on_toggle_theme: props.on_toggle_theme,
                on_show_about: props.on_show_about,
                on_show_settings: props.on_show_settings,
                on_show_network_settings: props.on_show_network_settings,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkbenchTab {
    Overview,
    Settings,
    Network,
}

#[derive(Props, Clone, PartialEq)]
struct WorkbenchProps {
    active_tab: WorkbenchTab,
    audio_status: String,
    audio_muted: bool,
    audio_volume: f64,
    mode_indicator: String,
    dark_mode: bool,
    network_status: String,
    remote_controlled: bool,
    executing_remotely: bool,
    scanning: bool,
    allow_remote_control: bool,
    settings_display_name: String,
    settings_save_status: String,
    app_version: String,
    peers: Vec<PeerDisplayInfo>,
    connected_peer_index: i32,
    on_tab_change: EventHandler<WorkbenchTab>,
    on_switch_audio_mode: EventHandler<()>,
    on_toggle_mute: EventHandler<()>,
    on_volume_changed: EventHandler<f64>,
    on_toggle_theme: EventHandler<()>,
    on_show_about: EventHandler<()>,
    on_save_display_name: EventHandler<String>,
    on_connect_to_peer: EventHandler<String>,
    on_disconnect_peer: EventHandler<String>,
    on_scan_peers: EventHandler<()>,
    on_toggle_remote_control: EventHandler<()>,
}

#[component]
fn Workbench(props: WorkbenchProps) -> Element {
    let current_tab = props.active_tab;

    let overview_tab_class = if current_tab == WorkbenchTab::Overview {
        "workbench-tab is-active"
    } else {
        "workbench-tab"
    };
    let settings_tab_class = if current_tab == WorkbenchTab::Settings {
        "workbench-tab is-active"
    } else {
        "workbench-tab"
    };
    let network_tab_class = if current_tab == WorkbenchTab::Network {
        "workbench-tab is-active"
    } else {
        "workbench-tab"
    };

    let net_state = if props.executing_remotely {
        "正在远程执行".to_string()
    } else if props.remote_controlled {
        "受远程控制".to_string()
    } else if props.network_status.is_empty() {
        "未连接".to_string()
    } else {
        props.network_status.clone()
    };
    let status_class = if props.executing_remotely {
        "workbench-status workbench-status--executing"
    } else if props.remote_controlled {
        "workbench-status workbench-status--remote"
    } else {
        "workbench-status"
    };
    let mute_text = if props.audio_muted {
        "取消静音"
    } else {
        "静音"
    };
    let mute_state_text = if props.audio_muted {
        "已静音"
    } else {
        "播放中"
    };
    let remote_state_text = if props.allow_remote_control {
        "允许请求"
    } else {
        "禁止"
    };
    let peer_count = props.peers.len();
    let connected_count = props.peers.iter().filter(|peer| peer.is_connected).count();
    let volume_percent = (props.audio_volume * 100.0).round() as i32;

    rsx! {
        aside { class: "workbench", aria_label: "状态工作区",
            div { class: "workbench-tabs", role: "tablist", aria_label: "工作区标签",
                button {
                    class: overview_tab_class,
                    role: "tab",
                    aria_selected: if current_tab == WorkbenchTab::Overview { "true" } else { "false" },
                    onclick: move |_| props.on_tab_change.call(WorkbenchTab::Overview),
                    "概览"
                }
                button {
                    class: settings_tab_class,
                    role: "tab",
                    aria_selected: if current_tab == WorkbenchTab::Settings { "true" } else { "false" },
                    onclick: move |_| props.on_tab_change.call(WorkbenchTab::Settings),
                    "设置"
                }
                button {
                    class: network_tab_class,
                    role: "tab",
                    aria_selected: if current_tab == WorkbenchTab::Network { "true" } else { "false" },
                    onclick: move |_| props.on_tab_change.call(WorkbenchTab::Network),
                    "网络"
                }
            }

            div { class: "workbench-content",
                {
                    match current_tab {
                        WorkbenchTab::Overview => rsx! {
                            div { class: "workbench-panel",
                                div { class: "workbench-metric-grid",
                                    div { class: "workbench-metric",
                                        span { class: "workbench-metric__label", "音频" }
                                        strong { class: "workbench-metric__value", "{props.mode_indicator}" }
                                        span { class: "workbench-metric__detail", "{mute_state_text}" }
                                    }
                                    div { class: "workbench-metric",
                                        span { class: "workbench-metric__label", "音量" }
                                        strong { class: "workbench-metric__value", "{volume_percent}%" }
                                        span { class: "workbench-metric__detail", "{props.audio_status}" }
                                    }
                                    div { class: "workbench-metric",
                                        span { class: "workbench-metric__label", "网络" }
                                        strong { class: "workbench-metric__value", "{connected_count}/{peer_count}" }
                                        span { class: "workbench-metric__detail", "{net_state}" }
                                    }
                                    div { class: "workbench-metric",
                                        span { class: "workbench-metric__label", "远控" }
                                        strong { class: "workbench-metric__value", "{remote_state_text}" }
                                        span { class: "workbench-metric__detail", "{props.settings_display_name}" }
                                    }
                                }

                                section { class: "workbench-section workbench-section--dense",
                                    div { class: "workbench-section__title", "快速控制" }
                                    div { class: "workbench-actions",
                                        button {
                                            class: "panel-action",
                                            onclick: move |_| props.on_switch_audio_mode.call(()),
                                            "切换音频"
                                        }
                                        button {
                                            class: "panel-action panel-action--secondary",
                                            onclick: move |_| props.on_toggle_mute.call(()),
                                            "{mute_text}"
                                        }
                                        button {
                                            class: "panel-action panel-action--secondary",
                                            onclick: move |_| {
                                                props.on_tab_change.call(WorkbenchTab::Network);
                                                props.on_scan_peers.call(());
                                            },
                                            if props.scanning { "扫描中..." } else { "扫描网络" }
                                        }
                                    }
                                }

                                section { class: "workbench-section workbench-section--dense",
                                    div { class: "workbench-section__title", "当前状态" }
                                    div { class: status_class, "{net_state}" }
                                    div { class: "workbench-row",
                                        span { class: "workbench-row__label", "可用设备" }
                                        span { class: "workbench-row__value", "{props.peers.len()}" }
                                    }
                                }
                            }
                        },
                        WorkbenchTab::Settings => rsx! {
                            div { class: "workbench-panel workbench-panel--settings",
                                SettingsContent {
                                    display_name: props.settings_display_name.clone(),
                                    save_status: props.settings_save_status.clone(),
                                    audio_status: props.audio_status.clone(),
                                    audio_muted: props.audio_muted,
                                    audio_volume: props.audio_volume,
                                    mode_indicator: props.mode_indicator.clone(),
                                    dark_mode: props.dark_mode,
                                    app_version: props.app_version.clone(),
                                    on_save_name: props.on_save_display_name,
                                    on_switch_audio_mode: props.on_switch_audio_mode,
                                    on_toggle_mute: props.on_toggle_mute,
                                    on_volume_changed: props.on_volume_changed,
                                    on_toggle_theme: props.on_toggle_theme,
                                    on_show_about: props.on_show_about,
                                }
                            }
                        },
                        WorkbenchTab::Network => rsx! {
                            div { class: "workbench-panel workbench-panel--network",
                                NetworkPanelContent {
                                    network_status: props.network_status.clone(),
                                    remote_controlled: props.remote_controlled,
                                    executing_remotely: props.executing_remotely,
                                    scanning: props.scanning,
                                    allow_remote_control: props.allow_remote_control,
                                    audio_muted: props.audio_muted,
                                    peers: props.peers.clone(),
                                    connected_peer_index: props.connected_peer_index,
                                    onconnect: move |id| props.on_connect_to_peer.call(id),
                                    ondisconnect: move |id| props.on_disconnect_peer.call(id),
                                    onscan: move |_| props.on_scan_peers.call(()),
                                    ontoggle_remote_control: move |_| props.on_toggle_remote_control.call(()),
                                    ontoggle_mute: move |_| props.on_toggle_mute.call(()),
                                }
                            }
                        },
                    }
                }
            }
        }
    }
}
