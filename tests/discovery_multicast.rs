//! Standalone integration test for TCP-based discovery (Localsend pattern).
//!
//! Creates two `DiscoveryService` instances on different TCP ports and
//! verifies that:
//! 1. UDP announcements are received by both sides.
//! 2. TCP peer exchange works (connect_and_exchange).
//!
//! Run with:  cargo test --test discovery_multicast -- --nocapture --test-threads=1

use std::time::{Duration, Instant};
use uuid::Uuid;

use vocal_calculator::net::discovery::DiscoveryService;
use vocal_calculator::net::protocol::{
    Capabilities, DiscoveryMessage, PROTOCOL_VERSION,
};

/// Helper: build a unique AnnounceV2 message.
fn make_announce(display_name: &str, tcp_port: u16, session_port: u16) -> DiscoveryMessage {
    DiscoveryMessage::AnnounceV2 {
        node_id: Uuid::new_v4(),
        display_name: display_name.into(),
        tcp_port,
        capabilities: Capabilities {
            can_execute: true,
            can_control: true,
            protocol_version: PROTOCOL_VERSION,
        },
        transport_hint: vocal_calculator::net::protocol::TransportHint::Multicast,
        hostname: "test-host".into(),
        session_port,
    }
}

/// Helper: drain recv_announce() until an Announce with `expected_name`
/// arrives, or timeout expires.
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
        match tokio::time::timeout(remaining, svc.recv_announce()).await {
            Ok(Ok((DiscoveryMessage::Announce {
                display_name,
                tcp_port,
                ..
            }, _addr))) => {
                if display_name == expected_name {
                    return Some((display_name, tcp_port));
                }
                continue;
            }
            Ok(Ok((DiscoveryMessage::AnnounceV2 {
                display_name,
                tcp_port,
                ..
            }, _addr))) => {
                if display_name == expected_name {
                    return Some((display_name, tcp_port));
                }
                continue;
            }
            Ok(Ok((DiscoveryMessage::Discover, _))) => continue,
            Ok(Err(_)) => continue,
            Err(_) => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test: A sends Announce -> B receives it via UDP within 5 seconds.
/// Then: B sends Announce -> A receives it via UDP within 5 seconds.
///
/// Both instances use different TCP ports so they can coexist.
#[tokio::test]
async fn discovery_announce_bidirectional() {
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();

    let svc_a = DiscoveryService::new_with_port(id_a, "NodeA".into(), 42101, 50101)
        .await
        .expect("Failed to create DiscoveryService A");
    let svc_b = DiscoveryService::new_with_port(id_b, "NodeB".into(), 42102, 50102)
        .await
        .expect("Failed to create DiscoveryService B");

    // -- Direction 1: A announces, B should receive via UDP ----------------
    let msg_a = make_announce("NodeA", 42101, 50101);
    svc_a
        .announce(&msg_a)
        .await
        .expect("A failed to announce");

    let result_b = recv_announce_from(&svc_b, "NodeA", Duration::from_secs(5)).await;
    assert!(
        result_b.is_some(),
        "B did not receive A's Announce within 5 seconds — UDP may be broken"
    );
    let (name, port) = result_b.unwrap();
    assert_eq!(name, "NodeA");
    assert_eq!(port, 42101);
    println!("[OK] A -> B: B received Announce from NodeA:42101");

    // -- Direction 2: B announces, A should receive via UDP ----------------
    let msg_b = make_announce("NodeB", 42102, 50102);
    svc_b
        .announce(&msg_b)
        .await
        .expect("B failed to announce");

    let result_a = recv_announce_from(&svc_a, "NodeB", Duration::from_secs(5)).await;
    assert!(
        result_a.is_some(),
        "A did not receive B's Announce within 5 seconds — UDP may be broken"
    );
    let (name, port) = result_a.unwrap();
    assert_eq!(name, "NodeB");
    assert_eq!(port, 42102);
    println!("[OK] B -> A: A received Announce from NodeB:42102");
}

/// Test: TCP peer exchange — A connects to B's TCP port, both sides
/// exchange DiscoveryMessage, and A learns B's identity.
#[tokio::test]
async fn discovery_tcp_exchange() {
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();

    let svc_a = DiscoveryService::new_with_port(id_a, "NodeA".into(), 42103, 50103)
        .await
        .expect("Failed to create DiscoveryService A");
    let svc_b = DiscoveryService::new_with_port(id_b, "NodeB".into(), 42104, 50104)
        .await
        .expect("Failed to create DiscoveryService B");

    let local_msg_a = svc_a.announce_msg().clone();

    // Spawn B's accept task.
    let accept_handle = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), svc_b.accept_peer())
            .await
            .expect("B accept timed out")
            .expect("B accept failed")
    });

    // A connects to B's TCP port.
    let connect_result = tokio::time::timeout(
        Duration::from_secs(5),
        DiscoveryService::connect_and_exchange(
            "127.0.0.1:42104".parse().unwrap(),
            &local_msg_a,
        ),
    )
    .await
    .expect("A connect timed out")
    .expect("A connect failed");

    // Verify A learned B's identity.
    assert_eq!(connect_result.node_id, id_b);
    assert_eq!(connect_result.display_name, "NodeB");
    assert_eq!(connect_result.tcp_port, 42104);
    assert_eq!(connect_result.session_port, 50104);
    println!(
        "[OK] A connected to B: learned {} ({})",
        connect_result.display_name, connect_result.node_id,
    );

    // Verify B learned A's identity.
    let b_exchange = accept_handle.await.expect("B accept task panicked");
    assert_eq!(b_exchange.node_id, id_a);
    assert_eq!(b_exchange.display_name, "NodeA");
    assert_eq!(b_exchange.tcp_port, 42103);
    assert_eq!(b_exchange.session_port, 50103);
    println!(
        "[OK] B accepted from A: learned {} ({})",
        b_exchange.display_name, b_exchange.node_id,
    );
}

/// Test: Discover message round-trips via UDP.
#[tokio::test]
async fn discovery_discover_roundtrip() {
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();

    let svc_a = DiscoveryService::new_with_port(id_a, "NodeA".into(), 42105, 50105)
        .await
        .expect("Failed to create DiscoveryService A");
    let svc_b = DiscoveryService::new_with_port(id_b, "NodeB".into(), 42106, 50106)
        .await
        .expect("Failed to create DiscoveryService B");

    let discover = DiscoveryMessage::Discover;
    svc_a
        .announce(&discover)
        .await
        .expect("A failed to send Discover");

    // Wait for the first Discover at B.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got_discover = false;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, svc_b.recv_announce()).await {
            Ok(Ok((DiscoveryMessage::Discover, _))) => {
                got_discover = true;
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => continue,
            Err(_) => break,
        }
    }
    assert!(
        got_discover,
        "B did not receive Discover from A within 5 seconds"
    );
    println!("[OK] Discover round-trip succeeded");
}

/// Test: PROTOCOL_MAGIC is correctly encoded.
#[test]
fn protocol_magic_byte_layout() {
    use vocal_calculator::net::protocol::PROTOCOL_MAGIC;

    assert_eq!(PROTOCOL_MAGIC.len(), 8, "PROTOCOL_MAGIC should be 8 bytes");
    assert_eq!(
        &PROTOCOL_MAGIC[..6],
        b"VOCALC",
        "First 6 bytes should be 'VOCALC'"
    );
    assert_eq!(PROTOCOL_MAGIC[6], 0x01, "Byte 6 should be version 0x01");
    assert_eq!(PROTOCOL_MAGIC[7], 0x00, "Byte 7 should be reserved 0x00");
    println!("[OK] PROTOCOL_MAGIC layout is correct");
}
