#[cfg(target_os = "android")]
pub mod android;
pub mod discovery;
mod handle;
mod handshake;
pub mod protocol;
pub mod router;
mod runtime;
pub mod session;
pub mod state;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::app::network_mode::NetworkMode;
use crate::app::storage::Storage;
use crate::ui::events::UiEvent;
use ed25519_dalek::SigningKey;
use protocol::{NetworkCommand, NetworkMessage, NodeId};
use session::SessionSender;
use tokio::sync::{Notify, mpsc};

// UI-facing display types live in the Dioxus UI layer.

pub use handle::NetworkHandle;
pub use router::{Router, RoutingConfig};
pub use state::{NetworkState, PeerInfo};

// ---------------------------------------------------------------------------
// NetworkManager -- runtime host + session registry
// ---------------------------------------------------------------------------

/// Manages the networking runtime, session registry, and peer state.
pub struct NetworkManager {
    /// Shared network state (peers, latency, etc.).
    state: Arc<Mutex<NetworkState>>,
    /// This node's unique identifier.
    local_node_id: NodeId,
    /// Display name for handshake and discovery.
    local_display_name: Arc<RwLock<String>>,
    /// Ed25519 public key advertised during handshake (32 bytes).
    local_pubkey: [u8; 32],
    /// Ed25519 signing key used for protocol v4 possession proofs.
    local_signing_key: SigningKey,

    /// Outgoing message channel: Router sends `(target_node_id, msg)` here.
    outgoing_tx: mpsc::UnboundedSender<(NodeId, NetworkMessage)>,

    /// Channel for sending [`UiEvent`]s to the UI layer.
    ui_event_tx: mpsc::UnboundedSender<UiEvent>,

    /// Command channel to the runtime.
    command_tx: mpsc::UnboundedSender<NetworkCommand>,

    /// Active sessions (managed by the runtime; the manager holds a clone).
    sessions: Arc<Mutex<HashMap<NodeId, SessionSender>>>,

    /// Tokio runtime handle.
    runtime_handle: Option<tokio::runtime::Handle>,

    /// Join handle for the dedicated OS thread.
    _thread_handle: Option<std::thread::JoinHandle<()>>,

    /// Shutdown flag.
    shutdown_flag: Arc<AtomicBool>,

    /// Shutdown notification (replaces busy-poll in wait_for_shutdown).
    shutdown_notify: Arc<Notify>,
}

impl NetworkManager {
    /// Create a new `NetworkManager` (does not start the runtime).
    ///
    /// Identity fields (`node_id`, `display_name`, `pubkey`) are derived
    /// from the provided [`Storage`], ensuring a stable identity across
    /// restarts.
    pub fn new(storage: Arc<Storage>, ui_event_tx: mpsc::UnboundedSender<UiEvent>) -> Self {
        let local_node_id = storage.identity().node_id();
        let local_display_name =
            Arc::new(RwLock::new(storage.config().network.display_name.clone()));
        let local_pubkey = storage.identity().public_key_bytes();
        let local_signing_key = storage.identity().signing_key();

        let (outgoing_tx, _) = mpsc::unbounded_channel();
        let (command_tx, _) = mpsc::unbounded_channel();

        Self {
            state: Arc::new(Mutex::new(NetworkState::default())),
            local_node_id,
            local_display_name,
            local_pubkey,
            local_signing_key,
            outgoing_tx,
            ui_event_tx,
            command_tx,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            runtime_handle: None,
            _thread_handle: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
        }
    }

    /// Shared network state handle.
    pub fn state(&self) -> Arc<Mutex<NetworkState>> {
        self.state.clone()
    }

    /// This node's unique identifier.
    pub fn local_node_id(&self) -> NodeId {
        self.local_node_id
    }

    /// Start the networking runtime on a dedicated OS thread.
    ///
    /// `mode` controls socket behavior inside the runtime: `Lan` binds the
    /// fixed session port and starts discovery; `LoopbackTest` binds an
    /// ephemeral loopback-only port and skips discovery entirely; `Offline`
    /// is rejected defense-in-depth (callers should not reach this method
    /// at all in that mode -- see `ui::bridge::init_networking`).
    ///
    /// Returns a [`NetworkHandle`] that the Router can use to send messages.
    pub fn start(&mut self, mode: NetworkMode) -> NetworkHandle {
        let local_id = self.local_node_id;
        let display_name = self.local_display_name.clone();
        let local_pubkey = self.local_pubkey;
        let local_signing_key = self.local_signing_key.clone();
        let net_state = self.state.clone();
        let shutdown = self.shutdown_flag.clone();
        let shutdown_notify = self.shutdown_notify.clone();
        let sessions = self.sessions.clone();
        let ui_event_tx = self.ui_event_tx.clone();

        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel::<(NodeId, NetworkMessage)>();
        let (command_tx, command_rx) = mpsc::unbounded_channel::<NetworkCommand>();

        self.outgoing_tx = outgoing_tx.clone();
        self.command_tx = command_tx.clone();

        // The runtime handle is sent back from the spawned OS thread via a
        // standard channel.  This method can be called while Dioxus is running
        // inside a Tokio runtime, so do not use Tokio's blocking_recv here.
        let (handle_tx, handle_rx) = std::sync::mpsc::sync_channel(1);

        let thread_handle = std::thread::Builder::new()
            .name("vocal-calc-net".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        log::error!("Failed to create tokio runtime: {}", e);
                        return;
                    }
                };

                let handle = rt.handle().clone();
                let _ = handle_tx.send(handle);

                rt.block_on(async move {
                    runtime::run_network_runtime(
                        local_id,
                        display_name,
                        local_pubkey,
                        local_signing_key,
                        net_state,
                        sessions,
                        outgoing_rx,
                        ui_event_tx,
                        command_rx,
                        shutdown,
                        shutdown_notify,
                        mode,
                    )
                    .await;
                });
            })
            .expect("Failed to spawn network thread");

        // Block briefly to obtain the runtime handle.
        let runtime_handle = handle_rx
            .recv()
            .expect("Runtime thread panicked before sending handle");

        self.runtime_handle = Some(runtime_handle.clone());
        self._thread_handle = Some(thread_handle);

        handle::new_handle(outgoing_tx, runtime_handle)
    }

    /// Gracefully shut down the networking runtime.
    pub fn shutdown(&mut self) {
        self.shutdown_flag.store(true, Ordering::Relaxed);
        self.shutdown_notify.notify_waiters();
        log::info!("Network shutdown requested");
    }

    /// Initiate a TCP connection to a peer at the given address.
    ///
    /// Sends a `ConnectToPeer` command through the command channel to the
    /// networking runtime, which will spawn the actual TCP connection task.
    pub fn connect_to_peer(&self, addr: SocketAddr, target_node_id: Option<NodeId>) {
        let _ = self
            .command_tx
            .send(NetworkCommand::ConnectToPeer(addr, target_node_id));
    }

    /// Trigger a LAN peer discovery scan (broadcasts Discover + Announce).
    pub fn trigger_scan(&self) {
        let _ = self.command_tx.send(NetworkCommand::Scan);
    }

    /// Update the local display name, broadcast it to connected peers, and
    /// trigger a discovery announce so future handshakes use the same name.
    pub fn update_display_name(&mut self, name: String) {
        {
            let mut display_name = self
                .local_display_name
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *display_name = name.clone();
        }
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let msg = protocol::NetworkMessage::PeerNameUpdate { display_name: name };
        for sender in sessions.values() {
            let _ = sender.send(msg.clone());
        }
        let _ = self.command_tx.send(NetworkCommand::Scan);
    }

    /// Return the set of node IDs that have active TCP sessions.
    ///
    /// Used by the poll timer to keep the Router's broadcast list in sync
    /// with the networking runtime's session registry.  Only nodes with
    /// live TCP sessions appear here — discovered-but-not-connected peers
    /// are excluded.
    pub fn active_session_ids(&self) -> HashSet<NodeId> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect()
    }
}
