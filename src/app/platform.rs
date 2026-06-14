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
