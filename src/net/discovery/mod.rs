mod mdns;
mod multicast;
mod peer_table;

use std::net::SocketAddr;

use mdns::MdnsDiscovery;
use multicast::MulticastTransport;

use crate::net::protocol::{
    Capabilities, DiscoveryMessage, NodeId, PROTOCOL_VERSION, TransportHint,
};

pub use peer_table::PeerTable;

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
/// owns a TCP listener, so there is no fixed discovery/session port conflict.
pub struct DiscoveryService {
    multicast: Option<MulticastTransport>,
    mdns: Option<MdnsDiscovery>,
    announce_msg: DiscoveryMessage,
    local_node_id: NodeId,
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
    /// The discovery TCP listener has been removed. Runtime code passes the
    /// same value for `tcp_port` and `session_port`; legacy AnnounceV2 peers can
    /// still read `tcp_port`, but it now names the session endpoint.
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

        let announce_msg = DiscoveryMessage::AnnounceV2 {
            node_id: local_node_id,
            display_name: display_name.clone(),
            tcp_port,
            capabilities: Capabilities {
                can_execute: true,
                can_control: true,
                protocol_version: PROTOCOL_VERSION,
            },
            transport_hint: TransportHint::Multicast,
            hostname,
            session_port,
        };

        let mdns = match MdnsDiscovery::new(local_node_id, display_name, session_port, public_key) {
            Ok(mdns) => Some(mdns),
            Err(e) => {
                log::warn!("mDNS discovery unavailable: {}", e);
                None
            }
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
            announce_msg,
            local_node_id,
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

    /// Return a reference to the pre-built announcement message.
    pub fn announce_msg(&self) -> &DiscoveryMessage {
        &self.announce_msg
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
