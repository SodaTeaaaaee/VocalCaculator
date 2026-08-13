use dioxus::prelude::*;

use crate::components::device_card::DeviceCard;
use crate::components::icon::{Icon, IconName};
use crate::components::overlay::{Overlay, OverlayVariant};
use crate::components::this_device::ThisDevicePanel;
use crate::net::protocol::NodeId;
use crate::net::view::{BindStatus, PeerViewModel};

// Inbound switch copy lives on ThisDevicePanel as 允许其他设备控制本机.

#[derive(Props, Clone, PartialEq)]
pub struct NetworkPanelProps {
    pub visible: bool,
    pub display_name: String,
    pub save_status: String,
    pub fingerprint: String,
    pub network_mode_label: String,
    pub bind: BindStatus,
    pub allow_remote_control: bool,
    pub controller_names: Vec<String>,
    pub selected_executor_name: Option<String>,
    pub scanning: bool,
    pub peers: Vec<PeerViewModel>,
    pub onclose: EventHandler<MouseEvent>,
    pub on_save_name: EventHandler<String>,
    pub ontoggle_remote_control: EventHandler<MouseEvent>,
    pub onscan: EventHandler<MouseEvent>,
    pub onconnect: EventHandler<NodeId>,
    pub ondisconnect: EventHandler<NodeId>,
}

#[component]
pub fn NetworkPanel(props: NetworkPanelProps) -> Element {
    rsx! {
        Overlay {
            visible: props.visible,
            title: "附近".to_string(),
            icon: Some(IconName::Network),
            variant: OverlayVariant::Large,
            onclose: move |evt| props.onclose.call(evt),

            NetworkPanelContent {
                display_name: props.display_name.clone(),
                save_status: props.save_status.clone(),
                fingerprint: props.fingerprint.clone(),
                network_mode_label: props.network_mode_label.clone(),
                bind: props.bind.clone(),
                allow_remote_control: props.allow_remote_control,
                controller_names: props.controller_names.clone(),
                selected_executor_name: props.selected_executor_name.clone(),
                scanning: props.scanning,
                peers: props.peers.clone(),
                on_save_name: props.on_save_name,
                ontoggle_remote_control: props.ontoggle_remote_control,
                onscan: props.onscan,
                onconnect: props.onconnect,
                ondisconnect: props.ondisconnect,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct NetworkPanelContentProps {
    pub display_name: String,
    pub save_status: String,
    pub fingerprint: String,
    pub network_mode_label: String,
    pub bind: BindStatus,
    pub allow_remote_control: bool,
    pub controller_names: Vec<String>,
    pub selected_executor_name: Option<String>,
    pub scanning: bool,
    pub peers: Vec<PeerViewModel>,
    pub on_save_name: EventHandler<String>,
    pub ontoggle_remote_control: EventHandler<MouseEvent>,
    pub onscan: EventHandler<MouseEvent>,
    pub onconnect: EventHandler<NodeId>,
    pub ondisconnect: EventHandler<NodeId>,
}

#[component]
pub fn NetworkPanelContent(props: NetworkPanelContentProps) -> Element {
    rsx! {
        div { class: "network-panel-content",
            ThisDevicePanel {
                display_name: props.display_name.clone(),
                save_status: props.save_status.clone(),
                fingerprint: props.fingerprint.clone(),
                network_mode_label: props.network_mode_label.clone(),
                bind: props.bind.clone(),
                allow_remote_control: props.allow_remote_control,
                controller_names: props.controller_names.clone(),
                selected_executor_name: props.selected_executor_name.clone(),
                on_save_name: props.on_save_name,
                on_toggle_remote_control: props.ontoggle_remote_control,
            }
            NearbyDevices {
                scanning: props.scanning,
                peers: props.peers.clone(),
                on_scan: props.onscan,
                on_use_executor: props.onconnect,
                on_stop_executor: props.ondisconnect,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct NearbyDevicesProps {
    pub scanning: bool,
    pub peers: Vec<PeerViewModel>,
    pub on_scan: EventHandler<MouseEvent>,
    pub on_use_executor: EventHandler<NodeId>,
    pub on_stop_executor: EventHandler<NodeId>,
}

/// Scan control plus LocalSend device cards for nearby calculators.
#[component]
pub fn NearbyDevices(props: NearbyDevicesProps) -> Element {
    rsx! {
        section { class: "nearby-devices",
            div { class: "nearby-devices__header",
                span { class: "nearby-devices__title", "附近的设备" }
                button {
                    class: if props.scanning {
                        "network-action-btn network-action-btn--scan is-active"
                    } else {
                        "network-action-btn network-action-btn--scan"
                    },
                    r#type: "button",
                    title: "扫描网络 (F5)",
                    onclick: move |evt| props.on_scan.call(evt),
                    Icon { name: IconName::Search, class: Some("network-action-btn__icon".to_string()) }
                    if props.scanning {
                        span { "扫描中..." }
                    } else {
                        span { "扫描" }
                    }
                }
            }

            div { class: "device-card-list",
                if props.peers.is_empty() {
                    div { class: "device-card-list__empty", "暂无可用设备" }
                } else {
                    for peer in props.peers.iter() {
                        DeviceCard {
                            key: "{peer.node_id}",
                            peer: peer.clone(),
                            on_use: props.on_use_executor,
                            on_stop: props.on_stop_executor,
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
