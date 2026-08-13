use dioxus::prelude::*;

use crate::components::icon::IconName;
use crate::components::overlay::ToggleSwitch;
use crate::net::protocol::LAN_FIXED_PORT;
use crate::net::view::BindStatus;

const FIREWALL_HINT_PATH: &str = "packaging/windows/configure-firewall.ps1";

#[derive(Props, Clone, PartialEq)]
pub struct ThisDeviceProps {
    pub display_name: String,
    pub save_status: String,
    pub fingerprint: String,
    pub network_mode_label: String,
    pub bind: BindStatus,
    pub allow_remote_control: bool,
    pub controller_names: Vec<String>,
    pub selected_executor_name: Option<String>,
    pub on_save_name: EventHandler<String>,
    pub on_toggle_remote_control: EventHandler<MouseEvent>,
}

/// This-device identity card: name, fingerprint, bind, inbound switch.
#[component]
pub fn ThisDevicePanel(props: ThisDeviceProps) -> Element {
    let mut input_value = use_signal(|| props.display_name.clone());
    let display_name = props.display_name.clone();
    use_effect(move || {
        input_value.set(display_name.clone());
    });

    let status_class = if props.save_status == "已保存" {
        "settings-save-status settings-save-status--success"
    } else {
        "settings-save-status"
    };
    let bind_text = bind_status_text(&props.bind);
    let bind_class = bind_status_class(&props.bind);
    let remote_state = if props.allow_remote_control {
        "已允许"
    } else {
        "已禁止"
    };
    let fingerprint_short = short_local_fingerprint(&props.fingerprint);
    let fingerprint_full = props.fingerprint.clone();
    let controllers = props.controller_names.clone();
    let controller_list = controllers.join("、");
    let executor_name = props.selected_executor_name.clone();
    let show_controllers = !controllers.is_empty();

    rsx! {
        article { class: "this-device",
            div { class: "this-device__header",
                span { class: "this-device__kicker", "本机" }
                span { class: "this-device__title", "{props.display_name}" }
            }

            div { class: "this-device__name-row",
                input {
                    class: "settings-input this-device__name-input",
                    r#type: "text",
                    placeholder: "输入设备名称...",
                    value: "{input_value}",
                    oninput: move |evt| input_value.set(evt.value()),
                }
                button {
                    class: "panel-action this-device__save",
                    r#type: "button",
                    onclick: move |_| props.on_save_name.call(input_value()),
                    "保存"
                }
            }

            if !props.save_status.is_empty() {
                div {
                    class: "{status_class}",
                    span { "{props.save_status}" }
                }
            }

            div { class: "this-device__chips",
                span {
                    class: "fingerprint-chip",
                    title: "{fingerprint_full}",
                    "{fingerprint_short}"
                }
                span { class: "this-device__mode", "{props.network_mode_label}" }
                span { class: "this-device__port", "端口 {LAN_FIXED_PORT}" }
            }

            div { class: "{bind_class}", "{bind_text}" }

            div { class: "this-device__firewall",
                span { class: "this-device__firewall-label", "防火墙脚本（仅复制，请勿执行）" }
                code { class: "this-device__firewall-path", "{FIREWALL_HINT_PATH}" }
            }

            div { class: "this-device__remote",
                ToggleSwitch {
                    on: props.allow_remote_control,
                    label: "允许其他设备控制本机".to_string(),
                    icon: Some(IconName::Lock),
                    on_toggle: move |evt| props.on_toggle_remote_control.call(evt),
                }
                span { class: "this-device__remote-state", "{remote_state}" }
            }

            if show_controllers {
                div { class: "this-device__accepting",
                    span { class: "this-device__accepting-label", "正在接受远程控制 {controller_list}" }
                }
            }

            if let Some(name) = executor_name {
                div { class: "this-device__executing", "正在远程执行 {name}" }
            }
        }
    }
}

fn bind_status_text(bind: &BindStatus) -> String {
    match bind {
        BindStatus::Offline => "离线".to_string(),
        BindStatus::Bound { addr } => format!("监听 {addr}"),
        BindStatus::BindFailed { port } => format!("端口 {port} 无法监听"),
        BindStatus::Unavailable => "网络不可用".to_string(),
    }
}

fn bind_status_class(bind: &BindStatus) -> &'static str {
    match bind {
        BindStatus::Offline => "this-device__bind this-device__bind--offline",
        BindStatus::Bound { .. } => "this-device__bind this-device__bind--bound",
        BindStatus::BindFailed { .. } => "this-device__bind this-device__bind--failed",
        BindStatus::Unavailable => "this-device__bind this-device__bind--unavailable",
    }
}

fn short_local_fingerprint(fingerprint: &str) -> String {
    let chars: Vec<char> = fingerprint.chars().collect();
    if chars.len() > 16 {
        let start: String = chars[..8].iter().collect();
        let end: String = chars[chars.len() - 8..].iter().collect();
        format!("{start}…{end}")
    } else {
        fingerprint.to_string()
    }
}
