use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// Overlay — reusable backdrop + card wrapper shared by Network, Settings, About
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct OverlayProps {
    pub visible: bool,
    pub title: String,
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

    rsx! {
        if props.visible {
            div {
                class: backdrop_class,
                onclick: move |evt| props.onclose.call(evt),

                div {
                    class: "overlay-card",
                    onclick: move |evt| evt.stop_propagation(),

                    div {
                        class: "overlay-card__inner",

                        div {
                            class: "overlay-title",

                            span {
                                class: "overlay-title__text",
                                "{props.title}"
                            }

                            button {
                                class: "overlay-title__close",
                                onclick: move |evt| props.onclose.call(evt),
                                "\u{00D7}"
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
                "{props.label}"
            }

            div {
                class: switch_class,
                onclick: move |evt| props.on_toggle.call(evt),

                div {
                    class: "toggle-switch__knob"
                }
            }
        }
    }
}
