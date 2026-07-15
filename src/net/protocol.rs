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
    // Routing matrix
    /// Incremental routing delta from an owner node.
    RoutingDelta {
        owner: NodeId,
        version: u64,
        cells: Vec<(NodeId, NodeId, bool)>,
    },
    /// Full routing matrix snapshot.
    RoutingSync {
        entries: Vec<(NodeId, NodeId, bool, u64)>,
    },
    /// Request an owner-signed routing row. The receiver may answer from its
    /// own row or from a cached owner-signed row.
    RoutingRowRequest {
        owner: NodeId,
    },
    /// Owner-signed routing row that can be relayed by intermediate peers.
    RoutingRowAnnounce {
        owner: NodeId,
        version: u64,
        cells: Vec<(NodeId, NodeId, bool)>,
        owner_public_key: [u8; 32],
        signature: Vec<u8>,
    },
    /// Revoke a specific route.
    RouteRevoke {
        from: NodeId,
        to: NodeId,
        version: u64,
    },
    /// Ask an executor to allow this controller to route actions to it.
    RouteRequest {
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
    },
    /// Authorize a pending route request.
    RouteGrant {
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
    },
    /// Deny a pending route request.
    RouteDenied {
        request_id: u64,
        controller: NodeId,
        executor: NodeId,
        reason: String,
    },
    /// Release an authorized or pending route.
    RouteRelease {
        controller: NodeId,
        executor: NodeId,
    },
    /// Challenge used by protocol v4 peers to prove key ownership.
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
    // Pairing (ed25519)
    /// Request to pair with a remote node, carrying sender's public key and a
    /// SHA-256 hash of the pairing code for out-of-band verification.
    PairingRequest {
        public_key: [u8; 32],
        pairing_code_hash: [u8; 32],
    },
    /// Confirmation that the remote node accepted the pairing, signed with the
    /// sender's Ed25519 private key over the concatenation of both public keys.
    PairingConfirm {
        signature: Vec<u8>,
    },
    /// The remote node rejected the pairing request.
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

/// Current protocol version for handshake negotiation (v4 = explicit route auth messages).
pub const PROTOCOL_VERSION: u16 = 4;
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

/// A session registration request (sent by a session task after handshake).
pub(crate) struct SessionRegister {
    pub node_id: NodeId,
    pub sender: SessionSender,
    pub info: PeerInfo,
    /// Whether this connection was inbound or outbound (for dedup).
    pub direction: ConnectionDirection,
}

/// Commands from the tokio tasks back to the NetworkManager runtime.
pub(crate) enum NetworkCommand {
    /// A new session completed handshake and wants to register.
    RegisterSession(SessionRegister),
    /// A session has closed; remove from the active set.
    UnregisterSession(NodeId),
    /// An inbound message that should be forwarded to the Router.
    /// The `NodeId` is the sender of the message.
    IncomingMessage(NodeId, NetworkMessage),
    /// Initiate an outbound TCP connection to a peer.
    ConnectToPeer(std::net::SocketAddr, Option<NodeId>),
    /// Update the measured round-trip latency (in milliseconds).
    UpdateLatency(u32),
    /// Trigger a LAN peer discovery scan.
    Scan,
}
