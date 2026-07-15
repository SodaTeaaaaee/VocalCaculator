mod mdns;
mod multicast;
mod peer_table;

use std::net::SocketAddr;

use sha2::{Digest, Sha256};

use mdns::MdnsDiscovery;
use multicast::MulticastTransport;

use crate::net::protocol::{
    Capabilities, DiscoveryMessage, NodeId, PROTOCOL_VERSION, TransportHint,
};

pub use peer_table::PeerTable;

/// Whether mDNS discovery should be attempted on this platform.
///
/// mDNS is disabled on Windows by design: the bundled `mdns-sd` daemon has
/// proven unreliable across Windows Firewall / adapter configurations, and
/// UDP multicast fallback on the fixed [`crate::net::protocol::LAN_FIXED_PORT`]
/// covers the same LAN discovery need there. Pure function of the OS name
/// so it can be unit-tested without platform-specific `cfg`.
pub(crate) fn should_start_mdns(target_os: &str) -> bool {
    target_os != "windows"
}

/// Pure construction of an `AnnounceV2` message from explicit fields.
///
/// This is the seam [`DiscoveryService::announce_msg`] delegates to; it
/// exists separately so the wire-format contract (in particular, that
/// `session_port` carries whatever port was passed in -- [`crate::net::protocol::SESSION_TCP_PORT`]
/// in production) can be unit-tested without constructing any socket.
fn make_announce(
    node_id: NodeId,
    display_name: &str,
    tcp_port: u16,
    session_port: u16,
    hostname: &str,
) -> DiscoveryMessage {
    DiscoveryMessage::AnnounceV2 {
        node_id,
        display_name: display_name.to_string(),
        tcp_port,
        capabilities: Capabilities {
            can_execute: true,
            can_control: true,
            protocol_version: PROTOCOL_VERSION,
        },
        transport_hint: TransportHint::Multicast,
        hostname: hostname.to_string(),
        session_port,
    }
}

/// Short hex fingerprint of an Ed25519 public key for discovery advertisements.
pub fn public_key_fingerprint(public_key: &[u8; 32]) -> String {
    let digest = Sha256::digest(public_key);
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Endpoint information produced by discovery transports.
#[derive(Debug, Clone)]
pub struct DiscoveryEndpoint {
    pub node_id: NodeId,
    pub display_name: String,
    pub address: SocketAddr,
    pub session_port: u16,
    pub transport_hint: TransportHint,
    pub public_key_fingerprint: Option<String>,
}

/// Discovery service that only publishes and resolves session endpoints.
///
/// Identity confirmation, authorization, and routing matrix exchange happen on
/// the session TCP connection after the normal handshake. Discovery no longer
/// owns a TCP listener; the session endpoint it advertises is the fixed
/// [`crate::net::protocol::SESSION_TCP_PORT`] in `Lan` mode, not a runtime-assigned ephemeral port.
pub struct DiscoveryService {
    multicast: Option<MulticastTransport>,
    mdns: Option<MdnsDiscovery>,
    local_node_id: NodeId,
    hostname: String,
    tcp_port: u16,
    session_port: u16,
}

impl DiscoveryService {
    /// Create a discovery service that advertises the current session port.
    pub async fn new(
        local_node_id: NodeId,
        display_name: String,
        session_port: u16,
        public_key: [u8; 32],
    ) -> Result<Self, anyhow::Error> {
        Self::new_with_port(
            local_node_id,
            display_name,
            session_port,
            session_port,
            public_key,
        )
        .await
    }

    /// Test hook for supplying a legacy TCP port field.
    ///
    /// The discovery TCP listener has been removed. Production runtime code
    /// passes [`crate::net::protocol::SESSION_TCP_PORT`] for both `tcp_port` and `session_port`
    /// (the fixed LAN port); legacy AnnounceV2 peers can still read
    /// `tcp_port`, but it now names the session endpoint. Tests may pass
    /// other values to exercise multiple in-process instances without
    /// binding the same fixed port twice.
    pub async fn new_with_port(
        local_node_id: NodeId,
        display_name: String,
        tcp_port: u16,
        session_port: u16,
        public_key: [u8; 32],
    ) -> Result<Self, anyhow::Error> {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let mdns = if should_start_mdns(std::env::consts::OS) {
            match MdnsDiscovery::new(local_node_id, display_name, session_port, public_key) {
                Ok(mdns) => Some(mdns),
                Err(e) => {
                    log::warn!("mDNS discovery unavailable: {}", e);
                    None
                }
            }
        } else {
            log::info!("mDNS disabled on this platform by design");
            None
        };
        let multicast = match MulticastTransport::new().await {
            Ok(multicast) => {
                log::info!("Multicast discovery fallback active");
                Some(multicast)
            }
            Err(e) => {
                log::warn!("Multicast discovery fallback unavailable: {}", e);
                None
            }
        };

        if mdns.is_none() && multicast.is_none() {
            return Err(anyhow::anyhow!(
                "no discovery transports available (mDNS and multicast failed)"
            ));
        }

        Ok(Self {
            multicast,
            mdns,
            local_node_id,
            hostname,
            tcp_port,
            session_port,
        })
    }

    /// Send a short UDP multicast burst. This is used only at startup, when a
    /// peer asks us to scan, or when the user explicitly scans.
    pub async fn announce(&self, msg: &DiscoveryMessage) -> Result<(), anyhow::Error> {
        match &self.multicast {
            Some(multicast) => multicast.announce(msg).await,
            None => Ok(()),
        }
    }

    /// Build an announcement message using the current display name.
    pub fn announce_msg(&self, display_name: &str) -> DiscoveryMessage {
        make_announce(
            self.local_node_id,
            display_name,
            self.tcp_port,
            self.session_port,
            &self.hostname,
        )
    }

    /// Re-register mDNS metadata with the current display name.
    pub fn update_display_name(&self, display_name: &str) -> Result<(), anyhow::Error> {
        if let Some(mdns) = &self.mdns {
            mdns.update_display_name(display_name)?;
        }
        Ok(())
    }

    /// Wait for the next UDP multicast announcement.
    pub async fn recv_announce(&self) -> Result<(DiscoveryMessage, SocketAddr), anyhow::Error> {
        match &self.multicast {
            Some(multicast) => multicast.recv().await,
            None => {
                std::future::pending::<Result<(DiscoveryMessage, SocketAddr), anyhow::Error>>()
                    .await
            }
        }
    }

    /// Wait for the next mDNS/DNS-SD resolved endpoint.
    pub async fn recv_mdns_endpoint(&self) -> Result<DiscoveryEndpoint, anyhow::Error> {
        match &self.mdns {
            Some(mdns) => mdns.recv_endpoint().await,
            None => std::future::pending::<Result<DiscoveryEndpoint, anyhow::Error>>().await,
        }
    }

    /// Convert a UDP discovery message into a session endpoint.
    pub fn endpoint_from_announcement(
        &self,
        msg: &DiscoveryMessage,
        udp_addr: SocketAddr,
    ) -> Option<DiscoveryEndpoint> {
        let (node_id, display_name, tcp_port, session_port, transport_hint) = match msg {
            DiscoveryMessage::AnnounceV2 {
                node_id,
                display_name,
                tcp_port,
                session_port,
                transport_hint,
                ..
            } => (
                *node_id,
                display_name.clone(),
                *tcp_port,
                *session_port,
                *transport_hint,
            ),
            DiscoveryMessage::Announce {
                node_id,
                display_name,
                tcp_port,
                ..
            } => (
                *node_id,
                display_name.clone(),
                *tcp_port,
                *tcp_port,
                TransportHint::Multicast,
            ),
            DiscoveryMessage::Discover => return None,
        };

        if node_id == self.local_node_id {
            return None;
        }

        let port = if session_port == 0 {
            tcp_port
        } else {
            session_port
        };
        Some(DiscoveryEndpoint {
            node_id,
            display_name,
            address: SocketAddr::new(udp_addr.ip(), port),
            session_port: port,
            transport_hint,
            public_key_fingerprint: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::{
        DISCOVERY_MULTICAST_ADDR, DISCOVERY_PORT, LAN_FIXED_PORT, SESSION_TCP_PORT,
    };

    #[test]
    fn mdns_disabled_on_windows() {
        assert!(!should_start_mdns("windows"));
    }

    #[test]
    fn mdns_enabled_on_non_windows() {
        assert!(should_start_mdns("linux"));
        assert!(should_start_mdns("macos"));
        assert!(should_start_mdns("android"));
    }

    #[test]
    fn fixed_port_constants_agree() {
        assert_eq!(SESSION_TCP_PORT, DISCOVERY_PORT);
        assert_eq!(DISCOVERY_PORT, LAN_FIXED_PORT);
        assert_eq!(LAN_FIXED_PORT, 42420);
        assert_eq!(DISCOVERY_MULTICAST_ADDR, "224.0.0.167");
    }

    #[test]
    fn make_announce_carries_fixed_session_port() {
        let node_id = NodeId::new_v4();
        let msg = make_announce(
            node_id,
            "Test Node",
            SESSION_TCP_PORT,
            SESSION_TCP_PORT,
            "host",
        );
        match msg {
            DiscoveryMessage::AnnounceV2 {
                session_port,
                tcp_port,
                ..
            } => {
                assert_eq!(session_port, SESSION_TCP_PORT);
                assert_eq!(tcp_port, SESSION_TCP_PORT);
            }
            other => panic!("expected AnnounceV2, got {other:?}"),
        }
    }
}
