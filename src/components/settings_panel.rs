use dioxus::prelude::*;

use super::icon::{Icon, IconName};
use super::overlay::{Overlay, OverlayVariant};
use super::panel_controls::{AudioControlGroup, ControlRow, PanelSection};

// ---------------------------------------------------------------------------
// SettingsPanel — audio, theme, about (optional device name)
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct SettingsPanelProps {
    /// Current display name shown in the input field.
    pub display_name: String,
    /// Status message shown after a save attempt (empty when idle).
    pub save_status: String,
    pub audio_status: String,
    pub audio_muted: bool,
    pub audio_volume: f64,
    pub mode_indicator: String,
    pub dark_mode: bool,
    pub app_version: String,
    /// Fires when the user clicks the backdrop or close button.
    pub onclose: EventHandler<MouseEvent>,
    /// Fires when the user clicks "Save"; carries the current input value.
    pub on_save_name: EventHandler<String>,
    pub on_switch_audio_mode: EventHandler<()>,
    pub on_toggle_mute: EventHandler<()>,
    pub on_volume_changed: EventHandler<f64>,
    pub on_toggle_theme: EventHandler<()>,
    pub on_show_about: EventHandler<()>,
}

#[component]
pub fn SettingsPanel(props: SettingsPanelProps) -> Element {
    rsx! {
        Overlay {
            visible: true,
            title: "设置".to_string(),
            icon: Some(IconName::Settings),
            variant: OverlayVariant::Compact,
            onclose: move |evt| props.onclose.call(evt),

            SettingsContent {
                display_name: props.display_name.clone(),
                save_status: props.save_status.clone(),
                audio_status: props.audio_status.clone(),
                audio_muted: props.audio_muted,
                audio_volume: props.audio_volume,
                mode_indicator: props.mode_indicator.clone(),
                dark_mode: props.dark_mode,
                app_version: props.app_version.clone(),
                on_save_name: props.on_save_name,
                on_switch_audio_mode: props.on_switch_audio_mode,
                on_toggle_mute: props.on_toggle_mute,
                on_volume_changed: props.on_volume_changed,
                on_toggle_theme: props.on_toggle_theme,
                on_show_about: props.on_show_about,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SettingsContentProps {
    /// Current display name shown in the input field.
    pub display_name: String,
    /// Status message shown after a save attempt (empty when idle).
    pub save_status: String,
    pub audio_status: String,
    pub audio_muted: bool,
    pub audio_volume: f64,
    pub mode_indicator: String,
    pub dark_mode: bool,
    pub app_version: String,
    /// Fires when the user clicks "Save"; carries the current input value.
    pub on_save_name: EventHandler<String>,
    pub on_switch_audio_mode: EventHandler<()>,
    pub on_toggle_mute: EventHandler<()>,
    pub on_volume_changed: EventHandler<f64>,
    pub on_toggle_theme: EventHandler<()>,
    pub on_show_about: EventHandler<()>,
}

#[component]
pub fn SettingsContent(props: SettingsContentProps) -> Element {
    let mut input_value = use_signal(|| props.display_name.clone());

    // Keep the local signal in sync when the parent pushes a new value.
    let display_name = props.display_name.clone();
    use_effect(move || {
        input_value.set(display_name.clone());
    });

    let status_class = if props.save_status == "已保存" {
        "settings-save-status settings-save-status--success"
    } else {
        "settings-save-status"
    };
    let theme_icon = if props.dark_mode {
        IconName::Sun
    } else {
        IconName::Moon
    };
    let theme_text = if props.dark_mode {
        "浅色主题"
    } else {
        "深色主题"
    };
    let theme_state = if props.dark_mode { "深色" } else { "浅色" };

    rsx! {
        div { class: "settings-content",
            PanelSection {
                title: "设备".to_string(),
                icon: Some(IconName::User),

                ControlRow {
                    label: "设备名称".to_string(),
                    icon: Some(IconName::User),
                    input {
                        class: "settings-input",
                        r#type: "text",
                        placeholder: "输入设备名称...",
                        value: "{input_value}",
                        oninput: move |evt| input_value.set(evt.value()),
                    }
                }

                if !props.save_status.is_empty() {
                    div {
                        class: "{status_class}",
                        span { "{props.save_status}" }
                    }
                }

                div {
                    class: "settings-save-row",
                    button {
                        class: "panel-action panel-action--with-icon",
                        r#type: "button",
                        onclick: move |_| props.on_save_name.call(input_value()),
                        Icon { name: IconName::Check, class: Some("panel-action__icon".to_string()) }
                        span { "保存" }
                    }
                }
            }

            AudioControlGroup {
                audio_status: props.audio_status.clone(),
                audio_muted: props.audio_muted,
                audio_volume: props.audio_volume,
                mode_indicator: props.mode_indicator.clone(),
                on_switch_audio_mode: props.on_switch_audio_mode,
                on_toggle_mute: props.on_toggle_mute,
                on_volume_changed: props.on_volume_changed,
            }

            PanelSection {
                title: "外观".to_string(),
                icon: Some(theme_icon),

                ControlRow {
                    label: "主题".to_string(),
                    icon: Some(theme_icon),
                    value: Some(theme_state.to_string()),
                    button {
                        class: "panel-action panel-action--secondary panel-action--with-icon",
                        r#type: "button",
                        onclick: move |_| props.on_toggle_theme.call(()),
                        Icon { name: theme_icon, class: Some("panel-action__icon".to_string()) }
                        span { "切换{theme_text}" }
                    }
                }
            }

            PanelSection {
                title: "关于".to_string(),
                icon: Some(IconName::Info),
                class: Some("panel-section--last".to_string()),

                ControlRow {
                    label: "版本".to_string(),
                    icon: Some(IconName::Info),
                    value: Some(props.app_version.clone()),
                    button {
                        class: "panel-action panel-action--secondary panel-action--with-icon",
                        r#type: "button",
                        onclick: move |_| props.on_show_about.call(()),
                        Icon { name: IconName::Info, class: Some("panel-action__icon".to_string()) }
                        span { "详情" }
                    }
                }
            }
        }
    }
}
