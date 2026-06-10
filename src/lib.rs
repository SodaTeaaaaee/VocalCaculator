#![deny(unsafe_code)]

pub mod app;
pub mod audio;
pub mod core;

/// Detect whether the OS is running in dark mode.
///
/// On Windows, queries the registry for `AppsUseLightTheme`.
/// Returns `true` if dark mode, `false` otherwise (default).
pub fn detect_system_dark_mode() -> bool {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output()
            .ok()
            .and_then(|output| {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.contains("AppsUseLightTheme") {
                        if line.contains("0x0") {
                            return Some(true); // dark mode
                        } else if line.contains("0x1") {
                            return Some(false); // light mode
                        }
                    }
                }
                None
            })
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

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
