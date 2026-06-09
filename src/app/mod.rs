pub mod config;

use std::cell::RefCell;
use std::rc::Rc;

use crate::audio::VocalAudio;
use crate::core::calculator::Calculator;
use crate::core::token::BinaryOp;

slint::include_modules!();

/// Central application controller.
pub struct App {
    calculator: Calculator,
    audio: Option<VocalAudio>,
    window: CalculatorWindow,
}

impl App {
    pub fn new() -> Result<Self, slint::PlatformError> {
        let window = CalculatorWindow::new()?;
        let (audio, status) = match VocalAudio::new() {
            Some(a) => {
                let n = a.sound_count();
                (Some(a), format!("Audio OK ({n} sounds)"))
            }
            None => (None, "No audio device".to_string()),
        };
        let calculator = Calculator::new();
        let app = Self {
            calculator,
            audio,
            window,
        };
        app.window.set_audio_status(status.into());

        // Detect system dark mode and apply
        let dark = crate::detect_system_dark_mode();
        app.window.set_dark_mode(dark);

        Ok(app)
    }

    pub fn run(mut self) -> Result<(), slint::PlatformError> {
        let w = self.window.clone_strong();
        let calc_ref = Rc::new(RefCell::new(self.calculator));
        let audio_ref = Rc::new(RefCell::new(self.audio.take()));

        w.set_display_text("0".into());
        w.set_mode_indicator("Normal".into());
        if audio_ref.borrow().is_none() {
            w.set_audio_status("No audio device".into());
        }

        // Helper closure macro to reduce boilerplate
        // Digit (takes int argument)
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_digit_pressed(move |d| {
                let r = c.borrow_mut().digit(d as u8);
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_memory_indicator(r.memory_indicator.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Decimal point
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_decimal_point(move || {
                let r = c.borrow_mut().decimal_point();
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Operator (takes string argument)
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_operator_pressed(move |op| {
                let binary_op = match op.as_str() {
                    "+" => BinaryOp::Add,
                    "-" => BinaryOp::Subtract,
                    "*" => BinaryOp::Multiply,
                    "/" => BinaryOp::Divide,
                    _ => return,
                };
                let r = c.borrow_mut().operator(binary_op);
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Equals
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_equals(move || {
                let r = c.borrow_mut().equals();
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Percent
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_percent(move || {
                let r = c.borrow_mut().percent();
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // MU
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_mu(move || {
                let r = c.borrow_mut().mu();
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Square root
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_square_root(move || {
                let r = c.borrow_mut().square_root();
                w2.set_display_text(r.display.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Backspace
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_backspace(move || {
                let r = c.borrow_mut().backspace();
                w2.set_display_text(r.display.clone().into());
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Clear
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_clear_input(move || {
                let r = c.borrow_mut().clear();
                w2.set_display_text(r.display.clone().into());
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // All Clear
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_all_clear(move || {
                let r = c.borrow_mut().all_clear();
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_memory_indicator(r.memory_indicator.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Plus/Minus
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_plus_minus(move || {
                let r = c.borrow_mut().plus_minus();
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Memory Recall
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_memory_recall(move || {
                let r = c.borrow_mut().memory_recall();
                w2.set_display_text(r.display.clone().into());
                w2.set_memory_indicator(r.memory_indicator.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Memory Add
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_memory_add(move || {
                let r = c.borrow_mut().memory_add();
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_memory_indicator(r.memory_indicator.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Memory Subtract
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_memory_subtract(move || {
                let r = c.borrow_mut().memory_subtract();
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_memory_indicator(r.memory_indicator.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Memory Clear
        {
            let w2 = w.clone_strong();
            let c = calc_ref.clone();
            let a = audio_ref.clone();
            w.on_memory_clear(move || {
                let r = c.borrow_mut().memory_clear();
                w2.set_display_text(r.display.clone().into());
                w2.set_history_text(r.history.clone().into());
                w2.set_memory_indicator(r.memory_indicator.clone().into());
                w2.set_error_state(r.is_error);
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.play_events(&r.events);
                }
            });
        }

        // Switch audio mode
        {
            let a = audio_ref.clone();
            let w2 = w.clone_strong();
            w.on_switch_audio_mode(move || {
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.cycle_mode();
                    w2.set_mode_indicator(audio.mode().name().into());
                }
            });
        }

        // Show about (placeholder)
        w.on_show_about(move || {});

        // Toggle theme
        {
            let w2 = w.clone_strong();
            w.on_toggle_theme(move || {
                let current = w2.get_dark_mode();
                w2.set_dark_mode(!current);
            });
        }


        // Volume changed (slider 0.0..1.0 -> dB-scaled volume)
        {
            let a = audio_ref.clone();
            w.on_volume_changed(move |v| {
                if let Some(ref mut audio) = *a.borrow_mut() {
                    audio.set_volume(v as f64);
                }
            });
        }
        w.run()
    }
}
