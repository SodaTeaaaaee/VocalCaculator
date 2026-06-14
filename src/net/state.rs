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
    pub address: SocketAddr,
    pub tcp_port: u16,
    pub last_seen: std::time::Instant,
}

/// Snapshot of the current network state (peers, connection status, latency).
#[derive(Debug, Clone, Default)]
pub struct NetworkState {
    pub peers: PeerTable,
    pub is_connected: bool,
    pub latency_ms: Option<u32>,
}
