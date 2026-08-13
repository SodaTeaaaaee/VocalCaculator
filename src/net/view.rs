//! UI-facing network view types.
//!
//! These are plain, `PartialEq` snapshots. They must not contain `Instant`
//! so a latency tick cannot rebuild the calculator body.

use std::net::SocketAddr;

use crate::app::network_mode::NetworkMode;
use crate::net::protocol::NodeId;

/// Discovery / session liveness of one peer. Role is separate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerPresence {
    Nearby,
    Connecting,
    Connected,
    Unreachable,
    Stale,
    FingerprintMismatch,
}

/// Product role of one peer relative to this calculator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRole {
    Idle,
    SelectedExecutor,
    ControllingUs,
}

/// The only peer type that crosses the runtime → UI boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerViewModel {
    pub node_id: NodeId,
    pub display_name: String,
    pub endpoint: Option<SocketAddr>,
    pub fingerprint: Option<String>,
    pub presence: PeerPresence,
    pub role: PeerRole,
    pub latency_ms: Option<u32>,
    pub session_id: Option<NodeId>,
}

impl PeerViewModel {
    pub fn address_label(&self) -> String {
        self.endpoint
            .map(|addr| addr.to_string())
            .unwrap_or_default()
    }

    pub fn is_connected(&self) -> bool {
        matches!(
            self.presence,
            PeerPresence::Connected | PeerPresence::Connecting
        ) || self.role == PeerRole::SelectedExecutor
            || self.session_id.is_some()
    }

    pub fn is_selected_executor(&self) -> bool {
        self.role == PeerRole::SelectedExecutor
    }
}

/// TCP listener bind projection for This Device / the presence banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindStatus {
    Offline,
    Bound { addr: SocketAddr },
    BindFailed { port: u16 },
    Unavailable,
}

/// Runtime-owned scan progress. The UI must not fake this with a sleep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Idle,
    InFlight,
}

/// Coarse status kind used with a Chinese status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkStatusKind {
    Offline,
    Enabled,
    LoopbackTest,
    Connecting,
    ExecutingOn,
    BeingControlled,
    ListenerUnavailable,
    StorageUnavailable,
    Error,
}

/// Local identity + receive-side controls shown on the 本机 tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDeviceView {
    pub node_id: NodeId,
    pub display_name: String,
    pub fingerprint: String,
    pub network_mode: NetworkMode,
    pub bind: BindStatus,
    pub allow_remote_control: bool,
    pub controllers: Vec<NodeId>,
    pub selected_executor: Option<NodeId>,
}

/// Typed connect / bind failure. Keep `as_reason_code` stable for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectErrorKind {
    Timeout,
    HandshakeTimeout,
    BindFailed,
    ConnectionRefused,
    ConnectionReset,
    HostUnreachable,
    NetworkUnreachable,
    PermissionDenied,
    RemoteControlDisabled,
    FingerprintMismatch,
    LoopbackAddressRequired,
    ConnectOverloaded,
    CommandOverloaded,
    Other,
}

impl ConnectErrorKind {
    pub fn as_reason_code(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::HandshakeTimeout => "handshake_timeout",
            Self::BindFailed => "bind_failed",
            Self::ConnectionRefused => "connection_refused",
            Self::ConnectionReset => "connection_reset",
            Self::HostUnreachable => "host_unreachable",
            Self::NetworkUnreachable => "network_unreachable",
            Self::PermissionDenied => "permission_denied",
            Self::RemoteControlDisabled => "remote_control_disabled",
            Self::FingerprintMismatch => "fingerprint_mismatch",
            Self::LoopbackAddressRequired => "loopback_required",
            Self::ConnectOverloaded => "connect_overloaded",
            Self::CommandOverloaded => "command_overloaded",
            Self::Other => "other",
        }
    }

    pub fn from_reason_code(code: &str) -> Self {
        match code {
            "timeout" => Self::Timeout,
            "handshake_timeout" => Self::HandshakeTimeout,
            "bind_failed" => Self::BindFailed,
            "connection_refused" => Self::ConnectionRefused,
            "connection_reset" => Self::ConnectionReset,
            "host_unreachable" => Self::HostUnreachable,
            "network_unreachable" => Self::NetworkUnreachable,
            "permission_denied" => Self::PermissionDenied,
            "remote_control_disabled" => Self::RemoteControlDisabled,
            "fingerprint_mismatch" => Self::FingerprintMismatch,
            "loopback_required" => Self::LoopbackAddressRequired,
            "connect_overloaded" => Self::ConnectOverloaded,
            "command_overloaded" => Self::CommandOverloaded,
            _ => Self::Other,
        }
    }

    pub fn to_zh(self) -> &'static str {
        match self {
            Self::Timeout | Self::HandshakeTimeout => "连接超时",
            Self::BindFailed => "网络端口无法监听",
            Self::ConnectionRefused => "连接被拒绝",
            Self::ConnectionReset => "连接中断",
            Self::HostUnreachable => "设备不可达",
            Self::NetworkUnreachable => "网络不可达",
            Self::PermissionDenied => "访问被拒绝",
            Self::RemoteControlDisabled => "对方未开启远程控制",
            Self::FingerprintMismatch => "发现信息与连接密钥不一致",
            Self::LoopbackAddressRequired => "回环模式只能连接本机地址",
            Self::ConnectOverloaded => "连接队列已满",
            Self::CommandOverloaded => "网络命令队列已满",
            Self::Other => "连接失败",
        }
    }
}

/// Router projection consumed by the UI event loop.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutingView {
    pub active_executor: Option<NodeId>,
    pub active_controllers: Vec<NodeId>,
    pub allow_remote_control: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_round_trip() {
        for kind in [
            ConnectErrorKind::Timeout,
            ConnectErrorKind::BindFailed,
            ConnectErrorKind::FingerprintMismatch,
            ConnectErrorKind::CommandOverloaded,
        ] {
            assert_eq!(
                ConnectErrorKind::from_reason_code(kind.as_reason_code()),
                kind
            );
        }
    }
}
