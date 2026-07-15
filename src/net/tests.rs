use super::*;
use crate::app::identity::DeviceIdentity;
use crate::core::action::CalcAction;
use crate::core::token::BinaryOp;
use crate::net::protocol::*;
use tokio::sync::mpsc;

// ---- Protocol serialization round-trip tests --------------------------

#[test]
fn roundtrip_hello() {
    let msg = NetworkMessage::Hello {
        node_id: NodeId::new_v4(),
        display_name: "TestNode".into(),
        protocol_version: PROTOCOL_VERSION,
        app_id: APP_ID.to_string(),
        public_key: [0u8; 32],
    };
    let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _) =
        bincode::serde::decode_from_slice::<NetworkMessage, _>(&bytes, bincode::config::standard())
            .unwrap();
    match decoded {
        NetworkMessage::Hello {
            node_id,
            display_name,
            protocol_version,
            ..
        } => {
            assert_eq!(
                node_id,
                match &msg {
                    NetworkMessage::Hello { node_id, .. } => *node_id,
                    _ => unreachable!(),
                }
            );
            assert_eq!(display_name, "TestNode");
            assert_eq!(protocol_version, PROTOCOL_VERSION);
        }
        _ => panic!("Expected Hello"),
    }
}

#[test]
fn roundtrip_action_envelope() {
    let msg = NetworkMessage::Action(ActionEnvelope {
        seq: 42,
        source_id: NodeId::new_v4(),
        timestamp_ms: 1234567890,
        action: CalcAction::Operator(BinaryOp::Add),
    });
    let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _) =
        bincode::serde::decode_from_slice::<NetworkMessage, _>(&bytes, bincode::config::standard())
            .unwrap();
    match decoded {
        NetworkMessage::Action(env) => {
            assert_eq!(env.seq, 42);
            assert_eq!(env.action, CalcAction::Operator(BinaryOp::Add));
        }
        _ => panic!("Expected Action"),
    }
}

#[test]
fn roundtrip_state_update() {
    let msg = NetworkMessage::StateUpdate(StateSnapshot {
        display: "42".into(),
        history: "6 * 7 = ".into(),
        memory_indicator: "M".into(),
        is_error: false,
        last_seq_applied: 10,
    });
    let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _) =
        bincode::serde::decode_from_slice::<NetworkMessage, _>(&bytes, bincode::config::standard())
            .unwrap();
    match decoded {
        NetworkMessage::StateUpdate(snap) => {
            assert_eq!(snap.display, "42");
            assert_eq!(snap.history, "6 * 7 = ");
            assert_eq!(snap.last_seq_applied, 10);
        }
        _ => panic!("Expected StateUpdate"),
    }
}

#[test]
fn roundtrip_discovery_announce() {
    let msg = DiscoveryMessage::Announce {
        node_id: NodeId::new_v4(),
        display_name: "Peer".into(),
        tcp_port: 4242,
        capabilities: Capabilities {
            can_execute: true,
            can_control: false,
            protocol_version: PROTOCOL_VERSION,
        },
    };
    let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _) = bincode::serde::decode_from_slice::<DiscoveryMessage, _>(
        &bytes,
        bincode::config::standard(),
    )
    .unwrap();
    match decoded {
        DiscoveryMessage::Announce {
            display_name,
            tcp_port,
            capabilities,
            ..
        } => {
            assert_eq!(display_name, "Peer");
            assert_eq!(tcp_port, 4242);
            assert!(capabilities.can_execute);
            assert!(!capabilities.can_control);
        }
        _ => panic!("Expected Announce"),
    }
}

#[test]
fn roundtrip_announce_v2() {
    let msg = DiscoveryMessage::AnnounceV2 {
        node_id: NodeId::new_v4(),
        display_name: "V2Peer".into(),
        tcp_port: 9999,
        capabilities: Capabilities {
            can_execute: false,
            can_control: true,
            protocol_version: PROTOCOL_VERSION,
        },
        transport_hint: TransportHint::Mdns,
        hostname: "my-host".into(),
        session_port: 54321,
    };
    let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _) = bincode::serde::decode_from_slice::<DiscoveryMessage, _>(
        &bytes,
        bincode::config::standard(),
    )
    .unwrap();
    match decoded {
        DiscoveryMessage::AnnounceV2 {
            display_name,
            tcp_port,
            capabilities,
            transport_hint,
            hostname,
            session_port,
            ..
        } => {
            assert_eq!(display_name, "V2Peer");
            assert_eq!(tcp_port, 9999);
            assert!(!capabilities.can_execute);
            assert!(capabilities.can_control);
            assert_eq!(transport_hint, TransportHint::Mdns);
            assert_eq!(hostname, "my-host");
            assert_eq!(session_port, 54321);
        }
        other => panic!("Expected AnnounceV2, got {:?}", other),
    }
}

#[test]
fn roundtrip_transport_hint() {
    // Verify every TransportHint variant survives serialization.
    let hints = [
        TransportHint::Multicast,
        TransportHint::Broadcast,
        TransportHint::Mdns,
    ];
    for hint in &hints {
        let bytes = bincode::serde::encode_to_vec(hint, bincode::config::standard()).unwrap();
        let (decoded, _) = bincode::serde::decode_from_slice::<TransportHint, _>(
            &bytes,
            bincode::config::standard(),
        )
        .unwrap();
        assert_eq!(*hint, decoded);
    }
}

#[test]
fn announce_v2_discriminant_is_2() {
    // Verify that AnnounceV2 serializes with discriminant 2, not 1.
    // DiscoveryMessage variants: Announce=0, Discover=1, AnnounceV2=2.
    let v2 = DiscoveryMessage::AnnounceV2 {
        node_id: NodeId::new_v4(),
        display_name: "X".into(),
        tcp_port: 0,
        capabilities: Capabilities {
            can_execute: false,
            can_control: false,
            protocol_version: 0,
        },
        transport_hint: TransportHint::Multicast,
        hostname: String::new(),
        session_port: 0,
    };
    let v1 = DiscoveryMessage::Discover;
    let bytes_v2 = bincode::serde::encode_to_vec(&v2, bincode::config::standard()).unwrap();
    let bytes_v1 = bincode::serde::encode_to_vec(&v1, bincode::config::standard()).unwrap();
    // The first byte is the enum discriminant.
    assert_eq!(bytes_v1[0], 1, "Discover should be discriminant 1");
    assert_eq!(bytes_v2[0], 2, "AnnounceV2 should be discriminant 2, not 1");
}

#[test]
fn roundtrip_all_message_variants() {
    // Verify every NetworkMessage variant survives serialization.
    let messages = vec![
        NetworkMessage::Hello {
            node_id: NodeId::new_v4(),
            display_name: "A".into(),
            protocol_version: 1,
            app_id: APP_ID.to_string(),
            public_key: [0u8; 32],
        },
        NetworkMessage::HelloAck {
            node_id: NodeId::new_v4(),
            display_name: "B".into(),
            protocol_version: 1,
            app_id: APP_ID.to_string(),
            public_key: [0u8; 32],
        },
        NetworkMessage::Subscribe,
        NetworkMessage::Unsubscribe,
        NetworkMessage::Action(ActionEnvelope {
            seq: 1,
            source_id: NodeId::new_v4(),
            timestamp_ms: 0,
            action: CalcAction::Digit(5),
        }),
        NetworkMessage::StateUpdate(StateSnapshot {
            display: "0".into(),
            history: String::new(),
            memory_indicator: String::new(),
            is_error: false,
            last_seq_applied: 0,
        }),
        NetworkMessage::RouteRevoke {
            from: NodeId::new_v4(),
            to: NodeId::new_v4(),
            version: 1,
        },
        NetworkMessage::RouteRequest {
            request_id: 1,
            controller: NodeId::new_v4(),
            executor: NodeId::new_v4(),
        },
        NetworkMessage::RouteGrant {
            request_id: 1,
            controller: NodeId::new_v4(),
            executor: NodeId::new_v4(),
        },
        NetworkMessage::RouteDenied {
            request_id: 1,
            controller: NodeId::new_v4(),
            executor: NodeId::new_v4(),
            reason: "denied".into(),
        },
        NetworkMessage::RouteRelease {
            controller: NodeId::new_v4(),
            executor: NodeId::new_v4(),
        },
        NetworkMessage::AuthChallenge { nonce: [7u8; 32] },
        NetworkMessage::AuthProof {
            signature: vec![1, 2, 3],
        },
        NetworkMessage::RoutingDelta {
            owner: NodeId::new_v4(),
            version: 1,
            cells: vec![(NodeId::new_v4(), NodeId::new_v4(), true)],
        },
        NetworkMessage::RoutingSync {
            entries: vec![(NodeId::new_v4(), NodeId::new_v4(), true, 1)],
        },
        NetworkMessage::RoutingRowRequest {
            owner: NodeId::new_v4(),
        },
        NetworkMessage::RoutingRowAnnounce {
            owner: NodeId::new_v4(),
            version: 1,
            cells: vec![(NodeId::new_v4(), NodeId::new_v4(), true)],
            owner_public_key: [8u8; 32],
            signature: vec![9, 10, 11],
        },
        NetworkMessage::PairingRequest {
            public_key: [1u8; 32],
            pairing_code_hash: [2u8; 32],
        },
        NetworkMessage::PairingConfirm {
            signature: vec![3, 4, 5],
        },
        NetworkMessage::PairingReject,
        NetworkMessage::Ping,
        NetworkMessage::Pong,
        NetworkMessage::PeerNameUpdate {
            display_name: "NewName".into(),
        },
    ];

    for msg in &messages {
        let bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard()).unwrap();
        let (decoded, _) = bincode::serde::decode_from_slice::<NetworkMessage, _>(
            &bytes,
            bincode::config::standard(),
        )
        .unwrap();
        // At minimum, the discriminant should match.
        assert_eq!(
            std::mem::discriminant(msg),
            std::mem::discriminant(&decoded),
        );
    }
}

#[test]
fn protocol_magic_byte_layout() {
    assert_eq!(PROTOCOL_MAGIC.len(), 8, "PROTOCOL_MAGIC should be 8 bytes");
    assert_eq!(
        &PROTOCOL_MAGIC[..6],
        b"VOCALC",
        "First 6 bytes should be 'VOCALC'"
    );
    assert_eq!(PROTOCOL_MAGIC[6], 0x01, "Byte 6 should be version 0x01");
    assert_eq!(PROTOCOL_MAGIC[7], 0x00, "Byte 7 should be reserved 0x00");
}

// ---- TCP session integration test ------------------------------------

#[tokio::test]
async fn tcp_session_handshake_and_message_passing() {
    // Spin up a TCP listener, connect, perform the full handshake,
    // exchange an action, and verify the message is received.

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let client_identity = DeviceIdentity::generate();
    let server_identity = DeviceIdentity::generate();
    let client_id = client_identity.node_id();
    let server_id = server_identity.node_id();
    let client_pubkey = client_identity.public_key_bytes();
    let server_pubkey = server_identity.public_key_bytes();
    let client_signing_key = client_identity.signing_key();
    let server_signing_key = server_identity.signing_key();

    // Shared channel to collect messages the server-side session
    // forwards to the "Router" (i.e. IncomingMessage commands).
    let (server_cmd_tx, mut server_cmd_rx) = mpsc::unbounded_channel::<NetworkCommand>();

    // Server task: accept one connection and run the session.
    let server_handle = tokio::spawn(async move {
        let (stream, peer_addr) = listener.accept().await.unwrap();
        let _ = session::run_accepted_session(
            stream,
            peer_addr,
            server_id,
            "Server".into(),
            server_pubkey,
            server_signing_key,
            server_cmd_tx.clone(),
        )
        .await;
    });

    // Client task: connect and run the client session.
    let (client_cmd_tx, mut client_cmd_rx) = mpsc::unbounded_channel::<NetworkCommand>();
    let client_handle = tokio::spawn(async move {
        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let _ = session::run_connecting_session(
            stream,
            addr,
            client_id,
            "Client".into(),
            client_pubkey,
            client_signing_key,
            client_cmd_tx,
        )
        .await;
    });

    // Wait for the session to register.
    // The server session task sends RegisterSession through server_cmd_tx.
    // But wait -- the server session's command_tx is server_cmd_tx, which
    // we own the rx for. Let's poll it.
    let register_timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match server_cmd_rx.recv().await {
                Some(NetworkCommand::RegisterSession(reg)) => {
                    return reg;
                }
                Some(_) => continue,
                None => panic!("Command channel closed"),
            }
        }
    })
    .await;

    assert!(register_timeout.is_ok(), "Session registration timed out");
    let reg = register_timeout.unwrap();
    assert_eq!(reg.info.display_name, "Client");

    // Send a StateUpdate from the server to the client via the session sender.
    let test_snapshot = StateSnapshot {
        display: "123".into(),
        history: "test".into(),
        memory_indicator: String::new(),
        is_error: false,
        last_seq_applied: 0,
    };
    reg.sender
        .send(NetworkMessage::StateUpdate(test_snapshot.clone()))
        .unwrap();

    // Wait for the client to receive the StateUpdate via its command channel.
    // The client session forwards incoming wire messages as IncomingMessage.
    let receive_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match client_cmd_rx.recv().await {
                Some(NetworkCommand::IncomingMessage(_sender_id, msg)) => return msg,
                Some(_) => continue,
                None => panic!("Client command channel closed before receiving StateUpdate"),
            }
        }
    })
    .await;

    assert!(
        receive_result.is_ok(),
        "Client did not receive StateUpdate within timeout"
    );
    let received = receive_result.unwrap();
    match received {
        NetworkMessage::StateUpdate(snap) => {
            assert_eq!(snap.display, "123");
            assert_eq!(snap.history, "test");
        }
        other => panic!("Expected StateUpdate on client, got {:?}", other),
    }

    // Clean up: drop the session sender to trigger disconnect.
    drop(reg.sender);

    // Wait for both tasks to complete (with timeout).
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let _ = tokio::join!(server_handle, client_handle);
    })
    .await;
}

#[test]
fn network_manager_new_has_default_state() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let (tx, _rx) = mpsc::unbounded_channel();
    let nm = NetworkManager::new(storage, tx);
    let state = nm.state();
    let state = state.lock().unwrap();
    assert!(state.peers.is_empty());
    assert!(!state.is_connected);
    assert!(state.latency_ms.is_none());
}

#[test]
fn network_manager_uses_provided_node_id() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let expected_id = storage.identity().node_id();
    let (tx, _rx) = mpsc::unbounded_channel();
    let nm = NetworkManager::new(storage, tx);
    assert_eq!(nm.local_node_id(), expected_id);
}

// ---- Handshake failure-path tests ------------------------------------

mod handshake_failure_tests {
    use super::super::handshake::server_handshake;
    use super::super::session::FramedStream;
    use crate::app::identity::DeviceIdentity;
    use crate::net::protocol::*;
    use futures::SinkExt;
    use hmac::Mac;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};
    use uuid::Uuid;

    /// Helper: serialize a `NetworkMessage` with the protocol magic prefix
    /// and send it as a single length-delimited frame.
    async fn send_magic_msg(framed: &mut FramedStream, msg: &NetworkMessage) {
        let bincode_bytes =
            bincode::serde::encode_to_vec(msg, bincode::config::standard()).unwrap();
        let mut payload = Vec::with_capacity(PROTOCOL_MAGIC.len() + bincode_bytes.len());
        payload.extend_from_slice(&PROTOCOL_MAGIC);
        payload.extend_from_slice(&bincode_bytes);
        framed
            .send(tokio_util::bytes::Bytes::from(payload))
            .await
            .unwrap();
    }

    /// Helper: send raw bytes as a single length-delimited frame (no magic prefix).
    async fn send_raw_frame(framed: &mut FramedStream, data: &[u8]) {
        framed
            .send(tokio_util::bytes::Bytes::from(data.to_vec()))
            .await
            .unwrap();
    }

    /// Helper: compute the HMAC-SHA256 tag for already-serialized Hello bytes.
    fn compute_hmac(hello_bytes: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(APP_KEY).unwrap();
        mac.update(hello_bytes);
        mac.finalize().into_bytes().to_vec()
    }

    /// Helper: serialize a Hello message into raw bincode bytes.
    fn serialize_hello(hello: &NetworkMessage) -> Vec<u8> {
        bincode::serde::encode_to_vec(hello, bincode::config::standard()).unwrap()
    }

    /// Build a correctly-formed client-side Hello + HMAC pair and return the
    /// (hello_msg, hmac_tag) ready for sending.
    fn build_valid_hello(
        identity: &DeviceIdentity,
        name: &str,
        version: u16,
        app_id: &str,
    ) -> (NetworkMessage, Vec<u8>) {
        let hello = NetworkMessage::Hello {
            node_id: identity.node_id(),
            display_name: name.to_string(),
            protocol_version: version,
            app_id: app_id.to_string(),
            public_key: identity.public_key_bytes(),
        };
        let raw = serialize_hello(&hello);
        let tag = compute_hmac(&raw);
        (hello, tag)
    }

    /// Helper: accept one TCP connection, run `server_handshake`, return the result.
    /// The error is converted to `String` so the future is `Send`-safe for `tokio::spawn`.
    async fn accept_and_handshake(
        listener: TcpListener,
        server_identity: DeviceIdentity,
    ) -> Result<(Uuid, String, [u8; 32], FramedStream), String> {
        let (stream, _peer) = listener.accept().await.unwrap();
        let framed = Framed::new(stream, LengthDelimitedCodec::new());
        server_handshake(
            framed,
            server_identity.node_id(),
            "Server",
            server_identity.public_key_bytes(),
            &server_identity.signing_key(),
        )
        .await
        .map_err(|e| e.to_string())
    }

    // -----------------------------------------------------------------------
    // Test 1: App ID mismatch -- client sends wrong app_id, server rejects.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn rejects_wrong_app_id() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();

        let server = tokio::spawn(accept_and_handshake(listener, server_identity));

        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

            // Send Hello with a bogus app_id but HMAC computed over it.
            let client_identity = DeviceIdentity::generate();
            let (hello, tag) =
                build_valid_hello(&client_identity, "BadClient", PROTOCOL_VERSION, "WRONG_APP");
            send_magic_msg(&mut framed, &hello).await;
            send_raw_frame(&mut framed, &tag).await;

            // Keep connection alive until server processes.
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                futures::future::pending::<()>(),
            )
            .await;
        });

        let result = server.await.unwrap();
        assert!(result.is_err(), "server should reject wrong app_id");
        let err = result.unwrap_err();
        assert!(err.contains("App ID mismatch"), "unexpected error: {err}");
        client.abort();
    }

    // -----------------------------------------------------------------------
    // Test 2: Protocol version mismatch -- client sends wrong version, server rejects.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn rejects_wrong_protocol_version() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();

        let server = tokio::spawn(accept_and_handshake(listener, server_identity));

        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

            // Correct app_id but wrong protocol version.
            let client_identity = DeviceIdentity::generate();
            let (hello, tag) =
                build_valid_hello(&client_identity, "BadClient", PROTOCOL_VERSION + 99, APP_ID);
            send_magic_msg(&mut framed, &hello).await;
            send_raw_frame(&mut framed, &tag).await;

            // The server sends a HelloAck(version=0) before returning Err on
            // version mismatch -- drain that frame so the server write doesn't
            // block.
            use futures::StreamExt;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), framed.next()).await;

            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                futures::future::pending::<()>(),
            )
            .await;
        });

        let result = server.await.unwrap();
        assert!(
            result.is_err(),
            "server should reject wrong protocol version"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("Protocol version mismatch"),
            "unexpected error: {err}"
        );
        client.abort();
    }

    // -----------------------------------------------------------------------
    // Test 3: HMAC failure -- client sends bad HMAC tag, server rejects.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn rejects_bad_hmac() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();

        let server = tokio::spawn(accept_and_handshake(listener, server_identity));

        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

            // Valid Hello with correct fields.
            let client_identity = DeviceIdentity::generate();
            let hello = NetworkMessage::Hello {
                node_id: client_identity.node_id(),
                display_name: "BadClient".to_string(),
                protocol_version: PROTOCOL_VERSION,
                app_id: APP_ID.to_string(),
                public_key: client_identity.public_key_bytes(),
            };
            send_magic_msg(&mut framed, &hello).await;

            // Send 32 bytes of garbage instead of a valid HMAC tag.
            let bad_tag = vec![0xABu8; 32];
            send_raw_frame(&mut framed, &bad_tag).await;

            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                futures::future::pending::<()>(),
            )
            .await;
        });

        let result = server.await.unwrap();
        assert!(result.is_err(), "server should reject bad HMAC");
        let err = result.unwrap_err();
        assert!(
            err.contains("HMAC verification failed"),
            "unexpected error: {err}"
        );
        client.abort();
    }

    // -----------------------------------------------------------------------
    // Test 4: Wrong message type -- client sends non-Hello, server rejects.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn rejects_non_hello_message() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();

        let server = tokio::spawn(accept_and_handshake(listener, server_identity));

        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

            // Send a Ping instead of Hello.
            send_magic_msg(&mut framed, &NetworkMessage::Ping).await;

            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                futures::future::pending::<()>(),
            )
            .await;
        });

        let result = server.await.unwrap();
        assert!(result.is_err(), "server should reject non-Hello message");
        let err = result.unwrap_err();
        assert!(err.contains("Expected Hello"), "unexpected error: {err}");
        client.abort();
    }

    // -----------------------------------------------------------------------
    // Test 5: Truncated HMAC -- client sends HMAC shorter than 32 bytes.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn rejects_truncated_hmac() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();

        let server = tokio::spawn(accept_and_handshake(listener, server_identity));

        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

            // Valid Hello with correct fields.
            let client_identity = DeviceIdentity::generate();
            let hello = NetworkMessage::Hello {
                node_id: client_identity.node_id(),
                display_name: "BadClient".to_string(),
                protocol_version: PROTOCOL_VERSION,
                app_id: APP_ID.to_string(),
                public_key: client_identity.public_key_bytes(),
            };
            send_magic_msg(&mut framed, &hello).await;

            // Send only 16 bytes instead of the required 32.
            let short_tag = vec![0x42u8; 16];
            send_raw_frame(&mut framed, &short_tag).await;

            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                futures::future::pending::<()>(),
            )
            .await;
        });

        let result = server.await.unwrap();
        assert!(result.is_err(), "server should reject truncated HMAC");
        let err = result.unwrap_err();
        assert!(
            err.contains("HMAC tag length mismatch"),
            "unexpected error: {err}"
        );
        client.abort();
    }

    #[tokio::test]
    async fn rejects_zero_public_key_for_protocol_v4() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();

        let server = tokio::spawn(accept_and_handshake(listener, server_identity));

        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

            let hello = NetworkMessage::Hello {
                node_id: Uuid::new_v4(),
                display_name: "LegacyClient".to_string(),
                protocol_version: PROTOCOL_VERSION,
                app_id: APP_ID.to_string(),
                public_key: [0u8; 32],
            };
            let raw = serialize_hello(&hello);
            let tag = compute_hmac(&raw);
            send_magic_msg(&mut framed, &hello).await;
            send_raw_frame(&mut framed, &tag).await;

            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                futures::future::pending::<()>(),
            )
            .await;
        });

        let result = server.await.unwrap();
        assert!(result.is_err(), "server should reject v4 zero public key");
        let err = result.unwrap_err();
        assert!(
            err.contains("requires an Ed25519 public key"),
            "unexpected error: {err}"
        );
        client.abort();
    }

    #[tokio::test]
    async fn rejects_node_id_public_key_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();

        let server = tokio::spawn(accept_and_handshake(listener, server_identity));

        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
            let client_identity = DeviceIdentity::generate();

            let hello = NetworkMessage::Hello {
                node_id: Uuid::new_v4(),
                display_name: "MismatchClient".to_string(),
                protocol_version: PROTOCOL_VERSION,
                app_id: APP_ID.to_string(),
                public_key: client_identity.public_key_bytes(),
            };
            let raw = serialize_hello(&hello);
            let tag = compute_hmac(&raw);
            send_magic_msg(&mut framed, &hello).await;
            send_raw_frame(&mut framed, &tag).await;

            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                futures::future::pending::<()>(),
            )
            .await;
        });

        let result = server.await.unwrap();
        assert!(result.is_err(), "server should reject mismatched node id");
        let err = result.unwrap_err();
        assert!(
            err.contains("Node ID/public key mismatch"),
            "unexpected error: {err}"
        );
        client.abort();
    }
}

// ---- Fixed-port / mode-threading tests (Task B) -----------------------
//
// Loopback-only, pure-function tests: no real LAN sockets, no multicast
// group join, no mDNS daemon.

mod fixed_port_tests {
    use crate::app::network_mode::NetworkMode;
    use crate::net::protocol::{DISCOVERY_PORT, LAN_FIXED_PORT, SESSION_TCP_PORT};
    use crate::net::runtime::{bind_tcp_with_reuse, session_bind_addr};

    #[test]
    fn session_bind_addr_lan_is_fixed_port_all_interfaces() {
        let addr = session_bind_addr(NetworkMode::Lan);
        assert_eq!(addr.ip().to_string(), "0.0.0.0");
        assert_eq!(addr.port(), 42420);
        assert_eq!(addr.port(), SESSION_TCP_PORT);
    }

    #[test]
    fn session_bind_addr_loopback_test_is_loopback_ephemeral() {
        let addr = session_bind_addr(NetworkMode::LoopbackTest);
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 0);
    }

    #[test]
    fn fixed_port_constants_are_consistent() {
        assert_eq!(SESSION_TCP_PORT, DISCOVERY_PORT);
        assert_eq!(DISCOVERY_PORT, LAN_FIXED_PORT);
        assert_eq!(LAN_FIXED_PORT, 42420);
    }

    #[test]
    fn bind_tcp_with_reuse_errors_cleanly_on_occupied_loopback_port() {
        // Occupy an ephemeral loopback port with a plain std listener first,
        // then try to bind the same address again through the helper used
        // by the network runtime. This must fail cleanly (no panic) rather
        // than silently succeeding or hanging -- SO_REUSEADDR alone does not
        // allow two independent listeners on the same address.
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = occupied.local_addr().unwrap();

        let result = bind_tcp_with_reuse(addr);
        assert!(
            result.is_err(),
            "expected bind_tcp_with_reuse to fail on an already-bound loopback port"
        );

        drop(occupied);
    }
}
