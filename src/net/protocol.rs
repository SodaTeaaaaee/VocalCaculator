use hmac::Hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::core::action::CalcAction;

/// HMAC-SHA256 type alias for handshake authentication.
pub type HmacSha256 = Hmac<Sha256>;

/// Unique identifier for a network node (UUID v4).
pub type NodeId = Uuid;

/// Human-readable peer names are shared by configuration, handshake,
/// discovery, and steady-state name updates. Keep one byte-oriented schema at
/// every boundary so a peer cannot bypass UI validation through another
/// transport.
pub const MAX_DISPLAY_NAME_BYTES: usize = 64;

pub fn valid_display_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= MAX_DISPLAY_NAME_BYTES
        && !name.chars().any(char::is_control)
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
        /// Ed25519 public key (32 bytes). Present when protocol_version >= 3.
        public_key: [u8; 32],
    },
    HelloAck {
        node_id: NodeId,
        display_name: String,
        protocol_version: u16,
        app_id: String,
        /// Ed25519 public key (32 bytes). Present when protocol_version >= 3.
        public_key: [u8; 32],
    },
    // Subscription
    Subscribe,
    Unsubscribe,
    // Steady state
    Action(ActionEnvelope),
    StateUpdate(StateSnapshot),
    // Legacy v5 routing/pairing variants below retain their wire positions.
    // The calculator-first Router ignores them and never emits them.
    /// Legacy v5 incremental routing delta.
    RoutingDelta {
        owner: NodeId,
        version: u64,
        cells: Vec<(NodeId, NodeId, bool)>,
    },
    /// Legacy v5 full routing snapshot.
    RoutingSync {
        entries: Vec<(NodeId, NodeId, bool, u64)>,
    },
    /// Legacy v5 request for an owner-signed routing row.
    RoutingRowRequest {
        owner: NodeId,
    },
    /// Legacy v5 owner-signed routing row.
    RoutingRowAnnounce {
        owner: NodeId,
        version: u64,
        cells: Vec<(NodeId, NodeId, bool)>,
        owner_public_key: [u8; 32],
        signature: Vec<u8>,
    },
    /// Legacy v5 route revocation.
    RouteRevoke {
        from: NodeId,
        to: NodeId,
        version: u64,
    },
    /// Legacy v5 per-request permission request.
    RouteRequest {
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
    },
    /// Legacy v5 per-request grant.
    RouteGrant {
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
    },
    /// Legacy v5 per-request denial.
    RouteDenied {
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
        reason: String,
    },
    /// Legacy v5 route release.
    RouteRelease {
        controller: NodeId,
        executor: NodeId,
    },
    /// Challenge used by protocol v5 peers to prove key ownership.
    AuthChallenge {
        nonce: [u8; 32],
    },
    /// Proof-of-possession signature over an AuthChallenge nonce.
    AuthProof {
        signature: Vec<u8>,
    },
    // Keepalive
    Ping,
    Pong,
    // Name update
    /// A node has updated its display name.
    PeerNameUpdate {
        display_name: String,
    },
    // Legacy v5 application-level pairing. Session identity is still proven
    // with the authenticated Ed25519 handshake.
    /// Legacy v5 pairing request.
    PairingRequest {
        public_key: [u8; 32],
        pairing_code_hash: [u8; 32],
    },
    /// Legacy v5 pairing confirmation.
    PairingConfirm {
        signature: Vec<u8>,
    },
    /// Legacy v5 pairing rejection.
    PairingReject,
    // Connection failure notification (local-only, not sent over the wire).
    /// A TCP connection attempt failed. Used to propagate errors from the
    /// connect task back to the main thread for UI feedback.
    ConnectionFailed {
        addr: std::net::SocketAddr,
        reason: String,
        /// The peer we were trying to connect to (if known).
        target_node_id: Option<NodeId>,
    },
}

impl NetworkMessage {
    /// Messages retained in the v5 enum for local compatibility but forbidden
    /// on an authenticated peer session.
    pub fn is_local_only(&self) -> bool {
        matches!(self, Self::ConnectionFailed { .. })
    }
}

/// Hint to peers indicating which transport mechanism the sender used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportHint {
    /// Message was sent via IP multicast.
    Multicast,
    /// Message was sent via UDP broadcast.
    Broadcast,
    /// Message was sent via mDNS.
    Mdns,
}

/// Message exchanged over UDP multicast for peer discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiscoveryMessage {
    /// Legacy announce (protocol v1).
    Announce {
        node_id: NodeId,
        display_name: String,
        tcp_port: u16,
        capabilities: Capabilities,
    },
    /// Discovery request from a peer.
    Discover,
    /// Extended announce (protocol v2) with transport metadata and hostname.
    AnnounceV2 {
        node_id: NodeId,
        display_name: String,
        tcp_port: u16,
        capabilities: Capabilities,
        transport_hint: TransportHint,
        hostname: String,
        session_port: u16,
    },
}

/// Current protocol version for handshake negotiation.
///
/// Version 5 freezes the wire layout after adding owner-signed routing-row
/// messages and the mandatory Ed25519 challenge/proof exchange.  Those
/// additions changed bincode enum discriminants and are therefore not wire
/// compatible with protocol v4.
pub const PROTOCOL_VERSION: u16 = 5;
/// Minimum Hello protocol version that carries Ed25519 public keys.
pub const HELLO_VERSION_WITH_KEYS: u16 = 3;
/// IPv4 multicast address used for LAN peer discovery.
pub const DISCOVERY_MULTICAST_ADDR: &str = "224.0.0.167";
/// The single fixed LAN port used by this application.
///
/// This is the one authoritative `42420` literal in `src/`. UDP multicast
/// discovery and the TCP session listener both bind this port in
/// [`crate::app::network_mode::NetworkMode::Lan`] -- there is no longer a
/// separate ephemeral session port, so peers can be reached without
/// resolving a port out-of-band.
pub const LAN_FIXED_PORT: u16 = 42420;
/// UDP port for multicast discovery messages. Alias of [`LAN_FIXED_PORT`].
pub const DISCOVERY_PORT: u16 = LAN_FIXED_PORT;
/// TCP port for session connections in `Lan` mode. Alias of [`LAN_FIXED_PORT`].
pub const SESSION_TCP_PORT: u16 = LAN_FIXED_PORT;
/// mDNS service type for LAN discovery.
pub const MDNS_SERVICE_TYPE: &str = "_vocalcalc._tcp.local.";
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
/// Retained as a fallback for protocol version 2 compatibility.
pub const APP_KEY: &[u8] = b"vocal_calculator_hmac_key_v1";

// ---------------------------------------------------------------------------
// Commands from session tasks -> command processor
// ---------------------------------------------------------------------------

use super::session::SessionSender;
use super::state::PeerInfo;

/// Direction of a TCP connection (for dedup tie-breaking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionDirection {
    /// We accepted an inbound TCP connection.
    Inbound,
    /// We initiated an outbound TCP connection.
    Outbound,
}

/// Unique generation identifier for one concrete TCP session.
///
/// A node may have overlapping old/new session tasks during reconnect and
/// deterministic deduplication.  Commands that mutate the active-session map
/// carry this ID so a late teardown from an old task cannot remove its
/// replacement.
pub(crate) type SessionId = Uuid;

/// Identity asserted by discovery or by a user-selected peer entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedPeerIdentity {
    pub node_id: NodeId,
    pub public_key_fingerprint: Option<String>,
}

/// One outbound TCP connection request after policy resolution.
#[derive(Debug, Clone)]
pub(crate) struct OutboundConnectRequest {
    pub addr: std::net::SocketAddr,
    pub expected_peer: Option<ExpectedPeerIdentity>,
    /// Discovery retries are intentionally quiet; direct user actions report
    /// failures through the UI event channel.
    pub report_errors: bool,
}

/// A session registration request (sent by a session task after handshake).
pub(crate) struct SessionRegister {
    pub session_id: SessionId,
    pub node_id: NodeId,
    pub sender: SessionSender,
    pub info: PeerInfo,
    /// Whether this connection was inbound or outbound (for dedup).
    pub direction: ConnectionDirection,
    /// Cancellation signal retained by the active-session registry. Replacing
    /// a generation sets it to `true`, which stops the old relay promptly.
    pub cancel_tx: tokio::sync::watch::Sender<bool>,
    /// The runtime must explicitly accept or reject the registration before
    /// the session enters its steady-state relay loop.
    pub decision_tx: tokio::sync::oneshot::Sender<bool>,
}

/// Commands from the tokio tasks back to the NetworkManager runtime.
pub(crate) enum NetworkCommand {
    /// A new session completed handshake and wants to register.
    RegisterSession(SessionRegister),
    /// A session has closed; remove from the active set.
    UnregisterSession {
        node_id: NodeId,
        session_id: SessionId,
    },
    /// An inbound message that should be forwarded to the Router.
    /// The `NodeId` is the sender of the message.
    IncomingMessage(NodeId, NetworkMessage),
    /// Initiate an outbound TCP connection to a peer.
    ConnectToPeer(OutboundConnectRequest),
    /// Update the measured round-trip latency for one authenticated peer.
    UpdateLatency { node_id: NodeId, ms: u32 },
    /// Trigger a LAN peer discovery scan.
    Scan,
    /// The in-flight scan burst finished.
    ScanFinished,
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    fn encoded(message: &NetworkMessage) -> Vec<u8> {
        bincode::serde::encode_to_vec(message, bincode::config::standard()).unwrap()
    }

    #[test]
    fn protocol_v5_number_is_stable() {
        assert_eq!(PROTOCOL_VERSION, 5);
    }

    #[test]
    fn deprecated_v5_variant_discriminants_remain_stable() {
        let id = NodeId::nil();
        assert_eq!(
            encoded(&NetworkMessage::RouteRequest {
                request_id: 0,
                controller: id,
                executor: id,
            })[0],
            11
        );
        assert_eq!(encoded(&NetworkMessage::Ping), vec![17]);
        assert_eq!(encoded(&NetworkMessage::PairingReject), vec![22]);
    }
}
