use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};

use ed25519_dalek::SigningKey;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinHandle;

use super::discovery::{DiscoveryEndpoint, DiscoveryService, public_key_fingerprint};
use super::protocol::{
    DiscoveryMessage, ExpectedPeerIdentity, NetworkCommand, NetworkMessage, NodeId,
    OutboundConnectRequest, SESSION_TCP_PORT, SessionId,
};
use super::session::{self, ActiveSession};
use super::session_registry::SessionRegistry;
use super::state::{NetworkState, PeerInfo};
use crate::app::network_mode::NetworkMode;
use crate::net::limits::{
    DISCOVERY_ENDPOINT_RETRY_SECS, MAX_DISCOVERY_ENDPOINT_ATTEMPTS, MAX_IN_FLIGHT_CONNECTS,
    MAX_INBOUND_SESSIONS, MERGED_COMMAND_CAPACITY, SCAN_COMMAND_CAPACITY, SESSION_COMMAND_CAPACITY,
};
use crate::net::view::{
    BindStatus, ConnectErrorKind, NetworkStatusKind, PeerPresence, PeerRole, PeerViewModel,
    ScanState,
};
use crate::ui::events::UiEvent;

const DISCOVERY_RESTART_BACKOFF_START: std::time::Duration = std::time::Duration::from_secs(1);
const DISCOVERY_RESTART_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ConnectAttemptKey {
    Peer(NodeId),
    Address(SocketAddr),
}

impl ConnectAttemptKey {
    fn from_request(request: &OutboundConnectRequest) -> Self {
        request
            .expected_peer
            .as_ref()
            .map(|peer| Self::Peer(peer.node_id))
            .unwrap_or(Self::Address(request.addr))
    }
}

pub(crate) fn outbound_addr_allowed(mode: NetworkMode, addr: SocketAddr) -> bool {
    match mode {
        NetworkMode::Lan => true,
        NetworkMode::LoopbackTest => addr.ip().is_loopback(),
        NetworkMode::Offline => false,
    }
}

/// Apply the network-mode policy before invoking the actual connector.
/// Keeping this generic makes the "no syscall on rejection" boundary directly
/// testable without touching a real LAN interface.
pub(crate) async fn connect_tcp_checked<F, Fut, T>(
    mode: NetworkMode,
    addr: SocketAddr,
    connector: F,
) -> std::io::Result<T>
where
    F: FnOnce(SocketAddr) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<T>>,
{
    if !outbound_addr_allowed(mode, addr) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("network mode {mode:?} forbids outbound address {addr}"),
        ));
    }
    connector(addr).await
}

/// Pure selection of the TCP session-listener bind address for a given
/// [`NetworkMode`].
///
/// `Lan` always binds the fixed [`SESSION_TCP_PORT`] on all interfaces so
/// peers can reach this node without an out-of-band port exchange.
/// `LoopbackTest` binds an OS-assigned ephemeral port on the loopback
/// interface only, so no traffic ever reaches the real LAN. `Offline` has
/// no meaningful bind address; callers must not reach the bind step in
/// that mode (see the defense-in-depth check in
/// [`run_network_runtime`]) -- it is included here only so the function is
/// total.
pub(crate) fn session_bind_addr(mode: NetworkMode) -> SocketAddr {
    match mode {
        NetworkMode::Lan => SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            SESSION_TCP_PORT,
        ),
        NetworkMode::LoopbackTest | NetworkMode::Offline => {
            SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0)
        }
    }
}

fn peer_view_from_info(
    peer: &PeerInfo,
    presence: PeerPresence,
    session_id: Option<SessionId>,
    latency_ms: Option<u32>,
) -> PeerViewModel {
    PeerViewModel {
        node_id: peer.node_id,
        display_name: peer.display_name.clone(),
        endpoint: peer.display_endpoint(),
        fingerprint: peer.public_key_fingerprint.clone(),
        presence,
        role: PeerRole::Idle,
        latency_ms,
        session_id,
    }
}

fn emit_peer_upsert(
    state: &NetworkState,
    sessions: &SessionRegistry,
    node_id: NodeId,
    presence: PeerPresence,
    ui_tx: &mpsc::Sender<UiEvent>,
) {
    if let Some(peer) = state.peers.get_peer(&node_id) {
        let session_id = sessions.get(node_id).map(|session| session.session_id);
        try_emit_ui_event(
            ui_tx,
            UiEvent::PeerUpsert(peer_view_from_info(peer, presence, session_id, None)),
        );
    }
}

fn emit_peer_presence(
    state: &Arc<Mutex<NetworkState>>,
    sessions: &SessionRegistry,
    node_id: NodeId,
    presence: PeerPresence,
    ui_tx: &mpsc::Sender<UiEvent>,
) {
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    emit_peer_upsert(&state, sessions, node_id, presence, ui_tx);
}

fn prune_expired_peers(state: &mut NetworkState, ui_tx: &mpsc::Sender<UiEvent>) {
    let before: HashSet<NodeId> = state.peers.iter().map(|(id, _)| *id).collect();
    state.peers.remove_expired();
    for node_id in before {
        if state.peers.get_peer(&node_id).is_none() {
            try_emit_ui_event(ui_tx, UiEvent::PeerLost { node_id });
        }
    }
}

fn connect_error_kind_from_reason(reason: &str) -> ConnectErrorKind {
    let code = reason.split(':').next().unwrap_or(reason).trim();
    ConnectErrorKind::from_reason_code(code)
}

fn emit_connection_error(ui_tx: &mpsc::Sender<UiEvent>, target: Option<NodeId>, reason: &str) {
    try_emit_ui_event(
        ui_tx,
        UiEvent::ConnectionError {
            target,
            kind: connect_error_kind_from_reason(reason),
        },
    );
}

fn emit_network_status(
    ui_tx: &mpsc::Sender<UiEvent>,
    kind: NetworkStatusKind,
    text: impl Into<String>,
) {
    try_emit_ui_event(
        ui_tx,
        UiEvent::NetworkStatus {
            kind,
            text: text.into(),
        },
    );
}

fn io_connect_reason(kind: std::io::ErrorKind) -> &'static str {
    match kind {
        std::io::ErrorKind::ConnectionRefused => "connection_refused",
        std::io::ErrorKind::TimedOut => "timeout",
        std::io::ErrorKind::ConnectionReset => "connection_reset",
        std::io::ErrorKind::HostUnreachable => "host_unreachable",
        std::io::ErrorKind::NetworkUnreachable => "network_unreachable",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        _ => "connect_error",
    }
}

// ---------------------------------------------------------------------------
// Network runtime — runs inside the tokio runtime on the dedicated thread
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_network_runtime(
    local_id: NodeId,
    display_name: Arc<RwLock<String>>,
    local_pubkey: [u8; 32],
    local_signing_key: SigningKey,
    net_state: Arc<Mutex<NetworkState>>,
    sessions: SessionRegistry,
    outgoing_rx: mpsc::Receiver<(NodeId, NetworkMessage)>,
    ui_event_tx: mpsc::Sender<UiEvent>,
    command_rx: mpsc::Receiver<NetworkCommand>,
    shutdown: watch::Receiver<bool>,
    mode: NetworkMode,
) {
    log::info!(
        "Network runtime started (node={}, mode={:?})",
        local_id,
        mode
    );

    // Defense in depth: the composition root (`ui::bridge::init_networking`)
    // already gates on `NetworkMode::Offline` and never constructs a
    // `NetworkManager` in that case. If `Offline` reaches here anyway,
    // refuse to create any socket and exit immediately -- the dedicated
    // OS thread that called this function simply ends.
    if mode == NetworkMode::Offline {
        log::error!(
            "run_network_runtime invoked with NetworkMode::Offline; this should have been \
             gated at the composition root. Refusing to create any network socket."
        );
        return;
    }

    let (session_cmd_tx, command_rx) = merge_command_channels(command_rx);
    let (scan_cmd_tx, scan_cmd_rx) = mpsc::channel::<()>(SCAN_COMMAND_CAPACITY);

    let bind_addr = session_bind_addr(mode);
    let bound_listener = bind_session_listener(bind_addr, mode, &ui_event_tx);

    let mut listener_handle = bound_listener.map(|listener| {
        spawn_session_listener(
            listener,
            local_id,
            display_name.clone(),
            local_pubkey,
            local_signing_key.clone(),
            session_cmd_tx.clone(),
            shutdown.clone(),
        )
    });

    // Discovery is supervised on its own task. Its JoinHandle is never
    // included in the shutdown `select!` below, so a discovery crash cannot
    // tear down the listener, router, or command loop. LoopbackTest and
    // Offline create no discovery sockets. Bind failure must not advertise
    // the fixed LAN port.
    if mode == NetworkMode::Lan && listener_handle.is_some() {
        spawn_discovery_supervisor(DiscoverySupervisorArgs {
            local_id,
            display_name: display_name.clone(),
            local_pubkey,
            net_state: net_state.clone(),
            sessions: sessions.clone(),
            ui_tx: ui_event_tx.clone(),
            connect_tx: session_cmd_tx.clone(),
            scan_rx: scan_cmd_rx,
            shutdown: shutdown.clone(),
        });
    } else {
        drop(scan_cmd_rx);
        if mode != NetworkMode::Lan {
            log::info!(
                "Discovery skipped for network mode {:?} (loopback-only, no LAN sockets)",
                mode
            );
        } else {
            log::warn!("Discovery skipped because the session listener did not bind");
        }
    }

    let mut router_handle = spawn_outgoing_router(sessions.clone(), outgoing_rx);
    let mut cmd_handle = spawn_command_processor(
        CommandCtx {
            local_id,
            display_name,
            local_pubkey,
            local_signing_key,
            net_state,
            sessions: sessions.clone(),
            ui_tx: ui_event_tx,
            session_cmd_tx,
            scan_tx: scan_cmd_tx,
            mode,
            in_flight: Arc::new(Mutex::new(HashSet::new())),
        },
        command_rx,
    );

    // The discovery supervisor is intentionally NOT part of this select.
    tokio::select! {
        _ = wait_optional_join(&mut listener_handle) => log::info!("TCP session listener exited"),
        _ = &mut router_handle => log::info!("Outgoing router exited"),
        _ = &mut cmd_handle => log::info!("Command processor exited"),
        _ = wait_for_shutdown(shutdown) => {
            log::info!("Shutdown signal received");
        }
    }

    for session in sessions.snapshot() {
        let _ = session.cancel_tx.send(true);
    }
    if let Some(handle) = listener_handle.as_mut() {
        handle.abort();
    }
    router_handle.abort();
    cmd_handle.abort();
    if let Some(handle) = listener_handle.take() {
        let _ = handle.await;
    }
    let _ = router_handle.await;
    let _ = cmd_handle.await;

    log::info!("Network runtime stopped");
}

async fn wait_optional_join(handle: &mut Option<JoinHandle<()>>) {
    match handle.as_mut() {
        Some(handle) => {
            let _ = handle.await;
        }
        None => std::future::pending::<()>().await,
    }
}

fn merge_command_channels(
    mut external_rx: mpsc::Receiver<NetworkCommand>,
) -> (mpsc::Sender<NetworkCommand>, mpsc::Receiver<NetworkCommand>) {
    let (session_cmd_tx, mut session_cmd_rx) =
        mpsc::channel::<NetworkCommand>(SESSION_COMMAND_CAPACITY);
    let (merged_cmd_tx, merged_cmd_rx) = mpsc::channel::<NetworkCommand>(MERGED_COMMAND_CAPACITY);

    let merger_ext = merged_cmd_tx.clone();
    tokio::spawn(async move {
        while let Some(cmd) = external_rx.recv().await {
            if merger_ext.send(cmd).await.is_err() {
                break;
            }
        }
    });

    tokio::spawn(async move {
        while let Some(cmd) = session_cmd_rx.recv().await {
            if merged_cmd_tx.send(cmd).await.is_err() {
                break;
            }
        }
    });

    (session_cmd_tx, merged_cmd_rx)
}

fn bind_session_listener(
    bind_addr: SocketAddr,
    mode: NetworkMode,
    ui_tx: &mpsc::Sender<UiEvent>,
) -> Option<tokio::net::TcpListener> {
    match bind_tcp_listener(bind_addr) {
        Ok(listener) => {
            let addr = listener.local_addr().unwrap_or(bind_addr);
            log::info!("TCP session listener bound on {}", addr);
            try_emit_ui_event(ui_tx, UiEvent::ListenerBound { addr });
            try_emit_ui_event(ui_tx, UiEvent::BindStatus(BindStatus::Bound { addr }));
            let kind = match mode {
                NetworkMode::Lan => NetworkStatusKind::Enabled,
                NetworkMode::LoopbackTest => NetworkStatusKind::LoopbackTest,
                NetworkMode::Offline => NetworkStatusKind::Offline,
            };
            emit_network_status(ui_tx, kind, format!("监听 {addr}"));
            Some(listener)
        }
        Err(error) => {
            log::error!(
                "Failed to bind TCP session listener on {}: {}",
                bind_addr,
                error
            );
            emit_bind_failure(bind_addr.port(), mode, ui_tx);
            None
        }
    }
}

fn emit_bind_failure(port: u16, mode: NetworkMode, ui_tx: &mpsc::Sender<UiEvent>) {
    try_emit_ui_event(ui_tx, UiEvent::ListenerFailed { port });
    try_emit_ui_event(ui_tx, UiEvent::BindStatus(BindStatus::BindFailed { port }));
    emit_connection_error(ui_tx, None, ConnectErrorKind::BindFailed.as_reason_code());
    let text = if mode == NetworkMode::Lan {
        format!(
            "网络端口 {} 被占用或不可用，局域网协作暂时无法使用，本机计算器仍可正常使用",
            port
        )
    } else {
        "网络端口无法监听，本机计算器仍可正常使用".to_string()
    };
    emit_network_status(ui_tx, NetworkStatusKind::ListenerUnavailable, text);
}

fn spawn_session_listener(
    listener: tokio::net::TcpListener,
    local_id: NodeId,
    display_name: Arc<RwLock<String>>,
    local_pubkey: [u8; 32],
    signing_key: SigningKey,
    cmd_tx: mpsc::Sender<NetworkCommand>,
    shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    let inbound_limiter = inbound_session_limiter();
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }

            let accept_result =
                tokio::time::timeout(std::time::Duration::from_secs(1), listener.accept()).await;

            match accept_result {
                Ok(Ok((stream, peer_addr))) => {
                    let permit = match inbound_limiter.clone().try_acquire_owned() {
                        Ok(permit) => permit,
                        Err(_) => {
                            log::warn!(
                                "Inbound session limit ({}) reached; rejecting {} before handshake",
                                MAX_INBOUND_SESSIONS,
                                peer_addr,
                            );
                            drop(stream);
                            continue;
                        }
                    };
                    stream.set_nodelay(true).unwrap_or_else(|e| {
                        log::warn!("set_nodelay failed on accepted stream: {e}");
                    });
                    log::info!("Accepted TCP session from {}", peer_addr);
                    let cmd_tx = cmd_tx.clone();
                    let name = read_display_name(&display_name);
                    let signing_key = signing_key.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        session::run_accepted_session(
                            stream,
                            peer_addr,
                            local_id,
                            name,
                            local_pubkey,
                            signing_key,
                            cmd_tx,
                        )
                        .await;
                    });
                }
                Ok(Err(e)) => {
                    log::warn!("TCP session accept error: {}", e);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(_) => {}
            }
        }
    })
}

struct DiscoverySupervisorArgs {
    local_id: NodeId,
    display_name: Arc<RwLock<String>>,
    local_pubkey: [u8; 32],
    net_state: Arc<Mutex<NetworkState>>,
    sessions: SessionRegistry,
    ui_tx: mpsc::Sender<UiEvent>,
    connect_tx: mpsc::Sender<NetworkCommand>,
    scan_rx: mpsc::Receiver<()>,
    shutdown: watch::Receiver<bool>,
}

enum DiscoveryLoopEnd {
    Shutdown,
    Failed(String),
}

fn spawn_discovery_supervisor(args: DiscoverySupervisorArgs) {
    tokio::spawn(async move {
        let DiscoverySupervisorArgs {
            local_id,
            display_name,
            local_pubkey,
            net_state,
            sessions,
            ui_tx,
            connect_tx,
            mut scan_rx,
            shutdown,
        } = args;
        let mut backoff = DISCOVERY_RESTART_BACKOFF_START;
        loop {
            if *shutdown.borrow() {
                break;
            }
            match run_discovery_loop(
                local_id,
                &display_name,
                local_pubkey,
                &net_state,
                &sessions,
                &ui_tx,
                &connect_tx,
                &mut scan_rx,
                shutdown.clone(),
            )
            .await
            {
                DiscoveryLoopEnd::Shutdown => break,
                DiscoveryLoopEnd::Failed(error) => {
                    log::warn!(
                        "Discovery ended ({error}); backing off {backoff:?} then restarting"
                    );
                    let _ = connect_tx.try_send(NetworkCommand::ScanFinished);
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = wait_for_shutdown(shutdown.clone()) => break,
                    }
                    backoff = backoff.saturating_mul(2).min(DISCOVERY_RESTART_BACKOFF_MAX);
                }
            }
        }
        log::info!("Discovery supervisor stopped");
    });
}

#[allow(clippy::too_many_arguments)]
async fn run_discovery_loop(
    local_id: NodeId,
    display_name: &Arc<RwLock<String>>,
    local_pubkey: [u8; 32],
    net_state: &Arc<Mutex<NetworkState>>,
    sessions: &SessionRegistry,
    ui_tx: &mpsc::Sender<UiEvent>,
    connect_tx: &mpsc::Sender<NetworkCommand>,
    scan_rx: &mut mpsc::Receiver<()>,
    mut shutdown: watch::Receiver<bool>,
) -> DiscoveryLoopEnd {
    let discovery = match DiscoveryService::new(
        local_id,
        read_display_name(display_name),
        SESSION_TCP_PORT,
        local_pubkey,
    )
    .await
    {
        Ok(discovery) => Arc::new(discovery),
        Err(error) => {
            return DiscoveryLoopEnd::Failed(format!("service unavailable: {error}"));
        }
    };

    let mut announce_interval = tokio::time::interval(std::time::Duration::from_secs(60));
    announce_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut endpoint_attempts: HashMap<NodeId, (SocketAddr, std::time::Instant)> = HashMap::new();
    let mut scan_open = true;

    loop {
        if *shutdown.borrow() {
            return DiscoveryLoopEnd::Shutdown;
        }

        tokio::select! {
            result = discovery.recv_mdns_endpoint() => {
                match result {
                    Ok(endpoint) => {
                        handle_discovered_endpoint(
                            endpoint,
                            net_state,
                            sessions,
                            ui_tx,
                            connect_tx,
                            &mut endpoint_attempts,
                        );
                    }
                    Err(error) => {
                        log::debug!("mDNS discovery receive error: {}", error);
                    }
                }
            }
            result = discovery.recv_announce() => {
                match result {
                    Ok((msg, udp_addr)) => {
                        if matches!(msg, DiscoveryMessage::Discover) {
                            let name = read_display_name(display_name);
                            let msg = discovery.announce_msg(&name);
                            if let Err(error) = discovery.announce(&msg).await {
                                log::warn!("Discovery reply-announce error: {}", error);
                            }
                            continue;
                        }
                        if let Some(endpoint) = discovery.endpoint_from_announcement(&msg, udp_addr) {
                            handle_discovered_endpoint(
                                endpoint,
                                net_state,
                                sessions,
                                ui_tx,
                                connect_tx,
                                &mut endpoint_attempts,
                            );
                        }
                    }
                    Err(error) => {
                        log::debug!("UDP recv error: {}", error);
                    }
                }
            }
            maybe_scan = scan_rx.recv(), if scan_open => {
                match maybe_scan {
                    Some(()) => {
                        let name = read_display_name(display_name);
                        if let Err(error) = discovery.update_display_name(&name) {
                            log::warn!("Discovery display-name update failed: {}", error);
                        }
                        let msg = discovery.announce_msg(&name);
                        if let Err(error) = discovery.announce(&DiscoveryMessage::Discover).await {
                            log::warn!("Discovery scan discover error: {}", error);
                        }
                        if let Err(error) = discovery.announce(&msg).await {
                            log::warn!("Discovery scan announce error: {}", error);
                        }
                        let _ = connect_tx.try_send(NetworkCommand::ScanFinished);
                    }
                    None => scan_open = false,
                }
            }
            _ = announce_interval.tick() => {
                let name = read_display_name(display_name);
                let msg = discovery.announce_msg(&name);
                if let Err(error) = discovery.announce(&msg).await {
                    log::warn!("Discovery announce error: {}", error);
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow_and_update() {
                    return DiscoveryLoopEnd::Shutdown;
                }
            }
        }
    }
}

fn handle_discovered_endpoint(
    endpoint: DiscoveryEndpoint,
    state: &Arc<Mutex<NetworkState>>,
    sessions: &SessionRegistry,
    ui_tx: &mpsc::Sender<UiEvent>,
    connect_tx: &mpsc::Sender<NetworkCommand>,
    attempts: &mut HashMap<NodeId, (SocketAddr, std::time::Instant)>,
) {
    let node_id = endpoint.node_id;
    let expected_peer = ExpectedPeerIdentity {
        node_id,
        public_key_fingerprint: endpoint.public_key_fingerprint.clone(),
    };
    let peer_addr = register_discovered_endpoint(endpoint, state, sessions, ui_tx);
    if should_attempt_discovered_session(attempts, node_id, peer_addr) {
        let _ = connect_tx.try_send(NetworkCommand::ConnectToPeer(OutboundConnectRequest {
            addr: peer_addr,
            expected_peer: Some(expected_peer),
            report_errors: false,
        }));
    }
}

fn spawn_outgoing_router(
    sessions: SessionRegistry,
    mut outgoing_rx: mpsc::Receiver<(NodeId, NetworkMessage)>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some((target_id, msg)) = outgoing_rx.recv().await {
            match sessions.get(target_id) {
                Some(session) => {
                    if session.sender.try_send(msg).is_err() {
                        log::trace!("Session {} closed; removing from registry", target_id);
                        sessions.cancel_generation(target_id, session.session_id);
                        sessions.remove_if_current(target_id, session.session_id);
                    }
                }
                None => {
                    log::trace!("No session for {}; dropping message", target_id);
                }
            }
        }
    })
}

struct CommandCtx {
    local_id: NodeId,
    display_name: Arc<RwLock<String>>,
    local_pubkey: [u8; 32],
    local_signing_key: SigningKey,
    net_state: Arc<Mutex<NetworkState>>,
    sessions: SessionRegistry,
    ui_tx: mpsc::Sender<UiEvent>,
    session_cmd_tx: mpsc::Sender<NetworkCommand>,
    scan_tx: mpsc::Sender<()>,
    mode: NetworkMode,
    in_flight: Arc<Mutex<HashSet<ConnectAttemptKey>>>,
}

fn spawn_command_processor(
    ctx: CommandCtx,
    mut command_rx: mpsc::Receiver<NetworkCommand>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Some(cmd) = command_rx.recv().await else {
                break;
            };
            handle_network_command(&ctx, cmd);
        }
    })
}

fn handle_network_command(ctx: &CommandCtx, cmd: NetworkCommand) {
    match cmd {
        NetworkCommand::RegisterSession(reg) => handle_register_session(ctx, reg),
        NetworkCommand::UnregisterSession {
            node_id,
            session_id,
        } => {
            handle_unregister_session(ctx, node_id, session_id);
        }
        NetworkCommand::IncomingMessage(sender_id, msg) => {
            forward_incoming_message_to_ui(&ctx.ui_tx, &ctx.sessions, sender_id, msg);
        }
        NetworkCommand::ConnectToPeer(request) => handle_connect_to_peer(ctx, request),
        NetworkCommand::UpdateLatency { node_id, ms } => handle_update_latency(ctx, node_id, ms),
        NetworkCommand::Scan => handle_scan(ctx),
        NetworkCommand::ScanFinished => {
            try_emit_ui_event(&ctx.ui_tx, UiEvent::ScanState(ScanState::Idle));
        }
    }
}

fn handle_register_session(ctx: &CommandCtx, reg: super::protocol::SessionRegister) {
    let super::protocol::SessionRegister {
        session_id,
        node_id,
        sender,
        info,
        direction,
        cancel_tx,
        decision_tx,
    } = reg;
    if decision_tx.is_closed() {
        log::trace!(
            "Ignoring expired registration for {} generation {}",
            node_id,
            session_id,
        );
        return;
    }
    let fingerprint_ok = {
        let state = ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
        match state.peers.get_peer(&node_id) {
            Some(peer) => match &peer.public_key_fingerprint {
                Some(expected) => {
                    let actual = public_key_fingerprint(&info.public_key);
                    if *expected != actual {
                        log::warn!(
                            "Session fingerprint mismatch for {}: expected {}, got {}",
                            node_id,
                            expected,
                            actual,
                        );
                        false
                    } else {
                        true
                    }
                }
                None => true,
            },
            None => true,
        }
    };
    if !fingerprint_ok {
        let _ = decision_tx.send(false);
        emit_connection_error(&ctx.ui_tx, Some(node_id), "fingerprint_mismatch");
        emit_peer_presence(
            &ctx.net_state,
            &ctx.sessions,
            node_id,
            PeerPresence::FingerprintMismatch,
            &ctx.ui_tx,
        );
        return;
    }
    log::info!(
        "Session registered: {} ({}) dir={:?}",
        info.display_name,
        node_id,
        direction,
    );
    let inserted = ctx.sessions.insert(
        ctx.local_id,
        node_id,
        ActiveSession {
            session_id,
            sender,
            direction,
            cancel_tx,
        },
    );
    if inserted.accepted {
        if let Some(existing) = inserted.replaced.as_ref() {
            log::info!(
                "Dedup: replacing session {} generation {} with {}",
                node_id,
                existing.session_id,
                inserted.session_id,
            );
        }
    } else {
        log::info!(
            "Dedup: rejecting duplicate session {} generation {} dir={:?}",
            node_id,
            session_id,
            direction,
        );
    }
    if decision_tx.send(inserted.accepted).is_err() {
        if inserted.accepted {
            if ctx.sessions.remove_if_current(node_id, inserted.session_id)
                && let Some(previous) = inserted.replaced
            {
                ctx.sessions.restore(node_id, previous);
            }
        }
        return;
    }
    if let Some(previous) = inserted.replaced {
        let _ = previous.cancel_tx.send(true);
    }
    {
        let mut state = ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
        state.peers.add_peer(info);
        state.is_connected = !ctx.sessions.is_empty();
        emit_peer_upsert(
            &state,
            &ctx.sessions,
            node_id,
            PeerPresence::Connected,
            &ctx.ui_tx,
        );
    }
    if inserted.accepted {
        try_emit_ui_event(
            &ctx.ui_tx,
            UiEvent::SessionEstablished {
                node_id,
                session_id: inserted.session_id,
            },
        );
    }
}

fn handle_unregister_session(ctx: &CommandCtx, node_id: NodeId, session_id: SessionId) {
    let (removed, has_sessions) = {
        let removed = ctx.sessions.remove_if_current(node_id, session_id);
        (removed, !ctx.sessions.is_empty())
    };
    if removed {
        log::info!(
            "Session unregistered: {} generation {}",
            node_id,
            session_id
        );
        let mut state = ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
        state.is_connected = has_sessions;
        emit_peer_upsert(
            &state,
            &ctx.sessions,
            node_id,
            PeerPresence::Nearby,
            &ctx.ui_tx,
        );
        try_emit_ui_event(
            &ctx.ui_tx,
            UiEvent::SessionLost {
                node_id,
                session_id,
            },
        );
    } else {
        log::trace!(
            "Ignoring stale unregister for {} generation {}",
            node_id,
            session_id,
        );
    }
}

fn handle_connect_to_peer(ctx: &CommandCtx, request: OutboundConnectRequest) {
    let addr = request.addr;
    let target = request.expected_peer.as_ref().map(|peer| peer.node_id);
    if !outbound_addr_allowed(ctx.mode, addr) {
        log::warn!(
            "Runtime refused outbound connection to {} in mode {:?}",
            addr,
            ctx.mode,
        );
        if request.report_errors {
            emit_connection_error(&ctx.ui_tx, target, "loopback_address_required");
        }
        return;
    }
    if request
        .expected_peer
        .as_ref()
        .is_some_and(|expected| ctx.sessions.contains(expected.node_id))
    {
        log::trace!("Peer already has an active session; skipping connect");
        return;
    }
    let attempt_key = ConnectAttemptKey::from_request(&request);
    {
        let mut in_flight = ctx.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        if in_flight.contains(&attempt_key) {
            log::info!("Connect to {} already in-flight, skipping", addr);
            return;
        }
        if in_flight.len() >= MAX_IN_FLIGHT_CONNECTS {
            log::warn!(
                "Outbound connect limit ({}) reached; rejecting {}",
                MAX_IN_FLIGHT_CONNECTS,
                addr,
            );
            if request.report_errors {
                emit_connection_error(&ctx.ui_tx, target, "connect_overloaded");
            }
            return;
        }
        in_flight.insert(attempt_key);
    }
    log::info!(
        "Connecting to peer at {} (target={:?})",
        addr,
        request.expected_peer.as_ref().map(|peer| peer.node_id),
    );
    if let Some(node_id) = target {
        emit_peer_presence(
            &ctx.net_state,
            &ctx.sessions,
            node_id,
            PeerPresence::Connecting,
            &ctx.ui_tx,
        );
    }

    let ses_tx = ctx.session_cmd_tx.clone();
    let name = read_display_name(&ctx.display_name);
    let id = ctx.local_id;
    let pubkey = ctx.local_pubkey;
    let signing_key = ctx.local_signing_key.clone();
    let incoming = ctx.ui_tx.clone();
    let in_flight = ctx.in_flight.clone();
    let sessions = ctx.sessions.clone();
    let net_state = ctx.net_state.clone();
    let expected_peer = request.expected_peer;
    let report_errors = request.report_errors;
    let mode = ctx.mode;

    tokio::spawn(async move {
        let connect_result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            connect_tcp_checked(mode, addr, tokio::net::TcpStream::connect),
        )
        .await;

        let failed_reason = match connect_result {
            Ok(Ok(stream)) => {
                stream.set_nodelay(true).unwrap_or_else(|e| {
                    log::warn!("set_nodelay failed on outgoing stream: {e}");
                });
                match session::run_connecting_session(
                    stream,
                    addr,
                    id,
                    name,
                    pubkey,
                    signing_key,
                    expected_peer.clone(),
                    ses_tx,
                )
                .await
                {
                    Ok(()) => None,
                    Err(error) => {
                        log::warn!("Session failed to {}: {}", addr, error);
                        Some(error)
                    }
                }
            }
            Ok(Err(error)) => {
                log::warn!("Failed to connect to {}: {}", addr, error);
                Some(io_connect_reason(error.kind()).to_string())
            }
            Err(_) => {
                log::warn!("Connect to {} timed out", addr);
                Some("timeout".to_string())
            }
        };

        if let Some(reason) = failed_reason {
            if report_errors {
                emit_connection_error(&incoming, target, &reason);
            }
            if let Some(node_id) = target {
                emit_peer_presence(
                    &net_state,
                    &sessions,
                    node_id,
                    PeerPresence::Unreachable,
                    &incoming,
                );
            }
        }
        if let Ok(mut in_flight) = in_flight.lock() {
            in_flight.remove(&attempt_key);
        }
    });
}

fn handle_update_latency(ctx: &CommandCtx, node_id: NodeId, ms: u32) {
    {
        let mut state = ctx.net_state.lock().unwrap_or_else(|e| e.into_inner());
        state.latency_ms = Some(ms);
    }
    try_emit_ui_event(
        &ctx.ui_tx,
        UiEvent::LatencyUpdate {
            node_id,
            latency_ms: Some(ms),
        },
    );
}

fn handle_scan(ctx: &CommandCtx) {
    try_emit_ui_event(&ctx.ui_tx, UiEvent::ScanState(ScanState::InFlight));
    if ctx.scan_tx.try_send(()).is_err() {
        try_emit_ui_event(&ctx.ui_tx, UiEvent::ScanState(ScanState::Idle));
    }
}

pub(crate) async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    // Level-triggered fast path: a cancellation sent before this future is
    // first polled is retained by the watch channel.
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow_and_update() {
            return;
        }
    }
}

fn try_emit_ui_event(ui_tx: &mpsc::Sender<UiEvent>, event: UiEvent) -> bool {
    match ui_tx.try_send(event) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            // Status/discovery events are best-effort snapshots. Drop newest
            // rather than letting any producer allocate without bound.
            log::warn!("UI event queue is full; dropping newest event");
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            log::trace!("UI event queue is closed");
            false
        }
    }
}

pub(crate) fn forward_incoming_message_to_ui(
    ui_tx: &mpsc::Sender<UiEvent>,
    sessions: &SessionRegistry,
    sender_id: NodeId,
    message: NetworkMessage,
) -> bool {
    match ui_tx.try_send(UiEvent::InboundMessage {
        sender: sender_id,
        message,
    }) {
        Ok(()) => true,
        Err(error) => {
            log::warn!("UI ingress queue rejected message from {sender_id}: {error}");
            if let Some(session) = sessions.get(sender_id) {
                sessions.cancel_generation(sender_id, session.session_id);
            }
            false
        }
    }
}

fn read_display_name(display_name: &Arc<RwLock<String>>) -> String {
    display_name
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn register_discovered_endpoint(
    endpoint: DiscoveryEndpoint,
    state: &Arc<Mutex<NetworkState>>,
    sessions: &SessionRegistry,
    ui_tx: &mpsc::Sender<UiEvent>,
) -> SocketAddr {
    let peer_addr = endpoint.address;
    let node_id = endpoint.node_id;
    let transport = endpoint.transport_hint;
    let fingerprint = endpoint.public_key_fingerprint.clone();

    {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        state.peers.add_peer(PeerInfo {
            node_id,
            display_name: endpoint.display_name,
            service_endpoint: Some(peer_addr),
            session_peer_addr: None,
            last_seen: std::time::Instant::now(),
            public_key: [0u8; 32],
            public_key_fingerprint: fingerprint.clone(),
        });
        prune_expired_peers(&mut state, ui_tx);
        if let Some(peer) = state.peers.get_peer(&node_id) {
            let session_id = sessions.get(node_id).map(|session| session.session_id);
            let presence = if session_id.is_some() {
                PeerPresence::Connected
            } else {
                PeerPresence::Nearby
            };
            try_emit_ui_event(
                ui_tx,
                UiEvent::PeerUpsert(peer_view_from_info(peer, presence, session_id, None)),
            );
        }
    }

    log::debug!(
        "Discovery endpoint via {:?}: {} at {}{}",
        transport,
        node_id,
        peer_addr,
        fingerprint
            .as_deref()
            .map(|fp| format!(" pkfp={fp}"))
            .unwrap_or_default(),
    );

    peer_addr
}

pub(crate) fn should_attempt_discovered_session(
    attempts: &mut HashMap<NodeId, (SocketAddr, std::time::Instant)>,
    node_id: NodeId,
    addr: SocketAddr,
) -> bool {
    let now = std::time::Instant::now();
    match attempts.get_mut(&node_id) {
        Some((last_addr, last_time))
            if *last_addr == addr
                && now.duration_since(*last_time)
                    < std::time::Duration::from_secs(DISCOVERY_ENDPOINT_RETRY_SECS) =>
        {
            false
        }
        Some((last_addr, last_time)) => {
            *last_addr = addr;
            *last_time = now;
            true
        }
        None => {
            if attempts.len() >= MAX_DISCOVERY_ENDPOINT_ATTEMPTS
                && let Some(oldest) = attempts
                    .iter()
                    .min_by_key(|(_, (_, seen_at))| *seen_at)
                    .map(|(node_id, _)| *node_id)
            {
                attempts.remove(&oldest);
            }
            attempts.insert(node_id, (addr, now));
            true
        }
    }
}

pub(crate) fn inbound_session_limiter() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(MAX_INBOUND_SESSIONS))
}

/// Create a production TCP listener bound to `addr`.
///
/// Windows uses `SO_EXCLUSIVEADDRUSE` so a second application instance cannot
/// share the fixed session endpoint. Other platforms retain `SO_REUSEADDR` for
/// quick restarts; it does not change the separate UDP discovery socket.
pub(crate) fn bind_tcp_listener(
    addr: SocketAddr,
) -> Result<tokio::net::TcpListener, std::io::Error> {
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    configure_tcp_bind_options(&socket)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    socket.set_nonblocking(true)?;
    let std_listener: std::net::TcpListener = socket.into();
    tokio::net::TcpListener::from_std(std_listener)
}

#[cfg(not(windows))]
fn configure_tcp_bind_options(socket: &Socket) -> std::io::Result<()> {
    socket.set_reuse_address(true)
}

#[cfg(windows)]
fn configure_tcp_bind_options(socket: &Socket) -> std::io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        SO_EXCLUSIVEADDRUSE, SOCKET_ERROR, SOL_SOCKET, WSAGetLastError, setsockopt,
    };

    let enabled: i32 = 1;
    // SAFETY: `socket` owns a valid Winsock SOCKET. `enabled` remains alive for
    // the duration of the call and its byte length matches the value passed to
    // `setsockopt`.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&enabled as *const i32).cast(),
            std::mem::size_of_val(&enabled) as i32,
        )
    };
    if result == SOCKET_ERROR {
        // SAFETY: WSAGetLastError has no preconditions and reads thread-local
        // Winsock error state set by the failed call above.
        Err(std::io::Error::from_raw_os_error(unsafe {
            WSAGetLastError()
        }))
    } else {
        Ok(())
    }
}
