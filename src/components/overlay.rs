use dioxus::prelude::*;

use super::icon::{Icon, IconName};

// ---------------------------------------------------------------------------
// Overlay — reusable backdrop + card wrapper shared by Network, Settings, About
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayVariant {
    Compact,
    Large,
}

#[derive(Props, Clone, PartialEq)]
pub struct OverlayProps {
    pub visible: bool,
    pub title: String,
    #[props(default)]
    pub icon: Option<IconName>,
    pub variant: OverlayVariant,
    pub onclose: EventHandler<MouseEvent>,
    pub children: Element,
}

#[component]
pub fn Overlay(props: OverlayProps) -> Element {
    let backdrop_class = if props.visible {
        "overlay-backdrop visible"
    } else {
        "overlay-backdrop"
    };
    let card_class = match props.variant {
        OverlayVariant::Compact => "overlay-card overlay-card--compact",
        OverlayVariant::Large => "overlay-card overlay-card--large",
    };

    rsx! {
        if props.visible {
            div {
                class: backdrop_class,
                onclick: move |evt| props.onclose.call(evt),

                div {
                    class: card_class,
                    onclick: move |evt| evt.stop_propagation(),

                    div {
                        class: "overlay-card__inner",

                        div {
                            class: "overlay-title",

                            if let Some(icon) = props.icon {
                                Icon { name: icon, class: Some("overlay-title__icon".to_string()) }
                            }

                            span {
                                class: "overlay-title__text",
                                "{props.title}"
                            }

                            button {
                                class: "overlay-title__close",
                                r#type: "button",
                                title: "关闭",
                                aria_label: "关闭",
                                onclick: move |evt| props.onclose.call(evt),
                                Icon { name: IconName::X }
                            }
                        }

                        {props.children}
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ToggleSwitch — pill-shaped on/off toggle
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct ToggleSwitchProps {
    pub on: bool,
    pub label: String,
    #[props(default)]
    pub icon: Option<IconName>,
    pub on_toggle: EventHandler<MouseEvent>,
}

#[component]
pub fn ToggleSwitch(props: ToggleSwitchProps) -> Element {
    let switch_class = if props.on {
        "toggle-switch toggle-switch--on"
    } else {
        "toggle-switch"
    };

    rsx! {
        div {
            class: "toggle-row",

            span {
                class: "toggle-row__label",
                if let Some(icon) = props.icon {
                    Icon { name: icon, class: Some("toggle-row__icon".to_string()) }
                }
                "{props.label}"
            }

            button {
                class: switch_class,
                r#type: "button",
                role: "switch",
                aria_checked: if props.on { "true" } else { "false" },
                aria_label: "{props.label}",
                onclick: move |evt| props.on_toggle.call(evt),

                div {
                    class: "toggle-switch__knob"
                }
            }
        }
    }
}
