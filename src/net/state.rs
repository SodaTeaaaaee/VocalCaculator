use std::net::SocketAddr;

use crate::net::discovery::PeerTable;
use crate::net::protocol::NodeId;

// ---------------------------------------------------------------------------
// Peer / session types
// ---------------------------------------------------------------------------

/// Information about a discovered or connected peer.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub node_id: NodeId,
    pub display_name: String,
    /// Stable service endpoint advertised by discovery.
    pub service_endpoint: Option<SocketAddr>,
    /// Address of the concrete TCP connection.  For accepted sessions this
    /// contains the caller's ephemeral source port and must never replace the
    /// advertised service endpoint.
    pub session_peer_addr: Option<SocketAddr>,
    pub last_seen: std::time::Instant,
    /// Ed25519 public key received during handshake (32 bytes).
    /// All-zeros if the remote did not provide one (legacy peer).
    pub public_key: [u8; 32],
    /// SHA-256 fingerprint prefix advertised during discovery (mDNS `pkfp`).
    /// Used to detect discovery/session public-key mismatches before trust.
    pub public_key_fingerprint: Option<String>,
}

impl PeerInfo {
    /// Best address for display/diagnostics.  This is not necessarily safe to
    /// dial; callers initiating a new connection must use `service_endpoint`.
    pub fn display_endpoint(&self) -> Option<SocketAddr> {
        self.service_endpoint.or(self.session_peer_addr)
    }
}

/// Snapshot of the current network state (peers, connection status, latency).
#[derive(Debug, Clone, Default)]
pub struct NetworkState {
    pub peers: PeerTable,
    pub is_connected: bool,
    pub latency_ms: Option<u32>,
}
