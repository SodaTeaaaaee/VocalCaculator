use dioxus::prelude::*;

use super::icon::{Icon, IconName};

#[derive(Props, Clone, PartialEq)]
pub struct IconButtonProps {
    pub icon: IconName,
    pub title: String,
    #[props(default)]
    pub active: Option<bool>,
    #[props(default)]
    pub variant: Option<String>,
    #[props(default)]
    pub class: Option<String>,
    pub onclick: EventHandler<MouseEvent>,
}

#[component]
pub fn IconButton(props: IconButtonProps) -> Element {
    let mut classes = vec!["icon-button"];

    if let Some(ref variant) = props.variant {
        match variant.as_str() {
            "remote-controlled" => classes.push("icon-button--remote-controlled"),
            "executing-remotely" => classes.push("icon-button--executing-remotely"),
            "network" => classes.push("icon-button--network"),
            "quiet" => classes.push("icon-button--quiet"),
            _ => {}
        }
    }

    if props.active == Some(false) {
        classes.push("icon-button--inactive");
    }

    let extra_class = props.class.clone();
    if let Some(ref class) = extra_class {
        classes.push(class.as_str());
    }

    let class_str = classes.join(" ");

    rsx! {
        button {
            class: "{class_str}",
            r#type: "button",
            title: "{props.title}",
            aria_label: "{props.title}",
            onclick: move |evt| props.onclick.call(evt),
            Icon { name: props.icon, class: Some("icon-button__icon".to_string()) }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct PanelSectionProps {
    pub title: String,
    #[props(default)]
    pub icon: Option<IconName>,
    #[props(default)]
    pub class: Option<String>,
    pub children: Element,
}

#[component]
pub fn PanelSection(props: PanelSectionProps) -> Element {
    let class_str = props
        .class
        .as_deref()
        .map(|class| format!("panel-section {class}"))
        .unwrap_or_else(|| "panel-section".to_string());

    rsx! {
        section { class: "{class_str}",
            div { class: "panel-section__header",
                if let Some(icon) = props.icon {
                    Icon { name: icon, class: Some("panel-section__icon".to_string()) }
                }
                span { class: "panel-section__title", "{props.title}" }
            }

            div { class: "panel-section__body",
                {props.children}
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ControlRowProps {
    pub label: String,
    #[props(default)]
    pub icon: Option<IconName>,
    #[props(default)]
    pub value: Option<String>,
    #[props(default)]
    pub class: Option<String>,
    pub children: Element,
}

#[component]
pub fn ControlRow(props: ControlRowProps) -> Element {
    let class_str = props
        .class
        .as_deref()
        .map(|class| format!("control-row {class}"))
        .unwrap_or_else(|| "control-row".to_string());
    let value = props.value.clone();

    rsx! {
        div { class: "{class_str}",
            div { class: "control-row__label",
                if let Some(icon) = props.icon {
                    Icon { name: icon, class: Some("control-row__icon".to_string()) }
                }
                span { "{props.label}" }
            }

            div { class: "control-row__content",
                if let Some(value) = value {
                    span { class: "control-row__value", "{value}" }
                }
                {props.children}
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct AudioControlGroupProps {
    pub audio_status: String,
    pub audio_muted: bool,
    pub audio_volume: f64,
    pub mode_indicator: String,
    pub on_switch_audio_mode: EventHandler<()>,
    pub on_toggle_mute: EventHandler<()>,
    pub on_volume_changed: EventHandler<f64>,
}

#[component]
pub fn AudioControlGroup(props: AudioControlGroupProps) -> Element {
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
    let volume_percent = (props.audio_volume.clamp(0.0, 1.0) * 100.0).round() as i32;

    rsx! {
        PanelSection {
            title: "音频".to_string(),
            icon: Some(IconName::VolumeLow),
            class: Some("audio-control-group".to_string()),

            ControlRow {
                label: "模式".to_string(),
                icon: Some(IconName::Music),
                value: Some(props.mode_indicator.clone()),
                button {
                    class: "panel-action panel-action--secondary panel-action--with-icon",
                    r#type: "button",
                    onclick: move |_| props.on_switch_audio_mode.call(()),
                    Icon { name: IconName::Music, class: Some("panel-action__icon".to_string()) }
                    span { "切换" }
                }
            }

            ControlRow {
                label: "静音".to_string(),
                icon: Some(if props.audio_muted { IconName::VolumeMuted } else { IconName::VolumeHigh }),
                value: Some(mute_state_text.to_string()),
                button {
                    class: "panel-action panel-action--secondary panel-action--with-icon",
                    r#type: "button",
                    onclick: move |_| props.on_toggle_mute.call(()),
                    Icon {
                        name: if props.audio_muted { IconName::VolumeMuted } else { IconName::VolumeHigh },
                        class: Some("panel-action__icon".to_string()),
                    }
                    span { "{mute_text}" }
                }
            }

            ControlRow {
                label: "音量".to_string(),
                icon: Some(IconName::VolumeHigh),
                value: Some(format!("{volume_percent}%")),
                input {
                    class: "panel-volume",
                    r#type: "range",
                    min: "0",
                    max: "100",
                    value: "{volume_percent}",
                    oninput: move |evt| {
                        if let Ok(value) = evt.value().parse::<f64>() {
                            props.on_volume_changed.call((value / 100.0).clamp(0.0, 1.0));
                        }
                    },
                }
            }

            div { class: "audio-control-group__status", "{props.audio_status}" }
        }
    }
}
