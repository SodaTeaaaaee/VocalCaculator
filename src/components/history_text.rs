use dioxus::prelude::*;

/// History text line rendered above the LCD display.
#[derive(Props, Clone, PartialEq)]
pub struct HistoryTextProps {
    pub history_text: String,
    #[props(default)]
    pub dark_mode: bool,
}

#[component]
pub fn HistoryText(props: HistoryTextProps) -> Element {
    let class = if props.dark_mode {
        "history-text dark"
    } else {
        "history-text"
    };

    rsx! {
        div { class,
            "{props.history_text}"
        }
    }
}
