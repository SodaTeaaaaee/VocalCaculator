use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::net::protocol::NodeId;
use crate::net::state::PeerInfo;

/// Peers not re-announced within this window are pruned from the table.
const PEER_EXPIRY_DURATION: Duration = Duration::from_secs(90);
const MAX_DISCOVERED_PEERS: usize = 64;

/// In-memory table of discovered peers, keyed by [`NodeId`].
///
/// Guarantees:
/// - **Deduplication**: a node discovered via multiple transports (e.g. both
///   multicast announce and a direct TCP connect) is stored as a single entry.
///   When an existing node is re-added, the entry is updated in-place with the
///   latest metadata and `last_seen` is refreshed. Discovery service endpoints
///   and concrete session peer addresses are merged independently so an
///   inbound connection's ephemeral source port cannot overwrite the endpoint
///   peers should dial.
/// - **Expiry**: [`PeerTable::remove_expired`] prunes any entry whose
///   `last_seen` timestamp is older than [`PEER_EXPIRY_DURATION`].
#[derive(Clone, Debug)]
pub struct PeerTable {
    peers: HashMap<NodeId, PeerInfo>,
}

impl PeerTable {
    /// Create an empty peer table.
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    /// Insert or update a peer.
    ///
    /// If a peer with the same [`NodeId`] already exists, independently known
    /// service/session endpoints are merged and its `last_seen` is refreshed.
    /// This ensures that a node arriving via a second transport does not
    /// create a duplicate entry or corrupt its stable service endpoint.
    pub fn add_peer(&mut self, peer: PeerInfo) {
        let now = peer.last_seen;
        match self.peers.get_mut(&peer.node_id) {
            Some(existing) => {
                // Merge: update mutable fields and refresh the timestamp.
                existing.display_name = peer.display_name;
                if peer.service_endpoint.is_some() {
                    existing.service_endpoint = peer.service_endpoint;
                }
                if peer.session_peer_addr.is_some() {
                    existing.session_peer_addr = peer.session_peer_addr;
                }
                if peer.public_key != [0u8; 32] {
                    existing.public_key = peer.public_key;
                }
                if peer.public_key_fingerprint.is_some() {
                    existing.public_key_fingerprint = peer.public_key_fingerprint;
                }
                existing.last_seen = now;
            }
            None => {
                if self.peers.len() >= MAX_DISCOVERED_PEERS
                    && let Some(oldest) = self
                        .peers
                        .iter()
                        .min_by_key(|(_, peer)| peer.last_seen)
                        .map(|(node_id, _)| *node_id)
                {
                    self.peers.remove(&oldest);
                }
                self.peers.insert(peer.node_id, peer);
            }
        }
    }

    /// Look up a single peer by [`NodeId`].
    pub fn get_peer(&self, id: &NodeId) -> Option<&PeerInfo> {
        self.peers.get(id)
    }

    /// Return a snapshot of every (non-expired) peer.
    ///
    /// Expired entries are **not** pruned automatically; call
    /// [`remove_expired`] beforehand if you want a clean set.
    pub fn get_all_peers(&self) -> Vec<&PeerInfo> {
        self.peers.values().collect()
    }

    /// Remove all peers whose `last_seen` is older than [`PEER_EXPIRY_DURATION`].
    ///
    /// Returns the number of entries that were pruned (useful for logging).
    pub fn remove_expired(&mut self) -> usize {
        let before = self.peers.len();
        let now = Instant::now();
        self.peers
            .retain(|_, peer| now.duration_since(peer.last_seen) < PEER_EXPIRY_DURATION);
        before - self.peers.len()
    }

    /// Number of peers currently in the table (including possibly-expired ones
    /// if [`remove_expired`] has not been called recently).
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Remove a specific peer by [`NodeId`].
    ///
    /// Returns the removed [`PeerInfo`] if the peer was present.
    pub fn remove(&mut self, id: &NodeId) -> Option<PeerInfo> {
        self.peers.remove(id)
    }

    /// Update only the `display_name` of an existing peer.
    ///
    /// Returns `true` if the peer was found and updated, `false` otherwise.
    pub fn update_name(&mut self, id: &NodeId, name: &str) -> bool {
        if let Some(peer) = self.peers.get_mut(id) {
            peer.display_name = name.to_string();
            peer.last_seen = Instant::now();
            true
        } else {
            false
        }
    }

    /// Iterate over all `(NodeId, PeerInfo)` pairs in the table.
    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &PeerInfo)> {
        self.peers.iter()
    }
}

impl Default for PeerTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn make_peer(id: NodeId, name: &str, port: u16) -> PeerInfo {
        PeerInfo {
            node_id: id,
            display_name: name.to_string(),
            service_endpoint: Some(SocketAddr::new("192.168.1.1".parse().unwrap(), port)),
            session_peer_addr: None,
            last_seen: Instant::now(),
            public_key: [0u8; 32],
            public_key_fingerprint: None,
        }
    }

    #[test]
    fn add_and_get() {
        let mut table = PeerTable::new();
        let id = NodeId::new_v4();
        table.add_peer(make_peer(id, "Alice", 4242));

        let peer = table.get_peer(&id).expect("peer should exist");
        assert_eq!(peer.display_name, "Alice");
        assert_eq!(peer.service_endpoint.unwrap().port(), 4242);
    }

    #[test]
    fn dedup_merges_same_node_id() {
        let mut table = PeerTable::new();
        let id = NodeId::new_v4();

        // First arrival via multicast.
        table.add_peer(make_peer(id, "Alice", 4242));
        // Second arrival via TCP (different address/port).
        table.add_peer(make_peer(id, "Alice-v2", 5000));

        assert_eq!(table.len(), 1, "should deduplicate by NodeId");
        let peer = table.get_peer(&id).unwrap();
        assert_eq!(peer.display_name, "Alice-v2");
        assert_eq!(peer.service_endpoint.unwrap().port(), 5000);
    }

    #[test]
    fn inbound_session_source_port_does_not_overwrite_service_endpoint() {
        let mut table = PeerTable::new();
        let id = NodeId::new_v4();
        let service = SocketAddr::new("192.168.1.10".parse().unwrap(), 42420);
        let ephemeral_source = SocketAddr::new("192.168.1.10".parse().unwrap(), 53124);

        table.add_peer(PeerInfo {
            node_id: id,
            display_name: "Alice".to_string(),
            service_endpoint: Some(service),
            session_peer_addr: None,
            last_seen: Instant::now(),
            public_key: [0u8; 32],
            public_key_fingerprint: Some("expected".to_string()),
        });
        table.add_peer(PeerInfo {
            node_id: id,
            display_name: "Alice".to_string(),
            service_endpoint: None,
            session_peer_addr: Some(ephemeral_source),
            last_seen: Instant::now(),
            public_key: [7u8; 32],
            public_key_fingerprint: None,
        });

        let peer = table.get_peer(&id).unwrap();
        assert_eq!(peer.service_endpoint, Some(service));
        assert_eq!(peer.session_peer_addr, Some(ephemeral_source));
        assert_eq!(peer.public_key_fingerprint.as_deref(), Some("expected"));
    }

    #[test]
    fn remove_expired_prunes_stale_entries() {
        let mut table = PeerTable::new();
        let fresh_id = NodeId::new_v4();
        let stale_id = NodeId::new_v4();

        // Fresh peer (just seen).
        table.add_peer(make_peer(fresh_id, "Fresh", 4242));

        // Stale peer (last_seen pushed back past the expiry window).
        let mut stale = make_peer(stale_id, "Stale", 5000);
        stale.last_seen = Instant::now() - Duration::from_secs(120);
        table.add_peer(stale);

        assert_eq!(table.len(), 2);
        let pruned = table.remove_expired();
        assert_eq!(pruned, 1);
        assert_eq!(table.len(), 1);
        assert!(table.get_peer(&fresh_id).is_some());
        assert!(table.get_peer(&stale_id).is_none());
    }

    #[test]
    fn get_all_peers_returns_everyone() {
        let mut table = PeerTable::new();
        table.add_peer(make_peer(NodeId::new_v4(), "A", 1));
        table.add_peer(make_peer(NodeId::new_v4(), "B", 2));
        table.add_peer(make_peer(NodeId::new_v4(), "C", 3));
        assert_eq!(table.get_all_peers().len(), 3);
    }

    #[test]
    fn default_is_empty() {
        let table = PeerTable::default();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn peer_table_is_bounded_under_identity_flood() {
        let mut table = PeerTable::new();
        for index in 0..100u128 {
            table.add_peer(make_peer(NodeId::from_u128(index + 1), "peer", 42420));
        }
        assert_eq!(table.len(), MAX_DISCOVERED_PEERS);
    }
}
