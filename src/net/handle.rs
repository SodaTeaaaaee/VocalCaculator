use tokio::sync::mpsc;

use crate::net::protocol::{NetworkMessage, NodeId};

// ---------------------------------------------------------------------------
// NetworkHandle — passed to the Router for sending messages
// ---------------------------------------------------------------------------

/// Thread-safe handle that the Router (running on the Slint main thread)
/// uses to send messages into the networking runtime.
#[derive(Clone)]
pub struct NetworkHandle {
    /// Send a message to a specific peer (routed to the correct session).
    outgoing_tx: mpsc::UnboundedSender<(NodeId, NetworkMessage)>,
    /// Tokio runtime handle for `block_on` from the sync Slint thread.
    runtime_handle: tokio::runtime::Handle,
}

impl NetworkHandle {
    /// Get a clone of the outgoing message sender for routing messages to peers.
    pub fn outgoing_sender(&self) -> mpsc::UnboundedSender<(NodeId, NetworkMessage)> {
        self.outgoing_tx.clone()
    }

    /// Access the underlying tokio runtime handle.
    pub fn runtime_handle(&self) -> &tokio::runtime::Handle {
        &self.runtime_handle
    }
}

// Constructor used by NetworkManager::start().
pub(super) fn new_handle(
    outgoing_tx: mpsc::UnboundedSender<(NodeId, NetworkMessage)>,
    runtime_handle: tokio::runtime::Handle,
) -> NetworkHandle {
    NetworkHandle {
        outgoing_tx,
        runtime_handle,
    }
}
