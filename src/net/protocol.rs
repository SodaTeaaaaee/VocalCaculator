use hmac::Hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::core::action::CalcAction;

/// HMAC-SHA256 type alias for handshake authentication.
pub type HmacSha256 = Hmac<Sha256>;

/// Unique identifier for a network node (UUID v4).
pub type NodeId = Uuid;

/// Policy for resolving concurrent action conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictPolicy {
    /// All actions are interleaved in arrival order.
    Interleaved,
    /// Only the granted controller may send actions.
    Exclusive,
}

/// Advertised capabilities of a network node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub can_execute: bool,
    pub can_control: bool,
    pub protocol_version: u16,
}

/// A sequenced calculator action wrapped for network transmission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEnvelope {
    pub seq: u64,
    pub source_id: NodeId,
    pub timestamp_ms: u64,
    pub action: CalcAction,
}

/// A point-in-time snapshot of calculator display state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub display: String,
    pub history: String,
    pub memory_indicator: String,
    pub is_error: bool,
    pub last_seq_applied: u64,
}

/// Top-level message enum for the calculator network protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkMessage {
    // Handshake
    Hello {
        node_id: NodeId,
        display_name: String,
        protocol_version: u16,
        app_id: String,
    },
    HelloAck {
        node_id: NodeId,
        display_name: String,
        protocol_version: u16,
        app_id: String,
    },
    // Subscription
    Subscribe,
    Unsubscribe,
    // Steady state
    Action(ActionEnvelope),
    StateUpdate(StateSnapshot),
    // Control arbitration
    ControlRequest,
    ControlGrant(bool),
    ControlRelease,
    // Keepalive
    Ping,
    Pong,
}

/// Message exchanged over UDP multicast for peer discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiscoveryMessage {
    Announce {
        node_id: NodeId,
        display_name: String,
        tcp_port: u16,
        capabilities: Capabilities,
    },
    Discover,
}

/// Current protocol version for handshake negotiation.
pub const PROTOCOL_VERSION: u16 = 1;
/// IPv4 multicast address used for LAN peer discovery.
pub const DISCOVERY_MULTICAST_ADDR: &str = "239.255.42.99";
/// UDP port for multicast discovery messages.
pub const DISCOVERY_PORT: u16 = 4242;
/// Interval between heartbeat pings in seconds.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 5;
/// Silence threshold before a peer is considered disconnected.
pub const HEARTBEAT_TIMEOUT_SECS: u64 = 15;

/// Magic bytes prepended to every protocol frame for LAN isolation.
/// Format: `VOCALC` + version byte `\x01` + reserved `\x00`.
pub const PROTOCOL_MAGIC: [u8; 8] = *b"VOCALC\x01\x00";
/// Application identifier sent in handshake messages.
pub const APP_ID: &str = "vocal_calculator";
/// Shared HMAC key for handshake authentication (HMAC-SHA256).
pub const APP_KEY: &[u8] = b"vocal_calculator_hmac_key_v1";
