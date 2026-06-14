//! Per-connection session lifecycle: handshake dispatch, bidirectional
//! message relay, heartbeat, and teardown.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::net::protocol::{
    ConnectionDirection, NetworkCommand, NetworkMessage, NodeId, SessionRegister,
    PROTOCOL_MAGIC, HEARTBEAT_INTERVAL_SECS, HEARTBEAT_TIMEOUT_SECS,
};
use crate::net::state::PeerInfo;

use super::handshake::{server_handshake, client_handshake};

/// Outgoing-message channel sender: the Router pushes messages here,
/// and the session task forwards them over the TCP wire.
pub type SessionSender = mpsc::UnboundedSender<NetworkMessage>;

/// Framed TCP stream with length-delimited codec.
pub(super) type FramedStream = Framed<TcpStream, LengthDelimitedCodec>;

// Public entry points

/// Run a session for an **accepted** (inbound) TCP connection.
///
/// The server side of the handshake: receives `Hello`, replies with
/// `HelloAck`, then waits for `Subscribe`.
pub(crate) async fn run_accepted_session(
    stream: TcpStream,
    peer_addr: std::net::SocketAddr,
    local_node_id: NodeId,
    local_display_name: String,
    command_tx: mpsc::UnboundedSender<NetworkCommand>,
) {
    let framed = Framed::new(stream, LengthDelimitedCodec::new());

    // -- Server-side handshake ------------------------------------------
    let (remote_node_id, remote_display_name, mut framed) =
        match server_handshake(framed, local_node_id, &local_display_name).await {
            Ok(r) => r,
            Err(e) => {
                log::warn!("Inbound handshake failed from {}: {}", peer_addr, e);
                return;
            }
        };

    log::info!(
        "Inbound session established: {} ({}) from {}",
        remote_display_name,
        remote_node_id,
        peer_addr,
    );

    // Wait for Subscribe before entering steady-state.
    match recv_msg(&mut framed).await {
        Ok(Some(NetworkMessage::Subscribe)) => {
            log::trace!("Received Subscribe from {}", remote_node_id);
        }
        Ok(other) => {
            log::warn!(
                "Expected Subscribe from {}, got {:?}; dropping",
                remote_node_id,
                other,
            );
            return;
        }
        Err(e) => {
            log::warn!("Failed reading Subscribe from {}: {}", remote_node_id, e);
            return;
        }
    }

    let info = PeerInfo {
        node_id: remote_node_id,
        display_name: remote_display_name,
        address: peer_addr,
        tcp_port: peer_addr.port(),
        last_seen: std::time::Instant::now(),
    };

    run_session_loop(framed, remote_node_id, command_tx, info, ConnectionDirection::Inbound).await;
}

/// Run a session for an **outgoing** (client-initiated) TCP connection.
///
/// The client side of the handshake: sends `Hello`, waits for `HelloAck`,
/// then sends `Subscribe`.
pub(crate) async fn run_connecting_session(
    stream: TcpStream,
    peer_addr: std::net::SocketAddr,
    local_node_id: NodeId,
    local_display_name: String,
    command_tx: mpsc::UnboundedSender<NetworkCommand>,
) -> Result<(), String> {
    let framed = Framed::new(stream, LengthDelimitedCodec::new());

    // -- Client-side handshake with 8-second timeout --------------------
    let (remote_node_id, remote_display_name, mut framed) =
        match tokio::time::timeout(
            std::time::Duration::from_secs(8),
            client_handshake(framed, local_node_id, &local_display_name),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                log::warn!("Outbound handshake failed to {}: {}", peer_addr, e);
                return Err(format!("handshake: {}", e));
            }
            Err(_) => {
                log::warn!("Outbound handshake timed out to {}", peer_addr);
                return Err("handshake_timeout".to_string());
            }
        };

    log::info!(
        "Outbound session established: {} ({}) to {}",
        remote_display_name,
        remote_node_id,
        peer_addr,
    );

    // Send Subscribe to start receiving state updates.
    if let Err(e) = send_msg(&mut framed, &NetworkMessage::Subscribe).await {
        log::warn!("Failed sending Subscribe to {}: {}", remote_node_id, e);
        return Err(format!("subscribe: {}", e));
    }

    let info = PeerInfo {
        node_id: remote_node_id,
        display_name: remote_display_name,
        address: peer_addr,
        tcp_port: peer_addr.port(),
        last_seen: std::time::Instant::now(),
    };

    run_session_loop(framed, remote_node_id, command_tx, info, ConnectionDirection::Outbound).await;
    Ok(())
}

// Session main loop

/// Split the framed stream, spawn the heartbeat task, and run the
/// bidirectional message relay until the connection closes or times out.
async fn run_session_loop(
    framed: FramedStream,
    remote_id: NodeId,
    command_tx: mpsc::UnboundedSender<NetworkCommand>,
    info: PeerInfo,
    direction: ConnectionDirection,
) {
    let (mut writer, mut reader) = framed.split();
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<NetworkMessage>();

    // Register with the NetworkManager.
    let _ = command_tx.send(NetworkCommand::RegisterSession(SessionRegister {
        node_id: remote_id,
        sender: outgoing_tx.clone(),
        info: info.clone(),
        direction,
    }));

    // Shared heartbeat timestamp (seconds elapsed since reference, monotonic).
    let last_pong = Arc::new(AtomicU64::new(0));
    // Timestamp (ms since epoch) of when the last Ping was sent, for RTT calculation.
    let last_ping_sent = Arc::new(AtomicU64::new(0));

    // -- Heartbeat task --------------------------------------------------
    let hb_last_pong = last_pong.clone();
    let hb_last_ping = last_ping_sent.clone();
    let hb_outgoing = outgoing_tx.clone();
    let hb_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            HEARTBEAT_INTERVAL_SECS,
        ));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            let elapsed = hb_last_pong.load(Ordering::Relaxed);
            if elapsed > HEARTBEAT_TIMEOUT_SECS {
                log::warn!(
                    "Heartbeat timeout for {} ({}s since last pong)",
                    remote_id,
                    elapsed,
                );
                break;
            }
            // Record the send time for RTT calculation.
            let send_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            hb_last_ping.store(send_time, Ordering::Relaxed);
            if hb_outgoing.send(NetworkMessage::Ping).is_err() {
                break; // session ended
            }
        }
    });

    // -- Pong tracker task -----------------------------------------------
    // Periodically increments the elapsed-since-last-pong counter.
    let tracker_last_pong = last_pong.clone();
    let tracker_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            tracker_last_pong.fetch_add(1, Ordering::Relaxed);
        }
    });

    // -- Bidirectional relay ---------------------------------------------
    let mut relay_error = false;

    loop {
        tokio::select! {
            // Incoming TCP -> process or forward to Router
            result = reader.next() => {
                match result {
                    Some(Ok(bytes)) => {
                        // Protocol magic must be verified before any deserialization.
                        if bytes.len() < PROTOCOL_MAGIC.len()
                            || bytes[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC
                        {
                            log::warn!(
                                "Invalid protocol magic from {}; closing session",
                                remote_id,
                            );
                            relay_error = true;
                            break;
                        }
                        match bincode::serde::decode_from_slice::<NetworkMessage, _>(
                            &bytes[PROTOCOL_MAGIC.len()..],
                            bincode::config::standard(),
                        ) {
                            Ok((msg, _)) => {
                                if !handle_incoming_message(
                                    msg,
                                    remote_id,
                                    &command_tx,
                                    &outgoing_tx,
                                    &last_pong,
                                    &last_ping_sent,
                                )
                                .await
                                {
                                    relay_error = true;
                                    break;
                                }
                            }
                            Err(e) => {
                                log::warn!("Decode error from {}: {}", remote_id, e);
                                relay_error = true;
                                break;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        log::warn!("TCP read error from {}: {}", remote_id, e);
                        relay_error = true;
                        break;
                    }
                    None => {
                        log::info!("Connection closed by remote {}", remote_id);
                        break;
                    }
                }
            }
            // Outgoing from Router -> TCP
            Some(msg) = outgoing_rx.recv() => {
                if let Err(e) = send_msg(&mut writer, &msg).await {
                    log::warn!("TCP send error to {}: {}", remote_id, e);
                    relay_error = true;
                    break;
                }
            }
            else => {
                // Both channels closed — session ended.
                break;
            }
        }
    }

    // -- Cleanup ---------------------------------------------------------
    hb_handle.abort();
    tracker_handle.abort();

    if relay_error {
        log::info!("Session with {} ended (error)", remote_id);
    } else {
        log::info!("Session with {} ended (clean)", remote_id);
    }

    let _ = command_tx.send(NetworkCommand::UnregisterSession(remote_id));
}

// Message handling inside the session loop

/// Process one incoming message. Returns `false` if the session should close.
async fn handle_incoming_message(
    msg: NetworkMessage,
    remote_id: NodeId,
    command_tx: &mpsc::UnboundedSender<NetworkCommand>,
    outgoing_tx: &mpsc::UnboundedSender<NetworkMessage>,
    last_pong: &Arc<AtomicU64>,
    last_ping_sent: &Arc<AtomicU64>,
) -> bool {
    match msg {
        NetworkMessage::Ping => {
            // Respond directly; no Router involvement needed.
            if outgoing_tx.send(NetworkMessage::Pong).is_err() {
                return false;
            }
        }
        NetworkMessage::Pong => {
            // Reset the heartbeat timer.
            last_pong.store(0, Ordering::Relaxed);
            // Calculate round-trip latency from the last Ping send time.
            let ping_sent = last_ping_sent.load(Ordering::Relaxed);
            if ping_sent > 0 {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let rtt = now.saturating_sub(ping_sent) as u32;
                let _ = command_tx.send(NetworkCommand::UpdateLatency(rtt));
            }
        }
        NetworkMessage::Hello { .. } | NetworkMessage::HelloAck { .. } => {
            log::warn!(
                "Received spurious handshake message from {} in steady state; ignoring",
                remote_id,
            );
        }
        _ => {
            // Forward to the NetworkManager -> Router bridge.
            let _ = command_tx.send(NetworkCommand::IncomingMessage(remote_id, msg));
        }
    }
    true
}

// Wire-level helpers (thin wrappers around the framed codec)

/// Serialize and send a [`NetworkMessage`] with [`PROTOCOL_MAGIC`] prefix.
pub(super) async fn send_msg<S>(
    writer: &mut S,
    msg: &NetworkMessage,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: futures::Sink<tokio_util::bytes::Bytes> + Unpin,
    S::Error: std::error::Error + 'static,
{
    let bincode_bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard())?;
    let mut payload = Vec::with_capacity(PROTOCOL_MAGIC.len() + bincode_bytes.len());
    payload.extend_from_slice(&PROTOCOL_MAGIC);
    payload.extend_from_slice(&bincode_bytes);
    writer.send(tokio_util::bytes::Bytes::from(payload)).await?;
    Ok(())
}

/// Receive a frame, verify [`PROTOCOL_MAGIC`], and deserialize a
/// [`NetworkMessage`]. Returns `Ok(None)` on clean close.
pub(super) async fn recv_msg(
    reader: &mut FramedStream,
) -> Result<Option<NetworkMessage>, Box<dyn std::error::Error>> {
    match reader.next().await {
        Some(Ok(bytes)) => {
            if bytes.len() < PROTOCOL_MAGIC.len()
                || bytes[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC
            {
                return Err("Invalid protocol magic bytes".into());
            }
            let (msg, _) = bincode::serde::decode_from_slice(
                &bytes[PROTOCOL_MAGIC.len()..],
                bincode::config::standard(),
            )?;
            Ok(Some(msg))
        }
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}



