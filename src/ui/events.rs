//! Channel-driven UI event types for network state updates.
//!
//! The runtime sends typed [`UiEvent`] values. Node identities are
//! [`NodeId`], inbound calculator messages are [`NetworkMessage`] values
//! (no re-bincode hop), and peer rows are [`PeerViewModel`].

use std::net::SocketAddr;

use tokio::sync::mpsc;

use crate::net::protocol::{NetworkMessage, NodeId};
use crate::net::view::{BindStatus, ConnectErrorKind, NetworkStatusKind, PeerViewModel, ScanState};

pub use crate::net::limits::UI_EVENT_CAPACITY;

/// Events dispatched from the networking layer to the UI thread.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// Insert or replace one peer card.
    PeerUpsert(PeerViewModel),
    /// A previously discovered peer disappeared.
    PeerLost { node_id: NodeId },
    /// An authenticated TCP session is now the current generation.
    SessionEstablished { node_id: NodeId, session_id: NodeId },
    /// That session generation is gone.
    SessionLost { node_id: NodeId, session_id: NodeId },
    /// Inbound protocol message from an authenticated peer.
    InboundMessage {
        sender: NodeId,
        message: NetworkMessage,
    },
    /// A connect / bind / policy failure.
    ConnectionError {
        target: Option<NodeId>,
        kind: ConnectErrorKind,
    },
    /// Per-session RTT. `None` means unknown.
    LatencyUpdate {
        node_id: NodeId,
        latency_ms: Option<u32>,
    },
    /// Controller / executor flags changed.
    RemoteControl {
        controllers: Vec<NodeId>,
        executor: Option<NodeId>,
    },
    /// Human-readable status line plus a coarse kind.
    NetworkStatus {
        kind: NetworkStatusKind,
        text: String,
    },
    /// Runtime-owned scan progress.
    ScanState(ScanState),
    /// TCP listener is bound.
    ListenerBound { addr: SocketAddr },
    /// TCP listener failed; the rest of the runtime must stay up.
    ListenerFailed { port: u16 },
    /// Full bind projection (optional convenience for This Device).
    BindStatus(BindStatus),
}

/// Create a bounded channel pair for [`UiEvent`] delivery.
pub fn create_ui_channel() -> (mpsc::Sender<UiEvent>, mpsc::Receiver<UiEvent>) {
    mpsc::channel(UI_EVENT_CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_event_channel_has_a_hard_capacity() {
        let (sender, _receiver) = create_ui_channel();
        for index in 0..UI_EVENT_CAPACITY {
            sender
                .try_send(UiEvent::NetworkStatus {
                    kind: NetworkStatusKind::Enabled,
                    text: index.to_string(),
                })
                .unwrap();
        }
        assert!(matches!(
            sender.try_send(UiEvent::NetworkStatus {
                kind: NetworkStatusKind::Error,
                text: "overflow".to_string(),
            }),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }
}
