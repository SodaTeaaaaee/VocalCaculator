use serde::{Deserialize, Serialize};

/// Generate a random short display name in the style of LocalSend.
///
/// Format: `{Adjective} {Animal}` — e.g., "Happy Panda", "Swift Fox".
/// Uses UUID v4 random bytes for selection (20 x 20 = 400 combinations).
fn generate_random_name() -> String {
    const ADJECTIVES: [&str; 20] = [
        "Happy", "Swift", "Cool", "Brave", "Bright", "Calm", "Keen", "Warm", "Bold", "Cute",
        "Deft", "Fair", "Kind", "Neat", "Pure", "Wise", "Soft", "Wild", "Epic", "Nice",
    ];
    const NOUNS: [&str; 20] = [
        "Panda", "Fox", "Owl", "Bear", "Wolf", "Hawk", "Cat", "Deer", "Seal", "Wren", "Hare",
        "Lynx", "Mink", "Dove", "Crab", "Fish", "Moth", "Toad", "Ibis", "Goat",
    ];

    let uuid = uuid::Uuid::new_v4();
    let bytes = uuid.as_bytes();
    let adj = ADJECTIVES[bytes[0] as usize % ADJECTIVES.len()];
    let noun = NOUNS[bytes[1] as usize % NOUNS.len()];
    format!("{adj} {noun}")
}

/// Network-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    pub enabled: bool,
    pub display_name: String,
    /// The sole persisted inbound remote-control permission boundary.
    pub allow_remote_control: bool,
    /// Explicit network mode ("lan" | "offline" | "loopback-test"), as
    /// parsed by [`crate::app::network_mode`]. `None` when absent from
    /// an older config file; in that case `enabled` is used as a legacy
    /// fallback (see `resolve_network_mode`). This field intentionally
    /// has no default other than `None` so that the legacy migration
    /// path can be distinguished from an explicit choice.
    #[serde(default)]
    pub mode: Option<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            display_name: generate_random_name(),
            allow_remote_control: false,
            mode: None,
        }
    }
}

/// Application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub audio_mode: String,
    pub volume: f64,
    pub muted: bool,
    pub dark_mode: bool,
    pub music_assets_path: Option<String>,
    pub network: NetworkConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            audio_mode: "normal".to_string(),
            volume: 0.5,
            muted: false,
            dark_mode: false,
            music_assets_path: None,
            network: NetworkConfig::default(),
        }
    }
}

impl AppConfig {
    /// Build a fail-closed fallback for an existing configuration file that
    /// could not be read or parsed.
    ///
    /// A genuinely missing file still uses [`Default`] (and therefore the
    /// product's normal LAN default).  Once a configuration file exists,
    /// however, corruption, a type error, or an I/O failure must never turn
    /// networking back on implicitly.
    fn fail_closed_offline() -> Self {
        let mut config = Self::default();
        config.network.enabled = false;
        config.network.mode = Some("offline".to_string());
        config
    }

    fn load_from_path(config_file: &std::path::Path) -> Self {
        let contents = match std::fs::read_to_string(config_file) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::default();
            }
            Err(error) => {
                log::error!(
                    "Failed to read existing config file {}: {}. Networking is forced offline.",
                    config_file.display(),
                    error
                );
                return Self::fail_closed_offline();
            }
        };

        match toml::from_str(&contents) {
            Ok(config) => config,
            Err(error) => {
                log::error!(
                    "Failed to parse existing config file {}: {}. Networking is forced offline.",
                    config_file.display(),
                    error
                );
                Self::fail_closed_offline()
            }
        }
    }

    /// Load config from the standard config directory.
    ///
    /// A missing file uses the normal defaults.  An existing file that is
    /// unreadable or invalid fails closed to explicit offline mode.
    pub fn load() -> Self {
        let config_dir = match sysdirs::config_dir() {
            Some(dir) => dir.join("vocal_calculator"),
            None => return Self::default(),
        };
        let config_file = config_dir.join("config.toml");
        Self::load_from_path(&config_file)
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

    fn assert_networking_forced_offline(cfg: &AppConfig) {
        use crate::app::network_mode::{NetworkMode, resolve_network_mode};

        assert!(!cfg.network.enabled);
        assert_eq!(cfg.network.mode.as_deref(), Some("offline"));
        assert_eq!(
            resolve_network_mode(&[], None, &cfg.network),
            Ok(NetworkMode::Offline)
        );
    }

    #[test]
    fn app_config_default_roundtrip() {
        let original = AppConfig::default();
        let serialized = toml::to_string(&original).expect("serialization should succeed");
        let deserialized: AppConfig =
            toml::from_str(&serialized).expect("deserialization should succeed");
        assert_eq!(original.audio_mode, deserialized.audio_mode);
        assert_eq!(original.volume, deserialized.volume);
        assert_eq!(original.muted, deserialized.muted);
        assert_eq!(original.dark_mode, deserialized.dark_mode);
        assert_eq!(original.music_assets_path, deserialized.music_assets_path);
        assert_eq!(original.network.enabled, deserialized.network.enabled);
        assert_eq!(
            original.network.allow_remote_control,
            deserialized.network.allow_remote_control
        );
    }

    #[test]
    fn network_config_defaults() {
        let nc = NetworkConfig::default();
        assert!(nc.enabled);
        assert!(!nc.allow_remote_control);
        // display_name is a random "Adjective Noun" pair
        assert!(!nc.display_name.is_empty());
        assert!(
            nc.display_name.contains(' '),
            "random name should contain a space: {}",
            nc.display_name
        );
        let parts: Vec<&str> = nc.display_name.splitn(3, ' ').collect();
        assert_eq!(
            parts.len(),
            2,
            "random name should be two words: {}",
            nc.display_name
        );
    }

    #[test]
    fn app_config_defaults() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.audio_mode, "normal");
        assert_eq!(cfg.volume, 0.5);
        assert!(!cfg.muted);
        assert!(!cfg.dark_mode);
        assert!(cfg.music_assets_path.is_none());
    }

    #[test]
    fn missing_config_file_uses_normal_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::load_from_path(&dir.path().join("missing-config.toml"));

        assert!(cfg.network.enabled);
        assert!(cfg.network.mode.is_none());
        assert_eq!(
            crate::app::network_mode::resolve_network_mode(&[], None, &cfg.network),
            Ok(crate::app::network_mode::NetworkMode::Lan)
        );
    }

    #[test]
    fn invalid_toml_forces_networking_offline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not [[[ valid toml").unwrap();

        let cfg = AppConfig::load_from_path(&path);

        assert_networking_forced_offline(&cfg);
    }

    #[test]
    fn invalid_network_mode_type_forces_networking_offline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[network]\nmode = 123\n").unwrap();

        let cfg = AppConfig::load_from_path(&path);

        assert_networking_forced_offline(&cfg);
    }

    #[test]
    fn unreadable_existing_config_path_forces_networking_offline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::create_dir(&path).unwrap();

        let cfg = AppConfig::load_from_path(&path);

        assert_networking_forced_offline(&cfg);
    }

    #[test]
    fn altered_toml_values_parse_correctly() {
        let toml_str = r#"
audio_mode = "silent"
volume = 0.25
muted = true
dark_mode = true
music_assets_path = "/assets"

[network]
enabled = false
display_name = "TestHost"
allow_remote_control = false
conflict_policy = "strict"
"#;
        let cfg: AppConfig = toml::from_str(toml_str).expect("valid TOML should parse");
        assert_eq!(cfg.audio_mode, "silent");
        assert_eq!(cfg.volume, 0.25);
        assert!(cfg.muted);
        assert!(cfg.dark_mode);
        assert_eq!(cfg.music_assets_path.as_deref(), Some("/assets"));
        assert!(!cfg.network.enabled);
        assert_eq!(cfg.network.display_name, "TestHost");
        // Legacy routing-policy keys are ignored rather than interpreted as
        // remote-control permission.
        assert!(!cfg.network.allow_remote_control);
        assert!(cfg.network.mode.is_none());
    }

    #[test]
    fn missing_fields_parse_with_defaults() {
        let toml_str = r#"
audio_mode = "music"

[network]
display_name = "Existing"
"#;
        let cfg: AppConfig = toml::from_str(toml_str).expect("missing fields should default");
        assert_eq!(cfg.audio_mode, "music");
        assert_eq!(cfg.volume, 0.5);
        assert!(!cfg.muted);
        assert!(!cfg.dark_mode);
        assert!(cfg.network.enabled);
        assert_eq!(cfg.network.display_name, "Existing");
        assert!(!cfg.network.allow_remote_control);
        assert!(cfg.network.mode.is_none());
    }

    #[test]
    fn legacy_toml_without_mode_field_migrates_via_resolve_network_mode() {
        use crate::app::network_mode::{NetworkMode, resolve_network_mode};

        let toml_str = r#"
audio_mode = "normal"

[network]
enabled = false
display_name = "LegacyHost"
"#;
        let cfg: AppConfig = toml::from_str(toml_str).expect("legacy TOML should parse");
        assert!(cfg.network.mode.is_none());
        let resolved = resolve_network_mode(&[], None, &cfg.network)
            .expect("legacy config should resolve without error");
        assert_eq!(resolved, NetworkMode::Offline);

        let toml_str_enabled = r#"
audio_mode = "normal"

[network]
enabled = true
display_name = "LegacyHost"
"#;
        let cfg_enabled: AppConfig =
            toml::from_str(toml_str_enabled).expect("legacy TOML should parse");
        let resolved_enabled = resolve_network_mode(&[], None, &cfg_enabled.network)
            .expect("legacy config should resolve without error");
        assert_eq!(resolved_enabled, NetworkMode::Lan);
    }

    #[test]
    fn roundtrip_with_non_default_values() {
        let cfg = AppConfig {
            audio_mode: "silent".to_string(),
            volume: 0.75,
            muted: true,
            dark_mode: true,
            music_assets_path: Some("/custom/path".to_string()),
            network: NetworkConfig {
                enabled: false,
                display_name: "MyDevice".to_string(),
                allow_remote_control: false,
                mode: None,
            },
        };

        let serialized = toml::to_string(&cfg).expect("serialization should succeed");
        let deserialized: AppConfig =
            toml::from_str(&serialized).expect("deserialization should succeed");
        assert_eq!(cfg.audio_mode, deserialized.audio_mode);
        assert_eq!(cfg.volume, deserialized.volume);
        assert_eq!(cfg.muted, deserialized.muted);
        assert_eq!(cfg.dark_mode, deserialized.dark_mode);
        assert_eq!(cfg.music_assets_path, deserialized.music_assets_path);
        assert_eq!(cfg.network.enabled, deserialized.network.enabled);
        assert_eq!(cfg.network.display_name, deserialized.network.display_name);
        assert_eq!(
            cfg.network.allow_remote_control,
            deserialized.network.allow_remote_control
        );
    }
}
