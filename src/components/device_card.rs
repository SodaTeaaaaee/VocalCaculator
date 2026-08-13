use dioxus::prelude::*;

use crate::net::protocol::NodeId;
use crate::net::view::{PeerPresence, PeerRole, PeerViewModel};

/// LocalSend-style nearby device card.
///
/// One primary CTA: connect-and-execute, execute-on-connected, or stop.
#[component]
pub fn DeviceCard(
    peer: PeerViewModel,
    on_use: EventHandler<NodeId>,
    on_stop: EventHandler<NodeId>,
) -> Element {
    let class = device_card_class(&peer);
    let initial = avatar_initial(&peer.display_name);
    let color = avatar_color(peer.node_id);
    let address = peer.address_label();
    let latency = match peer.latency_ms {
        Some(ms) => format!("{ms} ms"),
        None => "—".to_string(),
    };
    let role_presence = role_presence_label(&peer);
    let fingerprint = peer
        .fingerprint
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(short_fingerprint);
    let node_id = peer.node_id;
    let is_executor = peer.role == PeerRole::SelectedExecutor;
    let is_connected = peer.presence == PeerPresence::Connected;
    let name = if peer.display_name.trim().is_empty() {
        "未命名设备".to_string()
    } else {
        peer.display_name.clone()
    };

    rsx! {
        article { class: "{class}",
            div {
                class: "device-card__avatar",
                style: "background-color: {color};",
                span { class: "device-card__initial", "{initial}" }
                span { class: "device-card__pip", aria_hidden: "true" }
            }

            div { class: "device-card__body",
                div { class: "device-card__name", "{name}" }
                if !address.is_empty() {
                    div { class: "device-card__address", "{address}" }
                }
                div { class: "device-card__meta",
                    if let Some(fingerprint) = fingerprint {
                        span {
                            class: "device-card__fingerprint fingerprint-chip",
                            title: peer.fingerprint.clone().unwrap_or_default(),
                            "{fingerprint}"
                        }
                    }
                    span { class: "device-card__latency", "{latency}" }
                    span { class: "device-card__role", "{role_presence}" }
                }
            }

            div { class: "device-card__action",
                if is_executor {
                    button {
                        class: "device-card__cta device-card__cta--stop",
                        r#type: "button",
                        onclick: move |_| on_stop.call(node_id),
                        "停止远程执行"
                    }
                } else if is_connected {
                    button {
                        class: "device-card__cta",
                        r#type: "button",
                        onclick: move |_| on_use.call(node_id),
                        "在此设备执行"
                    }
                } else {
                    button {
                        class: "device-card__cta",
                        r#type: "button",
                        onclick: move |_| on_use.call(node_id),
                        "连接并执行"
                    }
                }
            }
        }
    }
}

fn device_card_class(peer: &PeerViewModel) -> String {
    let mut class = String::from("device-card ");
    class.push_str(match peer.presence {
        PeerPresence::Nearby => "device-card--nearby",
        PeerPresence::Connecting => "device-card--connecting",
        PeerPresence::Connected => "device-card--connected",
        PeerPresence::Unreachable => "device-card--unreachable",
        PeerPresence::Stale => "device-card--stale",
        PeerPresence::FingerprintMismatch => "device-card--mismatch",
    });
    if peer.role == PeerRole::SelectedExecutor {
        class.push_str(" device-card--executor");
    }
    if peer.role == PeerRole::ControllingUs {
        class.push_str(" device-card--controller");
    }
    class
}

fn avatar_initial(name: &str) -> String {
    name.chars()
        .find(|ch| !ch.is_whitespace())
        .map(|ch| ch.to_string())
        .unwrap_or_else(|| "计".to_string())
}

fn avatar_color(node_id: NodeId) -> String {
    let bytes = node_id.as_bytes();
    let hue = u32::from(bytes[0]) * 360 / 255;
    let sat = 48 + u32::from(bytes[1] % 18);
    let light = 42 + u32::from(bytes[2] % 10);
    format!("hsl({hue} {sat}% {light}%)")
}

fn short_fingerprint(fingerprint: &str) -> String {
    let compact: String = fingerprint
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .take(8)
        .collect();
    if compact.len() >= 8 {
        format!("{}·{}", &compact[..4], &compact[4..8])
    } else if compact.is_empty() {
        fingerprint.chars().take(8).collect()
    } else {
        compact
    }
}

fn role_presence_label(peer: &PeerViewModel) -> &'static str {
    match peer.role {
        PeerRole::SelectedExecutor => "远程执行",
        PeerRole::ControllingUs => "正在控制本机",
        PeerRole::Idle => match peer.presence {
            PeerPresence::Nearby => "附近",
            PeerPresence::Connecting => "连接中",
            PeerPresence::Connected => "已连接",
            PeerPresence::Unreachable => "不可达",
            PeerPresence::Stale => "已过期",
            PeerPresence::FingerprintMismatch => "密钥不一致",
        },
    }
}
