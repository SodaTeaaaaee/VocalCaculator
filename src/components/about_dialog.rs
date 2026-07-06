use dioxus::prelude::*;

use super::overlay::Overlay;

/// About dialog component.
///
/// Wraps the reusable [`Overlay`] to display application credits.
/// The parent is expected to conditionally render this component
/// only when the about dialog should be visible.
#[component]
pub fn AboutDialog(
    /// Semantic version string (e.g. "v0.1.0").
    app_version: String,
    /// Fired when the user closes the dialog (backdrop click or close button).
    onclose: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        Overlay {
            visible: true,
            title: "语音计算器",
            onclose: move |evt| onclose.call(evt),

            div { class: "about-content",
                span { class: "about-content__version", "{app_version}" }
                span { class: "about-content__author", "by Starberry" }
            }
        }
    }
}
