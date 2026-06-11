use std::net::SocketAddr;
use tokio::net::UdpSocket;

use crate::net::protocol::{
    DiscoveryMessage, DISCOVERY_MULTICAST_ADDR, DISCOVERY_PORT, PROTOCOL_MAGIC,
};

/// UDP multicast service for peer discovery on the local network.
pub struct DiscoveryService {
    socket: UdpSocket,
    /// Android MulticastLock guard. Held for the lifetime of the discovery
    /// service so that the Wi-Fi stack does not drop multicast traffic.
    #[cfg(target_os = "android")]
    _multicast_lock: crate::net::android::MulticastLockGuard,
}

impl DiscoveryService {
    /// Create a new discovery service, binding to the multicast group.
    ///
    /// Uses `SO_REUSEADDR` so multiple instances on the same machine can
    /// all receive multicast traffic on the shared discovery port.
    pub async fn new() -> Result<Self, anyhow::Error> {
        // On Android, acquire a Wi-Fi MulticastLock before binding.
        // Without this, the Wi-Fi stack silently drops all multicast traffic
        // to save power, and discovery never sees any peers.
        #[cfg(target_os = "android")]
        let multicast_lock = match crate::net::android::MulticastLockGuard::acquire() {
            Ok(lock) => lock,
            Err(e) => {
                log::warn!("Failed to acquire Android MulticastLock: {e}");
                return Err(anyhow::anyhow!("MulticastLock acquisition failed: {e}"));
            }
        };

        // Use socket2 to set SO_REUSEADDR so multiple instances on the same
        // machine can bind to the multicast port simultaneously.
        let addr = SocketAddr::new("0.0.0.0".parse()?, DISCOVERY_PORT);
        let socket2_addr: socket2::SockAddr = addr.into();
        let sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        sock.set_reuse_address(true)?;
        #[cfg(unix)]
        sock.set_reuse_port(true)?;
        sock.bind(&socket2_addr)?;
        // Convert to tokio UdpSocket.
        sock.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(sock.into())?;

        // Join multicast group.
        let multicast_addr: std::net::Ipv4Addr = DISCOVERY_MULTICAST_ADDR.parse()?;
        socket.join_multicast_v4(multicast_addr, std::net::Ipv4Addr::UNSPECIFIED)?;

        // Enable multicast loopback so the sender can receive its own packets
        // (needed for same-machine testing; on Windows the default may be off).
        socket.set_multicast_loop_v4(true)?;
        // Set multicast TTL to 1 (local subnet only).
        socket.set_multicast_ttl_v4(1)?;

        log::info!("Discovery service bound on UDP {}:{}", "0.0.0.0", DISCOVERY_PORT);

        Ok(Self {
            socket,
            #[cfg(target_os = "android")]
            _multicast_lock: multicast_lock,
        })
    }

    /// Broadcast a discovery message to the multicast group.
    ///
    /// Prepends [`PROTOCOL_MAGIC`] before the bincode payload so that
    /// receivers can filter out non-Vocal-Calc traffic on the shared
    /// multicast group.
    pub async fn announce(&self, msg: &DiscoveryMessage) -> Result<(), anyhow::Error> {
        let bincode_bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard())?;
        let mut payload = Vec::with_capacity(PROTOCOL_MAGIC.len() + bincode_bytes.len());
        payload.extend_from_slice(&PROTOCOL_MAGIC);
        payload.extend_from_slice(&bincode_bytes);
        let multicast_addr: SocketAddr =
            format!("{}:{}", DISCOVERY_MULTICAST_ADDR, DISCOVERY_PORT).parse()?;
        self.socket.send_to(&payload, multicast_addr).await?;
        Ok(())
    }

    /// Wait for and receive the next discovery message from the multicast group.
    ///
    /// Checks that the first 8 bytes match [`PROTOCOL_MAGIC`] before
    /// attempting to decode. Packets with a mismatched prefix are
    /// silently discarded.
    pub async fn recv(&self) -> Result<(DiscoveryMessage, SocketAddr), anyhow::Error> {
        let mut buf = vec![0u8; 1024];
        let (len, addr) = self.socket.recv_from(&mut buf).await?;
        if len < PROTOCOL_MAGIC.len()
            || buf[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC
        {
            return Err(anyhow::anyhow!(
                "Discovery packet from {} has invalid protocol magic; discarding",
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
