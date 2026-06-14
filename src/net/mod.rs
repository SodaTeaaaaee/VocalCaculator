#[cfg(target_os = "android")]
pub mod android;
pub mod discovery;
mod handle;
mod handshake;
pub mod protocol;
pub mod router;
pub mod session;
pub mod state;
mod runtime;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use protocol::{NetworkCommand, NetworkMessage, NodeId};
use session::SessionSender;
use tokio::sync::{mpsc, Notify};

slint::include_modules!();

pub use router::{Router, RoutingConfig};
pub use handle::NetworkHandle;
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
    local_display_name: String,

    /// Outgoing message channel: Router sends `(target_node_id, msg)` here.
    outgoing_tx: mpsc::UnboundedSender<(NodeId, NetworkMessage)>,

    /// Inbound message queue: the runtime pushes messages here; the main
    /// thread drains them via [`process_incoming`].
    /// Each message is paired with the sender's NodeId.
    incoming_rx: mpsc::UnboundedReceiver<(NodeId, NetworkMessage)>,

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
    pub fn new(local_display_name: String) -> Self {
        let (outgoing_tx, _) = mpsc::unbounded_channel();
        let (_, incoming_rx) = mpsc::unbounded_channel();
        let (command_tx, _) = mpsc::unbounded_channel();

        Self {
            state: Arc::new(Mutex::new(NetworkState::default())),
            local_node_id: NodeId::new_v4(),
            local_display_name,
            outgoing_tx,
            incoming_rx,
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
    /// Returns a [`NetworkHandle`] that the Router can use to send messages.
    pub fn start(&mut self) -> NetworkHandle {
        let local_id = self.local_node_id;
        let display_name = self.local_display_name.clone();
        let net_state = self.state.clone();
        let shutdown = self.shutdown_flag.clone();
        let shutdown_notify = self.shutdown_notify.clone();
        let sessions = self.sessions.clone();

        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel::<(NodeId, NetworkMessage)>();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel::<(NodeId, NetworkMessage)>();
        let (command_tx, command_rx) = mpsc::unbounded_channel::<NetworkCommand>();


        self.outgoing_tx = outgoing_tx.clone();
        self.incoming_rx = incoming_rx;
        self.command_tx = command_tx.clone();

        // The runtime handle is sent back from the spawned thread via a
        // oneshot channel.
        let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();

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
                        net_state,
                        sessions,
                        outgoing_rx,
                        incoming_tx,
                        command_rx,
                        shutdown,
                        shutdown_notify,
                    )
                    .await;
                });
            })
            .expect("Failed to spawn network thread");

        // Block briefly to obtain the runtime handle.
        let runtime_handle = handle_rx
            .blocking_recv()
            .expect("Runtime thread panicked before sending handle");

        self.runtime_handle = Some(runtime_handle.clone());
        self._thread_handle = Some(thread_handle);

        handle::new_handle(outgoing_tx, runtime_handle)
    }

    /// Drain incoming messages from the runtime and forward each to `handler`.
    ///
    /// Called from the Slint timer on the main thread.
    /// The handler receives (sender_id, message).
    pub fn process_incoming(&mut self, mut handler: impl FnMut(NodeId, NetworkMessage)) {
        while let Ok((sender_id, msg)) = self.incoming_rx.try_recv() {
            handler(sender_id, msg);
        }
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
        let _ = self.command_tx.send(NetworkCommand::ConnectToPeer(addr, target_node_id));
    }

    /// Trigger a LAN peer discovery scan (broadcasts Discover + Announce).
    pub fn trigger_scan(&self) {
        let _ = self.command_tx.send(NetworkCommand::Scan);
    }

    /// Update the local display name and broadcast the change to all
    /// connected peers immediately.
    ///
    /// The new name is persisted in config by the caller before this
    /// method is invoked.  Connected peers will see the change on the
    /// next poll-timer tick; new connections will use the updated name
    /// only after a restart (the runtime captures the name at startup).
    pub fn update_display_name(&mut self, name: String) {
        self.local_display_name = name.clone();
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let msg = protocol::NetworkMessage::PeerNameUpdate {
            display_name: name,
        };
        for sender in sessions.values() {
            let _ = sender.send(msg.clone());
        }
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
