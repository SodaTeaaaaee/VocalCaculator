use dioxus::prelude::*;

use super::icon::IconName;
use super::panel_controls::IconButton;

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
    pub error_state: bool,
    #[props(default)]
    pub remote_controlled: bool,
    #[props(default)]
    pub executing_remotely: bool,
}

#[component]
pub fn StatusBar(props: StatusBarProps) -> Element {
    let network_ready = !props.network_status.is_empty();

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
            } else if network_ready {
                span {
                    class: "status-chip status-chip--network",
                    "网络"
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
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct MobileQuickActionsProps {
    pub dark_mode: bool,
    pub network_status: String,
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
pub fn MobileQuickActions(props: MobileQuickActionsProps) -> Element {
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
    let (network_icon, network_variant) = if props.executing_remotely {
        (IconName::Bolt, "executing-remotely")
    } else if props.remote_controlled {
        (IconName::Network, "remote-controlled")
    } else {
        (IconName::Network, "network")
    };

    rsx! {
        div { class: "mobile-quick-actions", aria_label: "快捷操作",
            IconButton {
                icon: theme_icon,
                title: theme_title.to_string(),
                onclick: move |_| props.on_toggle_theme.call(()),
            }
            IconButton {
                icon: IconName::Music,
                title: "切换音频模式 (O)".to_string(),
                onclick: move |_| props.on_switch_audio_mode.call(()),
            }
            IconButton {
                icon: IconName::Info,
                title: "关于 (F1)".to_string(),
                onclick: move |_| props.on_show_about.call(()),
            }
            IconButton {
                icon: network_icon,
                title: "网络设置 (F3)".to_string(),
                active: Some(!props.network_status.is_empty()),
                variant: Some(network_variant.to_string()),
                onclick: move |_| props.on_show_network_settings.call(()),
            }
            IconButton {
                icon: IconName::Settings,
                title: "设置 (F2)".to_string(),
                onclick: move |_| props.on_show_settings.call(()),
            }
        }
    }
}
