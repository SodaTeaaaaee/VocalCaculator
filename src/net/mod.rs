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
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crate::app::network_mode::NetworkMode;
use crate::app::storage::Storage;
use crate::ui::events::UiEvent;
use ed25519_dalek::SigningKey;
use protocol::{
    ExpectedPeerIdentity, NetworkCommand, NetworkMessage, NodeId, OutboundConnectRequest,
    valid_display_name,
};
use session::ActiveSession;
use tokio::sync::{mpsc, watch};

const RUNTIME_COMMAND_CAPACITY: usize = 256;
pub(crate) const OUTGOING_MESSAGE_CAPACITY: usize = 256;
const NETWORK_THREAD_START_TIMEOUT: Duration = Duration::from_secs(5);
const NETWORK_THREAD_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
type OutgoingMessage = (NodeId, NetworkMessage);

fn runtime_command_channel() -> (mpsc::Sender<NetworkCommand>, mpsc::Receiver<NetworkCommand>) {
    mpsc::channel(RUNTIME_COMMAND_CAPACITY)
}

fn outgoing_message_channel() -> (
    mpsc::Sender<OutgoingMessage>,
    mpsc::Receiver<OutgoingMessage>,
) {
    mpsc::channel(OUTGOING_MESSAGE_CAPACITY)
}

// UI-facing display types live in the Dioxus UI layer.

pub use handle::NetworkHandle;
pub use router::{Router, RoutingConfig};
pub use state::{NetworkState, PeerInfo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkStartError {
    Offline,
    AlreadyRunning,
    ShutdownUnconfirmed,
    Startup(String),
}

impl std::fmt::Display for NetworkStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline => {
                formatter.write_str("Offline: networking cannot start in offline mode")
            }
            Self::AlreadyRunning => {
                formatter.write_str("AlreadyRunning: network runtime is healthy")
            }
            Self::ShutdownUnconfirmed => formatter.write_str(
                "ShutdownUnconfirmed: a previous network runtime did not confirm shutdown",
            ),
            Self::Startup(error) => write!(formatter, "Startup: {error}"),
        }
    }
}

impl std::error::Error for NetworkStartError {}

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
    /// Ed25519 signing key used for protocol v5 possession proofs.
    local_signing_key: SigningKey,

    /// Outgoing message channel: Router sends `(target_node_id, msg)` here.
    outgoing_tx: mpsc::Sender<(NodeId, NetworkMessage)>,

    /// Channel for sending [`UiEvent`]s to the UI layer.
    ui_event_tx: mpsc::Sender<UiEvent>,

    /// Command channel to the runtime.
    command_tx: mpsc::Sender<NetworkCommand>,

    /// Active sessions (managed by the runtime; the manager holds a clone).
    sessions: Arc<Mutex<HashMap<NodeId, ActiveSession>>>,

    /// Effective mode of the currently running network runtime. Defaults to
    /// Offline so calls made before `start()` fail closed.
    network_mode: NetworkMode,

    /// Tokio runtime handle.
    runtime_handle: Option<tokio::runtime::Handle>,

    /// Join handle for the dedicated OS thread.
    thread_handle: Option<std::thread::JoinHandle<()>>,

    /// Reliable level-triggered shutdown signal. Unlike `Notify::notify_waiters`,
    /// a watch value sent before the runtime begins waiting is retained.
    shutdown_tx: watch::Sender<bool>,

    /// Completion acknowledgement sent only after the runtime and Tokio
    /// executor have been fully dropped on the network thread.
    thread_done_rx: Option<std::sync::mpsc::Receiver<()>>,

    /// A timed-out shutdown deliberately detaches the OS thread to keep the
    /// caller bounded. Once that happens, the manager can no longer prove the
    /// old runtime has exited and must fail closed instead of starting a
    /// second listener/runtime over the same shared state.
    runtime_shutdown_unconfirmed: bool,
}

impl NetworkManager {
    /// Create a new `NetworkManager` (does not start the runtime).
    ///
    /// Identity fields (`node_id`, `display_name`, `pubkey`) are derived
    /// from the provided [`Storage`], ensuring a stable identity across
    /// restarts.
    pub fn new(storage: Arc<Storage>, ui_event_tx: mpsc::Sender<UiEvent>) -> Self {
        let local_node_id = storage.identity().node_id();
        let local_display_name =
            Arc::new(RwLock::new(storage.config().network.display_name.clone()));
        let local_pubkey = storage.identity().public_key_bytes();
        let local_signing_key = storage.identity().signing_key();

        let (outgoing_tx, _) = outgoing_message_channel();
        let (command_tx, _) = runtime_command_channel();
        let (shutdown_tx, _) = watch::channel(false);

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
            network_mode: NetworkMode::Offline,
            runtime_handle: None,
            thread_handle: None,
            shutdown_tx,
            thread_done_rx: None,
            runtime_shutdown_unconfirmed: false,
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
    /// Thread/runtime construction failures are reported instead of panicking.
    pub fn start(&mut self, mode: NetworkMode) -> Result<NetworkHandle, NetworkStartError> {
        if mode == NetworkMode::Offline {
            return Err(NetworkStartError::Offline);
        }
        if self.runtime_shutdown_unconfirmed {
            return Err(NetworkStartError::ShutdownUnconfirmed);
        }
        if let Some(thread_handle) = self.thread_handle.as_ref() {
            if !thread_handle.is_finished() {
                return Err(NetworkStartError::AlreadyRunning);
            }
            // A naturally exited runtime can be joined and replaced. A live
            // one is never stopped implicitly by a duplicate start call.
            if !self.shutdown() {
                return Err(NetworkStartError::ShutdownUnconfirmed);
            }
        }
        let local_id = self.local_node_id;
        let display_name = self.local_display_name.clone();
        let local_pubkey = self.local_pubkey;
        let local_signing_key = self.local_signing_key.clone();
        let net_state = self.state.clone();
        let sessions = self.sessions.clone();
        let ui_event_tx = self.ui_event_tx.clone();

        let (outgoing_tx, outgoing_rx) = outgoing_message_channel();
        let (command_tx, command_rx) = runtime_command_channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // The runtime handle is sent back from the spawned OS thread via a
        // standard channel.  This method can be called while Dioxus is running
        // inside a Tokio runtime, so do not use Tokio's blocking_recv here.
        let (handle_tx, handle_rx) =
            std::sync::mpsc::sync_channel::<Result<tokio::runtime::Handle, String>>(1);
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);

        let thread_handle = match std::thread::Builder::new()
            .name("vocal-calc-net".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let rt = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(2)
                        .enable_all()
                        .build()
                        .map_err(|e| format!("failed to create Tokio runtime: {e}"))?;

                    let handle = rt.handle().clone();
                    if handle_tx.send(Ok(handle)).is_err() {
                        return Err("network manager dropped during runtime startup".to_string());
                    }

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
                            shutdown_rx,
                            mode,
                        )
                        .await;
                    });
                    drop(rt);
                    Ok::<(), String>(())
                }));

                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        let _ = handle_tx.send(Err(error.clone()));
                        log::error!("Network runtime thread failed: {error}");
                    }
                    Err(_) => log::error!("Network runtime thread panicked"),
                }
                // The inner scope (including Tokio runtime) is fully dropped
                // before this acknowledgement is sent.
                let _ = done_tx.send(());
            }) {
            Ok(handle) => handle,
            Err(error) => {
                return Err(NetworkStartError::Startup(format!(
                    "failed to spawn network thread: {error}"
                )));
            }
        };

        // Bound startup waiting as well: a broken runtime constructor cannot
        // permanently block the UI thread.
        let runtime_handle = match handle_rx.recv_timeout(NETWORK_THREAD_START_TIMEOUT) {
            Ok(Ok(handle)) => handle,
            Ok(Err(error)) => {
                let stopped =
                    finish_network_thread(thread_handle, done_rx, NETWORK_THREAD_SHUTDOWN_TIMEOUT);
                self.runtime_shutdown_unconfirmed = !stopped;
                return Err(NetworkStartError::Startup(error));
            }
            Err(error) => {
                let _ = shutdown_tx.send(true);
                let stopped =
                    finish_network_thread(thread_handle, done_rx, NETWORK_THREAD_SHUTDOWN_TIMEOUT);
                self.runtime_shutdown_unconfirmed = !stopped;
                return Err(NetworkStartError::Startup(format!(
                    "network runtime startup timed out or disconnected: {error}"
                )));
            }
        };

        self.outgoing_tx = outgoing_tx.clone();
        self.command_tx = command_tx;
        self.shutdown_tx = shutdown_tx;
        self.network_mode = mode;
        self.runtime_handle = Some(runtime_handle.clone());
        self.thread_handle = Some(thread_handle);
        self.thread_done_rx = Some(done_rx);
        self.runtime_shutdown_unconfirmed = false;

        Ok(handle::new_handle(outgoing_tx, runtime_handle))
    }

    /// Gracefully shut down the networking runtime within a fixed deadline.
    /// Returns `true` when no runtime remains or the thread acknowledged full
    /// completion; returns `false` after detaching an unresponsive thread.
    pub fn shutdown(&mut self) -> bool {
        self.shutdown_with_timeout(NETWORK_THREAD_SHUTDOWN_TIMEOUT)
    }

    fn shutdown_with_timeout(&mut self, timeout: Duration) -> bool {
        let _ = self.shutdown_tx.send(true);
        log::info!("Network shutdown requested");
        self.runtime_handle.take();
        if self.runtime_shutdown_unconfirmed && self.thread_handle.is_none() {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            self.network_mode = NetworkMode::Offline;
            return false;
        }
        let stopped = match (self.thread_handle.take(), self.thread_done_rx.take()) {
            (Some(thread_handle), Some(done_rx)) => {
                finish_network_thread(thread_handle, done_rx, timeout)
            }
            (Some(thread_handle), None) => {
                log::error!("Network thread completion receiver missing; detaching safely");
                drop(thread_handle);
                false
            }
            (None, _) => true,
        };
        if !stopped {
            self.runtime_shutdown_unconfirmed = true;
        }
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.network_mode = NetworkMode::Offline;
        stopped
    }

    /// Initiate a TCP connection to a peer at the given address.
    ///
    /// Sends a `ConnectToPeer` command through the command channel to the
    /// networking runtime, which will spawn the actual TCP connection task.
    pub fn connect_to_peer(&self, addr: SocketAddr, target_node_id: Option<NodeId>) -> bool {
        if !runtime::outbound_addr_allowed(self.network_mode, addr) {
            log::warn!(
                "Refusing outbound connection to {} in network mode {:?}",
                addr,
                self.network_mode,
            );
            let _ = self.ui_event_tx.try_send(UiEvent::ConnectionError(
                "loopback_address_required".to_string(),
            ));
            return false;
        }
        if target_node_id.is_some_and(|node_id| {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&node_id)
        }) {
            log::trace!("Peer already has an active session; skipping TCP reconnect");
            return false;
        }
        let expected_peer = target_node_id.map(|node_id| {
            let public_key_fingerprint = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .peers
                .get_peer(&node_id)
                .and_then(|peer| peer.public_key_fingerprint.clone());
            ExpectedPeerIdentity {
                node_id,
                public_key_fingerprint,
            }
        });
        match self
            .command_tx
            .try_send(NetworkCommand::ConnectToPeer(OutboundConnectRequest {
                addr,
                expected_peer,
                report_errors: true,
            })) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("Network command queue rejected connect request: {error}");
                let _ = self
                    .ui_event_tx
                    .try_send(UiEvent::ConnectionError("command_overloaded".to_string()));
                false
            }
        }
    }

    /// Trigger a LAN peer discovery scan (broadcasts Discover + Announce).
    pub fn trigger_scan(&self) {
        let _ = self.command_tx.try_send(NetworkCommand::Scan);
    }

    /// Update the local display name, broadcast it to connected peers, and
    /// trigger a discovery announce so future handshakes use the same name.
    pub fn update_display_name(&mut self, name: String) {
        if !valid_display_name(&name) {
            log::warn!("Rejected invalid local display name update");
            return;
        }
        {
            let mut display_name = self
                .local_display_name
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *display_name = name.clone();
        }
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let msg = protocol::NetworkMessage::PeerNameUpdate { display_name: name };
        for session in sessions.values() {
            let _ = session.sender.try_send(msg.clone());
        }
        let _ = self.command_tx.try_send(NetworkCommand::Scan);
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

fn finish_network_thread(
    thread_handle: std::thread::JoinHandle<()>,
    done_rx: std::sync::mpsc::Receiver<()>,
    timeout: Duration,
) -> bool {
    if thread_handle.thread().id() == std::thread::current().id() {
        log::warn!("NetworkManager cannot join its own runtime thread; detaching");
        return false;
    }

    match done_rx.recv_timeout(timeout) {
        Ok(()) => {
            if thread_handle.join().is_err() {
                log::error!("Network runtime thread panicked during shutdown");
                false
            } else {
                true
            }
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            log::error!(
                "Network runtime did not stop within {:?}; detaching thread to keep shutdown bounded",
                timeout,
            );
            drop(thread_handle);
            false
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            // Without the explicit post-runtime acknowledgement we cannot
            // prove that a join is bounded, so detach instead of risking a
            // shutdown hang.
            log::error!("Network runtime completion channel disconnected; detaching thread");
            drop(thread_handle);
            false
        }
    }
}

impl Drop for NetworkManager {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
