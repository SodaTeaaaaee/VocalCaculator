use serde::{Deserialize, Serialize};

/// Generate a random short display name in the style of LocalSend.
///
/// Format: `{Adjective} {Animal}` — e.g., "Happy Panda", "Swift Fox".
/// Uses UUID v4 random bytes for selection (20 x 20 = 400 combinations).
fn generate_random_name() -> String {
    const ADJECTIVES: [&str; 20] = [
        "Happy", "Swift", "Cool", "Brave", "Bright",
        "Calm", "Keen", "Warm", "Bold", "Cute",
        "Deft", "Fair", "Kind", "Neat", "Pure",
        "Wise", "Soft", "Wild", "Epic", "Nice",
    ];
    const NOUNS: [&str; 20] = [
        "Panda", "Fox", "Owl", "Bear", "Wolf",
        "Hawk", "Cat", "Deer", "Seal", "Wren",
        "Hare", "Lynx", "Mink", "Dove", "Crab",
        "Fish", "Moth", "Toad", "Ibis", "Goat",
    ];

    let uuid = uuid::Uuid::new_v4();
    let bytes = uuid.as_bytes();
    let adj = ADJECTIVES[bytes[0] as usize % ADJECTIVES.len()];
    let noun = NOUNS[bytes[1] as usize % NOUNS.len()];
    format!("{adj} {noun}")
}

/// Network-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub enabled: bool,
    pub display_name: String,
    pub allow_remote_control: bool,
    pub conflict_policy: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            display_name: generate_random_name(),
            allow_remote_control: false,
            conflict_policy: "interleaved".to_string(),
        }
    }
}

/// Application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub audio_mode: String,
    pub music_assets_path: Option<String>,
    pub network: NetworkConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            audio_mode: "normal".to_string(),
            music_assets_path: None,
            network: NetworkConfig::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_config_default_roundtrip() {
        let original = AppConfig::default();
        let serialized = toml::to_string(&original).expect("serialization should succeed");
        let deserialized: AppConfig =
            toml::from_str(&serialized).expect("deserialization should succeed");
        assert_eq!(original.audio_mode, deserialized.audio_mode);
        assert_eq!(original.music_assets_path, deserialized.music_assets_path);
        assert_eq!(
            original.network.enabled,
            deserialized.network.enabled
        );
        assert_eq!(
            original.network.allow_remote_control,
            deserialized.network.allow_remote_control
        );
        assert_eq!(
            original.network.conflict_policy,
            deserialized.network.conflict_policy
        );
    }

    #[test]
    fn network_config_defaults() {
        let nc = NetworkConfig::default();
        assert!(nc.enabled);
        assert!(!nc.allow_remote_control);
        assert_eq!(nc.conflict_policy, "interleaved");
        // display_name is a random "Adjective Noun" pair
        assert!(!nc.display_name.is_empty());
        assert!(
            nc.display_name.contains(' '),
            "random name should contain a space: {}",
            nc.display_name
        );
        let parts: Vec<&str> = nc.display_name.splitn(3, ' ').collect();
        assert_eq!(parts.len(), 2, "random name should be two words: {}", nc.display_name);
    }

    #[test]
    fn app_config_defaults() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.audio_mode, "normal");
        assert!(cfg.music_assets_path.is_none());
    }

    #[test]
    fn invalid_toml_falls_back_to_defaults() {
        let garbage = "not [[[ valid toml";
        let result = toml::from_str::<AppConfig>(garbage);
        // `toml::from_str` returns an error for invalid TOML.
        // `AppConfig::load` uses `unwrap_or_default()` on this error path.
        assert!(result.is_err());

        // Simulate the same fallback behaviour `AppConfig::load` uses.
        let cfg = result.unwrap_or_default();
        assert_eq!(cfg.audio_mode, "normal");
        assert!(cfg.music_assets_path.is_none());
        assert!(cfg.network.enabled);
        assert_eq!(cfg.network.conflict_policy, "interleaved");
    }

    #[test]
    fn altered_toml_values_parse_correctly() {
        // Only set audio_mode; everything else should get serde defaults if
        // serde(default) were used, but since it is not, deserialization
        // requires all fields.  A fully-valid minimal TOML must include every
        // field, so test that the full default serialises and round-trips.
        //
        // Instead, test a complete but altered TOML:
        let toml_str = r#"
audio_mode = "silent"
music_assets_path = "/assets"

[network]
enabled = false
display_name = "TestHost"
allow_remote_control = false
conflict_policy = "strict"
"#;
        let cfg: AppConfig = toml::from_str(toml_str).expect("valid TOML should parse");
        assert_eq!(cfg.audio_mode, "silent");
        assert_eq!(cfg.music_assets_path.as_deref(), Some("/assets"));
        assert!(!cfg.network.enabled);
        assert_eq!(cfg.network.display_name, "TestHost");
        assert!(!cfg.network.allow_remote_control);
        assert_eq!(cfg.network.conflict_policy, "strict");
    }

    #[test]
    fn roundtrip_with_non_default_values() {
        let mut cfg = AppConfig::default();
        cfg.audio_mode = "silent".to_string();
        cfg.music_assets_path = Some("/custom/path".to_string());
        cfg.network.enabled = false;
        cfg.network.display_name = "MyDevice".to_string();
        cfg.network.allow_remote_control = false;
        cfg.network.conflict_policy = "strict".to_string();

        let serialized = toml::to_string(&cfg).expect("serialization should succeed");
        let deserialized: AppConfig =
            toml::from_str(&serialized).expect("deserialization should succeed");
        assert_eq!(cfg.audio_mode, deserialized.audio_mode);
        assert_eq!(cfg.music_assets_path, deserialized.music_assets_path);
        assert_eq!(cfg.network.enabled, deserialized.network.enabled);
        assert_eq!(cfg.network.display_name, deserialized.network.display_name);
        assert_eq!(
            cfg.network.allow_remote_control,
            deserialized.network.allow_remote_control
        );
        assert_eq!(
            cfg.network.conflict_policy,
            deserialized.network.conflict_policy
        );
    }
}
