#![deny(unsafe_code)]

pub mod app;
pub mod audio;
pub mod core;
pub mod error;
pub mod net;
pub mod traits;

/// Android entry point.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
#[allow(unsafe_code)]
unsafe fn android_main(app: slint::android::AndroidApp) {
    slint::android::init(app).unwrap();
    run_app();
}

/// Shared entry point for running the application.
pub fn run_app() {
    #[cfg(target_os = "android")]
    {
        android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Info),
        );
    }
    #[cfg(not(target_os = "android"))]
    {
        env_logger::Builder::new()
            .filter_level(log::LevelFilter::Info)
            .init();
    }

    log::info!("Vocal Calculator starting");

    let app = match app::App::new() {
        Ok(app) => app,
        Err(e) => {
            log::error!("Failed to create application: {e}");
            return;
        }
    };

    // Fonts are now loaded at compile time via .slint `import` directives.
    // No runtime font registration needed.

    if let Err(e) = app.run() {
        log::error!("Application error: {e}");
    }
}
