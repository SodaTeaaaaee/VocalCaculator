use serde::{Deserialize, Serialize};

/// Application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub audio_mode: String,
    pub music_assets_path: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            audio_mode: "normal".to_string(),
            music_assets_path: None,
        }
    }
}

impl AppConfig {
    /// Load config from the standard config directory, or return default.
    pub fn load() -> Self {
        let config_dir = match sysdirs::config_dir() {
            Some(dir) => dir.join("vocal_calculator"),
            None => return Self::default(),
        };
        let config_file = config_dir.join("config.toml");
        match std::fs::read_to_string(&config_file) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save config to the standard config directory.
    pub fn save(&self) -> Result<(), anyhow::Error> {
        let config_dir = sysdirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?
            .join("vocal_calculator");
        std::fs::create_dir_all(&config_dir)?;
        let config_file = config_dir.join("config.toml");
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(config_file, contents)?;
        Ok(())
    }
}
