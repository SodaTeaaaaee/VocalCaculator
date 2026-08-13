use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};

use ed25519_dalek::SigningKey;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::{Semaphore, mpsc, watch};

use super::discovery::{DiscoveryEndpoint, DiscoveryService, public_key_fingerprint};
use super::protocol::{
    ConnectionDirection, DiscoveryMessage, ExpectedPeerIdentity, NetworkCommand, NetworkMessage,
    NodeId, OutboundConnectRequest, SESSION_TCP_PORT, SessionId,
};
use super::session::{self, ActiveSession};
use super::state::{NetworkState, PeerInfo};
use crate::app::network_mode::NetworkMode;
use crate::ui::events::{PeerDiscoveryPayload, UiEvent};

const DISCOVERY_ENDPOINT_RETRY_SECS: u64 = 30;
const MAX_DISCOVERY_ENDPOINT_ATTEMPTS: usize = 256;
const MAX_INBOUND_SESSIONS: usize = 16;
const MAX_IN_FLIGHT_CONNECTS: usize = 32;
const SESSION_COMMAND_CAPACITY: usize = 256;
const MERGED_COMMAND_CAPACITY: usize = 512;
const SCAN_COMMAND_CAPACITY: usize = 1;

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

fn peer_payload(
    state: &NetworkState,
    node_id: &NodeId,
    is_connected: bool,
) -> Option<PeerDiscoveryPayload> {
    state
        .peers
        .iter()
        .enumerate()
        .find_map(|(index, (id, peer))| {
            if id == node_id {
                Some(PeerDiscoveryPayload {
                    name: peer.display_name.clone(),
                    address: peer
                        .display_endpoint()
                        .map(|address| address.to_string())
                        .unwrap_or_default(),
                    is_connected,
                    latency_ms: state.latency_ms.map(|v| v as i32).unwrap_or(-1),
                    index: index as i32,
                    node_id_string: id.to_string(),
                })
            } else {
                None
            }
        })
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
    sessions: Arc<Mutex<HashMap<NodeId, ActiveSession>>>,
    mut outgoing_rx: mpsc::Receiver<(NodeId, NetworkMessage)>,
    ui_event_tx: mpsc::Sender<UiEvent>,
    mut command_rx: mpsc::Receiver<NetworkCommand>,
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

    // -- Task 1: TCP session listener ----------------------------------------
    let listener_display = display_name.clone();
    let listener_id = local_id;
    let listener_shutdown = shutdown.clone();
    let listener_signing_key = local_signing_key.clone();

    // Session tasks send commands back to the runtime through this channel.
    // It is merged with the external command_rx below.
    let (session_cmd_tx, session_cmd_rx) =
        mpsc::channel::<NetworkCommand>(SESSION_COMMAND_CAPACITY);
    let (merged_cmd_tx, merged_cmd_rx) = mpsc::channel::<NetworkCommand>(MERGED_COMMAND_CAPACITY);

    // Forward external commands (from NetworkHandle) into the merged channel.
    let merger_ext = merged_cmd_tx.clone();
    tokio::spawn(async move {
        while let Some(cmd) = command_rx.recv().await {
            if merger_ext.send(cmd).await.is_err() {
                break;
            }
        }
    });

    // Forward session commands into the merged channel.
    let merger_ses = merged_cmd_tx;
    tokio::spawn(async move {
        let mut rx = session_cmd_rx;
        while let Some(cmd) = rx.recv().await {
            if merger_ses.send(cmd).await.is_err() {
                break;
            }
        }
    });

    // All commands now flow into merged_cmd_rx.
    let mut command_rx = merged_cmd_rx;

    // Scan signal channel: command processor -> discovery task.
    let (scan_cmd_tx, mut scan_cmd_rx) = mpsc::channel::<()>(SCAN_COMMAND_CAPACITY);

    // Clone the session command sender for the listener task.
    let listener_cmd_tx = session_cmd_tx.clone();
    let inbound_limiter = inbound_session_limiter();

    // In `Lan` mode this binds the fixed session port on all interfaces so
    // peers can reach it without an out-of-band port exchange; in
    // `LoopbackTest` it binds an OS-assigned ephemeral port on loopback only.
    let bind_addr = session_bind_addr(mode);
    let session_listener = match bind_tcp_listener(bind_addr) {
        Ok(l) => {
            match l.local_addr() {
                Ok(addr) => log::info!("TCP session listener bound on {}", addr),
                Err(e) => log::warn!("TCP session listener bound; local_addr failed: {}", e),
            }
            l
        }
        Err(e) => {
            log::error!(
                "Failed to bind TCP session listener on {}: {}",
                bind_addr,
                e
            );
            try_emit_ui_event(
                &ui_event_tx,
                UiEvent::ConnectionError("bind_failed".to_string()),
            );
            let status = if mode == NetworkMode::Lan {
                format!(
                    "网络端口 {} 被占用或不可用，局域网协作暂时无法使用，本机计算器仍可正常使用",
                    SESSION_TCP_PORT
                )
            } else {
                "网络端口无法监听，本机计算器仍可正常使用".to_string()
            };
            try_emit_ui_event(&ui_event_tx, UiEvent::NetworkStatusUpdate(status));
            return;
        }
    };
    let session_port = match session_listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            log::error!("Failed to read assigned session port: {}", e);
            try_emit_ui_event(
                &ui_event_tx,
                UiEvent::ConnectionError("bind_failed".to_string()),
            );
            try_emit_ui_event(
                &ui_event_tx,
                UiEvent::NetworkStatusUpdate("网络端口无法监听".to_string()),
            );
            return;
        }
    };
    // Diagnostic only: in `Lan` mode this always equals `SESSION_TCP_PORT`
    // (the bind itself requested that fixed port); in `LoopbackTest` it is
    // the OS-assigned ephemeral loopback port, which is never advertised
    // since discovery does not run in that mode.
    log::debug!("Session listener local port resolved to {}", session_port);

    let mut listener_handle = tokio::spawn(async move {
        let listener = session_listener;

        loop {
            if *listener_shutdown.borrow() {
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
                    // Disable Nagle's algorithm — every button press is a
                    // small message and Nagle would buffer it for up to 200ms.
                    stream.set_nodelay(true).unwrap_or_else(|e| {
                        log::warn!("set_nodelay failed on accepted stream: {e}");
                    });
                    log::info!("Accepted TCP session from {}", peer_addr);
                    let cmd_tx = listener_cmd_tx.clone();
                    let name = read_display_name(&listener_display);
                    let id = listener_id;
                    let pubkey = local_pubkey;
                    let signing_key = listener_signing_key.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        session::run_accepted_session(
                            stream,
                            peer_addr,
                            id,
                            name,
                            pubkey,
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
                Err(_) => {
                    // Timeout -- loop back to check shutdown flag.
                }
            }
        }
    });

    // -- Task 2: Discovery endpoint publishing/resolution -------------------
    // Discovery is intentionally decoupled from the rest of the runtime's
    // lifetime -- see the final `select!` below, which no longer includes
    // this task's `JoinHandle`. It is also skipped entirely outside `Lan`
    // mode: `LoopbackTest` must not create any mDNS daemon, multicast
    // socket, or other non-loopback socket.
    let discovery_display = display_name.clone();
    let discovery_state = net_state.clone();
    let discovery_id = local_id;
    let discovery_shutdown = shutdown.clone();
    let discovery_ui = ui_event_tx.clone();

    // Discovery and user-triggered connects share one command path so mode
    // policy, active-session dedup, and expected-identity checks cannot drift.
    let discovery_connect_tx = session_cmd_tx.clone();

    if mode != NetworkMode::Lan {
        log::info!(
            "Discovery skipped for network mode {:?} (loopback-only, no LAN sockets)",
            mode
        );
    } else {
        tokio::spawn(async move {
            let discovery = match DiscoveryService::new(
                discovery_id,
                read_display_name(&discovery_display),
                SESSION_TCP_PORT,
                local_pubkey,
            )
            .await
            {
                Ok(d) => Arc::new(d),
                Err(e) => {
                    log::warn!("Discovery service unavailable: {}", e);
                    return;
                }
            };

            // Low-rate fallback announce. The first tick fires immediately and
            // sends a short burst; later ticks keep stale UDP-only peers refreshed
            // without constant LAN chatter.
            let mut announce_interval = tokio::time::interval(std::time::Duration::from_secs(60));
            announce_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let mut endpoint_attempts: HashMap<NodeId, (SocketAddr, std::time::Instant)> =
                HashMap::new();

            loop {
                if *discovery_shutdown.borrow() {
                    break;
                }

                tokio::select! {
                    // -------------------------------------------------------
                    // Path A: mDNS/DNS-SD resolved endpoint.
                    // -------------------------------------------------------
                    result = discovery.recv_mdns_endpoint() => {
                        match result {
                            Ok(endpoint) => {
                                let node_id = endpoint.node_id;
                                let expected_peer = ExpectedPeerIdentity {
                                    node_id,
                                    public_key_fingerprint: endpoint.public_key_fingerprint.clone(),
                                };
                                let peer_addr = register_discovered_endpoint(
                                    endpoint,
                                    &discovery_state,
                                    &discovery_ui,
                                );
                                if should_attempt_discovered_session(&mut endpoint_attempts, node_id, peer_addr) {
                                    let _ = discovery_connect_tx.try_send(NetworkCommand::ConnectToPeer(
                                        OutboundConnectRequest {
                                            addr: peer_addr,
                                            expected_peer: Some(expected_peer),
                                            report_errors: false,
                                        },
                                    ));
                                }
                            }
                            Err(e) => {
                                log::debug!("mDNS discovery receive error: {}", e);
                            }
                        }
                    }

                    // -------------------------------------------------------
                    // Path B: UDP multicast fallback announcement.
                    // -------------------------------------------------------
                    result = discovery.recv_announce() => {
                        match result {
                            Ok((msg, udp_addr)) => {
                                if matches!(msg, DiscoveryMessage::Discover) {
                                    let name = read_display_name(&discovery_display);
                                    let msg = discovery.announce_msg(&name);
                                    // Keep unauthenticated discovery replies
                                    // serialized inside this single task. A
                                    // LAN packet flood must not create an
                                    // unbounded number of child futures.
                                    if let Err(e) = discovery.announce(&msg).await {
                                        log::warn!("Discovery reply-announce error: {}", e);
                                    }
                                    continue;
                                }

                                if let Some(endpoint) = discovery.endpoint_from_announcement(&msg, udp_addr) {
                                    let node_id = endpoint.node_id;
                                    let expected_peer = ExpectedPeerIdentity {
                                        node_id,
                                        public_key_fingerprint: endpoint.public_key_fingerprint.clone(),
                                    };
                                    let peer_addr = register_discovered_endpoint(
                                        endpoint,
                                        &discovery_state,
                                        &discovery_ui,
                                    );
                                    if should_attempt_discovered_session(&mut endpoint_attempts, node_id, peer_addr) {
                                        let _ = discovery_connect_tx.try_send(NetworkCommand::ConnectToPeer(
                                            OutboundConnectRequest {
                                                addr: peer_addr,
                                                expected_peer: Some(expected_peer),
                                                report_errors: false,
                                            },
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                log::debug!("UDP recv error: {}", e);
                            }
                        }
                    }

                    // -------------------------------------------------------
                    // Scan command from the command processor.
                    // -------------------------------------------------------
                    _ = scan_cmd_rx.recv() => {
                        let name = read_display_name(&discovery_display);
                        if let Err(e) = discovery.update_display_name(&name) {
                            log::warn!("Discovery display-name update failed: {}", e);
                        }
                        let msg = discovery.announce_msg(&name);
                        if let Err(e) = discovery.announce(&DiscoveryMessage::Discover).await {
                            log::warn!("Discovery scan discover error: {}", e);
                        }
                        if let Err(e) = discovery.announce(&msg).await {
                            log::warn!("Discovery scan announce error: {}", e);
                        }
                    }

                    // -------------------------------------------------------
                    // Low-rate UDP fallback announce.
                    // -------------------------------------------------------
                    _ = announce_interval.tick() => {
                        let name = read_display_name(&discovery_display);
                        let msg = discovery.announce_msg(&name);
                        if let Err(e) = discovery.announce(&msg).await {
                            log::warn!("Discovery announce error: {}", e);
                        }
                    }
                }
            }

            log::warn!("Discovery task exited");
        });
    }

    // -- Task 3: Outgoing message router ------------------------------------
    let router_sessions = sessions.clone();

    let mut router_handle = tokio::spawn(async move {
        while let Some((target_id, msg)) = outgoing_rx.recv().await {
            let active_session = {
                router_sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&target_id)
                    .cloned()
            };
            match active_session {
                Some(session) => {
                    if session.sender.try_send(msg).is_err() {
                        log::trace!("Session {} closed; removing from registry", target_id);
                        let _ = session.cancel_tx.send(true);
                        remove_session_if_current(
                            &mut router_sessions.lock().unwrap_or_else(|e| e.into_inner()),
                            target_id,
                            session.session_id,
                        );
                    }
                }
                None => {
                    log::trace!("No session for {}; dropping message", target_id);
                }
            }
        }
    });

    // -- Task 4: Command processor ------------------------------------------
    let cmd_sessions = sessions.clone();
    let cmd_incoming = ui_event_tx.clone();
    let cmd_display = display_name.clone();
    let cmd_id = local_id;
    let cmd_signing_key = local_signing_key.clone();
    let cmd_state = net_state.clone();
    let cmd_session_tx = session_cmd_tx;
    let cmd_scan_tx = scan_cmd_tx;

    // Track in-flight connect attempts to prevent duplicate TCP connects.
    let in_flight_connects: Arc<Mutex<std::collections::HashSet<ConnectAttemptKey>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));

    let mut cmd_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                // Commands from the NetworkHandle and session tasks.
                cmd = command_rx.recv() => {
                    let cmd = match cmd {
                        Some(c) => c,
                        None => break,
                    };
                    match cmd {
                        NetworkCommand::RegisterSession(reg) => {
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
                                continue;
                            }
                            let fingerprint_ok = {
                                let state = cmd_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                match state.peers.get_peer(&node_id) {
                                    Some(peer) => match &peer.public_key_fingerprint {
                                        Some(expected) => {
                                            let actual =
                                                public_key_fingerprint(&info.public_key);
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
                                try_emit_ui_event(
                                    &cmd_incoming,
                                    UiEvent::ConnectionError("fingerprint_mismatch".to_string()),
                                );
                                continue;
                            }
                            log::info!(
                                "Session registered: {} ({}) dir={:?}",
                                info.display_name,
                                node_id,
                                direction,
                            );
                            let (accepted, replaced_session) = {
                                let mut sessions = cmd_sessions
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                let keep_new = sessions
                                    .get(&node_id)
                                    .map(|existing| {
                                        should_replace_session(
                                            cmd_id,
                                            node_id,
                                            existing.direction,
                                            direction,
                                        )
                                    })
                                    .unwrap_or(true);
                                if keep_new {
                                    if let Some(existing) = sessions.get(&node_id) {
                                        log::info!(
                                            "Dedup: replacing session {} generation {} with {}",
                                            node_id,
                                            existing.session_id,
                                            session_id,
                                        );
                                    }
                                    let replaced = sessions.insert(
                                        node_id,
                                        ActiveSession {
                                            session_id,
                                            sender,
                                            direction,
                                            cancel_tx,
                                        },
                                    );
                                    (true, replaced)
                                } else {
                                    log::info!(
                                        "Dedup: rejecting duplicate session {} generation {} dir={:?}",
                                        node_id,
                                        session_id,
                                        direction,
                                    );
                                    (false, None)
                                }
                            };
                            if decision_tx.send(accepted).is_err() {
                                if accepted {
                                    let mut sessions = cmd_sessions
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    if sessions
                                        .get(&node_id)
                                        .is_some_and(|session| session.session_id == session_id)
                                    {
                                        sessions.remove(&node_id);
                                        if let Some(previous) = replaced_session {
                                            sessions.insert(node_id, previous);
                                        }
                                    }
                                }
                                continue;
                            }
                            if let Some(previous) = replaced_session {
                                let _ = previous.cancel_tx.send(true);
                            }
                            {
                                let mut state = cmd_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                state.peers.add_peer(info);
                                state.is_connected = !cmd_sessions
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .is_empty();
                                if let Some(payload) = peer_payload(&state, &node_id, true) {
                                    try_emit_ui_event(
                                        &cmd_incoming,
                                        UiEvent::PeerDiscovered(payload),
                                    );
                                }
                            }
                            if accepted {
                                try_emit_ui_event(
                                    &cmd_incoming,
                                    UiEvent::SessionEstablished(node_id.to_string()),
                                );
                            }
                        }
                        NetworkCommand::UnregisterSession { node_id, session_id } => {
                            let (removed, has_sessions) = {
                                let mut sessions = cmd_sessions
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                let removed = remove_session_if_current(
                                    &mut sessions,
                                    node_id,
                                    session_id,
                                );
                                (removed, !sessions.is_empty())
                            };
                            if removed {
                                log::info!("Session unregistered: {} generation {}", node_id, session_id);
                                let mut state = cmd_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                state.is_connected = has_sessions;
                                if let Some(payload) = peer_payload(&state, &node_id, false) {
                                    try_emit_ui_event(
                                        &cmd_incoming,
                                        UiEvent::PeerDiscovered(payload),
                                    );
                                }
                                try_emit_ui_event(
                                    &cmd_incoming,
                                    UiEvent::SessionLost(node_id.to_string()),
                                );
                            } else {
                                log::trace!(
                                    "Ignoring stale unregister for {} generation {}",
                                    node_id,
                                    session_id,
                                );
                            }
                        }
                        NetworkCommand::IncomingMessage(sender_id, msg) => {
                            // NOTE: Gossip forwarding of RoutingDelta and
                            // RouteRevoke was intentionally removed.  The
                            // originating node already broadcasts these to
                            // all its connected peers via
                            // `broadcast_routing_delta`.  Forwarding them
                            // again here created an infinite amplification
                            // loop in 3+ node topologies because there was
                            // no message-ID / TTL / dedup mechanism.
                            // Asymmetric topologies are repaired by
                            // owner-signed RoutingRowAnnounce messages:
                            // an intermediate node may relay another
                            // owner's row, and receivers verify the owner
                            // signature before applying it. RoutingSync is
                            // still sender-row-only and is not trusted for
                            // third-party rows.
                            // Serialize the message and forward as a UiEvent
                            // so the UI event loop can dispatch it.
                            forward_incoming_message_to_ui(
                                &cmd_incoming,
                                &cmd_sessions,
                                sender_id,
                                msg,
                            );
                        }
                        NetworkCommand::ConnectToPeer(request) => {
                            let addr = request.addr;
                            if !outbound_addr_allowed(mode, addr) {
                                log::warn!(
                                    "Runtime refused outbound connection to {} in mode {:?}",
                                    addr,
                                    mode,
                                );
                                if request.report_errors {
                                    try_emit_ui_event(
                                        &cmd_incoming,
                                        UiEvent::ConnectionError(
                                            "loopback_address_required".to_string(),
                                        ),
                                    );
                                }
                                continue;
                            }
                            if request.expected_peer.as_ref().is_some_and(|expected| {
                                cmd_sessions
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .contains_key(&expected.node_id)
                            }) {
                                log::trace!("Peer already has an active session; skipping connect");
                                continue;
                            }
                            let attempt_key = ConnectAttemptKey::from_request(&request);
                            {
                                let mut in_flight = in_flight_connects
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                if in_flight.contains(&attempt_key) {
                                    log::info!("Connect to {} already in-flight, skipping", addr);
                                    continue;
                                }
                                if in_flight.len() >= MAX_IN_FLIGHT_CONNECTS {
                                    log::warn!(
                                        "Outbound connect limit ({}) reached; rejecting {}",
                                        MAX_IN_FLIGHT_CONNECTS,
                                        addr,
                                    );
                                    if request.report_errors {
                                        try_emit_ui_event(
                                            &cmd_incoming,
                                            UiEvent::ConnectionError(
                                                "connect_overloaded".to_string(),
                                            ),
                                        );
                                    }
                                    continue;
                                }
                                in_flight.insert(attempt_key);
                            }
                            log::info!(
                                "Connecting to peer at {} (target={:?})",
                                addr,
                                request.expected_peer.as_ref().map(|peer| peer.node_id),
                            );
                            let ses_tx = cmd_session_tx.clone();
                            let name = read_display_name(&cmd_display);
                            let id = cmd_id;
                            let pubkey = local_pubkey;
                            let signing_key = cmd_signing_key.clone();
                            let incoming = cmd_incoming.clone();
                            let in_flight = in_flight_connects.clone();
                            let expected_peer = request.expected_peer;
                            let report_errors = request.report_errors;

                            tokio::spawn(async move {
                                // TCP connect with 5-second timeout.
                                let connect_result = tokio::time::timeout(
                                    std::time::Duration::from_secs(5),
                                    connect_tcp_checked(
                                        mode,
                                        addr,
                                        tokio::net::TcpStream::connect,
                                    ),
                                )
                                .await;

                                match connect_result {
                                    Ok(Ok(stream)) => {
                                        stream.set_nodelay(true).unwrap_or_else(|e| {
                                            log::warn!("set_nodelay failed on outgoing stream: {e}");
                                        });
                                        if let Err(e) = session::run_connecting_session(
                                            stream,
                                            addr,
                                            id,
                                            name,
                                            pubkey,
                                            signing_key,
                                            expected_peer,
                                            ses_tx,
                                        )
                                        .await {
                                            log::warn!("Session failed to {}: {}", addr, e);
                                            if report_errors {
                                                try_emit_ui_event(
                                                    &incoming,
                                                    UiEvent::ConnectionError(e),
                                                );
                                            }
                                        }
                                    }
                                    Ok(Err(e)) => {
                                        log::warn!("Failed to connect to {}: {}", addr, e);
                                        let reason = match e.kind() {
                                            std::io::ErrorKind::ConnectionRefused => "connection_refused",
                                            std::io::ErrorKind::TimedOut => "timeout",
                                            std::io::ErrorKind::ConnectionReset => "connection_reset",
                                            std::io::ErrorKind::HostUnreachable => "host_unreachable",
                                            std::io::ErrorKind::NetworkUnreachable => "network_unreachable",
                                            std::io::ErrorKind::PermissionDenied => "permission_denied",
                                            _ => "connect_error",
                                        }.to_string();
                                        if report_errors {
                                            try_emit_ui_event(
                                                &incoming,
                                                UiEvent::ConnectionError(reason),
                                            );
                                        }
                                    }
                                    Err(_) => {
                                        log::warn!("Connect to {} timed out", addr);
                                        if report_errors {
                                            try_emit_ui_event(
                                                &incoming,
                                                UiEvent::ConnectionError("timeout".to_string()),
                                            );
                                        }
                                    }
                                }
                                // Remove from in-flight tracking.
                                if let Ok(mut s) = in_flight.lock() {
                                    s.remove(&attempt_key);
                                }
                            });
                        }
                        NetworkCommand::UpdateLatency(ms) => {
                            let mut state = cmd_state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            state.latency_ms = Some(ms);
                            for (node_id, _) in state.peers.iter() {
                                try_emit_ui_event(
                                    &cmd_incoming,
                                    UiEvent::LatencyUpdate(node_id.to_string(), ms as i32),
                                );
                            }
                        }
                        NetworkCommand::Scan => {
                            let _ = cmd_scan_tx.try_send(());
                        }
                    }
                }

            }
        }
    });

    // -- Wait for shutdown or a core task to exit ------------------------
    // The discovery task is intentionally NOT part of this select: its
    // JoinHandle is detached (see above) so a discovery failure or exit
    // never tears down the listener/router/command-processor tasks or the
    // local calculator's ability to keep running.
    tokio::select! {
        _ = &mut listener_handle => log::info!("TCP session listener exited"),
        _ = &mut router_handle => log::info!("Outgoing router exited"),
        _ = &mut cmd_handle => log::info!("Command processor exited"),
        _ = wait_for_shutdown(shutdown) => {
            log::info!("Shutdown signal received");
        }
    }

    for session in sessions.lock().unwrap_or_else(|e| e.into_inner()).values() {
        let _ = session.cancel_tx.send(true);
    }
    listener_handle.abort();
    router_handle.abort();
    cmd_handle.abort();
    let _ = listener_handle.await;
    let _ = router_handle.await;
    let _ = cmd_handle.await;

    log::info!("Network runtime stopped");
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
    // Sender closure also means the owning manager/runtime is gone.
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
    sessions: &Arc<Mutex<HashMap<NodeId, ActiveSession>>>,
    sender_id: NodeId,
    message: NetworkMessage,
) -> bool {
    let bytes = match bincode::serde::encode_to_vec(
        &message,
        bincode::config::standard().with_limit::<4096>(),
    ) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!("Failed to serialize NetworkMessage for UiEvent: {error}");
            return false;
        }
    };
    match ui_tx.try_send(UiEvent::NetworkMessage(sender_id.to_string(), bytes)) {
        Ok(()) => true,
        Err(error) => {
            // Inbound messages are not best-effort status snapshots. If the
            // bounded UI queue cannot accept one, disconnect the producing
            // peer so it cannot continuously consume the pre-rate-limit
            // ingress budget.
            log::warn!("UI ingress queue rejected message from {sender_id}: {error}");
            if let Some(session) = sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&sender_id)
                .cloned()
            {
                let _ = session.cancel_tx.send(true);
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
        state.peers.remove_expired();
        if let Some(payload) = peer_payload(&state, &node_id, false) {
            try_emit_ui_event(ui_tx, UiEvent::PeerDiscovered(payload));
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

pub(crate) fn should_replace_session(
    local_id: NodeId,
    remote_id: NodeId,
    existing_direction: ConnectionDirection,
    new_direction: ConnectionDirection,
) -> bool {
    let preferred = if local_id < remote_id {
        ConnectionDirection::Outbound
    } else {
        ConnectionDirection::Inbound
    };
    new_direction == preferred && existing_direction != preferred
}

pub(crate) fn remove_session_if_current(
    sessions: &mut HashMap<NodeId, ActiveSession>,
    node_id: NodeId,
    session_id: SessionId,
) -> bool {
    if sessions
        .get(&node_id)
        .is_some_and(|session| session.session_id == session_id)
    {
        sessions.remove(&node_id);
        true
    } else {
        false
    }
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
