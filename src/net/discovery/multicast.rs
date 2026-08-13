use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;

use crate::net::protocol::{
    DISCOVERY_MULTICAST_ADDR, DISCOVERY_PORT, DiscoveryMessage, PROTOCOL_MAGIC, valid_display_name,
};

const MAX_DISCOVERY_DATAGRAM_LENGTH: usize = 1024;

/// UDP multicast transport for LAN peer discovery.
///
/// Following the Localsend pattern:
/// - **Send socket**: ephemeral port, sends announcements containing our
///   current session endpoint.
/// - **Receive socket**: shared UDP discovery port, joins the multicast group,
///   listens for announcements from peers.  `SO_REUSEADDR` is set so
///   multiple instances can share the port.
///
/// Identity confirmation and authorization happen later on the session TCP
/// connection, not on a separate discovery TCP listener.
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
        let multicast_target: SocketAddr = SocketAddr::new(multicast_addr.into(), DISCOVERY_PORT);

        // -- Send socket (ephemeral port, no group join) --------------------
        let send_sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        let send_addr: socket2::SockAddr = SocketAddr::new("0.0.0.0".parse()?, 0).into();
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

        // -- Receive socket (shared multicast port, joins group) -------------
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

        if let Err(e) = recv_socket.join_multicast_v4(multicast_addr, Ipv4Addr::UNSPECIFIED) {
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
        let bincode_bytes = bincode::serde::encode_to_vec(
            msg,
            bincode::config::standard().with_limit::<MAX_DISCOVERY_DATAGRAM_LENGTH>(),
        )?;
        let mut payload = Vec::with_capacity(PROTOCOL_MAGIC.len() + bincode_bytes.len());
        payload.extend_from_slice(&PROTOCOL_MAGIC);
        payload.extend_from_slice(&bincode_bytes);

        let mut last_err: Option<anyhow::Error> = None;
        for delay_ms in [50, 150, 300] {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            if let Err(e) = self
                .send_socket
                .send_to(&payload, self.multicast_target)
                .await
            {
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
        let mut buf = vec![0u8; MAX_DISCOVERY_DATAGRAM_LENGTH];
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
        let msg = decode_discovery_message(&buf[PROTOCOL_MAGIC.len()..len])?;
        Ok((msg, addr))
    }
}

fn decode_discovery_message(bytes: &[u8]) -> Result<DiscoveryMessage, anyhow::Error> {
    let (message, consumed) = bincode::serde::decode_from_slice::<DiscoveryMessage, _>(
        bytes,
        bincode::config::standard().with_limit::<MAX_DISCOVERY_DATAGRAM_LENGTH>(),
    )?;
    if consumed != bytes.len() {
        return Err(anyhow::anyhow!(
            "discovery message has trailing bytes: consumed {}, datagram {}",
            consumed,
            bytes.len(),
        ));
    }
    let name_is_valid = match &message {
        DiscoveryMessage::Announce { display_name, .. }
        | DiscoveryMessage::AnnounceV2 { display_name, .. } => valid_display_name(display_name),
        DiscoveryMessage::Discover => true,
    };
    if !name_is_valid {
        return Err(anyhow::anyhow!(
            "discovery message has invalid display name"
        ));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::Capabilities;

    #[test]
    fn discovery_decoder_requires_full_datagram_consumption() {
        let message = DiscoveryMessage::Announce {
            node_id: uuid::Uuid::new_v4(),
            display_name: "peer".to_string(),
            tcp_port: 42420,
            capabilities: Capabilities {
                can_execute: true,
                can_control: true,
                protocol_version: crate::net::protocol::PROTOCOL_VERSION,
            },
        };
        let mut encoded =
            bincode::serde::encode_to_vec(&message, bincode::config::standard()).unwrap();
        assert!(decode_discovery_message(&encoded).is_ok());
        encoded.push(0xff);
        assert!(decode_discovery_message(&encoded).is_err());
    }

    #[test]
    fn discovery_decoder_rejects_invalid_name_and_over_limit_payload() {
        let invalid_name = DiscoveryMessage::Announce {
            node_id: uuid::Uuid::new_v4(),
            display_name: "x".repeat(crate::net::protocol::MAX_DISPLAY_NAME_BYTES + 1),
            tcp_port: 42420,
            capabilities: Capabilities {
                can_execute: true,
                can_control: true,
                protocol_version: crate::net::protocol::PROTOCOL_VERSION,
            },
        };
        let encoded =
            bincode::serde::encode_to_vec(&invalid_name, bincode::config::standard()).unwrap();
        assert!(decode_discovery_message(&encoded).is_err());

        let oversized_message = DiscoveryMessage::Announce {
            node_id: uuid::Uuid::new_v4(),
            display_name: "x".repeat(MAX_DISCOVERY_DATAGRAM_LENGTH),
            tcp_port: 42420,
            capabilities: Capabilities {
                can_execute: true,
                can_control: true,
                protocol_version: crate::net::protocol::PROTOCOL_VERSION,
            },
        };
        let oversized =
            bincode::serde::encode_to_vec(&oversized_message, bincode::config::standard()).unwrap();
        assert!(oversized.len() > MAX_DISCOVERY_DATAGRAM_LENGTH);
        assert!(decode_discovery_message(&oversized).is_err());
    }
}
