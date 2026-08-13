//! Channel-driven UI event types for network state updates.
//!
//! Provides a typed [`UiEvent`] enum that bridges the async networking
//! runtime with the UI thread.  Network changes (new messages,
//! session connect/disconnect, latency measurements, connection errors)
//! are sent through a bounded mpsc channel and consumed by the UI
//! poll loop or a dedicated event handler.
//!
//! # Event-driven architecture
//!
//! Every discrete state change that the old poll timer handled is
//! represented as a distinct [`UiEvent`] variant.  The UI consumer
//! matches on these variants and updates the corresponding reactive
//! state -- no polling of the [`NetworkManager`] or [`Router`] is
//! needed on the UI side.

use tokio::sync::mpsc;

/// Hard cap between the network runtime and UI event loop. Producers use
/// `try_send`: ordinary status events drop newest on overload, while inbound
/// peer-message overload is handled by disconnecting that peer in the runtime.
pub const UI_EVENT_CAPACITY: usize = 256;

// ---------------------------------------------------------------------------
// PeerDiscoveryPayload
// ---------------------------------------------------------------------------

/// Plain-data snapshot of a discovered peer, safe to send across threads.
///
/// Unlike [`PeerDisplayInfo`] (which wraps Dioxus [`Signal`]s and is
/// confined to the UI thread), this struct carries only owned data and
/// implements `Send`.  The UI bridge converts it into a
/// [`PeerDisplayInfo`] with fresh `Signal` wrappers when processing
/// [`UiEvent::PeerDiscovered`].
#[derive(Debug, Clone, PartialEq)]
pub struct PeerDiscoveryPayload {
    pub name: String,
    pub address: String,
    pub is_connected: bool,
    pub latency_ms: i32,
    pub index: i32,
    pub node_id_string: String,
}

// ---------------------------------------------------------------------------
// UiEvent
// ---------------------------------------------------------------------------

/// Events dispatched from the networking layer to the UI thread.
///
/// Each variant represents a discrete state change that the UI may need
/// to react to (update peer list, refresh display, show an error, etc.).
/// These variants cover every piece of state that the old 50 ms poll
/// timer used to synchronise.
#[derive(Debug, Clone)]
pub enum UiEvent {
    // ---- Peer discovery ----
    /// A new peer was found via LAN discovery (multicast / broadcast).
    PeerDiscovered(PeerDiscoveryPayload),

    /// A previously discovered peer disappeared (expired or removed).
    /// The `String` is the node's UUID.
    PeerLost(String),

    // ---- TCP sessions ----
    /// A TCP session was successfully established with a remote peer.
    /// The `String` is the peer's node UUID.
    SessionEstablished(String),

    /// A TCP session was lost (peer disconnected or connection dropped).
    /// The `String` is the peer's node UUID.
    SessionLost(String),

    // ---- Messages ----
    /// An inbound network message received from a specific peer.
    ///
    /// The first element is the sender's node UUID (`String`); the
    /// second is the serialised [`NetworkMessage`] bytes.
    NetworkMessage(String, Vec<u8>),

    // ---- Errors ----
    /// A connection attempt failed.  The string is a machine-readable
    /// reason code (e.g. `"timeout"`, `"connection_refused"`) that the
    /// UI can map to a localised display string.
    ConnectionError(String),

    // ---- Latency ----
    /// Round-trip latency measurement updated for a specific peer.
    /// The first element is the peer's node UUID; the second is the
    /// measured latency in milliseconds (or `-1` if unknown).
    LatencyUpdate(String, i32),

    // ---- Remote control status ----
    /// The remote-control flags changed.
    /// - `remote_controlled`: the local node is being controlled by a peer.
    /// - `executing_remotely`: the local node is executing on behalf of a peer.
    RemoteControlStatus(bool, bool),

    // ---- Status text ----
    /// The human-readable network status line changed.
    NetworkStatusUpdate(String),
}

// ---------------------------------------------------------------------------
// Channel factory
// ---------------------------------------------------------------------------

/// Create a bounded channel pair for [`UiEvent`] delivery.
///
/// Returns `(sender, receiver)`.  The sender is `Clone + Send` and can
/// be handed out to any async task; the receiver should be polled from
/// the UI thread (e.g. inside a Dioxus coroutine or event loop
/// callback).
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
                .try_send(UiEvent::NetworkStatusUpdate(index.to_string()))
                .unwrap();
        }
        assert!(matches!(
            sender.try_send(UiEvent::NetworkStatusUpdate("overflow".to_string())),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }
}
