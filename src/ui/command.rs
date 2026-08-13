//! User-intent commands for chrome, settings, and LAN remote.
//!
//! Calculator buttons and the keyboard still produce [`CalcAction`] 1:1.
//! [`AppCommand::Calc`] is the only wrapper; do not invent stringly
//! command IDs for digits or operators.

use crate::core::action::CalcAction;
use crate::net::protocol::NodeId;

/// Wide-desktop workbench tabs. Mobile uses overlays instead of tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkbenchTab {
    ThisDevice,
    Nearby,
    Settings,
}

impl WorkbenchTab {
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisDevice => "本机",
            Self::Nearby => "附近",
            Self::Settings => "设置",
        }
    }
}

/// Chrome / network / audio intent. Calc actions stay [`CalcAction`].
#[derive(Debug, Clone, PartialEq)]
pub enum AppCommand {
    Calc(CalcAction),
    CycleAudioMode,
    ToggleMute,
    SetVolume(f64),
    ToggleTheme,
    SetWorkbenchTab(WorkbenchTab),
    ShowAbout,
    CloseOverlays,
    ShowSettingsOverlay,
    ShowNearbyOverlay,
    SetDisplayName(String),
    SetAllowRemoteControl(bool),
    ScanNearby,
    UseAsExecutor(NodeId),
    StopRemoteExecution,
}
