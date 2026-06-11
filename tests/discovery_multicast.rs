//! Standalone integration test for UDP multicast discovery.
//!
//! Creates two `DiscoveryService` instances on the same machine and verifies
//! that Announce messages are received in both directions (A->B and B->A).
//!
//! Run with:  cargo test --test discovery_multicast -- --nocapture --test-threads=1

use std::time::{Duration, Instant};
use uuid::Uuid;

use vocal_calculator::net::discovery::DiscoveryService;
use vocal_calculator::net::protocol::{
    Capabilities, DiscoveryMessage, PROTOCOL_VERSION,
};

/// Helper: build a unique Announce message.
fn make_announce(display_name: &str, tcp_port: u16) -> DiscoveryMessage {
    DiscoveryMessage::Announce {
        node_id: Uuid::new_v4(),
        display_name: display_name.into(),
        tcp_port,
        capabilities: Capabilities {
            can_execute: true,
            can_control: true,
            protocol_version: PROTOCOL_VERSION,
        },
    }
}

/// Helper: drain recv() until an Announce with `expected_name` arrives,
/// or timeout expires.  Skips all other messages (Discover, Announce from
/// other senders, protocol errors).
async fn recv_announce_from(
    svc: &DiscoveryService,
    expected_name: &str,
    timeout: Duration,
) -> Option<(String, u16)> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, svc.recv()).await {
            Ok(Ok((DiscoveryMessage::Announce {
                display_name,
                tcp_port,
                ..
            }, _addr))) => {
                if display_name == expected_name {
                    return Some((display_name, tcp_port));
                }
                // Different sender — skip (likely self-echo or cross-test).
                continue;
            }
            Ok(Ok((DiscoveryMessage::Discover, _))) => continue,
            Ok(Err(_)) => continue,
            Err(_) => return None,
        }
    }
}

/// Helper: drain recv() until the first Announce (any sender) arrives,
/// or timeout expires.
async fn recv_any_announce(
    svc: &DiscoveryService,
    timeout: Duration,
) -> Option<(String, u16)> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, svc.recv()).await {
            Ok(Ok((DiscoveryMessage::Announce {
                display_name,
                tcp_port,
                ..
            }, _))) => return Some((display_name, tcp_port)),
            Ok(Ok((DiscoveryMessage::Discover, _))) => continue,
            Ok(Err(_)) => continue,
            Err(_) => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test: A sends Announce -> B receives it within 5 seconds.
/// Then: B sends Announce -> A receives it within 5 seconds.
///
/// IMPORTANT: Because multicast loopback is enabled by default, the sender
/// also receives its own packet. The recv helpers filter by sender name to
/// avoid confusing self-echoes with the peer's packet.
#[tokio::test]
async fn discovery_announce_bidirectional() {
    let svc_a = DiscoveryService::new()
        .await
        .expect("Failed to create DiscoveryService A");
    let svc_b = DiscoveryService::new()
        .await
        .expect("Failed to create DiscoveryService B");

    // ── Direction 1: A announces, B should receive ────────────────────────
    let msg_a = make_announce("NodeA", 5001);
    svc_a
        .announce(&msg_a)
        .await
        .expect("A failed to announce");

    let result_b = recv_announce_from(&svc_b, "NodeA", Duration::from_secs(5)).await;
    assert!(
        result_b.is_some(),
        "B did not receive A's Announce within 5 seconds — multicast may be broken"
    );
    let (name, port) = result_b.unwrap();
    assert_eq!(name, "NodeA");
    assert_eq!(port, 5001);
    println!("[OK] A -> B: B received Announce from NodeA:5001");

    // ── Direction 2: B announces, A should receive ────────────────────────
    //
    // NOTE: A's socket also has its own loopback "NodeA" packet queued.
    // recv_announce_from filters by name, so it skips the self-echo and
    // waits for "NodeB".
    let msg_b = make_announce("NodeB", 5002);
    svc_b
        .announce(&msg_b)
        .await
        .expect("B failed to announce");

    let result_a = recv_announce_from(&svc_a, "NodeB", Duration::from_secs(5)).await;
    assert!(
        result_a.is_some(),
        "A did not receive B's Announce within 5 seconds — multicast may be broken"
    );
    let (name, port) = result_a.unwrap();
    assert_eq!(name, "NodeB");
    assert_eq!(port, 5002);
    println!("[OK] B -> A: A received Announce from NodeB:5002");
}

/// Test: Discover message round-trips.
#[tokio::test]
async fn discovery_discover_roundtrip() {
    let svc_a = DiscoveryService::new()
        .await
        .expect("Failed to create DiscoveryService A");
    let svc_b = DiscoveryService::new()
        .await
        .expect("Failed to create DiscoveryService B");

    let discover = DiscoveryMessage::Discover;
    svc_a
        .announce(&discover)
        .await
        .expect("A failed to send Discover");

    // Wait for the first Discover at B (skip any Announce if it appears).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got_discover = false;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, svc_b.recv()).await {
            Ok(Ok((DiscoveryMessage::Discover, _))) => {
                got_discover = true;
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => continue,
            Err(_) => break,
        }
    }
    assert!(got_discover, "B did not receive Discover from A within 5 seconds");
    println!("[OK] Discover round-trip succeeded");
}

/// Test: Announce sent from a socket is also received by itself (multicast loopback).
#[tokio::test]
async fn discovery_self_loopback() {
    let svc = DiscoveryService::new()
        .await
        .expect("Failed to create DiscoveryService");

    let msg = make_announce("SelfNode", 6000);
    svc.announce(&msg)
        .await
        .expect("Failed to announce");

    let result = recv_any_announce(&svc, Duration::from_secs(5)).await;
    assert!(
        result.is_some(),
        "Sender did not receive its own Announce — multicast loopback may be disabled"
    );
    let (name, port) = result.unwrap();
    assert_eq!(name, "SelfNode");
    assert_eq!(port, 6000);
    println!("[OK] Multicast loopback: sender received its own Announce");
}

/// Test: PROTOCOL_MAGIC is correctly encoded.
#[test]
fn protocol_magic_byte_layout() {
    use vocal_calculator::net::protocol::PROTOCOL_MAGIC;

    // Expected: b"VOCALC" + 0x01 + 0x00  (8 bytes total)
    assert_eq!(PROTOCOL_MAGIC.len(), 8, "PROTOCOL_MAGIC should be 8 bytes");
    assert_eq!(&PROTOCOL_MAGIC[..6], b"VOCALC", "First 6 bytes should be 'VOCALC'");
    assert_eq!(PROTOCOL_MAGIC[6], 0x01, "Byte 6 should be version 0x01");
    assert_eq!(PROTOCOL_MAGIC[7], 0x00, "Byte 7 should be reserved 0x00");
    println!("[OK] PROTOCOL_MAGIC layout is correct");
}
