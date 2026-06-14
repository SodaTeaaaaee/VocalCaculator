use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;

use crate::net::protocol::{
    DiscoveryMessage, DISCOVERY_MULTICAST_ADDR, DISCOVERY_PORT, PROTOCOL_MAGIC,
};

/// UDP multicast transport for LAN peer discovery.
///
/// Following the Localsend pattern:
/// - **Send socket**: ephemeral port, sends announcements containing our
///   TCP discovery port.
/// - **Receive socket**: fixed port 4242, joins the multicast group,
///   listens for announcements from peers.  `SO_REUSEADDR` is set so
///   multiple instances can share the port.
///
/// The actual peer confirmation happens via TCP (handled by
/// [`DiscoveryService`]), not via UDP.
pub struct MulticastTransport {
    send_socket: UdpSocket,
    recv_socket: Arc<UdpSocket>,
    multicast_target: SocketAddr,
    #[cfg(target_os = "android")]
    _multicast_lock: crate::net::android::MulticastLockGuard,
}

impl MulticastTransport {
    /// Create a new multicast transport with separate send/receive sockets.
    pub async fn new() -> Result<Self, anyhow::Error> {
        #[cfg(target_os = "android")]
        let multicast_lock = match crate::net::android::MulticastLockGuard::acquire() {
            Ok(lock) => lock,
            Err(e) => {
                log::warn!("Failed to acquire Android MulticastLock: {e}");
                return Err(anyhow::anyhow!("MulticastLock acquisition failed: {e}"));
            }
        };

        let multicast_addr: Ipv4Addr = DISCOVERY_MULTICAST_ADDR.parse()?;
        let multicast_target: SocketAddr =
            SocketAddr::new(multicast_addr.into(), DISCOVERY_PORT);

        // -- Send socket (ephemeral port, no group join) --------------------
        let send_sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        let send_addr: socket2::SockAddr =
            SocketAddr::new("0.0.0.0".parse()?, 0).into();
        send_sock.bind(&send_addr)?;
        send_sock.set_nonblocking(true)?;
        let send_socket = UdpSocket::from_std(send_sock.into())?;

        send_socket.set_multicast_loop_v4(true)?;

        if let Err(e) = send_socket.set_multicast_ttl_v4(1) {
            log::warn!("set_multicast_ttl_v4 failed (non-fatal): {e}");
        }

        log::info!(
            "Multicast send socket bound on ephemeral port {}",
            send_socket.local_addr().map(|a| a.port()).unwrap_or(0),
        );

        // -- Receive socket (fixed multicast port, joins group) -------------
        let recv_sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        recv_sock.set_reuse_address(true)?;

        let recv_addr: socket2::SockAddr =
            SocketAddr::new("0.0.0.0".parse()?, DISCOVERY_PORT).into();
        recv_sock.bind(&recv_addr)?;
        recv_sock.set_nonblocking(true)?;
        let recv_socket = UdpSocket::from_std(recv_sock.into())?;

        if let Err(e) =
            recv_socket.join_multicast_v4(multicast_addr, Ipv4Addr::UNSPECIFIED)
        {
            log::warn!(
                "IGMP join failed for {}: {e}. \
                 Multicast discovery will not work unless the OS \
                 joins the group through another mechanism.",
                multicast_addr,
            );
            return Err(e.into());
        } else {
            log::debug!("IGMP join succeeded for {}", multicast_addr);
        }

        log::info!(
            "Multicast transport ready -- recv on 0.0.0.0:{} (group {}), \
             send from ephemeral port",
            DISCOVERY_PORT,
            DISCOVERY_MULTICAST_ADDR,
        );

        Ok(Self {
            send_socket,
            recv_socket: Arc::new(recv_socket),
            multicast_target,
            #[cfg(target_os = "android")]
            _multicast_lock: multicast_lock,
        })
    }

    /// Send a discovery announcement to the multicast group.
    pub async fn announce(&self, msg: &DiscoveryMessage) -> Result<(), anyhow::Error> {
        let bincode_bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard())?;
        let mut payload = Vec::with_capacity(PROTOCOL_MAGIC.len() + bincode_bytes.len());
        payload.extend_from_slice(&PROTOCOL_MAGIC);
        payload.extend_from_slice(&bincode_bytes);

        let mut last_err: Option<anyhow::Error> = None;
        for delay_ms in [50, 150, 300] {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            if let Err(e) = self.send_socket.send_to(&payload, self.multicast_target).await {
                log::warn!("Multicast send_to failed: {}", e);
                last_err = Some(e.into());
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Wait for the next discovery announcement from the multicast group.
    pub async fn recv(&self) -> Result<(DiscoveryMessage, SocketAddr), anyhow::Error> {
        let mut buf = vec![0u8; 1024];
        let (len, addr) = self.recv_socket.recv_from(&mut buf).await?;
        if len < PROTOCOL_MAGIC.len() || buf[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC {
            return Err(anyhow::anyhow!(
                "Multicast packet from {} has invalid protocol magic; discarding",
                addr,
            ));
        }
        if len == PROTOCOL_MAGIC.len() {
            return Err(anyhow::anyhow!(
                "Multicast packet from {} is bare magic (no payload); skipping",
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
