mod broadcast;
mod multicast;
mod peer_table;

use std::net::SocketAddr;
use std::time::Duration;

use broadcast::BroadcastTransport;
use futures::{SinkExt, StreamExt};
use multicast::MulticastTransport;
use tokio::net::TcpListener;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::net::protocol::{DiscoveryMessage, PROTOCOL_MAGIC, TransportHint};

pub use peer_table::PeerTable;

/// Result of a successful TCP discovery exchange.
pub struct DiscoveryExchange {
    pub node_id: crate::net::protocol::NodeId,
    pub display_name: String,
    pub tcp_port: u16,
    pub peer_addr: SocketAddr,
    /// The peer's session listener port (ephemeral).
    /// For legacy Announce (v1) peers, falls back to `tcp_port`.
    pub session_port: u16,
}

/// Unified discovery service using the Localsend pattern:
///
/// - **UDP multicast + broadcast**: sends announcements and listens for
///   announcements from peers.
/// - **TCP listener**: accepts incoming connections for bidirectional
///   identity exchange and peer confirmation.
///
/// Flow:
/// 1. Periodically send UDP announcements (multicast + broadcast).
/// 2. Listen for UDP announcements from peers.
/// 3. When a UDP announcement is received, connect to the peer via TCP.
/// 4. Accept incoming TCP connections from peers who received our
///    announcements.
/// 5. TCP connection = peer confirmed.
pub struct DiscoveryService {
    tcp_listener: TcpListener,
    multicast: Option<MulticastTransport>,
    broadcast: BroadcastTransport,
    announce_msg: DiscoveryMessage,
}

impl DiscoveryService {
    /// Create a new discovery service with an ephemeral TCP port.
    ///
    /// Uses port 0 so the OS assigns a free port.  The actual port is
    /// advertised in the `AnnounceV2` message via UDP, so peers can
    /// always connect back correctly — no fixed-port conflicts.
    pub async fn new(
        local_node_id: crate::net::protocol::NodeId,
        display_name: String,
        session_port: u16,
    ) -> Result<Self, anyhow::Error> {
        Self::new_with_port(local_node_id, display_name, 0, session_port).await
    }

    /// Create a new discovery service with a custom TCP port.
    ///
    /// Binds a TCP listener on the given port for incoming peer connections.
    /// Also initializes UDP multicast and broadcast transports for
    /// announcements and listening.
    pub async fn new_with_port(
        local_node_id: crate::net::protocol::NodeId,
        display_name: String,
        tcp_port: u16,
        session_port: u16,
    ) -> Result<Self, anyhow::Error> {
        use crate::net::protocol::{Capabilities, PROTOCOL_VERSION};

        // -- TCP listener (ephemeral port for peer handshake) -----------------
        // Binds to port 0 so the OS picks a free port.  The actual port is
        // advertised in the AnnounceV2 message via UDP, so peers always
        // connect to the right address.  No SO_REUSEADDR needed — each
        // instance gets its own unique port, avoiding Windows "port stealing"
        // semantics where connections are non-deterministically delivered.
        let bind_addr = SocketAddr::new(
            "0.0.0.0".parse().expect("valid constant address"),
            tcp_port,
        );
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        let sock_addr: socket2::SockAddr = bind_addr.into();
        socket.bind(&sock_addr)?;
        socket.listen(128)?;
        let std_listener: std::net::TcpListener = socket.into();
        std_listener.set_nonblocking(true)?;
        let tcp_listener = TcpListener::from_std(std_listener)?;
        let actual_port = tcp_listener.local_addr().map(|a| a.port()).unwrap_or(0);
        log::info!("TCP discovery listener bound on 0.0.0.0:{}", actual_port);

        // -- UDP transports (send + receive) ---------------------------------
        let multicast = match MulticastTransport::new().await {
            Ok(t) => {
                log::info!("Multicast transport active");
                Some(t)
            }
            Err(e) => {
                log::warn!(
                    "Multicast transport unavailable, falling back to broadcast: {}",
                    e
                );
                None
            }
        };

        let broadcast = BroadcastTransport::new().await?;

        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let announce_msg = DiscoveryMessage::AnnounceV2 {
            node_id: local_node_id,
            display_name,
            tcp_port: actual_port,
            capabilities: Capabilities {
                can_execute: true,
                can_control: true,
                protocol_version: PROTOCOL_VERSION,
            },
            transport_hint: TransportHint::Multicast,
            hostname,
            session_port,
        };

        Ok(Self {
            tcp_listener,
            multicast,
            broadcast,
            announce_msg,
        })
    }

    /// Send a discovery announcement on **every** active UDP transport.
    ///
    /// Errors from individual transports are logged at warn level; the
    /// method returns `Err` only when **all** transports fail.
    pub async fn announce(&self, msg: &DiscoveryMessage) -> Result<(), anyhow::Error> {
        // Run multicast and broadcast in parallel to cut announce time in half.
        let (mc_result, bc_result) = tokio::join!(
            async {
                if let Some(mc) = &self.multicast {
                    mc.announce(msg).await
                } else {
                    Ok(())
                }
            },
            self.broadcast.announce(msg),
        );

        let mut any_succeeded = false;
        let mut last_err: Option<anyhow::Error> = None;

        match mc_result {
            Ok(()) => any_succeeded = true,
            Err(e) => {
                log::warn!("Multicast announce error: {}", e);
                last_err = Some(e);
            }
        }

        match bc_result {
            Ok(()) => any_succeeded = true,
            Err(e) => {
                log::warn!("Broadcast announce error: {}", e);
                last_err = Some(e);
            }
        }

        if any_succeeded {
            Ok(())
        } else {
            Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no transports available")))
        }
    }

    /// Return a reference to the pre-built announcement message.
    pub fn announce_msg(&self) -> &DiscoveryMessage {
        &self.announce_msg
    }

    /// Wait for the next **UDP announcement** from any transport.
    ///
    /// Uses `tokio::select!` to multiplex across multicast and broadcast
    /// receive sockets.  Returns the first successfully decoded message.
    pub async fn recv_announce(&self) -> Result<(DiscoveryMessage, SocketAddr), anyhow::Error> {
        match &self.multicast {
            Some(mc) => {
                tokio::select! {
                    result = mc.recv() => result,
                    result = self.broadcast.recv() => result,
                }
            }
            None => self.broadcast.recv().await,
        }
    }

    /// Wait for the next **incoming TCP connection** from a peer.
    ///
    /// Accepts a TCP connection on the discovery port and performs a
    /// bidirectional `DiscoveryMessage` exchange.  Returns the peer's
    /// identity on success.
    pub async fn accept_peer(&self) -> Result<DiscoveryExchange, anyhow::Error> {
        let (stream, _peer_addr) = self.tcp_listener.accept().await?;
        stream.set_nodelay(true).unwrap_or_else(|e| {
            log::warn!("set_nodelay failed on discovery accept stream: {e}");
        });

        let local_msg = self.announce_msg.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            Self::exchange_over_stream(stream, local_msg),
        )
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(anyhow::anyhow!("TCP discovery exchange timed out")),
        }
    }

    /// Perform the bidirectional DiscoveryMessage exchange over a TCP stream.
    ///
    /// Both sides send their Announce, then both sides read the peer's
    /// Announce.  Uses length-delimited framing with PROTOCOL_MAGIC prefix.
    async fn exchange_over_stream(
        stream: tokio::net::TcpStream,
        local_msg: DiscoveryMessage,
    ) -> Result<DiscoveryExchange, anyhow::Error> {
        let peer_addr = stream.peer_addr()?;
        let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

        // Send our identity.
        let bincode_bytes =
            bincode::serde::encode_to_vec(&local_msg, bincode::config::standard())?;
        let mut payload = Vec::with_capacity(PROTOCOL_MAGIC.len() + bincode_bytes.len());
        payload.extend_from_slice(&PROTOCOL_MAGIC);
        payload.extend_from_slice(&bincode_bytes);
        framed
            .send(tokio_util::bytes::Bytes::from(payload))
            .await?;

        // Read the peer's identity.
        let bytes = framed
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("Connection closed before discovery message"))?
            .map_err(|e| anyhow::anyhow!("TCP read error: {}", e))?;

        if bytes.len() < PROTOCOL_MAGIC.len()
            || bytes[..PROTOCOL_MAGIC.len()] != PROTOCOL_MAGIC
        {
            return Err(anyhow::anyhow!("Invalid protocol magic from {}", peer_addr));
        }

        let (msg, _) = bincode::serde::decode_from_slice::<DiscoveryMessage, _>(
            &bytes[PROTOCOL_MAGIC.len()..],
            bincode::config::standard(),
        )?;

        let (node_id, name, port, session_port) = match &msg {
            DiscoveryMessage::AnnounceV2 {
                node_id,
                display_name,
                tcp_port,
                session_port,
                ..
            } => (*node_id, display_name.clone(), *tcp_port, *session_port),
            DiscoveryMessage::Announce {
                node_id,
                display_name,
                tcp_port,
                ..
            } => (*node_id, display_name.clone(), *tcp_port, *tcp_port),
            _ => {
                return Err(anyhow::anyhow!(
                    "Expected Announce from {}, got {:?}",
                    peer_addr,
                    msg,
                ));
            }
        };

        // Ignore self.
        let local_id = match &local_msg {
            DiscoveryMessage::AnnounceV2 { node_id, .. }
            | DiscoveryMessage::Announce { node_id, .. } => *node_id,
            _ => unreachable!(),
        };
        if node_id == local_id {
            return Err(anyhow::anyhow!("Ignoring self-discovery"));
        }

        log::info!(
            "TCP discovery exchange: peer {} ({}) at {}:{} (session:{})",
            name,
            node_id,
            peer_addr.ip(),
            port,
            session_port,
        );

        Ok(DiscoveryExchange {
            node_id,
            display_name: name,
            tcp_port: port,
            peer_addr,
            session_port,
        })
    }

    /// Actively connect to a peer's TCP discovery port and exchange identity.
    ///
    /// Used when a peer is found via UDP announcement — we connect to their
    /// TCP port to confirm they are alive and exchange identities.
    pub async fn connect_and_exchange(
        peer_tcp_addr: SocketAddr,
        local_msg: &DiscoveryMessage,
    ) -> Result<DiscoveryExchange, anyhow::Error> {
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::TcpStream::connect(peer_tcp_addr),
        )
        .await;

        let stream = match result {
            Ok(Ok(s)) => {
                s.set_nodelay(true).unwrap_or_else(|e| {
                    log::warn!("set_nodelay failed on discovery connect stream: {e}");
                });
                s
            }
            Ok(Err(e)) => {
                return Err(anyhow::anyhow!(
                    "TCP connect to {} failed: {}",
                    peer_tcp_addr,
                    e,
                ));
            }
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "TCP connect to {} timed out",
                    peer_tcp_addr,
                ));
            }
        };

        match tokio::time::timeout(
            Duration::from_secs(5),
            Self::exchange_over_stream(stream, local_msg.clone()),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!(
                "TCP discovery exchange with {} timed out",
                peer_tcp_addr,
            )),
        }
    }
}
