use dioxus::prelude::*;

use crate::components::icon::{Icon, IconName};
use crate::components::overlay::{Overlay, OverlayVariant, ToggleSwitch};
use crate::components::panel_controls::PanelSection;

/// Display information for one discovered calculator peer.
#[derive(Props, Clone, PartialEq)]
pub struct PeerDisplayInfo {
    pub name: String,
    pub address: String,
    pub is_connected: bool,
    /// Whether this peer is the single selected remote executor.
    pub route_active: bool,
    pub latency_ms: i32,
    pub index: i32,
    pub node_id_string: String,
}

#[derive(Props, Clone, PartialEq)]
pub struct NetworkPanelProps {
    pub visible: bool,
    pub network_status: String,
    #[props(default)]
    pub remote_controlled: bool,
    #[props(default)]
    pub executing_remotely: bool,
    #[props(default)]
    pub scanning: bool,
    #[props(default)]
    pub allow_remote_control: bool,
    #[props(default)]
    pub audio_muted: bool,
    #[props(default)]
    pub peers: Vec<PeerDisplayInfo>,
    #[props(default)]
    pub connected_peer_index: i32,
    pub onclose: EventHandler<MouseEvent>,
    pub onconnect: EventHandler<String>,
    pub ondisconnect: EventHandler<String>,
    pub onscan: EventHandler<MouseEvent>,
    pub ontoggle_remote_control: EventHandler<MouseEvent>,
    pub ontoggle_mute: EventHandler<MouseEvent>,
}

#[component]
pub fn NetworkPanel(props: NetworkPanelProps) -> Element {
    rsx! {
        Overlay {
            visible: props.visible,
            title: "网络设置".to_string(),
            icon: Some(IconName::Network),
            variant: OverlayVariant::Large,
            onclose: move |evt| props.onclose.call(evt),

            NetworkPanelContent {
                network_status: props.network_status.clone(),
                remote_controlled: props.remote_controlled,
                executing_remotely: props.executing_remotely,
                scanning: props.scanning,
                allow_remote_control: props.allow_remote_control,
                audio_muted: props.audio_muted,
                peers: props.peers.clone(),
                connected_peer_index: props.connected_peer_index,
                onconnect: props.onconnect,
                ondisconnect: props.ondisconnect,
                onscan: props.onscan,
                ontoggle_remote_control: props.ontoggle_remote_control,
                ontoggle_mute: props.ontoggle_mute,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct NetworkPanelContentProps {
    pub network_status: String,
    #[props(default)]
    pub remote_controlled: bool,
    #[props(default)]
    pub executing_remotely: bool,
    #[props(default)]
    pub scanning: bool,
    #[props(default)]
    pub allow_remote_control: bool,
    #[props(default)]
    pub audio_muted: bool,
    #[props(default)]
    pub peers: Vec<PeerDisplayInfo>,
    #[props(default)]
    pub connected_peer_index: i32,
    pub onconnect: EventHandler<String>,
    pub ondisconnect: EventHandler<String>,
    pub onscan: EventHandler<MouseEvent>,
    pub ontoggle_remote_control: EventHandler<MouseEvent>,
    pub ontoggle_mute: EventHandler<MouseEvent>,
}

#[component]
pub fn NetworkPanelContent(props: NetworkPanelContentProps) -> Element {
    let status_text = if props.network_status.is_empty() {
        "未连接".to_string()
    } else {
        props.network_status.clone()
    };
    let status_class = if props.executing_remotely {
        "network-info-row__text executing-remotely"
    } else if props.remote_controlled {
        "network-info-row__text remote-controlled"
    } else {
        "network-info-row__text connected"
    };

    rsx! {
        div { class: "network-panel-content",
            div { class: "network-info-row",
                span { class: "network-info-row__icon", Icon { name: IconName::Network } }
                span { class: status_class, "{status_text}" }
            }

            ToggleSwitch {
                on: props.allow_remote_control,
                label: "允许其他设备控制本机".to_string(),
                icon: Some(IconName::Lock),
                on_toggle: move |evt| props.ontoggle_remote_control.call(evt),
            }
            ToggleSwitch {
                on: props.audio_muted,
                label: "静音模式".to_string(),
                icon: Some(IconName::VolumeMuted),
                on_toggle: move |evt| props.ontoggle_mute.call(evt),
            }

            PanelSection {
                title: "可用的计算设备".to_string(),
                icon: Some(IconName::Users),
                class: Some("network-section network-section--peers".to_string()),

                div { class: "network-toolbar",
                    button {
                        class: if props.scanning { "network-action-btn network-action-btn--scan is-active" } else { "network-action-btn network-action-btn--scan" },
                        r#type: "button",
                        title: "扫描网络 (F5)",
                        onclick: move |evt| props.onscan.call(evt),
                        Icon { name: IconName::Search, class: Some("network-action-btn__icon".to_string()) }
                        if props.scanning { span { "扫描中..." } } else { span { "扫描" } }
                    }
                }

                div { class: "peer-list",
                    if props.peers.is_empty() {
                        div { class: "peer-list__empty", "暂无可用设备" }
                    } else {
                        for peer in props.peers.iter() {
                            {
                                let is_connected = peer.is_connected;
                                let route_active = peer.route_active;
                                let row_class = if route_active {
                                    "peer-row peer-row--connected peer-row--route-active"
                                } else if is_connected {
                                    "peer-row peer-row--connected"
                                } else {
                                    "peer-row"
                                };
                                let latency_display = if peer.latency_ms < 0 {
                                    "-".to_string()
                                } else {
                                    format!("{}ms", peer.latency_ms)
                                };
                                let node_id = peer.node_id_string.clone();
                                let stop_node_id = node_id.clone();
                                let session_label = if route_active {
                                    "远程执行中"
                                } else if is_connected {
                                    "已连接"
                                } else {
                                    "未连接"
                                };

                                rsx! {
                                    div { class: "{row_class}",
                                        div { class: "peer-row__info",
                                            span {
                                                class: if route_active { "peer-row__status-icon peer-row__status-icon--route" } else if is_connected { "peer-row__status-icon peer-row__status-icon--connected" } else { "peer-row__status-icon" },
                                                if route_active { Icon { name: IconName::Bolt } }
                                                else if is_connected { Icon { name: IconName::Check } }
                                            }
                                            span { class: "peer-row__name", "{peer.name}" }
                                            span { class: "peer-row__address", "{peer.address}" }
                                            span { class: "peer-row__latency", "{session_label} · {latency_display}" }
                                        }
                                        div { class: "peer-row__actions",
                                            if route_active {
                                                button {
                                                    class: "network-action-btn",
                                                    r#type: "button",
                                                    onclick: move |_| props.ondisconnect.call(stop_node_id.clone()),
                                                    "停止远程执行"
                                                }
                                            } else {
                                                button {
                                                    class: "network-action-btn",
                                                    r#type: "button",
                                                    onclick: move |_| props.onconnect.call(node_id.clone()),
                                                    if is_connected { "在此设备执行" } else { "连接并执行" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    const SOURCE: &str = include_str!("network_panel.rs");

    #[test]
    fn network_panel_exposes_one_switch_without_approval_or_matrix_ui() {
        let approval = "\u{6388}\u{6743}";
        let denial = "\u{62d2}\u{7edd}";
        let matrix = "\u{8def}\u{7531}\u{77e9}\u{9635}";
        let switch_label = ["允许其他设备", "控制本机"].concat();
        let old_approval_field = ["approval", "_pending"].concat();
        let old_matrix_callback = ["onroute", "_toggled"].concat();
        assert!(SOURCE.contains(&switch_label));
        assert!(!SOURCE.contains(approval));
        assert!(!SOURCE.contains(denial));
        assert!(!SOURCE.contains(matrix));
        assert!(!SOURCE.contains(&old_approval_field));
        assert!(!SOURCE.contains(&old_matrix_callback));
    }
}
