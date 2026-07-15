//! Standalone integration test for TCP-based discovery (Localsend pattern).
//!
//! Creates two `DiscoveryService` instances on different TCP ports and
//! verifies that:
//! 1. UDP announcements are received by both sides.
//! 2. Announcements resolve directly to the advertised session endpoint.
//!
//! # Real network traffic -- double opt-in required
//!
//! Every test in this file constructs real `DiscoveryService` instances,
//! which start real mDNS daemons and join a real UDP multicast group on the
//! LAN interface. This file does not compile at all unless the
//! `real-network-tests` Cargo feature is enabled, and every test is
//! additionally `#[ignore]`d and panics immediately unless the
//! `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1` environment variable is set. Both
//! gates must be satisfied to actually run these tests:
//!
//!     cargo test --test discovery_multicast --features real-network-tests \
//!         -- --ignored --nocapture --test-threads=1
//!
//! with `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1` set in the environment. Do not
//! run this on a shared/CI machine without understanding that it will emit
//! real multicast packets and start real mDNS advertisements on whatever
//! network the host is attached to.
//!
//! (The one test that needed no real sockets, `protocol_magic_byte_layout`,
//! has moved to `src/net/tests.rs` as a plain unit test.)

#![cfg(feature = "real-network-tests")]

use std::time::{Duration, Instant};
use uuid::Uuid;

use vocal_calculator::net::discovery::DiscoveryService;
use vocal_calculator::net::protocol::{Capabilities, DiscoveryMessage, PROTOCOL_VERSION};

/// Panics unless `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1` is set in the
/// environment. Every test in this file calls this first, so that even a
/// direct `cargo test --features real-network-tests -- --ignored` run
/// (bypassing the `#[ignore]` ergonomics) still requires an explicit,
/// separate opt-in before touching real network sockets.
fn require_lan_opt_in() {
    let opted_in = std::env::var("VOCAL_CALCULATOR_ALLOW_LAN_TESTS")
        .map(|v| v == "1")
        .unwrap_or(false);
    assert!(
        opted_in,
        "this test starts real mDNS daemons and joins a real UDP multicast \
         group on the LAN; set VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1 to opt in \
         (this test is not meant to run in normal `cargo test` / CI)"
    );
}

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
) -> Option<(String, u16, u16)> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, svc.recv_announce()).await {
            Ok(Ok((
                DiscoveryMessage::Announce {
                    display_name,
                    tcp_port,
                    ..
                },
                _addr,
            ))) => {
                if display_name == expected_name {
                    return Some((display_name, tcp_port, tcp_port));
                }
                continue;
            }
            Ok(Ok((
                DiscoveryMessage::AnnounceV2 {
                    display_name,
                    tcp_port,
                    session_port,
                    ..
                },
                _addr,
            ))) => {
                if display_name == expected_name {
                    return Some((display_name, tcp_port, session_port));
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
#[ignore = "real LAN traffic — see doc header"]
async fn discovery_announce_bidirectional() {
    require_lan_opt_in();
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();

    let svc_a = DiscoveryService::new_with_port(id_a, "NodeA".into(), 42101, 50101, [0u8; 32])
        .await
        .expect("Failed to create DiscoveryService A");
    let svc_b = DiscoveryService::new_with_port(id_b, "NodeB".into(), 42102, 50102, [1u8; 32])
        .await
        .expect("Failed to create DiscoveryService B");

    // -- Direction 1: A announces, B should receive via UDP ----------------
    let msg_a = make_announce("NodeA", 42101, 50101);
    svc_a.announce(&msg_a).await.expect("A failed to announce");

    let result_b = recv_announce_from(&svc_b, "NodeA", Duration::from_secs(5)).await;
    assert!(
        result_b.is_some(),
        "B did not receive A's Announce within 5 seconds — UDP may be broken"
    );
    let (name, port, session_port) = result_b.unwrap();
    assert_eq!(name, "NodeA");
    assert_eq!(port, 42101);
    assert_eq!(session_port, 50101);
    println!("[OK] A -> B: B received Announce from NodeA:42101");

    // -- Direction 2: B announces, A should receive via UDP ----------------
    let msg_b = make_announce("NodeB", 42102, 50102);
    svc_b.announce(&msg_b).await.expect("B failed to announce");

    let result_a = recv_announce_from(&svc_a, "NodeB", Duration::from_secs(5)).await;
    assert!(
        result_a.is_some(),
        "A did not receive B's Announce within 5 seconds — UDP may be broken"
    );
    let (name, port, session_port) = result_a.unwrap();
    assert_eq!(name, "NodeB");
    assert_eq!(port, 42102);
    assert_eq!(session_port, 50102);
    println!("[OK] B -> A: A received Announce from NodeB:42102");
}

/// Test: discovery resolves directly to the current session port.
#[tokio::test]
#[ignore = "real LAN traffic — see doc header"]
async fn discovery_endpoint_uses_session_port() {
    require_lan_opt_in();
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();

    let svc_a = DiscoveryService::new_with_port(id_a, "NodeA".into(), 50103, 50103, [0u8; 32])
        .await
        .expect("Failed to create DiscoveryService A");
    let svc_b = DiscoveryService::new_with_port(id_b, "NodeB".into(), 50104, 50104, [1u8; 32])
        .await
        .expect("Failed to create DiscoveryService B");

    let announce = svc_b.announce_msg("NodeB");
    let endpoint = svc_a
        .endpoint_from_announcement(&announce, "127.0.0.1:9".parse().unwrap())
        .expect("B announcement should resolve to an endpoint");

    assert_eq!(endpoint.node_id, id_b);
    assert_eq!(endpoint.display_name, "NodeB");
    assert_eq!(endpoint.address.port(), 50104);
    assert_eq!(endpoint.session_port, 50104);
}

/// Test: Discover message round-trips via UDP.
#[tokio::test]
#[ignore = "real LAN traffic — see doc header"]
async fn discovery_discover_roundtrip() {
    require_lan_opt_in();
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();

    let svc_a = DiscoveryService::new_with_port(id_a, "NodeA".into(), 42105, 50105, [0u8; 32])
        .await
        .expect("Failed to create DiscoveryService A");
    let svc_b = DiscoveryService::new_with_port(id_b, "NodeB".into(), 42106, 50106, [1u8; 32])
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
