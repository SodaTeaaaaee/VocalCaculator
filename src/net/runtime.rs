use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use ed25519_dalek::SigningKey;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::{Notify, mpsc};

use super::discovery::{DiscoveryEndpoint, DiscoveryService, public_key_fingerprint};
use super::protocol::{
    ConnectionDirection, DiscoveryMessage, NetworkCommand, NetworkMessage, NodeId, SESSION_TCP_PORT,
};
use super::session::{self, SessionSender};
use super::state::{NetworkState, PeerInfo};
use crate::app::network_mode::NetworkMode;
use crate::ui::events::{PeerDiscoveryPayload, UiEvent};

const DISCOVERY_ENDPOINT_RETRY_SECS: u64 = 30;

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
            "0.0.0.0".parse().expect("valid constant address"),
            SESSION_TCP_PORT,
        ),
        NetworkMode::LoopbackTest => {
            SocketAddr::new("127.0.0.1".parse().expect("valid constant address"), 0)
        }
        NetworkMode::Offline => {
            SocketAddr::new("127.0.0.1".parse().expect("valid constant address"), 0)
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
                    address: format!("{}:{}", peer.address.ip(), peer.tcp_port),
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
    sessions: Arc<Mutex<HashMap<NodeId, SessionSender>>>,
    mut outgoing_rx: mpsc::UnboundedReceiver<(NodeId, NetworkMessage)>,
    ui_event_tx: mpsc::UnboundedSender<UiEvent>,
    mut command_rx: mpsc::UnboundedReceiver<NetworkCommand>,
    shutdown: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
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
    let (session_cmd_tx, session_cmd_rx) = mpsc::unbounded_channel::<NetworkCommand>();
    let (merged_cmd_tx, merged_cmd_rx) = mpsc::unbounded_channel::<NetworkCommand>();

    // Forward external commands (from NetworkHandle) into the merged channel.
    let merger_ext = merged_cmd_tx.clone();
    tokio::spawn(async move {
        while let Some(cmd) = command_rx.recv().await {
            if merger_ext.send(cmd).is_err() {
                break;
            }
        }
    });

    // Forward session commands into the merged channel.
    let merger_ses = merged_cmd_tx;
    tokio::spawn(async move {
        let mut rx = session_cmd_rx;
        while let Some(cmd) = rx.recv().await {
            if merger_ses.send(cmd).is_err() {
                break;
            }
        }
    });

    // All commands now flow into merged_cmd_rx.
    let mut command_rx = merged_cmd_rx;

    // Scan signal channel: command processor -> discovery task.
    let (scan_cmd_tx, mut scan_cmd_rx) = mpsc::unbounded_channel::<()>();

    // Clone the session command sender for the listener task.
    let listener_cmd_tx = session_cmd_tx.clone();

    // In `Lan` mode this binds the fixed session port on all interfaces so
    // peers can reach it without an out-of-band port exchange; in
    // `LoopbackTest` it binds an OS-assigned ephemeral port on loopback only.
    let bind_addr = session_bind_addr(mode);
    let session_listener = match bind_tcp_with_reuse(bind_addr) {
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
            let _ = ui_event_tx.send(UiEvent::ConnectionError("bind_failed".to_string()));
            let status = if mode == NetworkMode::Lan {
                format!(
                    "网络端口 {} 被占用或不可用，局域网协作暂时无法使用，本机计算器仍可正常使用",
                    SESSION_TCP_PORT
                )
            } else {
                "网络端口无法监听，本机计算器仍可正常使用".to_string()
            };
            let _ = ui_event_tx.send(UiEvent::NetworkStatusUpdate(status));
            return;
        }
    };
    let session_port = match session_listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            log::error!("Failed to read assigned session port: {}", e);
            let _ = ui_event_tx.send(UiEvent::ConnectionError("bind_failed".to_string()));
            let _ = ui_event_tx.send(UiEvent::NetworkStatusUpdate("网络端口无法监听".to_string()));
            return;
        }
    };
    // Diagnostic only: in `Lan` mode this always equals `SESSION_TCP_PORT`
    // (the bind itself requested that fixed port); in `LoopbackTest` it is
    // the OS-assigned ephemeral loopback port, which is never advertised
    // since discovery does not run in that mode.
    log::debug!("Session listener local port resolved to {}", session_port);

    let listener_handle = tokio::spawn(async move {
        let listener = session_listener;

        loop {
            if listener_shutdown.load(Ordering::Relaxed) {
                break;
            }

            let accept_result =
                tokio::time::timeout(std::time::Duration::from_secs(1), listener.accept()).await;

            match accept_result {
                Ok(Ok((stream, peer_addr))) => {
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

    // Channel for discovered peers -> command processor (TCP connect).
    let (discovered_peer_tx, mut discovered_peer_rx) = mpsc::unbounded_channel::<SocketAddr>();

    if mode != NetworkMode::Lan {
        log::info!(
            "Discovery skipped for network mode {:?} (loopback-only, no LAN sockets)",
            mode
        );
        drop(discovered_peer_tx);
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
                if discovery_shutdown.load(Ordering::Relaxed) {
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
                                let peer_addr = register_discovered_endpoint(
                                    endpoint,
                                    &discovery_state,
                                    &discovery_ui,
                                );
                                if should_attempt_discovered_session(&mut endpoint_attempts, node_id, peer_addr) {
                                    let _ = discovered_peer_tx.send(peer_addr);
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
                                    let disc = discovery.clone();
                                    let name = read_display_name(&discovery_display);
                                    let msg = disc.announce_msg(&name);
                                    tokio::spawn(async move {
                                        if let Err(e) = disc.announce(&msg).await {
                                            log::warn!("Discovery reply-announce error: {}", e);
                                        }
                                    });
                                    continue;
                                }

                                if let Some(endpoint) = discovery.endpoint_from_announcement(&msg, udp_addr) {
                                    let node_id = endpoint.node_id;
                                    let peer_addr = register_discovered_endpoint(
                                        endpoint,
                                        &discovery_state,
                                        &discovery_ui,
                                    );
                                    if should_attempt_discovered_session(&mut endpoint_attempts, node_id, peer_addr) {
                                        let _ = discovered_peer_tx.send(peer_addr);
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
                        let disc = discovery.clone();
                        let name = read_display_name(&discovery_display);
                        if let Err(e) = disc.update_display_name(&name) {
                            log::warn!("Discovery display-name update failed: {}", e);
                        }
                        let msg = disc.announce_msg(&name);
                        tokio::spawn(async move {
                            if let Err(e) = disc.announce(&DiscoveryMessage::Discover).await {
                                log::warn!("Discovery scan discover error: {}", e);
                            }
                            if let Err(e) = disc.announce(&msg).await {
                                log::warn!("Discovery scan announce error: {}", e);
                            }
                        });
                    }

                    // -------------------------------------------------------
                    // Low-rate UDP fallback announce.
                    // -------------------------------------------------------
                    _ = announce_interval.tick() => {
                        let disc = discovery.clone();
                        let name = read_display_name(&discovery_display);
                        let msg = disc.announce_msg(&name);
                        tokio::spawn(async move {
                            if let Err(e) = disc.announce(&msg).await {
                                log::warn!("Discovery announce error: {}", e);
                            }
                        });
                    }
                }
            }

            log::warn!("Discovery task exited");
        });
    }

    // -- Task 3: Outgoing message router ------------------------------------
    let router_sessions = sessions.clone();

    let router_handle = tokio::spawn(async move {
        while let Some((target_id, msg)) = outgoing_rx.recv().await {
            let sender = {
                router_sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&target_id)
                    .cloned()
            };
            match sender {
                Some(tx) => {
                    if tx.send(msg).is_err() {
                        log::trace!("Session {} closed; removing from registry", target_id);
                        router_sessions
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&target_id);
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

    // Track sessions that were replaced by dedup so their
    // UnregisterSession doesn't remove the winning session.
    let mut replaced_sessions: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    // Track in-flight connect attempts to prevent duplicate TCP connects.
    let in_flight_connects: Arc<Mutex<std::collections::HashSet<SocketAddr>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));

    let cmd_handle = tokio::spawn(async move {
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
                            let node_id = reg.node_id;
                            let fingerprint_ok = {
                                let state = cmd_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                match state.peers.get_peer(&node_id) {
                                    Some(peer) => match &peer.public_key_fingerprint {
                                        Some(expected) => {
                                            let actual =
                                                public_key_fingerprint(&reg.info.public_key);
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
                                drop(reg.sender);
                                let _ = cmd_incoming.send(UiEvent::ConnectionError(
                                    "fingerprint_mismatch".to_string(),
                                ));
                                continue;
                            }
                            log::info!(
                                "Session registered: {} ({}) dir={:?}",
                                reg.info.display_name,
                                node_id,
                                reg.direction,
                            );
                            // Dedup: if a session already exists for this node,
                            // apply NodeId-ordered tie-break to decide which
                            // connection survives. Lower NodeId keeps its
                            // outbound connection; higher NodeId keeps inbound.
                            {
                                let mut sessions = cmd_sessions
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                if sessions.contains_key(&node_id) {
                                    let keep_new = if cmd_id < node_id {
                                        // We are lower: keep outbound, reject inbound.
                                        reg.direction == ConnectionDirection::Outbound
                                    } else {
                                        // We are higher: keep inbound, reject outbound.
                                        reg.direction == ConnectionDirection::Inbound
                                    };
                                    if keep_new {
                                        log::info!(
                                            "Dedup: replacing session for {} (we are {:?})",
                                            node_id, reg.direction,
                                        );
                                        // Remove the old sender from the map.
                                        // The session task holds its own clone,
                                        // but removing ours reduces the refcount.
                                        // The old session will detect channel
                                        // closure on its next send attempt.
                                        sessions.remove(&node_id);
                                        sessions.insert(node_id, reg.sender);
                                    } else {
                                        log::info!(
                                            "Dedup: rejecting duplicate session for {} (we are {:?})",
                                            node_id, reg.direction,
                                        );
                                        // Mark this session as replaced so its
                                        // UnregisterSession won't remove the
                                        // winning session's entry.
                                        replaced_sessions.insert(node_id);
                                        // Drop the new sender — the new session
                                        // task will detect channel closure and exit.
                                        drop(reg.sender);
                                        // Still update the peer info.
                                        let mut state = cmd_state
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        state.peers.add_peer(reg.info);
                                        if let Some(payload) = peer_payload(&state, &node_id, true) {
                                            let _ = cmd_incoming.send(UiEvent::PeerDiscovered(payload));
                                        }
                                        continue;
                                    }
                                } else {
                                    // No existing session — insert directly.
                                    sessions.insert(node_id, reg.sender);
                                }
                            }
                            {
                                let mut state = cmd_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                state.peers.add_peer(reg.info);
                                state.is_connected = !cmd_sessions
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .is_empty();
                                if let Some(payload) = peer_payload(&state, &node_id, true) {
                                    let _ = cmd_incoming.send(UiEvent::PeerDiscovered(payload));
                                }
                            }
                            let _ = cmd_incoming.send(UiEvent::SessionEstablished(node_id.to_string()));
                        }
                        NetworkCommand::UnregisterSession(node_id) => {
                            // If this session was replaced by dedup, skip
                            // removal — the winning session is still active.
                            if replaced_sessions.remove(&node_id) {
                                log::info!(
                                    "Session unregistered (replaced by dedup): {}",
                                    node_id,
                                );
                                // Don't remove from sessions map — the winning
                                // session's entry is still valid.
                            } else {
                                log::info!("Session unregistered: {}", node_id);
                                let has_sessions = {
                                    let mut sessions = cmd_sessions
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    sessions.remove(&node_id);
                                    !sessions.is_empty()
                                };
                                let mut state = cmd_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                state.is_connected = has_sessions;
                                if let Some(payload) = peer_payload(&state, &node_id, false) {
                                    let _ = cmd_incoming.send(UiEvent::PeerDiscovered(payload));
                                }
                                let _ = cmd_incoming.send(UiEvent::SessionLost(node_id.to_string()));
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
                            match bincode::serde::encode_to_vec(&msg, bincode::config::standard()) {
                                Ok(bytes) => {
                                    let _ = cmd_incoming.send(
                                        UiEvent::NetworkMessage(sender_id.to_string(), bytes),
                                    );
                                }
                                Err(e) => {
                                    log::warn!("Failed to serialize NetworkMessage for UiEvent: {}", e);
                                }
                            }
                        }
                        NetworkCommand::ConnectToPeer(addr, target_node_id) => {
                            // Dedup: skip if a connect to this addr is already in-flight.
                            {
                                let mut in_flight = in_flight_connects
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                if in_flight.contains(&addr) {
                                    log::info!("Connect to {} already in-flight, skipping", addr);
                                    continue;
                                }
                                in_flight.insert(addr);
                            }
                            log::info!("Connecting to peer at {} (target={:?})", addr, target_node_id);
                            let ses_tx = cmd_session_tx.clone();
                            let name = read_display_name(&cmd_display);
                            let id = cmd_id;
                            let pubkey = local_pubkey;
                            let signing_key = cmd_signing_key.clone();
                            let incoming = cmd_incoming.clone();
                            let in_flight = in_flight_connects.clone();

                            tokio::spawn(async move {
                                // TCP connect with 5-second timeout.
                                let connect_result = tokio::time::timeout(
                                    std::time::Duration::from_secs(5),
                                    tokio::net::TcpStream::connect(addr),
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
                                            ses_tx,
                                        )
                                        .await {
                                            log::warn!("Session failed to {}: {}", addr, e);
                                            let _ = incoming.send(
                                                UiEvent::ConnectionError(e),
                                            );
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
                                        let _ = incoming.send(
                                            UiEvent::ConnectionError(reason),
                                        );
                                    }
                                    Err(_) => {
                                        log::warn!("Connect to {} timed out", addr);
                                        let _ = incoming.send(
                                            UiEvent::ConnectionError("timeout".to_string()),
                                        );
                                    }
                                }
                                // Remove from in-flight tracking.
                                if let Ok(mut s) = in_flight.lock() {
                                    s.remove(&addr);
                                }
                            });
                        }
                        NetworkCommand::UpdateLatency(ms) => {
                            let mut state = cmd_state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            state.latency_ms = Some(ms);
                            for (node_id, _) in state.peers.iter() {
                                let _ = cmd_incoming.send(
                                    UiEvent::LatencyUpdate(node_id.to_string(), ms as i32),
                                );
                            }
                        }
                        NetworkCommand::Scan => {
                            let _ = cmd_scan_tx.send(());
                        }
                    }
                }

                // Discovered peers from the discovery task -> establish sessions.
                Some(peer_addr) = discovered_peer_rx.recv() => {
                    log::info!("Discovery: establishing session with peer at {}", peer_addr);
                    let ses_tx = cmd_session_tx.clone();
                    let name = read_display_name(&cmd_display);
                    let id = cmd_id;
                    let pubkey = local_pubkey;
                    let signing_key = cmd_signing_key.clone();

                    tokio::spawn(async move {
                        // TCP connect with 5-second timeout (same as user-initiated).
                        let connect_result = tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            tokio::net::TcpStream::connect(peer_addr),
                        )
                        .await;

                        match connect_result {
                            Ok(Ok(stream)) => {
                                stream.set_nodelay(true).unwrap_or_else(|e| {
                                    log::warn!("set_nodelay failed on discovered peer stream: {e}");
                                });
                                // Discovery connections don't set pending_control_request,
                                // so we don't need to propagate errors to the UI.
                                let _ = session::run_connecting_session(
                                    stream,
                                    peer_addr,
                                    id,
                                    name,
                                    pubkey,
                                    signing_key,
                                    ses_tx,
                                )
                                .await;
                            }
                            Ok(Err(e)) => {
                                log::debug!(
                                    "Failed to connect to discovered peer {}: {}",
                                    peer_addr, e,
                                );
                            }
                            Err(_) => {
                                log::debug!(
                                    "Connect to discovered peer {} timed out",
                                    peer_addr,
                                );
                            }
                        }
                    });
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
        _ = listener_handle => log::info!("TCP session listener exited"),
        _ = router_handle => log::info!("Outgoing router exited"),
        _ = cmd_handle => log::info!("Command processor exited"),
        _ = wait_for_shutdown(shutdown, shutdown_notify) => {
            log::info!("Shutdown signal received");
        }
    }

    log::info!("Network runtime stopped");
}

async fn wait_for_shutdown(flag: Arc<AtomicBool>, notify: Arc<Notify>) {
    // Fast path: flag was already set before we started waiting.
    if flag.load(Ordering::Relaxed) {
        return;
    }
    // Park until the shutdown() call fires notify_waiters().
    // Loop to handle spurious wakeups.
    loop {
        notify.notified().await;
        if flag.load(Ordering::Relaxed) {
            return;
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
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
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
            address: peer_addr,
            tcp_port: endpoint.session_port,
            last_seen: std::time::Instant::now(),
            public_key: [0u8; 32],
            public_key_fingerprint: fingerprint.clone(),
        });
        state.peers.remove_expired();
        if let Some(payload) = peer_payload(&state, &node_id, false) {
            let _ = ui_tx.send(UiEvent::PeerDiscovered(payload));
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

fn should_attempt_discovered_session(
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
            attempts.insert(node_id, (addr, now));
            true
        }
    }
}

/// Create a `tokio::net::TcpListener` bound to `addr` with `SO_REUSEADDR`.
///
/// The socket is created via `socket2` so that socket options can be set
/// before `bind()`.  After binding and listening the socket is switched to
/// non-blocking mode and converted into a tokio listener.
pub(crate) fn bind_tcp_with_reuse(
    addr: SocketAddr,
) -> Result<tokio::net::TcpListener, std::io::Error> {
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    // NOTE: set_reuse_port() does not exist on Windows for TCP and is not
    // needed here — SO_REUSEADDR alone is sufficient to allow quick rebind.
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    socket.set_nonblocking(true)?;
    let std_listener: std::net::TcpListener = socket.into();
    tokio::net::TcpListener::from_std(std_listener)
}
