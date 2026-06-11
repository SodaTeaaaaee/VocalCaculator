//! Per-connection session lifecycle.
//!
//! Each TCP connection (inbound or outbound) is managed by a [`run_session`]
//! task that handles the full protocol lifecycle:
//!
//! 1. **Handshake** — Hello / HelloAck / Subscribe exchange
//! 2. **Steady state** — bidirectional message relay + heartbeat
//! 3. **Teardown** — connection close, channel drain, cleanup log

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::net::protocol::{
    NetworkMessage, NodeId, PROTOCOL_VERSION,
    HEARTBEAT_INTERVAL_SECS, HEARTBEAT_TIMEOUT_SECS,
    APP_ID, APP_KEY, HmacSha256, PROTOCOL_MAGIC,
};
use hmac::Mac;
use crate::net::{NetworkCommand, PeerInfo, SessionRegister};

/// Outgoing-message channel sender: the Router pushes messages here,
/// and the session task forwards them over the TCP wire.
pub type SessionSender = mpsc::UnboundedSender<NetworkMessage>;

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

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

    run_session_loop(framed, remote_node_id, command_tx, info).await;
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
) {
    let framed = Framed::new(stream, LengthDelimitedCodec::new());

    // -- Client-side handshake ------------------------------------------
    let (remote_node_id, remote_display_name, mut framed) =
        match client_handshake(framed, local_node_id, &local_display_name).await {
            Ok(r) => r,
            Err(e) => {
                log::warn!("Outbound handshake failed to {}: {}", peer_addr, e);
                return;
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
        return;
    }

    let info = PeerInfo {
        node_id: remote_node_id,
        display_name: remote_display_name,
        address: peer_addr,
        tcp_port: peer_addr.port(),
        last_seen: std::time::Instant::now(),
    };

    run_session_loop(framed, remote_node_id, command_tx, info).await;
}

// ---------------------------------------------------------------------------
// Handshake helpers
// ---------------------------------------------------------------------------

type FramedStream = Framed<TcpStream, LengthDelimitedCodec>;

/// Server-side handshake: receive Hello + HMAC, verify, send HelloAck.
///
/// 1. Receive Hello (magic-validated by [`recv_msg`]).
/// 2. Check `app_id` matches [`APP_ID`].
/// 3. Receive the 32-byte HMAC-SHA256 tag.
/// 4. Verify the HMAC against the raw Hello bytes.
/// 5. Send HelloAck (with `app_id`).
async fn server_handshake(
    mut framed: FramedStream,
    local_id: NodeId,
    local_name: &str,
) -> Result<(NodeId, String, FramedStream), Box<dyn std::error::Error>> {
    // -- Receive Hello ----------------------------------------------------
    let (msg, hello_raw) = recv_msg_with_raw(&mut framed)
        .await?
        .ok_or("Connection closed before Hello")?;

    let (remote_id, remote_name, remote_ver, remote_app_id) = match msg {
        NetworkMessage::Hello {
            node_id,
            display_name,
            protocol_version,
            app_id,
        } => (node_id, display_name, protocol_version, app_id),
        other => return Err(format!("Expected Hello, got {:?}", other).into()),
    };

    // -- App ID check -----------------------------------------------------
    if remote_app_id != APP_ID {
        return Err(format!(
            "App ID mismatch: remote='{}', local='{}'",
            remote_app_id, APP_ID,
        )
        .into());
    }

    // -- Protocol version check -------------------------------------------
    if remote_ver != PROTOCOL_VERSION {
        let _ = send_msg(
            &mut framed,
            &NetworkMessage::HelloAck {
                node_id: local_id,
                display_name: local_name.to_string(),
                protocol_version: 0,
                app_id: APP_ID.to_string(),
            },
        )
        .await;
        return Err(format!(
            "Protocol version mismatch: remote={}, local={}",
            remote_ver, PROTOCOL_VERSION,
        )
        .into());
    }

    // -- Receive & verify HMAC --------------------------------------------
    let hmac_bytes = recv_raw(&mut framed)
        .await?
        .ok_or("Connection closed before HMAC")?;

    if hmac_bytes.len() != 32 {
        return Err(format!(
            "HMAC tag length mismatch: expected 32, got {}",
            hmac_bytes.len(),
        )
        .into());
    }

    let mut mac =
        HmacSha256::new_from_slice(APP_KEY).map_err(|e| format!("HMAC init error: {}", e))?;
    mac.update(&hello_raw);
    if mac.verify_slice(&hmac_bytes).is_err() {
        return Err("HMAC verification failed".into());
    }

    // -- Send HelloAck ----------------------------------------------------
    send_msg(
        &mut framed,
        &NetworkMessage::HelloAck {
            node_id: local_id,
            display_name: local_name.to_string(),
            protocol_version: PROTOCOL_VERSION,
            app_id: APP_ID.to_string(),
        },
    )
    .await?;

    Ok((remote_id, remote_name, framed))
}

/// Client-side handshake: send Hello + HMAC, receive HelloAck.
///
/// 1. Serialize Hello, compute HMAC-SHA256, send Hello (magic-prefixed)
///    followed by the raw 32-byte HMAC tag.
/// 2. Receive HelloAck and verify `app_id`.
async fn client_handshake(
    mut framed: FramedStream,
    local_id: NodeId,
    local_name: &str,
) -> Result<(NodeId, String, FramedStream), Box<dyn std::error::Error>> {
    let hello = NetworkMessage::Hello {
        node_id: local_id,
        display_name: local_name.to_string(),
        protocol_version: PROTOCOL_VERSION,
        app_id: APP_ID.to_string(),
    };

    // Compute HMAC over the raw bincode-serialized Hello.
    let hello_bytes = bincode::serde::encode_to_vec(&hello, bincode::config::standard())?;
    let mut mac =
        HmacSha256::new_from_slice(APP_KEY).map_err(|e| format!("HMAC init error: {}", e))?;
    mac.update(&hello_bytes);
    let hmac_tag = mac.finalize().into_bytes();

    // Send Hello (magic-prefixed) then raw HMAC.
    send_msg(&mut framed, &hello).await?;
    send_raw(&mut framed, &hmac_tag).await?;

    // -- Receive HelloAck -------------------------------------------------
    let msg = recv_msg(&mut framed)
        .await?
        .ok_or("Connection closed before HelloAck")?;

    let (remote_id, remote_name, remote_ver, remote_app_id) = match msg {
        NetworkMessage::HelloAck {
            node_id,
            display_name,
            protocol_version,
            app_id,
        } => (node_id, display_name, protocol_version, app_id),
        other => return Err(format!("Expected HelloAck, got {:?}", other).into()),
    };

    if remote_app_id != APP_ID {
        return Err(format!(
            "App ID mismatch: remote='{}', local='{}'",
            remote_app_id, APP_ID,
        )
        .into());
    }

    if remote_ver != PROTOCOL_VERSION {
        return Err(format!(
            "Protocol version mismatch: remote={}, local={}",
            remote_ver, PROTOCOL_VERSION,
        )
        .into());
    }

    Ok((remote_id, remote_name, framed))
}

// ---------------------------------------------------------------------------
// Session main loop
// ---------------------------------------------------------------------------

/// Split the framed stream, spawn the heartbeat task, and run the
/// bidirectional message relay until the connection closes or times out.
async fn run_session_loop(
    framed: FramedStream,
    remote_id: NodeId,
    command_tx: mpsc::UnboundedSender<NetworkCommand>,
    info: PeerInfo,
) {
    let (mut writer, mut reader) = framed.split();
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<NetworkMessage>();

    // Register with the NetworkManager.
    let _ = command_tx.send(NetworkCommand::RegisterSession(SessionRegister {
        node_id: remote_id,
        sender: outgoing_tx.clone(),
        info: info.clone(),
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

// ---------------------------------------------------------------------------
// Message handling inside the session loop
// ---------------------------------------------------------------------------

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
            let _ = command_tx.send(NetworkCommand::IncomingMessage(msg));
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Wire-level helpers (thin wrappers around the framed codec)
// ---------------------------------------------------------------------------

/// Serialize and send a [`NetworkMessage`] with [`PROTOCOL_MAGIC`] prefix.
async fn send_msg<S>(
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
async fn recv_msg(
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

/// Like [`recv_msg`], but also returns the raw bincode bytes (after magic
/// stripping) so the caller can compute an HMAC over them.
async fn recv_msg_with_raw(
    reader: &mut FramedStream,
) -> Result<Option<(NetworkMessage, tokio_util::bytes::Bytes)>, Box<dyn std::error::Error>> {
    match reader.next().await {
        Some(Ok(bytes)) => {
            if bytes.len() < PROTOCOL_MAGIC.len()
                || bytes[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC
            {
                return Err("Invalid protocol magic bytes".into());
            }
            let bytes = bytes.freeze();
            let raw = bytes.slice(PROTOCOL_MAGIC.len()..);
            let (msg, _) = bincode::serde::decode_from_slice(
                &raw,
                bincode::config::standard(),
            )?;
            Ok(Some((msg, raw)))
        }
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Send raw bytes as a single length-delimited frame (no magic prefix).
/// Used for the HMAC tag during handshake.
async fn send_raw(
    framed: &mut FramedStream,
    data: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    framed
        .send(tokio_util::bytes::Bytes::from(data.to_vec()))
        .await?;
    Ok(())
}

/// Receive a single raw frame (no magic checking).
/// Used for the HMAC tag during handshake.
async fn recv_raw(
    reader: &mut FramedStream,
) -> Result<Option<tokio_util::bytes::Bytes>, Box<dyn std::error::Error>> {
    match reader.next().await {
        Some(Ok(bytes)) => Ok(Some(bytes.freeze())),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}
