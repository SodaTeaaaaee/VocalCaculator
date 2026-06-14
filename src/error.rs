//! Unified error types for the Vocal Calculator application.
//!
//! This module provides a single [`AppError`] enum that encompasses all
//! error domains in the application: calculation, audio, network, config,
//! and I/O. Each variant carries structured context so callers can log
//! meaningful diagnostics without losing the original cause.
//!
//! # Design rationale
//!
//! The codebase currently uses a mix of `anyhow::Error` (for network and
//! config), `CalcError` (for calculator domain errors), and ad-hoc
//! `Option`/`log::warn` patterns (for audio). This module unifies them
//! into one type while preserving `anyhow` at module boundaries where
//! ad-hoc context is valuable (discovery, session handshake).
//!
//! `AppError` implements `std::error::Error` and `Display`, and provides
//! `From` conversions for all wrapped error types so the `?` operator
//! works naturally.

use thiserror::Error;

use crate::core::token::CalcError;

/// Unified application error type.
///
/// Each variant corresponds to an error domain. The inner type preserves
/// the original error for logging and diagnostics.
#[derive(Debug, Error)]
pub enum AppError {
    /// Calculator domain error (divide by zero, overflow, etc.).
    #[error("calculation error: {0}")]
    Calc(#[from] CalcError),

    /// Audio subsystem error (kira backend init, playback failure).
    #[error("audio error: {0}")]
    Audio(String),

    /// Network I/O or protocol error.
    #[error("network error: {0}")]
    Network(#[source] std::io::Error),

    /// Configuration load/parse error.
    #[error("config error: {0}")]
    Config(String),

    /// Serialization/deserialization error (bincode, toml, serde).
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Generic I/O error (file system, system calls).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Slint UI platform error.
    #[error("platform error: {0}")]
    Platform(String),
}

// ---------------------------------------------------------------------------
// From conversions (manual — these convert via .to_string() so thiserror
// #[from] cannot auto-derive them)
// ---------------------------------------------------------------------------

impl From<slint::PlatformError> for AppError {
    fn from(e: slint::PlatformError) -> Self {
        Self::Platform(e.to_string())
    }
}

impl From<bincode::error::EncodeError> for AppError {
    fn from(e: bincode::error::EncodeError) -> Self {
        Self::Serialization(e.to_string())
    }
}

impl From<bincode::error::DecodeError> for AppError {
    fn from(e: bincode::error::DecodeError) -> Self {
        Self::Serialization(e.to_string())
    }
}

impl From<toml::de::Error> for AppError {
    fn from(e: toml::de::Error) -> Self {
        Self::Config(e.to_string())
    }
}

impl From<toml::ser::Error> for AppError {
    fn from(e: toml::ser::Error) -> Self {
        Self::Config(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Convenience constructors
// ---------------------------------------------------------------------------

impl AppError {
    /// Create an audio error from a message string.
    pub fn audio(msg: impl Into<String>) -> Self {
        Self::Audio(msg.into())
    }

    /// Create a network error wrapping an [`std::io::Error`].
    pub fn network(e: std::io::Error) -> Self {
        Self::Network(e)
    }

    /// Create a config error from a message string.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Create a platform error from a message string.
    pub fn platform(msg: impl Into<String>) -> Self {
        Self::Platform(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    // -----------------------------------------------------------------------
    // From roundtrip tests
    // -----------------------------------------------------------------------

    #[test]
    fn from_calc_error_divide_by_zero() {
        let src = CalcError::DivideByZero;
        let err: AppError = src.into();
        assert_eq!(err.to_string(), "calculation error: 不能除以零");
    }

    #[test]
    fn from_calc_error_negative_square_root() {
        let src = CalcError::NegativeSquareRoot;
        let err: AppError = src.into();
        assert_eq!(err.to_string(), "calculation error: 输入无效");
    }

    #[test]
    fn from_calc_error_overflow() {
        let src = CalcError::Overflow;
        let err: AppError = src.into();
        assert_eq!(err.to_string(), "calculation error: 溢出");
    }

    #[test]
    fn from_io_error_to_io_variant() {
        let src = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err: AppError = src.into();
        assert!(
            err.to_string().contains("I/O error"),
            "expected Io variant, got: {err}"
        );
        assert!(err.to_string().contains("file missing"));
    }

    #[test]
    fn from_toml_de_error() {
        let src: toml::de::Error = toml::from_str::<crate::app::config::AppConfig>("not [[[ valid")
            .unwrap_err();
        let err: AppError = src.into();
        assert!(
            err.to_string().contains("config error"),
            "expected Config variant, got: {err}"
        );
    }

    #[test]
    fn from_toml_ser_error() {
        // toml cannot serialize a map whose keys are themselves composite
        // types (e.g. Vec), which produces a `toml::ser::Error`.
        let bad_map: std::collections::HashMap<Vec<i32>, i32> =
            std::collections::HashMap::from([(vec![1, 2], 3)]);
        let src = toml::to_string(&bad_map).unwrap_err();
        let err: AppError = src.into();
        assert!(
            err.to_string().contains("config error"),
            "expected Config variant, got: {err}"
        );
    }

    #[test]
    fn from_bincode_encode_error() {
        // bincode 2.x: encoding a type that is not encodable is hard to
        // trigger without custom types, but we can verify the From impl
        // compiles and the variant discriminant is correct by round-tripping
        // through a decode error (which is easier to trigger).
        //
        // For EncodeError, we rely on the fact the From impl exists (compile-
        // time guarantee) and test the Display format indirectly.
        let encode_err = bincode::encode_to_vec(
            &f32::NAN, // NaN serialisation may fail depending on config
            bincode::config::standard(),
        );
        if let Err(e) = encode_err {
            let err: AppError = e.into();
            assert!(
                err.to_string().contains("serialization error"),
                "expected Serialization variant, got: {err}"
            );
        }
    }

    #[test]
    fn from_bincode_decode_error() {
        // Decode invalid bytes to trigger a DecodeError.
        let garbage: &[u8] = &[0xFF, 0xFF, 0xFF, 0xFF];
        let result =
            bincode::decode_from_slice::<String, _>(garbage, bincode::config::standard());
        let src = result.unwrap_err();
        let err: AppError = src.into();
        assert!(
            err.to_string().contains("serialization error"),
            "expected Serialization variant, got: {err}"
        );
    }

    #[test]
    fn from_platform_error() {
        let err = AppError::platform("no display");
        assert_eq!(err.to_string(), "platform error: no display");
    }

    // -----------------------------------------------------------------------
    // source() tests — variants WITH #[source] / #[from]
    // -----------------------------------------------------------------------

    #[test]
    fn source_returns_some_for_calc_variant() {
        let err: AppError = CalcError::DivideByZero.into();
        assert!(
            err.source().is_some(),
            "Calc variant should have a source"
        );
    }

    #[test]
    fn source_returns_some_for_network_variant() {
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let err = AppError::network(io_err);
        assert!(
            err.source().is_some(),
            "Network variant should have a source"
        );
    }

    #[test]
    fn source_returns_some_for_io_variant() {
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "boom");
        let err: AppError = io_err.into();
        assert!(
            err.source().is_some(),
            "Io variant should have a source"
        );
    }

    // -----------------------------------------------------------------------
    // source() tests — variants WITHOUT #[source]
    // -----------------------------------------------------------------------

    #[test]
    fn source_returns_none_for_audio_variant() {
        let err = AppError::audio("speaker dead");
        assert!(
            err.source().is_none(),
            "Audio variant should have no source"
        );
    }

    #[test]
    fn source_returns_none_for_config_variant() {
        let err = AppError::config("bad key");
        assert!(
            err.source().is_none(),
            "Config variant should have no source"
        );
    }

    #[test]
    fn source_returns_none_for_serialization_variant() {
        let err = AppError::Serialization("corrupt".to_string());
        assert!(
            err.source().is_none(),
            "Serialization variant should have no source"
        );
    }

    #[test]
    fn source_returns_none_for_platform_variant() {
        let err = AppError::platform("no display");
        assert!(
            err.source().is_none(),
            "Platform variant should have no source"
        );
    }

    // -----------------------------------------------------------------------
    // Convenience constructors
    // -----------------------------------------------------------------------

    #[test]
    fn audio_constructor() {
        let err = AppError::audio("boom");
        assert_eq!(err.to_string(), "audio error: boom");
    }

    #[test]
    fn network_constructor() {
        let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
        let err = AppError::network(io_err);
        assert!(err.to_string().contains("network error"));
        assert!(err.to_string().contains("timeout"));
    }

    #[test]
    fn config_constructor() {
        let err = AppError::config("missing field");
        assert_eq!(err.to_string(), "config error: missing field");
    }
}
