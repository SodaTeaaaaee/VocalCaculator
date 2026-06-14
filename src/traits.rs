//! Trait-based dependency injection interfaces for testability.
//!
//! These traits abstract the side-effectful subsystems (audio, network, display)
//! so that the core routing logic in [`Router`](crate::net::Router) can be tested
//! with mock implementations.

use crate::audio::AudioMode;
use crate::core::token::VocalEvent;

// ---------------------------------------------------------------------------
// AudioPlayer
// ---------------------------------------------------------------------------

/// Abstraction over audio playback.
///
/// The production implementation is [`VocalAudio`](crate::audio::VocalAudio).
pub trait AudioPlayer {
    /// Play sounds corresponding to the given vocal events.
    fn play_events(&mut self, events: &[VocalEvent]);

    /// Set the audio mode (normal, broken, music, silent).
    fn set_mode(&mut self, mode: AudioMode);

    /// Set the volume from a slider value (0.0..1.0).
    fn set_volume(&mut self, slider: f64);

    /// Return the current audio mode.
    fn mode(&self) -> AudioMode;
}

// ---------------------------------------------------------------------------
// DisplayUpdater
// ---------------------------------------------------------------------------

/// Abstraction over UI display updates.
///
/// The production implementation delegates to the Slint-generated
/// `CalculatorWindow` property setters.
pub trait DisplayUpdater {
    /// Update the main display text.
    fn update_display(&self, text: &str);

    /// Update the history line.
    fn update_history(&self, text: &str);

    /// Update the memory indicator (e.g. "M" or "").
    fn update_memory_indicator(&self, indicator: &str);

    /// Set the error visual state.
    fn set_error_state(&self, is_error: bool);
}
