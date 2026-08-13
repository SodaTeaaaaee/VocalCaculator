//! Per-connection session lifecycle: handshake dispatch, bidirectional
//! message relay, heartbeat, and teardown.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use ed25519_dalek::SigningKey;

use crate::net::protocol::{
    ConnectionDirection, ExpectedPeerIdentity, HEARTBEAT_INTERVAL_SECS, HEARTBEAT_TIMEOUT_SECS,
    NetworkCommand, NetworkMessage, NodeId, PROTOCOL_MAGIC, SessionId, SessionRegister,
    valid_display_name,
};
use crate::net::state::PeerInfo;

use super::handshake::{client_handshake, server_handshake};

/// Outgoing-message channel sender: the Router pushes messages here,
/// and the session task forwards them over the TCP wire.
pub type SessionSender = mpsc::Sender<NetworkMessage>;

/// Registry entry for the currently selected session generation of a peer.
#[derive(Clone)]
pub(crate) struct ActiveSession {
    pub session_id: SessionId,
    pub sender: SessionSender,
    pub direction: ConnectionDirection,
    pub cancel_tx: tokio::sync::watch::Sender<bool>,
}

/// Framed TCP stream with length-delimited codec.
pub(super) type FramedStream = Framed<TcpStream, LengthDelimitedCodec>;

#[cfg(not(test))]
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
#[cfg(test)]
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
#[cfg(not(test))]
const SUBSCRIBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(test)]
const SUBSCRIBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
const REGISTRATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const SESSION_OUTGOING_CAPACITY: usize = 256;
pub(crate) const MAX_FRAME_LENGTH: usize = 4 * 1024;
#[cfg(not(test))]
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(test)]
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

// Public entry points

pub(crate) fn session_codec() -> LengthDelimitedCodec {
    let mut codec = LengthDelimitedCodec::new();
    codec.set_max_frame_length(MAX_FRAME_LENGTH);
    codec
}

/// Run a session for an **accepted** (inbound) TCP connection.
///
/// The server side of the handshake: receives `Hello`, replies with
/// `HelloAck`, then waits for `Subscribe`.
pub(crate) async fn run_accepted_session(
    stream: TcpStream,
    peer_addr: std::net::SocketAddr,
    local_node_id: NodeId,
    local_display_name: String,
    local_pubkey: [u8; 32],
    local_signing_key: SigningKey,
    command_tx: mpsc::Sender<NetworkCommand>,
) {
    let framed = Framed::new(stream, session_codec());

    // -- Server-side handshake ------------------------------------------
    let (remote_node_id, remote_display_name, remote_pubkey, mut framed) =
        match tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            server_handshake(
                framed,
                local_node_id,
                &local_display_name,
                local_pubkey,
                &local_signing_key,
            ),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                log::warn!("Inbound handshake failed from {}: {}", peer_addr, e);
                return;
            }
            Err(_) => {
                log::warn!("Inbound handshake timed out from {}", peer_addr);
                return;
            }
        };

    log::info!(
        "Inbound session established: {} ({}) from {}",
        remote_display_name,
        remote_node_id,
        peer_addr,
    );

    log::debug!(
        "Remote {} provided verified ed25519 public key; trust check deferred to route authorization",
        remote_node_id,
    );

    // Wait for Subscribe before entering steady-state.
    match tokio::time::timeout(SUBSCRIBE_TIMEOUT, recv_msg(&mut framed)).await {
        Ok(Ok(Some(NetworkMessage::Subscribe))) => {
            log::trace!("Received Subscribe from {}", remote_node_id);
        }
        Ok(Ok(other)) => {
            log::warn!(
                "Expected Subscribe from {}, got {:?}; dropping",
                remote_node_id,
                other,
            );
            return;
        }
        Ok(Err(e)) => {
            log::warn!("Failed reading Subscribe from {}: {}", remote_node_id, e);
            return;
        }
        Err(_) => {
            log::warn!("Timed out waiting for Subscribe from {}", remote_node_id);
            return;
        }
    }

    let info = PeerInfo {
        node_id: remote_node_id,
        display_name: remote_display_name,
        // An accepted socket only reveals the caller's ephemeral source
        // endpoint. Preserve any independently discovered service endpoint in
        // PeerTable rather than deriving or overwriting it here.
        service_endpoint: None,
        session_peer_addr: Some(peer_addr),
        last_seen: std::time::Instant::now(),
        public_key: remote_pubkey,
        public_key_fingerprint: None,
    };

    run_session_loop(
        framed,
        remote_node_id,
        command_tx,
        info,
        ConnectionDirection::Inbound,
    )
    .await;
}

/// Run a session for an **outgoing** (client-initiated) TCP connection.
///
/// The client side of the handshake: sends `Hello`, waits for `HelloAck`,
/// then sends `Subscribe`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_connecting_session(
    stream: TcpStream,
    peer_addr: std::net::SocketAddr,
    local_node_id: NodeId,
    local_display_name: String,
    local_pubkey: [u8; 32],
    local_signing_key: SigningKey,
    expected_peer: Option<ExpectedPeerIdentity>,
    command_tx: mpsc::Sender<NetworkCommand>,
) -> Result<(), String> {
    let framed = Framed::new(stream, session_codec());

    // -- Client-side handshake with 8-second timeout --------------------
    let (remote_node_id, remote_display_name, remote_pubkey, mut framed) =
        match tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            client_handshake(
                framed,
                local_node_id,
                &local_display_name,
                local_pubkey,
                &local_signing_key,
            ),
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

    validate_expected_peer(remote_node_id, &remote_pubkey, expected_peer.as_ref())?;

    log::info!(
        "Outbound session established: {} ({}) to {}",
        remote_display_name,
        remote_node_id,
        peer_addr,
    );

    log::debug!(
        "Remote {} provided verified ed25519 public key; trust check deferred to route authorization",
        remote_node_id,
    );

    // Send Subscribe to start receiving state updates.
    match tokio::time::timeout(
        SUBSCRIBE_TIMEOUT,
        send_msg(&mut framed, &NetworkMessage::Subscribe),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::warn!("Failed sending Subscribe to {}: {}", remote_node_id, e);
            return Err(format!("subscribe: {}", e));
        }
        Err(_) => {
            log::warn!("Timed out sending Subscribe to {}", remote_node_id);
            return Err("subscribe_timeout".to_string());
        }
    }

    let info = PeerInfo {
        node_id: remote_node_id,
        display_name: remote_display_name,
        service_endpoint: Some(peer_addr),
        session_peer_addr: Some(peer_addr),
        last_seen: std::time::Instant::now(),
        public_key: remote_pubkey,
        public_key_fingerprint: None,
    };

    run_session_loop(
        framed,
        remote_node_id,
        command_tx,
        info,
        ConnectionDirection::Outbound,
    )
    .await;
    Ok(())
}

pub(crate) fn session_outgoing_channel()
-> (mpsc::Sender<NetworkMessage>, mpsc::Receiver<NetworkMessage>) {
    mpsc::channel(SESSION_OUTGOING_CAPACITY)
}

pub(crate) fn validate_expected_peer(
    remote_node_id: NodeId,
    remote_public_key: &[u8; 32],
    expected_peer: Option<&ExpectedPeerIdentity>,
) -> Result<(), String> {
    let Some(expected) = expected_peer else {
        return Ok(());
    };
    if remote_node_id != expected.node_id {
        return Err(format!(
            "identity_mismatch: expected node {}, handshake returned {}",
            expected.node_id, remote_node_id,
        ));
    }
    if let Some(expected_fingerprint) = &expected.public_key_fingerprint {
        let actual = crate::net::discovery::public_key_fingerprint(remote_public_key);
        if &actual != expected_fingerprint {
            return Err(format!(
                "fingerprint_mismatch: expected {}, handshake returned {}",
                expected_fingerprint, actual,
            ));
        }
    }
    Ok(())
}

// Session main loop

/// Split the framed stream, spawn the heartbeat task, and run the
/// bidirectional message relay until the connection closes or times out.
async fn run_session_loop(
    framed: FramedStream,
    remote_id: NodeId,
    command_tx: mpsc::Sender<NetworkCommand>,
    info: PeerInfo,
    direction: ConnectionDirection,
) {
    let (mut writer, mut reader) = framed.split();
    let (outgoing_tx, mut outgoing_rx) = session_outgoing_channel();
    let session_id = SessionId::new_v4();
    let (decision_tx, decision_rx) = oneshot::channel();
    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);

    // Register with the NetworkManager and wait for the generation-aware
    // deduplication decision before entering the relay loop.
    let registration = command_tx.send(NetworkCommand::RegisterSession(SessionRegister {
        session_id,
        node_id: remote_id,
        sender: outgoing_tx.clone(),
        info: info.clone(),
        direction,
        cancel_tx,
        decision_tx,
    }));
    if !matches!(
        tokio::time::timeout(REGISTRATION_TIMEOUT, registration).await,
        Ok(Ok(()))
    ) {
        return;
    }
    match tokio::time::timeout(REGISTRATION_TIMEOUT, decision_rx).await {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            log::info!(
                "Session {} generation {} rejected by dedup",
                remote_id,
                session_id
            );
            return;
        }
        Ok(Err(_)) | Err(_) => {
            log::warn!(
                "Session {} generation {} registration decision timed out",
                remote_id,
                session_id,
            );
            return;
        }
    }

    // Shared heartbeat timestamp (seconds elapsed since reference, monotonic).
    let last_pong = Arc::new(AtomicU64::new(0));
    // Timestamp (ms since epoch) of when the last Ping was sent, for RTT calculation.
    let last_ping_sent = Arc::new(AtomicU64::new(0));

    // -- Heartbeat task --------------------------------------------------
    let hb_last_pong = last_pong.clone();
    let hb_last_ping = last_ping_sent.clone();
    let hb_outgoing = outgoing_tx.clone();
    let (heartbeat_done_tx, mut heartbeat_done_rx) = oneshot::channel::<()>();
    let hb_handle = tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
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
            if hb_outgoing.try_send(NetworkMessage::Ping).is_err() {
                break; // session ended
            }
        }
        let _ = heartbeat_done_tx.send(());
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
    let mut cancel_channel_open = true;

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
                        match decode_network_message(&bytes[PROTOCOL_MAGIC.len()..]) {
                            Ok(msg) => {
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
            _ = &mut heartbeat_done_rx => {
                log::warn!("Heartbeat task ended for {}; closing session", remote_id);
                relay_error = true;
                break;
            }
            changed = cancel_rx.changed(), if cancel_channel_open => {
                if changed.is_ok() && *cancel_rx.borrow() {
                    log::info!("Session {} generation {} cancelled", remote_id, session_id);
                    break;
                }
                if changed.is_err() {
                    // Tests and embedders may not retain a registry sender.
                    // Channel closure alone is not a cancellation request and
                    // must not spin this select branch.
                    cancel_channel_open = false;
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
    let _ = hb_handle.await;
    let _ = tracker_handle.await;

    if relay_error {
        log::info!("Session with {} ended (error)", remote_id);
    } else {
        log::info!("Session with {} ended (clean)", remote_id);
    }

    let _ = tokio::time::timeout(
        REGISTRATION_TIMEOUT,
        command_tx.send(NetworkCommand::UnregisterSession {
            node_id: remote_id,
            session_id,
        }),
    )
    .await;
}

// Message handling inside the session loop

/// Process one incoming message. Returns `false` if the session should close.
async fn handle_incoming_message(
    msg: NetworkMessage,
    remote_id: NodeId,
    command_tx: &mpsc::Sender<NetworkCommand>,
    outgoing_tx: &mpsc::Sender<NetworkMessage>,
    last_pong: &Arc<AtomicU64>,
    last_ping_sent: &Arc<AtomicU64>,
) -> bool {
    if msg.is_local_only() {
        log::warn!("Rejected local-only message from authenticated peer {remote_id}");
        return false;
    }
    if let NetworkMessage::PeerNameUpdate { display_name } = &msg
        && !valid_display_name(display_name)
    {
        log::warn!("Rejected invalid peer display name from {remote_id}");
        return false;
    }
    match msg {
        NetworkMessage::Ping => {
            // Respond directly; no Router involvement needed.
            if outgoing_tx.try_send(NetworkMessage::Pong).is_err() {
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
                let _ = command_tx
                    .send(NetworkCommand::UpdateLatency {
                        node_id: remote_id,
                        ms: rtt,
                    })
                    .await;
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
            let _ = command_tx
                .send(NetworkCommand::IncomingMessage(remote_id, msg))
                .await;
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
    let bincode_bytes = bincode::serde::encode_to_vec(
        msg,
        bincode::config::standard().with_limit::<MAX_FRAME_LENGTH>(),
    )?;
    let mut payload = Vec::with_capacity(PROTOCOL_MAGIC.len() + bincode_bytes.len());
    payload.extend_from_slice(&PROTOCOL_MAGIC);
    payload.extend_from_slice(&bincode_bytes);
    match tokio::time::timeout(
        WRITE_TIMEOUT,
        writer.send(tokio_util::bytes::Bytes::from(payload)),
    )
    .await
    {
        Ok(result) => {
            result?;
            Ok(())
        }
        Err(_) => Err("network write timed out".into()),
    }
}

/// Receive a frame, verify [`PROTOCOL_MAGIC`], and deserialize a
/// [`NetworkMessage`]. Returns `Ok(None)` on clean close.
pub(super) async fn recv_msg(
    reader: &mut FramedStream,
) -> Result<Option<NetworkMessage>, Box<dyn std::error::Error>> {
    match reader.next().await {
        Some(Ok(bytes)) => {
            if bytes.len() < PROTOCOL_MAGIC.len() || bytes[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC
            {
                return Err("Invalid protocol magic bytes".into());
            }
            let msg = decode_network_message(&bytes[PROTOCOL_MAGIC.len()..])?;
            Ok(Some(msg))
        }
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Decode exactly one message from a frame. Accepting a valid prefix while
/// ignoring attacker-controlled suffix bytes makes the framing contract
/// ambiguous, so every ingress path requires full consumption.
pub(super) fn decode_network_message(bytes: &[u8]) -> Result<NetworkMessage, String> {
    let (message, consumed) = bincode::serde::decode_from_slice::<NetworkMessage, _>(
        bytes,
        bincode::config::standard().with_limit::<MAX_FRAME_LENGTH>(),
    )
    .map_err(|error| error.to_string())?;
    if consumed != bytes.len() {
        return Err(format!(
            "network message has trailing bytes: consumed {consumed}, frame {}",
            bytes.len(),
        ));
    }
    Ok(message)
}
