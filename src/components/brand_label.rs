use dioxus::prelude::*;

/// Compact product and mode label at the top of the calculator body.
#[derive(Props, Clone, PartialEq)]
pub struct BrandLabelProps {
    #[props(default)]
    pub dark_mode: bool,
    pub mode_indicator: String,
}

#[component]
pub fn BrandLabel(props: BrandLabelProps) -> Element {
    let class = if props.dark_mode {
        "brand-label dark"
    } else {
        "brand-label"
    };

    rsx! {
        div { class,
            span { class: "brand-label__name", "VocalCalculator" }
            span { class: "brand-label__mode", "{props.mode_indicator}" }
        }
    }
}
