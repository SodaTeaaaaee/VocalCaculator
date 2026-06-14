//! Handshake protocol for inbound and outbound TCP connections.
//!
//! Each connection performs the following exchange before entering the
//! steady-state session loop:
//!
//! 1. **Hello** — magic-prefixed, bincode-serialized [`NetworkMessage::Hello`]
//!    containing `node_id`, `display_name`, `protocol_version`, and `app_id`.
//! 2. **HMAC** — raw 32-byte HMAC-SHA256 tag computed over the Hello bytes,
//!    sent as a separate length-delimited frame (no magic prefix).
//! 3. **HelloAck** — the peer's Hello, same shape, sent by the server side.
//!
//! The *server* (accepted connection) receives Hello + HMAC, verifies, then
//! sends HelloAck.  The *client* (outgoing connection) sends Hello + HMAC,
//! then receives HelloAck.

use super::protocol::{
    NetworkMessage, NodeId, PROTOCOL_VERSION, PROTOCOL_MAGIC,
    APP_ID, APP_KEY, HmacSha256,
};
use super::session::{FramedStream, recv_msg, send_msg};
use futures::{SinkExt, StreamExt};
use hmac::Mac;

// ---------------------------------------------------------------------------
// Server-side handshake
// ---------------------------------------------------------------------------

/// Server-side handshake: receive Hello + HMAC, verify, send HelloAck.
///
/// 1. Receive Hello (magic-validated by [`recv_msg`]).
/// 2. Check `app_id` matches [`APP_ID`].
/// 3. Receive the 32-byte HMAC-SHA256 tag.
/// 4. Verify the HMAC against the raw Hello bytes.
/// 5. Send HelloAck (with `app_id`).
pub(super) async fn server_handshake(
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

// ---------------------------------------------------------------------------
// Client-side handshake
// ---------------------------------------------------------------------------

/// Client-side handshake: send Hello + HMAC, receive HelloAck.
///
/// 1. Serialize Hello, compute HMAC-SHA256, send Hello (magic-prefixed)
///    followed by the raw 32-byte HMAC tag.
/// 2. Receive HelloAck and verify `app_id`.
pub(super) async fn client_handshake(
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
// Message helpers (handshake-only)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Raw frame helpers (handshake-only)
// ---------------------------------------------------------------------------

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
