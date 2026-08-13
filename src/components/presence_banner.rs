//! Compact presence strip on the calculator body.
//!
//! Displays bind / remote-control state only. Firewall guidance is text;
//! this component never launches scripts or elevation prompts.

use dioxus::prelude::*;

use crate::net::view::BindStatus;

const FIREWALL_HINT: &str = "packaging/windows/configure-firewall.ps1";

#[derive(Props, Clone, PartialEq)]
pub struct PresenceBannerProps {
    pub bind: BindStatus,
    pub network_mode_label: String,
    pub status_text: String,
    pub remote_controlled: bool,
    pub executing_remotely: bool,
    pub controller_names: Vec<String>,
    pub selected_executor_name: Option<String>,
    pub port: u16,
}

#[component]
pub fn PresenceBanner(props: PresenceBannerProps) -> Element {
    let view = banner_view(&props);
    let title = if props.status_text.is_empty() {
        props.network_mode_label.clone()
    } else {
        format!("{} · {}", props.network_mode_label, props.status_text)
    };

    rsx! {
        div {
            class: "presence-banner {view.modifier}",
            title: "{title}",
            span { class: "presence-banner__pip", aria_hidden: "true" }
            span { class: "presence-banner__text", "{view.text}" }
            if let Some(hint) = view.hint {
                span { class: "presence-banner__hint", "{hint}" }
            }
        }
    }
}

struct BannerView {
    modifier: &'static str,
    text: String,
    hint: Option<&'static str>,
}

fn banner_view(props: &PresenceBannerProps) -> BannerView {
    if props.executing_remotely {
        let text = match props.selected_executor_name.as_deref() {
            Some(name) if !name.is_empty() => format!("正在远程执行 {name}"),
            _ => "正在远程执行".to_string(),
        };
        return BannerView {
            modifier: "presence-banner--executing",
            text,
            hint: None,
        };
    }

    if props.remote_controlled {
        let names = props
            .controller_names
            .iter()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("、");
        let text = if names.is_empty() {
            "正在接受远程控制".to_string()
        } else {
            format!("正在接受远程控制 {names}")
        };
        return BannerView {
            modifier: "presence-banner--controlled",
            text,
            hint: None,
        };
    }

    match &props.bind {
        BindStatus::Bound { .. } => BannerView {
            modifier: "presence-banner--bound presence-banner--listening",
            text: format!("监听 {}", props.port),
            hint: None,
        },
        BindStatus::BindFailed { .. } => BannerView {
            modifier: "presence-banner--failed presence-banner--bind-failed presence-banner--firewall",
            text: "端口无法监听".to_string(),
            hint: Some(FIREWALL_HINT),
        },
        BindStatus::Unavailable => BannerView {
            modifier: "presence-banner--failed presence-banner--unavailable presence-banner--firewall",
            text: "端口无法监听".to_string(),
            hint: Some(FIREWALL_HINT),
        },
        BindStatus::Offline => BannerView {
            modifier: "presence-banner--offline",
            text: "Offline".to_string(),
            hint: None,
        },
    }
}
