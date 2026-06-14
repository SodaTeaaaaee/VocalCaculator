use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::net::protocol::NodeId;
use crate::net::state::PeerInfo;

/// Peers not re-announced within this window are pruned from the table.
const PEER_EXPIRY_DURATION: Duration = Duration::from_secs(90);

/// In-memory table of discovered peers, keyed by [`NodeId`].
///
/// Guarantees:
/// - **Deduplication**: a node discovered via multiple transports (e.g. both
///   multicast announce and a direct TCP connect) is stored as a single entry.
///   When an existing node is re-added, the entry is updated in-place with the
///   latest metadata and `last_seen` is refreshed.
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
    /// If a peer with the same [`NodeId`] already exists, its `display_name`,
    /// `address`, and `tcp_port` are overwritten with the new values and its
    /// `last_seen` is refreshed. This ensures that a node arriving via a
    /// second transport does not create a duplicate entry.
    pub fn add_peer(&mut self, peer: PeerInfo) {
        let now = peer.last_seen;
        match self.peers.get_mut(&peer.node_id) {
            Some(existing) => {
                // Merge: update mutable fields and refresh the timestamp.
                existing.display_name = peer.display_name;
                existing.address = peer.address;
                existing.tcp_port = peer.tcp_port;
                existing.last_seen = now;
            }
            None => {
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
            address: SocketAddr::new("192.168.1.1".parse().unwrap(), port),
            tcp_port: port,
            last_seen: Instant::now(),
        }
    }

    #[test]
    fn add_and_get() {
        let mut table = PeerTable::new();
        let id = NodeId::new_v4();
        table.add_peer(make_peer(id, "Alice", 4242));

        let peer = table.get_peer(&id).expect("peer should exist");
        assert_eq!(peer.display_name, "Alice");
        assert_eq!(peer.tcp_port, 4242);
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
        assert_eq!(peer.tcp_port, 5000);
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
}
