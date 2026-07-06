use dioxus::prelude::*;

use super::icon::{Icon, IconName};

// ---------------------------------------------------------------------------
// StatusIconButton — compact SVG icon trigger
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct StatusIconButtonProps {
    /// Icon to render.
    pub icon: IconName,
    /// Tooltip text (title attribute).
    #[props(default)]
    pub title: Option<String>,
    /// Whether the button is in an active state.
    #[props(default)]
    pub active: Option<bool>,
    /// Visual variant: "remote-controlled", "executing-remotely", "network".
    #[props(default)]
    pub variant: Option<String>,
    pub onclick: EventHandler<()>,
}

#[component]
pub fn StatusIconButton(props: StatusIconButtonProps) -> Element {
    let mut classes = vec!["status-icon-btn"];

    if let Some(ref v) = props.variant {
        match v.as_str() {
            "remote-controlled" => classes.push("status-icon-btn--remote-controlled"),
            "executing-remotely" => classes.push("status-icon-btn--executing-remotely"),
            "network" => classes.push("status-icon-btn--network"),
            _ => {}
        }
    }

    if props.active == Some(false) {
        classes.push("status-icon-btn--inactive");
    }

    let class_str = classes.join(" ");
    let title_attr = props.title.as_deref().unwrap_or("");

    rsx! {
        button {
            class: "{class_str}",
            title: "{title_attr}",
            onclick: move |_| props.onclick.call(()),

            Icon { name: props.icon, class: Some("status-icon-btn__icon".to_string()) }
        }
    }
}

// ---------------------------------------------------------------------------
// StatusBar — memory indicator, audio/mode info, icon buttons
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct StatusBarProps {
    pub memory_indicator: String,
    pub audio_status: String,
    pub mode_indicator: String,
    pub network_status: String,
    #[props(default)]
    pub dark_mode: bool,
    #[props(default)]
    pub error_state: bool,
    #[props(default)]
    pub remote_controlled: bool,
    #[props(default)]
    pub executing_remotely: bool,
    pub on_toggle_theme: EventHandler<()>,
    pub on_switch_audio_mode: EventHandler<()>,
    pub on_show_about: EventHandler<()>,
    pub on_show_network_settings: EventHandler<()>,
    pub on_show_settings: EventHandler<()>,
}

#[component]
pub fn StatusBar(props: StatusBarProps) -> Element {
    // Network icon: lightning when executing remotely, topology otherwise.
    let (network_icon, network_variant) = if props.executing_remotely {
        (IconName::Bolt, "executing-remotely")
    } else if props.remote_controlled {
        (IconName::Network, "remote-controlled")
    } else {
        (IconName::Network, "network")
    };

    let network_active = !props.network_status.is_empty();
    let theme_icon = if props.dark_mode {
        IconName::Sun
    } else {
        IconName::Moon
    };
    let theme_title = if props.dark_mode {
        "切换浅色主题 (T)"
    } else {
        "切换深色主题 (T)"
    };

    rsx! {
        div {
            class: "status-bar",

            if !props.memory_indicator.is_empty() {
                span {
                    class: "status-chip status-chip--memory",
                    "{props.memory_indicator}"
                }
            }

            if props.error_state {
                span {
                    class: "status-chip status-chip--error",
                    "ERR"
                }
            }

            if props.executing_remotely {
                span {
                    class: "status-chip status-chip--executing",
                    "远程执行"
                }
            } else if props.remote_controlled {
                span {
                    class: "status-chip status-chip--remote",
                    "远控"
                }
            }

            div {
                class: "status-bar__spacer"
            }

            span {
                class: "status-bar__info status-bar__info--audio",
                "{props.audio_status}"
            }

            span {
                class: "status-bar__info status-bar__info--mode",
                "{props.mode_indicator}"
            }

            // Theme toggle
            StatusIconButton {
                icon: theme_icon,
                title: theme_title,
                onclick: move |_| props.on_toggle_theme.call(()),
            }

            // Audio mode
            StatusIconButton {
                icon: IconName::Music,
                title: "切换音频模式 (O)",
                onclick: move |_| props.on_switch_audio_mode.call(()),
            }

            // About
            StatusIconButton {
                icon: IconName::Info,
                title: "关于 (F1)",
                onclick: move |_| props.on_show_about.call(()),
            }

            // Network settings
            StatusIconButton {
                icon: network_icon,
                title: "网络设置 (F3)",
                active: network_active,
                variant: network_variant,
                onclick: move |_| props.on_show_network_settings.call(()),
            }

            // Settings
            StatusIconButton {
                icon: IconName::Settings,
                title: "设置 (F2)",
                onclick: move |_| props.on_show_settings.call(()),
            }
        }
    }
}
