use dioxus::prelude::*;

use super::overlay::Overlay;

// ---------------------------------------------------------------------------
// SettingsPanel — device name configuration overlay
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct SettingsPanelProps {
    /// Current display name shown in the input field.
    pub display_name: String,
    /// Status message shown after a save attempt (empty when idle).
    pub save_status: String,
    /// Fires when the user clicks the backdrop or close button.
    pub onclose: EventHandler<MouseEvent>,
    /// Fires when the user clicks "Save"; carries the current input value.
    pub on_save_name: EventHandler<String>,
}

#[component]
pub fn SettingsPanel(props: SettingsPanelProps) -> Element {
    rsx! {
        Overlay {
            visible: true,
            title: "\u{F013} 设置",
            onclose: move |evt| props.onclose.call(evt),

            SettingsContent {
                display_name: props.display_name.clone(),
                save_status: props.save_status.clone(),
                on_save_name: props.on_save_name,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SettingsContentProps {
    /// Current display name shown in the input field.
    pub display_name: String,
    /// Status message shown after a save attempt (empty when idle).
    pub save_status: String,
    /// Fires when the user clicks "Save"; carries the current input value.
    pub on_save_name: EventHandler<String>,
}

#[component]
pub fn SettingsContent(props: SettingsContentProps) -> Element {
    let mut input_value = use_signal(|| props.display_name.clone());

    // Keep the local signal in sync when the parent pushes a new value.
    let display_name = props.display_name.clone();
    use_effect(move || {
        input_value.set(display_name.clone());
    });

    let status_class = if props.save_status == "已保存" {
        "settings-save-status settings-save-status--success"
    } else {
        "settings-save-status"
    };

    rsx! {
        div { class: "settings-content",
            div {
                class: "settings-section-header",
                span {
                    class: "settings-section-header__text",
                    "设备"
                }
            }

            // Device name input row
            div {
                class: "settings-input-row",
                span {
                    class: "settings-input-row__label",
                    "\u{F007} 设备名称"
                }
                input {
                    class: "settings-input",
                    r#type: "text",
                    placeholder: "输入设备名称...",
                    value: "{input_value}",
                    oninput: move |evt| input_value.set(evt.value()),
                }
            }

            // Save status (conditionally rendered)
            if !props.save_status.is_empty() {
                div {
                    class: "{status_class}",
                    span { "{props.save_status}" }
                }
            }

            // Save button row
            div {
                class: "settings-save-row",
                div { class: "settings-save-row__spacer" }
                button {
                    class: "skeu-btn skeu-btn--eq",
                    onclick: move |_| props.on_save_name.call(input_value()),
                    span { class: "skeu-btn__label", "保存" }
                }
                div { class: "settings-save-row__spacer" }
            }
        }
    }
}
