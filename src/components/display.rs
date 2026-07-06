use dioxus::prelude::*;

/// LCD display component.
///
/// Renders a skeuomorphic LCD screen with optional error state
/// and dark mode support.
#[component]
pub fn LcdDisplay(
    /// Current display text (e.g. "0", "123.45", "Error").
    display_text: String,
    /// If true, swaps to the red error palette.
    #[props(default)]
    error_state: bool,
    /// Whether dark mode is active.
    #[props(default)]
    dark_mode: bool,
) -> Element {
    let mut class = String::from("lcd-display");
    if error_state {
        class.push_str(" error");
    }
    if dark_mode {
        class.push_str(" dark");
    }
    let clip_class = if display_text.chars().count() > 16 {
        "lcd-display__clip lcd-display__clip--truncated"
    } else {
        "lcd-display__clip"
    };

    rsx! {
        div { class,
            div { class: "lcd-display__inner",
                span { class: clip_class, title: "{display_text}",
                    span { class: "lcd-display__text", "{display_text}" }
                }
            }
        }
    }
}
