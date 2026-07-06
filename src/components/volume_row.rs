use dioxus::prelude::*;

use super::icon::{Icon, IconName};

/// Volume slider row with mute toggle.
#[derive(Props, Clone, PartialEq)]
pub struct VolumeRowProps {
    pub audio_muted: bool,
    pub volume: f64,
    #[props(default)]
    pub dark_mode: bool,
    pub on_toggle_mute: EventHandler<()>,
    pub on_volume_changed: EventHandler<f64>,
}

#[component]
pub fn VolumeRow(props: VolumeRowProps) -> Element {
    let mute_class = if props.audio_muted {
        "volume-row__mute-btn muted"
    } else {
        "volume-row__mute-btn"
    };
    let mute_icon = if props.audio_muted {
        IconName::VolumeMuted
    } else {
        IconName::VolumeLow
    };
    let mute_title = if props.audio_muted {
        "取消静音 (V)"
    } else {
        "静音 (V)"
    };
    let slider_value = ((props.volume.clamp(0.0, 1.0) * 100.0).round() as i32).to_string();

    rsx! {
        div { class: "volume-row",
            span {
                class: "volume-row__icon",
                Icon { name: IconName::VolumeLow }
            }

            button {
                class: mute_class,
                title: "{mute_title}",
                onclick: move |_| props.on_toggle_mute.call(()),
                Icon { name: mute_icon }
            }

            input {
                class: "volume-row__slider",
                r#type: "range",
                min: "0",
                max: "100",
                value: "{slider_value}",
                oninput: move |evt| {
                    if let Ok(value) = evt.value().parse::<f64>() {
                        props.on_volume_changed.call((value / 100.0).clamp(0.0, 1.0));
                    }
                },
            }

            span {
                class: "volume-row__icon",
                Icon { name: IconName::VolumeHigh }
            }
        }
    }
}
