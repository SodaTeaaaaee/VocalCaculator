use dioxus::prelude::*;

/// Skeuomorphic button component.
///
/// Supports two content modes (mutually exclusive):
/// - Text label: `SkeuBtn { label: "7".into(), .. }`
/// - Icon glyph: `SkeuBtn { icon: "\u{232B}".into(), .. }`
///
/// The `btn_type` maps to a CSS color variant class:
/// `digit`, `op`, `func`, `clear`, `ci`, `bs`, `eq`.
#[component]
pub fn SkeuBtn(
    /// Text to display inside the button.
    #[props(default)]
    label: Option<String>,
    /// Nerd Font glyph to display instead of text.
    #[props(default)]
    icon: Option<String>,
    /// CSS color variant: `digit`, `op`, `func`, `clear`, `ci`, `bs`, `eq`.
    btn_type: String,
    /// If true, the button spans 2 grid columns.
    #[props(default)]
    colspan: Option<bool>,
    /// If true, applies the keyboard-pressed visual state.
    #[props(default)]
    keyboard_active: Option<bool>,
    /// Optional tooltip, usually used for keyboard shortcut hints.
    #[props(default)]
    title: Option<String>,
    /// Click handler.
    onclick: EventHandler<MouseEvent>,
) -> Element {
    let mut class = format!("skeu-btn skeu-btn--{btn_type}");

    if colspan.unwrap_or(false) {
        class.push_str(" skeu-btn--span-2");
    }

    if keyboard_active.unwrap_or(false) {
        class.push_str(" skeu-btn--keyboard-active");
    }

    rsx! {
        button {
            class,
            title: title.unwrap_or_default(),
            tabindex: "-1",
            onmousedown: move |evt| evt.prevent_default(),
            onclick: move |evt| onclick.call(evt),

            if let Some(icon_text) = &icon {
                span { class: "skeu-btn__icon", "{icon_text}" }
            } else if let Some(label_text) = &label {
                span { class: "skeu-btn__label", "{label_text}" }
            }
        }
    }
}
