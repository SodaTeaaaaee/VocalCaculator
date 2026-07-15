//! Handshake protocol for inbound and outbound TCP connections.
//!
//! Each connection performs the following exchange before entering the
//! steady-state session loop:
//!
//! 1. **Hello** — magic-prefixed, bincode-serialized [`NetworkMessage::Hello`]
//!    containing `node_id`, `display_name`, `protocol_version`, `app_id`,
//!    and `public_key` (Ed25519, 32 bytes).
//! 2. **HMAC** — raw 32-byte HMAC-SHA256 tag computed over the Hello bytes,
//!    sent as a separate length-delimited frame (no magic prefix). This only
//!    isolates the protocol; it is not treated as peer authentication.
//! 3. **HelloAck** — the peer's HelloAck, same shape, sent by the server side,
//!    also carrying the server's `public_key`.
//! 4. **AuthChallenge/AuthProof** — both peers challenge the other side with
//!    a fresh nonce and verify an Ed25519 signature from the advertised key.
//!
//! The *server* (accepted connection) receives Hello + HMAC, verifies, then
//! sends HelloAck.  The *client* (outgoing connection) sends Hello + HMAC,
//! then receives HelloAck.
//!
//! After HMAC verification, the caller can check `paired_devices` to
//! determine whether the remote's public key is paired and trusted.

use super::protocol::{
    APP_ID, APP_KEY, HmacSha256, NetworkMessage, NodeId, PROTOCOL_MAGIC, PROTOCOL_VERSION,
};
use super::session::{FramedStream, recv_msg, send_msg};
use crate::app::identity::derive_node_id;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use futures::{SinkExt, StreamExt};
use hmac::Mac;

const AUTH_DOMAIN: &[u8] = b"vocal-calculator-auth-v4";
const CLIENT_PROOF_ROLE: &[u8] = b"client";
const SERVER_PROOF_ROLE: &[u8] = b"server";

// ---------------------------------------------------------------------------
// Server-side handshake
// ---------------------------------------------------------------------------

/// Server-side handshake: receive Hello + HMAC, verify, send HelloAck.
///
/// 1. Receive Hello (magic-validated by [`recv_msg`]).
/// 2. Check `app_id` matches [`APP_ID`].
/// 3. Receive the 32-byte HMAC-SHA256 tag.
/// 4. Verify the HMAC against the raw Hello bytes.
/// 5. Send HelloAck (with `app_id` and local public key).
///
/// Returns `(remote_node_id, remote_display_name, remote_public_key, framed)`.
/// The caller can then check `paired_devices` to determine pairing status.
pub(super) async fn server_handshake(
    mut framed: FramedStream,
    local_id: NodeId,
    local_name: &str,
    local_pubkey: [u8; 32],
    local_signing_key: &SigningKey,
) -> Result<(NodeId, String, [u8; 32], FramedStream), Box<dyn std::error::Error>> {
    // -- Receive Hello ----------------------------------------------------
    let (msg, hello_raw) = recv_msg_with_raw(&mut framed)
        .await?
        .ok_or("Connection closed before Hello")?;

    let (remote_id, remote_name, remote_ver, remote_app_id, remote_pubkey) = match msg {
        NetworkMessage::Hello {
            node_id,
            display_name,
            protocol_version,
            app_id,
            public_key,
        } => (node_id, display_name, protocol_version, app_id, public_key),
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
                public_key: local_pubkey,
            },
        )
        .await;
        return Err(format!(
            "Protocol version mismatch: remote={}, local={}",
            remote_ver, PROTOCOL_VERSION,
        )
        .into());
    }

    let remote_verifying_key = verify_remote_identity(remote_id, &remote_pubkey)?;

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

    // -- Send HelloAck (with our public key) ------------------------------
    send_msg(
        &mut framed,
        &NetworkMessage::HelloAck {
            node_id: local_id,
            display_name: local_name.to_string(),
            protocol_version: PROTOCOL_VERSION,
            app_id: APP_ID.to_string(),
            public_key: local_pubkey,
        },
    )
    .await?;

    send_challenge_and_verify_proof(
        &mut framed,
        CLIENT_PROOF_ROLE,
        &remote_verifying_key,
        remote_id,
        remote_pubkey,
        local_id,
        local_pubkey,
    )
    .await?;

    receive_challenge_and_send_proof(
        &mut framed,
        SERVER_PROOF_ROLE,
        local_signing_key,
        local_id,
        local_pubkey,
        remote_id,
        remote_pubkey,
    )
    .await?;

    log::debug!(
        "Handshake with {}: ed25519 possession proof verified",
        remote_id
    );

    Ok((remote_id, remote_name, remote_pubkey, framed))
}

// ---------------------------------------------------------------------------
// Client-side handshake
// ---------------------------------------------------------------------------

/// Client-side handshake: send Hello + HMAC, receive HelloAck.
///
/// 1. Serialize Hello (with local public key), compute HMAC-SHA256,
///    send Hello (magic-prefixed) followed by the raw 32-byte HMAC tag.
/// 2. Receive HelloAck and verify `app_id`.
///
/// Returns `(remote_node_id, remote_display_name, remote_public_key, framed)`.
/// The caller can then check `paired_devices` to determine pairing status.
pub(super) async fn client_handshake(
    mut framed: FramedStream,
    local_id: NodeId,
    local_name: &str,
    local_pubkey: [u8; 32],
    local_signing_key: &SigningKey,
) -> Result<(NodeId, String, [u8; 32], FramedStream), Box<dyn std::error::Error>> {
    let hello = NetworkMessage::Hello {
        node_id: local_id,
        display_name: local_name.to_string(),
        protocol_version: PROTOCOL_VERSION,
        app_id: APP_ID.to_string(),
        public_key: local_pubkey,
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

    let (remote_id, remote_name, remote_ver, remote_app_id, remote_pubkey) = match msg {
        NetworkMessage::HelloAck {
            node_id,
            display_name,
            protocol_version,
            app_id,
            public_key,
        } => (node_id, display_name, protocol_version, app_id, public_key),
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

    let remote_verifying_key = verify_remote_identity(remote_id, &remote_pubkey)?;

    receive_challenge_and_send_proof(
        &mut framed,
        CLIENT_PROOF_ROLE,
        local_signing_key,
        local_id,
        local_pubkey,
        remote_id,
        remote_pubkey,
    )
    .await?;

    send_challenge_and_verify_proof(
        &mut framed,
        SERVER_PROOF_ROLE,
        &remote_verifying_key,
        remote_id,
        remote_pubkey,
        local_id,
        local_pubkey,
    )
    .await?;

    Ok((remote_id, remote_name, remote_pubkey, framed))
}

fn verify_remote_identity(
    remote_id: NodeId,
    remote_pubkey: &[u8; 32],
) -> Result<VerifyingKey, Box<dyn std::error::Error>> {
    if *remote_pubkey == [0u8; 32] {
        return Err("Protocol v4 requires an Ed25519 public key".into());
    }
    let verifying_key = VerifyingKey::from_bytes(remote_pubkey)
        .map_err(|e| format!("Invalid Ed25519 public key: {e}"))?;
    let derived_id = derive_node_id(&verifying_key);
    if derived_id != remote_id {
        return Err(format!(
            "Node ID/public key mismatch: hello={}, derived={}",
            remote_id, derived_id
        )
        .into());
    }
    Ok(verifying_key)
}

async fn send_challenge_and_verify_proof(
    framed: &mut FramedStream,
    expected_role: &[u8],
    remote_verifying_key: &VerifyingKey,
    remote_id: NodeId,
    remote_pubkey: [u8; 32],
    local_id: NodeId,
    local_pubkey: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let nonce = fresh_nonce()?;
    send_msg(framed, &NetworkMessage::AuthChallenge { nonce }).await?;
    let proof = recv_msg(framed)
        .await?
        .ok_or("Connection closed before AuthProof")?;
    let signature = match proof {
        NetworkMessage::AuthProof { signature } => signature,
        other => return Err(format!("Expected AuthProof, got {:?}", other).into()),
    };
    let signature = Signature::from_slice(&signature)
        .map_err(|e| format!("Invalid AuthProof signature: {e}"))?;
    let payload = auth_payload(
        expected_role,
        &nonce,
        remote_id,
        &remote_pubkey,
        local_id,
        &local_pubkey,
    );
    remote_verifying_key
        .verify(&payload, &signature)
        .map_err(|e| format!("AuthProof verification failed: {e}"))?;
    Ok(())
}

async fn receive_challenge_and_send_proof(
    framed: &mut FramedStream,
    role: &[u8],
    local_signing_key: &SigningKey,
    local_id: NodeId,
    local_pubkey: [u8; 32],
    remote_id: NodeId,
    remote_pubkey: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let challenge = recv_msg(framed)
        .await?
        .ok_or("Connection closed before AuthChallenge")?;
    let nonce = match challenge {
        NetworkMessage::AuthChallenge { nonce } => nonce,
        other => return Err(format!("Expected AuthChallenge, got {:?}", other).into()),
    };
    let payload = auth_payload(
        role,
        &nonce,
        local_id,
        &local_pubkey,
        remote_id,
        &remote_pubkey,
    );
    let signature = local_signing_key.sign(&payload);
    send_msg(
        framed,
        &NetworkMessage::AuthProof {
            signature: signature.to_bytes().to_vec(),
        },
    )
    .await?;
    Ok(())
}

fn auth_payload(
    role: &[u8],
    nonce: &[u8; 32],
    signer_id: NodeId,
    signer_pubkey: &[u8; 32],
    verifier_id: NodeId,
    verifier_pubkey: &[u8; 32],
) -> Vec<u8> {
    let mut payload =
        Vec::with_capacity(AUTH_DOMAIN.len() + role.len() + nonce.len() + 16 + 32 + 16 + 32);
    payload.extend_from_slice(AUTH_DOMAIN);
    payload.extend_from_slice(role);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(signer_id.as_bytes());
    payload.extend_from_slice(signer_pubkey);
    payload.extend_from_slice(verifier_id.as_bytes());
    payload.extend_from_slice(verifier_pubkey);
    payload
}

fn fresh_nonce() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let mut nonce = [0u8; 32];
    getrandom::getrandom(&mut nonce).map_err(|e| format!("Nonce generation failed: {e}"))?;
    Ok(nonce)
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
            if bytes.len() < PROTOCOL_MAGIC.len() || bytes[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC
            {
                return Err("Invalid protocol magic bytes".into());
            }
            let bytes = bytes.freeze();
            let raw = bytes.slice(PROTOCOL_MAGIC.len()..);
            let (msg, _) = bincode::serde::decode_from_slice(&raw, bincode::config::standard())?;
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
