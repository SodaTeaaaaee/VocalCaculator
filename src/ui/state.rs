//! Dioxus signal-based UI state structs.
//!
//! Each struct groups related reactive state so that components can
//! subscribe to only the slice they care about.

use dioxus::prelude::Signal;
use dioxus::prelude::WritableExt;

use crate::audio::AudioMode;
use crate::net::protocol::NodeId;
use crate::net::view::{BindStatus, PeerViewModel};
use crate::traits::DisplayUpdater;
use crate::ui::command::WorkbenchTab;

// ---------------------------------------------------------------------------
// CalcDisplay
// ---------------------------------------------------------------------------

/// Calculator display state: main text, history line, memory indicator, error flag.
#[derive(Clone)]
pub struct CalcDisplay {
    pub text: Signal<String>,
    pub history: Signal<String>,
    pub memory_indicator: Signal<String>,
    pub is_error: Signal<bool>,
}

// ---------------------------------------------------------------------------
// AudioUiState
// ---------------------------------------------------------------------------

/// Audio-related UI state shared across the app.
#[derive(Clone)]
pub struct AudioUiState {
    pub mode_indicator: Signal<String>,
    pub mode: Signal<AudioMode>,
    pub volume: Signal<f64>,
    pub muted: Signal<bool>,
    pub dark_mode: Signal<bool>,
    pub about_visible: Signal<bool>,
    pub audio_status: Signal<String>,
}

// ---------------------------------------------------------------------------
// NetUiState
// ---------------------------------------------------------------------------

/// Network panel UI state.
#[derive(Clone)]
pub struct NetUiState {
    pub panel_visible: Signal<bool>,
    pub scanning: Signal<bool>,
    pub status: Signal<String>,
    pub peers: Signal<Vec<PeerViewModel>>,
    pub remote_controlled: Signal<bool>,
    pub executing_remotely: Signal<bool>,
    pub allow_remote_control: Signal<bool>,
    pub bind: Signal<BindStatus>,
    pub local_node_id: Signal<Option<NodeId>>,
    pub local_fingerprint: Signal<String>,
    pub controllers: Signal<Vec<NodeId>>,
    pub selected_executor: Signal<Option<NodeId>>,
    pub workbench_tab: Signal<WorkbenchTab>,
}

// ---------------------------------------------------------------------------
// SettingsState
// ---------------------------------------------------------------------------

/// Settings panel UI state.
#[derive(Clone)]
pub struct SettingsState {
    pub panel_visible: Signal<bool>,
    pub display_name: Signal<String>,
    pub save_status: Signal<String>,
}

// ---------------------------------------------------------------------------
// CalcContext
// ---------------------------------------------------------------------------

/// Top-level context that owns every reactive slice.
///
/// Components access this via `use_context::<CalcContext>()` and then
/// drill into whichever sub-struct they need.
#[derive(Clone)]
pub struct CalcContext {
    pub display: CalcDisplay,
    pub audio: AudioUiState,
    pub net: NetUiState,
    pub settings: SettingsState,
    pub app_version: Signal<String>,
    pub keyboard_pressed: Signal<bool>,
    pub last_keyboard_action: Signal<String>,
}

// ---------------------------------------------------------------------------
// DisplayUpdater implementation
// ---------------------------------------------------------------------------

impl DisplayUpdater for CalcContext {
    fn update_display(&self, text: &str) {
        let mut sig = self.display.text;
        *sig.write() = text.to_string();
    }

    fn update_history(&self, text: &str) {
        let mut sig = self.display.history;
        *sig.write() = text.to_string();
    }

    fn update_memory_indicator(&self, indicator: &str) {
        let mut sig = self.display.memory_indicator;
        *sig.write() = indicator.to_string();
    }

    fn set_error_state(&self, is_error: bool) {
        let mut sig = self.display.is_error;
        *sig.write() = is_error;
    }
}
