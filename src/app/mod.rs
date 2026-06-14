pub mod bridge;
pub mod callbacks;
pub mod config;
pub mod platform;

use std::cell::RefCell;
use std::rc::Rc;

use crate::audio::VocalAudio;
use crate::core::calculator::Calculator;
use crate::net::CalculatorWindow;
use slint::ComponentHandle;

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
                (Some(a), format!("音频正常 ({n} 个音效)"))
            }
            None => (None, "无音频设备".to_string()),
        };
        let calculator = Calculator::new();
        let app = Self {
            calculator,
            audio,
            window,
        };
        app.window.set_audio_status(status.into());

        app.window
            .set_app_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());

        let dark = platform::detect_system_dark_mode();
        app.window.set_dark_mode(dark);

        Ok(app)
    }

    pub fn run(mut self) -> Result<(), slint::PlatformError> {
        let w = self.window.clone_strong();
        let calc_ref = Rc::new(RefCell::new(self.calculator));
        let audio_ref = Rc::new(RefCell::new(self.audio.take()));

        // Build the Router that all callbacks will dispatch through.
        let router = bridge::create_router(calc_ref, audio_ref.clone(), &w);

        // Initial UI state.
        w.set_display_text("0".into());
        w.set_mode_indicator("普通".into());
        if audio_ref.borrow().is_none() {
            w.set_audio_status("无音频设备".into());
        }

        // Initialize networking (returns shared state for callbacks & timer).
        let app_config = config::AppConfig::load();
        let net_ctx = bridge::init_networking(&app_config, &router, &w);

        // Start the poll timer that drains network messages and syncs peers.
        // The timer handle must be kept alive; dropping it cancels the timer.
        let _poll_timer = bridge::start_poll_timer(
            &net_ctx.net_manager,
            &router,
            &net_ctx.net_state,
            &net_ctx.peers_model,
            &w,
            &net_ctx.matrix_node_ids,
        );

        // Register all UI callbacks.
        callbacks::register_calculator_callbacks(&w, &router);
        callbacks::register_keyboard_callbacks(&w, &router);
        callbacks::register_network_callbacks(
            &w,
            &router,
            &net_ctx.net_state,
            &net_ctx.net_manager,
            &net_ctx.scan_timer,
            &net_ctx.matrix_node_ids,
        );
        callbacks::register_audio_callbacks(&w, &audio_ref);
        callbacks::register_settings_callbacks(&w, &app_config, &net_ctx.net_manager);

        // Run the Slint event loop (blocks until window closes).
        let result = w.run();

        // Graceful shutdown.
        if let Some(ref nm) = net_ctx.net_manager {
            nm.borrow_mut().shutdown();
        }

        result
    }
}
