use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::{Notify, mpsc};

use super::discovery::DiscoveryService;
use super::protocol::{
    ConnectionDirection, DiscoveryMessage, NetworkCommand, NetworkMessage, NodeId,
};
use super::session::{self, SessionSender};
use super::state::{NetworkState, PeerInfo};
use crate::ui::events::{PeerDiscoveryPayload, UiEvent};

/// Fixed port for both the TCP session listener and discovery.
///
/// Using a fixed port (like LocalSend's 53317) means peers always know which
/// port to connect to.  `SO_REUSEADDR` is set so the socket can rebind
/// quickly after a restart without waiting for the TIME_WAIT state to expire.
const FIXED_PORT: u16 = 42420;

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
    display_name: String,
    local_pubkey: [u8; 32],
    net_state: Arc<Mutex<NetworkState>>,
    sessions: Arc<Mutex<HashMap<NodeId, SessionSender>>>,
    mut outgoing_rx: mpsc::UnboundedReceiver<(NodeId, NetworkMessage)>,
    ui_event_tx: mpsc::UnboundedSender<UiEvent>,
    mut command_rx: mpsc::UnboundedReceiver<NetworkCommand>,
    shutdown: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
) {
    log::info!("Network runtime started (node={})", local_id);

    // -- Task 1: TCP session listener ----------------------------------------
    let listener_display = display_name.clone();
    let listener_id = local_id;
    let listener_shutdown = shutdown.clone();

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

    // Bind the session listener on a fixed port with SO_REUSEADDR so the
    // socket can rebind quickly after restarts (no TIME_WAIT delay).
    let bind_addr = SocketAddr::new(
        "0.0.0.0".parse().expect("valid constant address"),
        FIXED_PORT,
    );
    let session_listener = match bind_tcp_with_reuse(bind_addr) {
        Ok(l) => {
            log::info!("TCP session listener bound on 0.0.0.0:{}", FIXED_PORT);
            l
        }
        Err(e) => {
            log::error!(
                "Failed to bind TCP session listener on port {}: {}",
                FIXED_PORT,
                e
            );
            let _ = ui_event_tx.send(UiEvent::ConnectionError("bind_failed".to_string()));
            let _ = ui_event_tx.send(UiEvent::NetworkStatusUpdate("网络端口无法监听".to_string()));
            return;
        }
    };
    let session_port = FIXED_PORT;

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
                    let name = listener_display.clone();
                    let id = listener_id;
                    let pubkey = local_pubkey;

                    tokio::spawn(async move {
                        session::run_accepted_session(stream, peer_addr, id, name, pubkey, cmd_tx)
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

    // -- Task 2: Discovery (TCP-based, Localsend pattern) --------------------
    let discovery_display = display_name.clone();
    let discovery_state = net_state.clone();
    let discovery_id = local_id;
    let discovery_shutdown = shutdown.clone();
    let discovery_ui = ui_event_tx.clone();

    // Channel for discovered peers -> command processor (TCP connect).
    let (discovered_peer_tx, mut discovered_peer_rx) = mpsc::unbounded_channel::<SocketAddr>();

    let discovery_handle = tokio::spawn(async move {
        let discovery = match DiscoveryService::new(
            discovery_id,
            discovery_display.clone(),
            session_port,
        )
        .await
        {
            Ok(d) => Arc::new(d),
            Err(e) => {
                log::warn!("Discovery service unavailable: {}", e);
                return;
            }
        };

        let announce_msg = discovery.announce_msg().clone();

        // --- Regular announce interval (10 s) ---
        // The first tick fires IMMEDIATELY (tokio::time::interval semantics).
        let mut announce_interval = tokio::time::interval(std::time::Duration::from_secs(10));
        announce_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // --- Startup scan timer ---
        let mut startup_scan = tokio::time::interval(std::time::Duration::from_secs(3));
        startup_scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        startup_scan.tick().await; // Skip the first immediate tick.

        let mut last_tcp_recv = std::time::Instant::now();
        let mut warned_no_recv = false;

        loop {
            if discovery_shutdown.load(Ordering::Relaxed) {
                break;
            }

            tokio::select! {
                // -------------------------------------------------------
                // Path A: Incoming TCP connection from a peer who received
                // our UDP announcement and is connecting to confirm.
                // -------------------------------------------------------
                result = discovery.accept_peer() => {
                    match result {
                        Ok(exchange) => {
                            last_tcp_recv = std::time::Instant::now();
                            warned_no_recv = false;

                            if exchange.node_id != discovery_id {
                                let peer_addr = SocketAddr::new(
                                    exchange.peer_addr.ip(),
                                    exchange.session_port,
                                );
                                log::debug!(
                                    "TCP discovery (inbound): {} ({}) at {}",
                                    exchange.display_name,
                                    exchange.node_id,
                                    peer_addr,
                                );
                                {
                                    let mut state = discovery_state
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    let node_id = exchange.node_id;
                                    state.peers.add_peer(PeerInfo {
                                        node_id,
                                        display_name: exchange.display_name,
                                        address: peer_addr,
                                        tcp_port: exchange.session_port,
                                        last_seen: std::time::Instant::now(),
                                        public_key: [0u8; 32], // not available from discovery exchange
                                    });
                                    state.peers.remove_expired();
                                    if let Some(payload) = peer_payload(&state, &node_id, false) {
                                        let _ = discovery_ui.send(UiEvent::PeerDiscovered(payload));
                                    }
                                }
                                let _ = discovered_peer_tx.send(peer_addr);
                            }
                        }
                        Err(e) => {
                            log::debug!("TCP discovery accept error: {}", e);
                        }
                    }
                }

                // -------------------------------------------------------
                // Path B: UDP announcement from a peer.  Connect to their
                // TCP port to confirm they are alive (Localsend pattern).
                // -------------------------------------------------------
                result = discovery.recv_announce() => {
                    match result {
                        Ok((msg, udp_addr)) => {
                            let (node_id, name, peer_tcp_port) = match &msg {
                                DiscoveryMessage::AnnounceV2 {
                                    node_id,
                                    display_name,
                                    tcp_port,
                                    ..
                                }
                                | DiscoveryMessage::Announce {
                                    node_id,
                                    display_name,
                                    tcp_port,
                                    ..
                                } => (*node_id, display_name.clone(), *tcp_port),
                                DiscoveryMessage::Discover => {
                                    // Another node is scanning -- re-announce.
                                    let disc = discovery.clone();
                                    let msg = announce_msg.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = disc.announce(&msg).await {
                                            log::warn!("Discovery reply-announce error: {}", e);
                                        }
                                    });
                                    continue;
                                }
                            };

                            if node_id == discovery_id {
                                continue; // Ignore self.
                            }

                            // Dedup: skip outbound TCP if we already have this
                            // peer in the table (e.g. from an inbound connection
                            // or a previous announcement cycle).
                            {
                                let state = discovery_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                if let Some(existing) = state.peers.get_peer(&node_id) {
                                    log::debug!(
                                        "UDP announce from {} ({}) — already known at {}, skipping TCP connect-back",
                                        name, node_id, existing.address,
                                    );
                                    continue;
                                }
                            }

                            log::debug!(
                                "UDP announce from {} ({}) at {}, connecting to TCP port {}",
                                name, node_id, udp_addr.ip(), peer_tcp_port,
                            );

                            // Connect back to the peer via TCP to confirm.
                            // Use the IP from the UDP source address.
                            let peer_tcp_addr = SocketAddr::new(
                                udp_addr.ip(),
                                peer_tcp_port,
                            );
                            let local_announce = announce_msg.clone();
                            let state_clone = discovery_state.clone();
                            let peer_tx_clone = discovered_peer_tx.clone();
                            let ui_tx_clone = discovery_ui.clone();

                            tokio::spawn(async move {
                                match DiscoveryService::connect_and_exchange(
                                    peer_tcp_addr,
                                    &local_announce,
                                )
                                .await
                                {
                                    Ok(exchange) => {
                                        let peer_addr = SocketAddr::new(
                                            exchange.peer_addr.ip(),
                                            exchange.session_port,
                                        );
                                        log::debug!(
                                            "TCP discovery (outbound): {} ({}) at {}",
                                            exchange.display_name,
                                            exchange.node_id,
                                            peer_addr,
                                        );
                                        {
                                            let mut state = state_clone
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner());
                                            let node_id = exchange.node_id;
                                            state.peers.add_peer(PeerInfo {
                                                node_id,
                                                display_name: exchange.display_name,
                                                address: peer_addr,
                                                tcp_port: exchange.session_port,
                                                last_seen: std::time::Instant::now(),
                                                public_key: [0u8; 32], // not available from discovery exchange
                                            });
                                            state.peers.remove_expired();
                                            if let Some(payload) = peer_payload(&state, &node_id, false) {
                                                let _ = ui_tx_clone.send(UiEvent::PeerDiscovered(payload));
                                            }
                                        }
                                        let _ = peer_tx_clone.send(peer_addr);
                                    }
                                    Err(e) => {
                                        log::debug!(
                                            "TCP connect-back to {} failed: {}",
                                            peer_tcp_addr, e,
                                        );
                                    }
                                }
                            });
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
                    let msg = announce_msg.clone();
                    tokio::spawn(async move {
                        if let Err(e) = disc.announce(&msg).await {
                            log::warn!("Discovery scan announce error: {}", e);
                        }
                    });
                }

                // -------------------------------------------------------
                // Startup scan: announce 3s after launch.
                // -------------------------------------------------------
                _ = startup_scan.tick() => {
                    log::info!("Discovery startup scan");
                    let disc = discovery.clone();
                    let msg = announce_msg.clone();
                    tokio::spawn(async move {
                        if let Err(e) = disc.announce(&msg).await {
                            log::warn!("Discovery startup announce error: {}", e);
                        }
                    });
                }

                // -------------------------------------------------------
                // Periodic announce.
                // -------------------------------------------------------
                _ = announce_interval.tick() => {
                    let disc = discovery.clone();
                    let msg = announce_msg.clone();
                    tokio::spawn(async move {
                        if let Err(e) = disc.announce(&msg).await {
                            log::warn!("Discovery announce error: {}", e);
                        }
                    });
                    if last_tcp_recv.elapsed() > std::time::Duration::from_secs(60)
                        && !warned_no_recv
                    {
                        log::info!(
                            "Discovery: no TCP connections in 60s -- \
                             check firewall and network settings"
                        );
                        warned_no_recv = true;
                    }
                }
            }
        }
    });

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
                                state.is_connected = true;
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
                                cmd_sessions
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .remove(&node_id);
                                let mut state = cmd_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                state.peers.remove(&node_id);
                                state.is_connected = !state.peers.is_empty();
                                let _ = cmd_incoming.send(UiEvent::SessionLost(node_id.to_string()));
                                let _ = cmd_incoming.send(UiEvent::PeerLost(node_id.to_string()));
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
                            // If a peer is only reachable through an
                            // intermediate node (asymmetric topology), it
                            // will receive the full matrix via RoutingSync
                            // on its next connection.
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
                            let name = cmd_display.clone();
                            let id = cmd_id;
                            let pubkey = local_pubkey;
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
                                            stream, addr, id, name, pubkey, ses_tx,
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
                    let name = cmd_display.clone();
                    let id = cmd_id;
                    let pubkey = local_pubkey;

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
                                    stream, peer_addr, id, name, pubkey, ses_tx,
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

    // -- Wait for shutdown or all tasks to exit -------------------------
    tokio::select! {
        _ = listener_handle => log::info!("TCP session listener exited"),
        _ = discovery_handle => log::info!("Discovery exited"),
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

/// Create a `tokio::net::TcpListener` bound to `addr` with `SO_REUSEADDR`.
///
/// The socket is created via `socket2` so that socket options can be set
/// before `bind()`.  After binding and listening the socket is switched to
/// non-blocking mode and converted into a tokio listener.
fn bind_tcp_with_reuse(addr: SocketAddr) -> Result<tokio::net::TcpListener, std::io::Error> {
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
