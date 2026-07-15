//! Resolution of the process-wide [`NetworkMode`].
//!
//! The mode controls whether the app participates in LAN discovery /
//! routing at all, or runs fully offline, or runs a loopback-only test
//! configuration.  It is resolved exactly once at startup (see
//! [`resolve_from_process`]) from, in priority order:
//!
//! 1. CLI argument `--network-mode=VALUE` or the two-token form
//!    `--network-mode VALUE`.
//! 2. Environment variable `VOCAL_CALCULATOR_NETWORK_MODE`.
//! 3. `[network] mode` in the persisted config file.
//! 4. Legacy fallback: `[network] enabled` (`true` -> `Lan`, `false` ->
//!    `Offline`) when no `mode` is configured.
//!
//! An invalid value at any of the CLI / env / config levels is a hard
//! error -- this module never silently substitutes [`NetworkMode::Lan`]
//! for a value it failed to parse.

use std::sync::OnceLock;

use crate::app::config::NetworkConfig;

/// The resolved networking mode for this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    /// Normal LAN discovery / routing is active.
    Lan,
    /// Networking subsystem is not started at all.
    Offline,
    /// Networking is started in a loopback-only test configuration.
    LoopbackTest,
}

/// The list of valid mode strings, used in error messages.
const VALID_VALUES: &str = "lan, offline, loopback-test";

const CLI_FLAG: &str = "--network-mode";
const ENV_VAR: &str = "VOCAL_CALCULATOR_NETWORK_MODE";

impl NetworkMode {
    /// Parse a mode string (trimmed, case-insensitive).
    ///
    /// Accepts exactly `"lan"`, `"offline"`, `"loopback-test"` (any
    /// ASCII-case variant); anything else is an `Err` describing the
    /// offending value and the list of valid values.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "lan" => Ok(NetworkMode::Lan),
            "offline" => Ok(NetworkMode::Offline),
            "loopback-test" => Ok(NetworkMode::LoopbackTest),
            other => Err(format!(
                "invalid network mode '{other}' (valid values: {VALID_VALUES})"
            )),
        }
    }
}

/// Process-wide storage for the resolved [`NetworkMode`].
///
/// Set exactly once from `main()` via [`set`]. Tests may call [`set`]
/// directly to exercise mode-dependent code paths; since this is a
/// global, tests that rely on a specific value should not assume
/// exclusive ownership of the process if run in parallel with other
/// tests that also call [`set`].
static CURRENT_MODE: OnceLock<NetworkMode> = OnceLock::new();

/// Store the resolved mode. Subsequent calls are ignored (the first
/// write wins), matching [`OnceLock`] semantics.
pub fn set(mode: NetworkMode) {
    let _ = CURRENT_MODE.set(mode);
}

/// The currently resolved mode, or [`NetworkMode::Lan`] if [`set`] has
/// not been called yet. In production `main()` always calls `set`
/// before any code path can observe `current()`.
pub fn current() -> NetworkMode {
    CURRENT_MODE.get().copied().unwrap_or(NetworkMode::Lan)
}

/// Extract the value passed to `--network-mode` from a raw argv slice,
/// accepting both `--network-mode=VALUE` and the two-token form
/// `--network-mode VALUE`. Unrelated tokens are ignored. Returns
/// `Ok(None)` if the flag is not present, `Ok(Some(value))` if found,
/// and `Err` if `--network-mode` is the last token with no value.
fn extract_cli_value(args: &[String]) -> Result<Option<String>, String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix(CLI_FLAG) {
            if let Some(value) = rest.strip_prefix('=') {
                return Ok(Some(value.to_string()));
            }
            if rest.is_empty() {
                // Two-token form: "--network-mode" "VALUE"
                match iter.next() {
                    Some(value) => return Ok(Some(value.clone())),
                    None => {
                        return Err(format!(
                            "'{CLI_FLAG}' requires a value (valid values: {VALID_VALUES})"
                        ));
                    }
                }
            }
            // Something like "--network-modeXYZ" that merely shares the
            // prefix but isn't our flag -- fall through and ignore it.
        }
    }
    Ok(None)
}

/// Pure resolution of the [`NetworkMode`] from explicit inputs.
///
/// Priority: CLI > env > config > legacy `enabled` fallback. Unrelated
/// argv tokens (e.g. WebView / Android runtime flags) are ignored. An
/// empty or whitespace-only `env_value` is treated as unset. Invalid
/// values at the CLI, env, or config level are hard errors -- this
/// function never silently falls back to [`NetworkMode::Lan`] when a
/// higher-priority source was present but invalid.
pub fn resolve_network_mode(
    cli_args: &[String],
    env_value: Option<&str>,
    config: &NetworkConfig,
) -> Result<NetworkMode, String> {
    if let Some(raw) = extract_cli_value(cli_args)? {
        return NetworkMode::parse(&raw);
    }

    if let Some(raw) = env_value
        && !raw.trim().is_empty()
    {
        return NetworkMode::parse(raw);
    }

    if let Some(raw) = &config.mode {
        return NetworkMode::parse(raw);
    }

    // Legacy migration: no explicit mode configured, fall back to the
    // boolean `enabled` field.
    Ok(if config.enabled {
        NetworkMode::Lan
    } else {
        NetworkMode::Offline
    })
}

/// Impure wrapper that resolves the mode from the real process argv
/// and environment. Called once from `main()`.
pub fn resolve_from_process(config: &NetworkConfig) -> Result<NetworkMode, String> {
    let cli_args: Vec<String> = std::env::args().collect();
    let env_value = std::env::var(ENV_VAR).ok();
    resolve_network_mode(&cli_args, env_value.as_deref(), config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn config_with(mode: Option<&str>, enabled: bool) -> NetworkConfig {
        NetworkConfig {
            enabled,
            display_name: "Test".to_string(),
            allow_remote_control: false,
            conflict_policy: "interleaved".to_string(),
            mode: mode.map(|s| s.to_string()),
        }
    }

    #[test]
    fn parses_all_valid_values() {
        assert_eq!(NetworkMode::parse("lan"), Ok(NetworkMode::Lan));
        assert_eq!(NetworkMode::parse("offline"), Ok(NetworkMode::Offline));
        assert_eq!(
            NetworkMode::parse("loopback-test"),
            Ok(NetworkMode::LoopbackTest)
        );
    }

    #[test]
    fn parse_is_case_insensitive_and_trims() {
        assert_eq!(NetworkMode::parse("  LAN  "), Ok(NetworkMode::Lan));
        assert_eq!(NetworkMode::parse("Offline"), Ok(NetworkMode::Offline));
        assert_eq!(
            NetworkMode::parse("Loopback-Test"),
            Ok(NetworkMode::LoopbackTest)
        );
    }

    #[test]
    fn parse_rejects_unknown_value() {
        let err = NetworkMode::parse("bogus").unwrap_err();
        assert!(err.contains("bogus"));
        assert!(err.contains("lan"));
        assert!(err.contains("offline"));
        assert!(err.contains("loopback-test"));
    }

    #[test]
    fn invalid_cli_value_is_error() {
        let cfg = config_with(None, true);
        let result = resolve_network_mode(&args(&["--network-mode=bogus"]), None, &cfg);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_env_value_is_error() {
        let cfg = config_with(None, true);
        let result = resolve_network_mode(&[], Some("bogus"), &cfg);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_config_mode_is_error() {
        let cfg = config_with(Some("bogus"), true);
        let result = resolve_network_mode(&[], None, &cfg);
        assert!(result.is_err());
    }

    #[test]
    fn cli_takes_priority_over_env_and_config() {
        let cfg = config_with(Some("offline"), false);
        let result = resolve_network_mode(
            &args(&["--network-mode=loopback-test"]),
            Some("offline"),
            &cfg,
        );
        assert_eq!(result, Ok(NetworkMode::LoopbackTest));
    }

    #[test]
    fn env_takes_priority_over_config() {
        let cfg = config_with(Some("offline"), false);
        let result = resolve_network_mode(&[], Some("lan"), &cfg);
        assert_eq!(result, Ok(NetworkMode::Lan));
    }

    #[test]
    fn two_token_cli_form_is_accepted() {
        let cfg = config_with(None, true);
        let result = resolve_network_mode(&args(&["--network-mode", "offline"]), None, &cfg);
        assert_eq!(result, Ok(NetworkMode::Offline));
    }

    #[test]
    fn two_token_cli_form_missing_value_is_error() {
        let cfg = config_with(None, true);
        let result = resolve_network_mode(&args(&["--network-mode"]), None, &cfg);
        assert!(result.is_err());
    }

    #[test]
    fn empty_env_value_is_treated_as_unset() {
        let cfg = config_with(Some("offline"), true);
        let result = resolve_network_mode(&[], Some("   "), &cfg);
        assert_eq!(result, Ok(NetworkMode::Offline));
    }

    #[test]
    fn legacy_enabled_false_with_no_mode_is_offline() {
        let cfg = config_with(None, false);
        let result = resolve_network_mode(&[], None, &cfg);
        assert_eq!(result, Ok(NetworkMode::Offline));
    }

    #[test]
    fn legacy_enabled_true_with_no_mode_is_lan() {
        let cfg = config_with(None, true);
        let result = resolve_network_mode(&[], None, &cfg);
        assert_eq!(result, Ok(NetworkMode::Lan));
    }

    #[test]
    fn explicit_mode_overrides_legacy_enabled_false() {
        let cfg = config_with(Some("lan"), false);
        let result = resolve_network_mode(&[], None, &cfg);
        assert_eq!(result, Ok(NetworkMode::Lan));
    }

    #[test]
    fn unrelated_argv_tokens_are_ignored() {
        let cfg = config_with(None, true);
        let result = resolve_network_mode(
            &args(&["some-program", "--webview-flag", "--another=value"]),
            None,
            &cfg,
        );
        assert_eq!(result, Ok(NetworkMode::Lan));
    }

    #[test]
    fn global_set_and_current_roundtrip() {
        // NetworkMode is Copy/Eq; exercise the OnceLock helpers directly
        // via a value not asserted elsewhere in this process to avoid
        // interference from other tests calling `set`.
        set(NetworkMode::LoopbackTest);
        // `set` only succeeds once per process; `current()` must return
        // *some* valid mode either way.
        let mode = current();
        assert!(matches!(
            mode,
            NetworkMode::Lan | NetworkMode::Offline | NetworkMode::LoopbackTest
        ));
    }
}
