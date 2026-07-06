use dioxus::prelude::*;

use super::button::SkeuBtn;
use super::calculator::BUTTON_ROWS;

fn keyboard_hint_for_action(action: &str) -> Option<&'static str> {
    match action {
        "digit:0" => Some("0 / 小键盘 0"),
        "digit:1" => Some("1 / 小键盘 1"),
        "digit:2" => Some("2 / 小键盘 2"),
        "digit:3" => Some("3 / 小键盘 3"),
        "digit:4" => Some("4 / 小键盘 4"),
        "digit:5" => Some("5 / 小键盘 5"),
        "digit:6" => Some("6 / 小键盘 6"),
        "digit:7" => Some("7 / 小键盘 7"),
        "digit:8" => Some("8 / 小键盘 8"),
        "digit:9" => Some("9 / 小键盘 9"),
        "decimal-point" => Some(". / ,"),
        "add" => Some("+"),
        "subtract" => Some("-"),
        "multiply" => Some("*"),
        "divide" => Some("/"),
        "equals" => Some("Enter / ="),
        "percent" => Some("% / P"),
        "mu" => Some("U"),
        "sqrt" => Some("R / S"),
        "backspace" => Some("Backspace"),
        "clear" => Some("Delete / C"),
        "all-clear" => Some("Esc"),
        "plus-minus" => Some("N / F9"),
        "memory-recall" => Some("M"),
        "memory-add" => Some("A"),
        "memory-subtract" => Some("B"),
        "memory-clear" => Some("X"),
        _ => None,
    }
}

/// Button grid -- renders the 7-row calculator button layout.
///
/// Each event handler corresponds to a calculator action.  The grid
/// delegates individual button rendering to [`super::button::SkeuBtn`].
#[derive(Props, Clone, PartialEq)]
pub struct ButtonGridProps {
    pub on_digit_pressed: EventHandler<u8>,
    pub on_decimal_point: EventHandler<()>,
    pub on_operator_pressed: EventHandler<String>,
    pub on_equals: EventHandler<()>,
    pub on_percent: EventHandler<()>,
    pub on_mu: EventHandler<()>,
    pub on_square_root: EventHandler<()>,
    pub on_backspace: EventHandler<()>,
    pub on_clear_input: EventHandler<()>,
    pub on_all_clear: EventHandler<()>,
    pub on_plus_minus: EventHandler<()>,
    pub on_memory_recall: EventHandler<()>,
    pub on_memory_add: EventHandler<()>,
    pub on_memory_subtract: EventHandler<()>,
    pub on_memory_clear: EventHandler<()>,
    #[props(default)]
    pub keyboard_pressed: bool,
    #[props(default)]
    pub last_keyboard_action: String,
}

#[component]
pub fn ButtonGrid(props: ButtonGridProps) -> Element {
    rsx! {
        div { class: "button-grid",
            for row in BUTTON_ROWS {
                for button in row.iter() {
                    {
                        let label = button.label.map(str::to_string);
                        let icon = button.icon.map(str::to_string);
                        let btn_type = button.btn_type.to_string();
                        let action = button.action.to_string();
                        let keyboard_active = props.keyboard_pressed
                            && props.last_keyboard_action == button.action;
                        let colspan = button.colspan > 1;
                        let title = keyboard_hint_for_action(button.action)
                            .map(|hint| format!("快捷键: {hint}"));

                        rsx! {
                            SkeuBtn {
                                key: "{button.action}",
                                label,
                                icon,
                                btn_type,
                                colspan,
                                keyboard_active,
                                title,
                                onclick: move |_| match action.as_str() {
                                    "digit:0" => props.on_digit_pressed.call(0),
                                    "digit:1" => props.on_digit_pressed.call(1),
                                    "digit:2" => props.on_digit_pressed.call(2),
                                    "digit:3" => props.on_digit_pressed.call(3),
                                    "digit:4" => props.on_digit_pressed.call(4),
                                    "digit:5" => props.on_digit_pressed.call(5),
                                    "digit:6" => props.on_digit_pressed.call(6),
                                    "digit:7" => props.on_digit_pressed.call(7),
                                    "digit:8" => props.on_digit_pressed.call(8),
                                    "digit:9" => props.on_digit_pressed.call(9),
                                    "decimal-point" => props.on_decimal_point.call(()),
                                    "add" => props.on_operator_pressed.call("+".to_string()),
                                    "subtract" => props.on_operator_pressed.call("-".to_string()),
                                    "multiply" => props.on_operator_pressed.call("*".to_string()),
                                    "divide" => props.on_operator_pressed.call("/".to_string()),
                                    "equals" => props.on_equals.call(()),
                                    "percent" => props.on_percent.call(()),
                                    "mu" => props.on_mu.call(()),
                                    "sqrt" => props.on_square_root.call(()),
                                    "backspace" => props.on_backspace.call(()),
                                    "clear" => props.on_clear_input.call(()),
                                    "all-clear" => props.on_all_clear.call(()),
                                    "plus-minus" => props.on_plus_minus.call(()),
                                    "memory-recall" => props.on_memory_recall.call(()),
                                    "memory-add" => props.on_memory_add.call(()),
                                    "memory-subtract" => props.on_memory_subtract.call(()),
                                    "memory-clear" => props.on_memory_clear.call(()),
                                    _ => {}
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}
