use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use tokio::net::UdpSocket;

use crate::net::protocol::{
    DiscoveryMessage, DISCOVERY_BROADCAST_PORT, PROTOCOL_MAGIC,
};

/// UDP broadcast transport for LAN peer discovery — a fallback for networks
/// where multicast traffic is blocked by the infrastructure.
///
/// Following the Localsend pattern:
/// - **Send socket**: ephemeral port with `SO_BROADCAST`, sends
///   announcements containing our TCP discovery port.
/// - **Receive socket**: fixed port 4243, listens for broadcast
///   announcements from peers.  `SO_REUSEADDR` is set so multiple
///   instances can share the port.
///
/// The actual peer confirmation happens via TCP (handled by
/// [`DiscoveryService`]), not via UDP.
pub struct BroadcastTransport {
    send_socket: UdpSocket,
    recv_socket: UdpSocket,
    targets: Vec<SocketAddr>,
}

impl BroadcastTransport {
    /// Create a new broadcast transport with separate send/receive sockets.
    pub async fn new() -> Result<Self, anyhow::Error> {
        // -- Send socket (ephemeral port, SO_BROADCAST) ---------------------
        let send_sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        send_sock.set_broadcast(true)?;
        let send_addr: socket2::SockAddr =
            SocketAddr::new("0.0.0.0".parse()?, 0).into();
        send_sock.bind(&send_addr)?;
        send_sock.set_nonblocking(true)?;
        let send_socket = UdpSocket::from_std(send_sock.into())?;

        log::info!(
            "Broadcast send socket bound on ephemeral port {}",
            send_socket.local_addr().map(|a| a.port()).unwrap_or(0),
        );

        // -- Receive socket (fixed broadcast port) --------------------------
        let recv_sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        recv_sock.set_reuse_address(true)?;
        recv_sock.set_broadcast(true)?;

        let recv_addr: socket2::SockAddr =
            SocketAddr::new("0.0.0.0".parse()?, DISCOVERY_BROADCAST_PORT).into();
        recv_sock.bind(&recv_addr)?;
        recv_sock.set_nonblocking(true)?;
        let recv_socket = UdpSocket::from_std(recv_sock.into())?;

        // -- Pre-resolve broadcast target addresses --------------------------
        let global_broadcast: SocketAddr =
            SocketAddr::new(Ipv4Addr::BROADCAST.into(), DISCOVERY_BROADCAST_PORT);
        let mut targets = vec![global_broadcast];
        for addr in subnet_broadcast_addrs() {
            if !targets.contains(&addr) {
                targets.push(addr);
            }
        }

        log::info!(
            "Broadcast transport ready -- recv on 0.0.0.0:{}, \
             send from ephemeral port, {} target(s)",
            DISCOVERY_BROADCAST_PORT,
            targets.len(),
        );

        Ok(Self {
            send_socket,
            recv_socket,
            targets,
        })
    }

    /// Send a discovery announcement via UDP broadcast.
    pub async fn announce(&self, msg: &DiscoveryMessage) -> Result<(), anyhow::Error> {
        let bincode_bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard())?;
        let mut payload = Vec::with_capacity(PROTOCOL_MAGIC.len() + bincode_bytes.len());
        payload.extend_from_slice(&PROTOCOL_MAGIC);
        payload.extend_from_slice(&bincode_bytes);

        let mut last_err: Option<anyhow::Error> = None;
        for delay_ms in [50, 150, 300] {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            for &addr in &self.targets {
                if let Err(e) = self.send_socket.send_to(&payload, addr).await {
                    log::warn!("Broadcast send_to {} failed: {}", addr, e);
                    last_err = Some(e.into());
                }
            }
        }

        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Wait for the next discovery announcement received via broadcast.
    pub async fn recv(&self) -> Result<(DiscoveryMessage, SocketAddr), anyhow::Error> {
        let mut buf = vec![0u8; 1024];
        let (len, addr) = self.recv_socket.recv_from(&mut buf).await?;
        if len < PROTOCOL_MAGIC.len() || buf[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC {
            return Err(anyhow::anyhow!(
                "Broadcast packet from {} has invalid protocol magic; discarding",
                addr,
            ));
        }
        if len == PROTOCOL_MAGIC.len() {
            return Err(anyhow::anyhow!(
                "Broadcast packet from {} is bare magic (no payload); skipping",
                addr,
            ));
        }
        let (msg, _) = bincode::serde::decode_from_slice(
            &buf[PROTOCOL_MAGIC.len()..len],
            bincode::config::standard(),
        )?;
        Ok((msg, addr))
    }
}

/// Discover local IPv4 addresses and derive /24 subnet broadcast addresses.
fn subnet_broadcast_addrs() -> Vec<SocketAddr> {
    let mut result = Vec::new();

    let probe_targets: &[(Ipv4Addr, u16)] = &[
        (Ipv4Addr::new(8, 8, 8, 8), 53),
        (Ipv4Addr::new(1, 1, 1, 1), 53),
    ];

    for &(ip, port) in probe_targets {
        let sock_addr = SocketAddrV4::new(ip, port);
        let socket2_addr: socket2::SockAddr = sock_addr.into();

        if let Ok(sock) = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        ) && sock.connect(&socket2_addr).is_ok()
            && let Ok(local) = sock.local_addr()
            && let Some(std::net::SocketAddr::V4(v4)) = local.as_socket()
        {
            let octets = v4.ip().octets();
            let bcast = Ipv4Addr::new(octets[0], octets[1], octets[2], 255);
            let addr = SocketAddr::new(bcast.into(), DISCOVERY_BROADCAST_PORT);
            if !result.contains(&addr) {
                result.push(addr);
            }
        }

        if !result.is_empty() {
            break;
        }
    }

    result
}
