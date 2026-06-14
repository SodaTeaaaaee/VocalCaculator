//! Standalone diagnostic test for UDP multicast and broadcast on this machine.
//!
//! This test does NOT depend on any `vocal_calculator` crate internals.
//! It uses raw `socket2` + `tokio` sockets to verify whether the OS and
//! network stack deliver multicast / broadcast traffic between two sockets
//! on the same host.
//!
//! Key observations this test checks:
//!   - Can two sockets bind to the same port with SO_REUSEADDR?
//!   - Does multicast group join succeed?
//!   - Does the peer receive the sender's multicast packet?
//!   - Does the sender receive its own loopback echo?
//!   - Does the peer receive the second sender's packet (after draining self-echoes)?
//!   - Same questions for broadcast.
//!
//! Run with:
//!     cargo test --test test_udp_transport -- --nocapture --test-threads=1

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::UdpSocket;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 99);
const MULTICAST_PORT: u16 = 4242;
const BROADCAST_PORT: u16 = 4243;

const RECV_TIMEOUT: Duration = Duration::from_secs(3);
/// Short timeout for draining self-echoes — packets should arrive near-instantly.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(200);

const MSG_A_TO_B: &[u8] = b"hello-from-A";
const MSG_B_TO_A: &[u8] = b"hello-from-B";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a UDP socket bound to `0.0.0.0:<port>` with `SO_REUSEADDR`.
/// Converts to a non-blocking tokio `UdpSocket`.
fn make_reuse_socket(port: u16) -> Result<UdpSocket, Box<dyn std::error::Error>> {
    let addr: SockAddr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port).into();
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    // On Windows, SO_REUSEADDR for UDP is sufficient for port sharing.
    // On Unix you would also call set_reuse_port(true).
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.bind(&addr)?;
    sock.set_nonblocking(true)?;
    let std_sock: std::net::UdpSocket = sock.into();
    let tokio_sock = UdpSocket::from_std(std_sock)?;
    Ok(tokio_sock)
}

/// Create a UDP socket with `SO_REUSEADDR + SO_BROADCAST` bound to `0.0.0.0:<port>`.
fn make_broadcast_socket(port: u16) -> Result<UdpSocket, Box<dyn std::error::Error>> {
    let addr: SockAddr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port).into();
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    sock.set_broadcast(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.bind(&addr)?;
    sock.set_nonblocking(true)?;
    let std_sock: std::net::UdpSocket = sock.into();
    let tokio_sock = UdpSocket::from_std(std_sock)?;
    // Re-assert SO_BROADCAST on the tokio socket as well (matches production code).
    tokio_sock.set_broadcast(true)?;
    Ok(tokio_sock)
}

/// Try to receive a single datagram within `timeout`.
/// Returns `Some((bytes, sender_addr))` or `None` on timeout / error.
async fn try_recv(
    sock: &UdpSocket,
    timeout: Duration,
) -> Option<(Vec<u8>, SocketAddr)> {
    let mut buf = vec![0u8; 2048];
    match tokio::time::timeout(timeout, sock.recv_from(&mut buf)).await {
        Ok(Ok((len, addr))) => {
            buf.truncate(len);
            Some((buf, addr))
        }
        Ok(Err(e)) => {
            println!("    [recv error] {e}");
            None
        }
        Err(_) => None,
    }
}

/// Drain ALL pending packets on `sock` within `timeout` (per-packet).
/// Returns the collected packets. Stops when a recv times out.
async fn drain_all(
    sock: &UdpSocket,
    per_packet_timeout: Duration,
) -> Vec<(Vec<u8>, SocketAddr)> {
    let mut packets = Vec::new();
    loop {
        match try_recv(sock, per_packet_timeout).await {
            Some(pkt) => {
                packets.push(pkt);
            }
            None => break,
        }
    }
    packets
}

/// Wait for a packet matching `expected_payload` on `sock`, skipping any
/// other packets (self-echoes, stale data). Returns `true` if found.
async fn wait_for_packet(
    sock: &UdpSocket,
    expected_payload: &[u8],
    timeout: Duration,
) -> (bool, Vec<(Vec<u8>, SocketAddr)>) {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut seen = Vec::new();
    loop {
        let remaining = deadline.duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return (false, seen);
        }
        match try_recv(sock, remaining).await {
            Some((data, addr)) => {
                if data == expected_payload {
                    seen.push((data, addr));
                    return (true, seen);
                }
                // Wrong payload — record and keep draining.
                seen.push((data, addr));
            }
            None => return (false, seen),
        }
    }
}

// ---------------------------------------------------------------------------
// Multicast Test
// ---------------------------------------------------------------------------

/// Multicast test: two sockets join `239.255.42.99:4242`, exchange messages.
///
/// With loopback enabled, the sender also receives its own packet. This test
/// drains self-echoes before checking the reverse direction.
#[tokio::test]
async fn multicast_send_recv() {
    println!("================================================================");
    println!("  MULTICAST TEST (239.255.42.99:{MULTICAST_PORT})");
    println!("================================================================");

    // --- Step 1 & 2: Create two sockets, both join the multicast group ---------
    println!("\n[1] Creating socket A bound to 0.0.0.0:{MULTICAST_PORT} ...");
    let sock_a = make_reuse_socket(MULTICAST_PORT)
        .expect("Failed to create socket A -- is port 4242 already in exclusive use?");
    println!("    socket A: local_addr={:?}", sock_a.local_addr());

    println!("[2] Creating socket B bound to 0.0.0.0:{MULTICAST_PORT} ...");
    let sock_b = make_reuse_socket(MULTICAST_PORT)
        .expect("Failed to create socket B -- SO_REUSEADDR may not be working");
    println!("    socket B: local_addr={:?}", sock_b.local_addr());

    println!("[3] Joining multicast group {MULTICAST_GROUP} on both sockets ...");
    if let Err(e) = sock_a.join_multicast_v4(MULTICAST_GROUP, Ipv4Addr::UNSPECIFIED) {
        println!("    [FAIL] socket A join_multicast_v4: {e}");
        panic!("Multicast join failed on socket A: {e}");
    }
    println!("    socket A: joined OK");
    if let Err(e) = sock_b.join_multicast_v4(MULTICAST_GROUP, Ipv4Addr::UNSPECIFIED) {
        println!("    [FAIL] socket B join_multicast_v4: {e}");
        panic!("Multicast join failed on socket B: {e}");
    }
    println!("    socket B: joined OK");

    // Enable loopback so we can test same-host delivery.
    println!("[4] Enabling multicast loopback on both sockets ...");
    sock_a.set_multicast_loop_v4(true).expect("set_multicast_loop_v4 on A");
    sock_b.set_multicast_loop_v4(true).expect("set_multicast_loop_v4 on B");
    println!("    loopback enabled on both");

    // Set TTL (best-effort, may fail on some Windows configs).
    if let Err(e) = sock_a.set_multicast_ttl_v4(1) {
        println!("    [warn] set_multicast_ttl_v4 on A: {e} (non-fatal)");
    }

    // Small delay for IGMP join to propagate.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // --- Direction A -> B -------------------------------------------------------
    let multicast_dest: SocketAddr = format!("{MULTICAST_GROUP}:{MULTICAST_PORT}").parse().unwrap();

    println!("\n--- Direction: A -> B ---");
    println!("[5] Socket A sending {:?} -> {multicast_dest} ...", String::from_utf8_lossy(MSG_A_TO_B));
    match sock_a.send_to(MSG_A_TO_B, multicast_dest).await {
        Ok(n) => println!("    sent {n} bytes"),
        Err(e) => {
            println!("    [FAIL] send: {e}");
            panic!("Socket A send failed: {e}");
        }
    }

    println!("[6] Socket B waiting for A's message (timeout {RECV_TIMEOUT:?}) ...");
    let (found_b, seen_b) = wait_for_packet(&sock_b, MSG_A_TO_B, RECV_TIMEOUT).await;
    if found_b {
        println!("    [OK] Socket B received A's multicast packet");
    } else {
        println!("    [FAIL] Socket B did not receive A's multicast packet within {RECV_TIMEOUT:?}");
        println!("    Packets seen by B: {seen_b:?}");
        panic!("Multicast A->B failed: no packet delivered to socket B");
    }

    // Drain socket A's self-loopback from A's send.
    println!("[7] Draining socket A's self-loopback echoes ...");
    let echoes_a = drain_all(&sock_a, DRAIN_TIMEOUT).await;
    println!("    drained {} stale packet(s) from socket A", echoes_a.len());
    for (i, (data, addr)) in echoes_a.iter().enumerate() {
        println!("      echo[{i}]: {} bytes from {addr}: {:?}",
            data.len(), String::from_utf8_lossy(data));
    }

    // --- Direction B -> A -------------------------------------------------------
    println!("\n--- Direction: B -> A ---");
    println!("[8] Socket B sending {:?} -> {multicast_dest} ...", String::from_utf8_lossy(MSG_B_TO_A));
    match sock_b.send_to(MSG_B_TO_A, multicast_dest).await {
        Ok(n) => println!("    sent {n} bytes"),
        Err(e) => {
            println!("    [FAIL] send: {e}");
            panic!("Socket B send failed: {e}");
        }
    }

    println!("[9] Socket A waiting for B's message (timeout {RECV_TIMEOUT:?}) ...");
    let (found_a, seen_a) = wait_for_packet(&sock_a, MSG_B_TO_A, RECV_TIMEOUT).await;
    if found_a {
        println!("    [OK] Socket A received B's multicast packet");
    } else {
        println!("    [FAIL] Socket A did not receive B's multicast packet within {RECV_TIMEOUT:?}");
        println!("    Packets seen by A: {seen_a:?}");
        panic!("Multicast B->A failed: no packet delivered to socket A");
    }

    // Drain B's self-loopback.
    println!("[10] Draining socket B's self-loopback echoes ...");
    let echoes_b = drain_all(&sock_b, DRAIN_TIMEOUT).await;
    println!("     drained {} stale packet(s) from socket B", echoes_b.len());

    println!("\n=== MULTICAST TEST PASSED ===");
    println!("    Multicast send/recv works in both directions on this machine.");
    println!("    Self-loopback echoes were observed and drained successfully.\n");
}

// ---------------------------------------------------------------------------
// Broadcast Test
// ---------------------------------------------------------------------------

/// Broadcast test: two sockets on port 4243 with SO_BROADCAST, exchange messages.
#[tokio::test]
async fn broadcast_send_recv() {
    println!("================================================================");
    println!("  BROADCAST TEST (255.255.255.255:{BROADCAST_PORT})");
    println!("================================================================");

    // --- Create two sockets with SO_REUSEADDR + SO_BROADCAST -------------------
    println!("\n[1] Creating socket A bound to 0.0.0.0:{BROADCAST_PORT} (SO_REUSEADDR + SO_BROADCAST) ...");
    let sock_a = make_broadcast_socket(BROADCAST_PORT)
        .expect("Failed to create broadcast socket A");
    println!("    socket A: local_addr={:?}", sock_a.local_addr());

    println!("[2] Creating socket B bound to 0.0.0.0:{BROADCAST_PORT} (SO_REUSEADDR + SO_BROADCAST) ...");
    let sock_b = make_broadcast_socket(BROADCAST_PORT)
        .expect("Failed to create broadcast socket B");
    println!("    socket B: local_addr={:?}", sock_b.local_addr());

    let broadcast_dest: SocketAddr = format!("255.255.255.255:{BROADCAST_PORT}").parse().unwrap();

    // --- Direction A -> B -------------------------------------------------------
    println!("\n--- Direction: A -> B ---");
    println!("[3] Socket A sending {:?} -> {broadcast_dest} ...", String::from_utf8_lossy(MSG_A_TO_B));
    match sock_a.send_to(MSG_A_TO_B, broadcast_dest).await {
        Ok(n) => println!("    sent {n} bytes"),
        Err(e) => {
            println!("    [FAIL] send: {e}");
            panic!("Socket A broadcast send failed: {e}");
        }
    }

    println!("[4] Socket B waiting for A's broadcast (timeout {RECV_TIMEOUT:?}) ...");
    let (found_b, seen_b) = wait_for_packet(&sock_b, MSG_A_TO_B, RECV_TIMEOUT).await;
    if found_b {
        println!("    [OK] Socket B received A's broadcast packet");
    } else {
        println!("    [FAIL] Socket B did not receive A's broadcast packet within {RECV_TIMEOUT:?}");
        println!("    Packets seen by B: {seen_b:?}");
        panic!("Broadcast A->B failed: no packet delivered to socket B");
    }

    // Drain socket A's self-loopback from A's send.
    println!("[5] Draining socket A's self-loopback echoes ...");
    let echoes_a = drain_all(&sock_a, DRAIN_TIMEOUT).await;
    println!("    drained {} stale packet(s) from socket A", echoes_a.len());
    for (i, (data, addr)) in echoes_a.iter().enumerate() {
        println!("      echo[{i}]: {} bytes from {addr}: {:?}",
            data.len(), String::from_utf8_lossy(data));
    }

    // --- Direction B -> A -------------------------------------------------------
    println!("\n--- Direction: B -> A ---");
    println!("[6] Socket B sending {:?} -> {broadcast_dest} ...", String::from_utf8_lossy(MSG_B_TO_A));
    match sock_b.send_to(MSG_B_TO_A, broadcast_dest).await {
        Ok(n) => println!("    sent {n} bytes"),
        Err(e) => {
            println!("    [FAIL] send: {e}");
            panic!("Socket B broadcast send failed: {e}");
        }
    }

    println!("[7] Socket A waiting for B's broadcast (timeout {RECV_TIMEOUT:?}) ...");
    let (found_a, seen_a) = wait_for_packet(&sock_a, MSG_B_TO_A, RECV_TIMEOUT).await;
    if found_a {
        println!("    [OK] Socket A received B's broadcast packet");
    } else {
        println!("    [FAIL] Socket A did not receive B's broadcast packet within {RECV_TIMEOUT:?}");
        println!("    Packets seen by A: {seen_a:?}");
        panic!("Broadcast B->A failed: no packet delivered to socket A");
    }

    // Drain B's self-loopback.
    println!("[8] Draining socket B's self-loopback echoes ...");
    let echoes_b = drain_all(&sock_b, DRAIN_TIMEOUT).await;
    println!("    drained {} stale packet(s) from socket B", echoes_b.len());

    println!("\n=== BROADCAST TEST PASSED ===");
    println!("    Broadcast send/recv works in both directions on this machine.");
    println!("    Self-loopback echoes were observed and drained successfully.\n");
}

// ---------------------------------------------------------------------------
// Additional diagnostic: verify multicast loopback delivers to self
// ---------------------------------------------------------------------------

/// Verifies that a single socket receives its own multicast packet (loopback).
/// This is the simplest possible multicast sanity check.
#[tokio::test]
async fn multicast_self_loopback() {
    println!("================================================================");
    println!("  MULTICAST SELF-LOOPBACK TEST");
    println!("================================================================\n");

    let port = 4244; // Use a separate port to avoid collision with the main test.
    println!("[1] Creating socket on 0.0.0.0:{port} ...");
    let sock = make_reuse_socket(port).expect("Failed to create socket");
    println!("    local_addr={:?}", sock.local_addr());

    println!("[2] Joining multicast group {MULTICAST_GROUP} ...");
    sock.join_multicast_v4(MULTICAST_GROUP, Ipv4Addr::UNSPECIFIED)
        .expect("join_multicast_v4 failed");
    println!("    joined OK");

    sock.set_multicast_loop_v4(true).expect("set_multicast_loop_v4");
    println!("    loopback enabled");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let dest: SocketAddr = format!("{MULTICAST_GROUP}:{port}").parse().unwrap();
    let payload = b"self-loopback-probe";

    println!("[3] Sending {payload:?} -> {dest} ...");
    sock.send_to(payload, dest).await.expect("send failed");

    println!("[4] Waiting for self-echo (timeout {RECV_TIMEOUT:?}) ...");
    let (found, seen) = wait_for_packet(&sock, payload, RECV_TIMEOUT).await;
    if found {
        println!("    [OK] Socket received its own multicast packet");
    } else {
        println!("    [FAIL] Self-echo not received");
        println!("    Packets seen: {seen:?}");
        panic!("Multicast self-loopback failed -- loopback may be disabled by OS/firewall");
    }

    println!("\n=== SELF-LOOPBACK TEST PASSED ===\n");
}

/// Verifies that a single socket receives its own broadcast packet (loopback).
#[tokio::test]
async fn broadcast_self_loopback() {
    println!("================================================================");
    println!("  BROADCAST SELF-LOOPBACK TEST");
    println!("================================================================\n");

    let port = 4245; // Separate port.
    println!("[1] Creating socket on 0.0.0.0:{port} (SO_BROADCAST) ...");
    let sock = make_broadcast_socket(port).expect("Failed to create socket");
    println!("    local_addr={:?}", sock.local_addr());

    let dest: SocketAddr = format!("255.255.255.255:{port}").parse().unwrap();
    let payload = b"self-loopback-probe";

    println!("[2] Sending {payload:?} -> {dest} ...");
    sock.send_to(payload, dest).await.expect("send failed");

    println!("[3] Waiting for self-echo (timeout {RECV_TIMEOUT:?}) ...");
    let (found, seen) = wait_for_packet(&sock, payload, RECV_TIMEOUT).await;
    if found {
        println!("    [OK] Socket received its own broadcast packet");
    } else {
        println!("    [FAIL] Self-echo not received");
        println!("    Packets seen: {seen:?}");
        panic!("Broadcast self-loopback failed -- broadcast may be blocked by firewall");
    }

    println!("\n=== BROADCAST SELF-LOOPBACK TEST PASSED ===\n");
}
