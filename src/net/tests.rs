use super::*;
use crate::app::identity::DeviceIdentity;
use crate::core::action::CalcAction;
use crate::core::token::BinaryOp;
use crate::net::protocol::*;
use tokio::sync::mpsc;

#[test]
fn protocol_v5_version_and_wire_discriminants_are_frozen() {
    assert_eq!(PROTOCOL_VERSION, 5);

    // Golden bytes for the unit-like variants most affected when variants are
    // inserted in the middle of the serde enum. Any future layout change must
    // intentionally bump PROTOCOL_VERSION again.
    let cases = [
        (NetworkMessage::Subscribe, vec![2]),
        (NetworkMessage::Unsubscribe, vec![3]),
        (NetworkMessage::Ping, vec![17]),
        (NetworkMessage::Pong, vec![18]),
        (NetworkMessage::PairingReject, vec![22]),
    ];
    for (message, golden) in cases {
        let encoded = bincode::serde::encode_to_vec(&message, bincode::config::standard()).unwrap();
        assert_eq!(encoded, golden, "wire discriminant changed for {message:?}");
    }
}

#[test]
fn display_name_schema_is_shared_and_byte_bounded() {
    assert!(valid_display_name("A"));
    assert!(valid_display_name(&"x".repeat(MAX_DISPLAY_NAME_BYTES)));
    assert!(!valid_display_name(""));
    assert!(!valid_display_name("   "));
    assert!(!valid_display_name(&"x".repeat(MAX_DISPLAY_NAME_BYTES + 1)));
    assert!(valid_display_name(&"界".repeat(21))); // 63 UTF-8 bytes
    assert!(!valid_display_name(&"界".repeat(22))); // 66 UTF-8 bytes
    assert!(!valid_display_name("bad\nname"));
}

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
    let (server_cmd_tx, mut server_cmd_rx) = mpsc::channel::<NetworkCommand>(256);

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
    let (client_cmd_tx, mut client_cmd_rx) = mpsc::channel::<NetworkCommand>(256);
    let client_handle = tokio::spawn(async move {
        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let _ = session::run_connecting_session(
            stream,
            addr,
            client_id,
            "Client".into(),
            client_pubkey,
            client_signing_key,
            None,
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
    reg.decision_tx.send(true).unwrap();

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
        .await
        .unwrap();

    // Wait for the client to receive the StateUpdate via its command channel.
    // The client session forwards incoming wire messages as IncomingMessage.
    let receive_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match client_cmd_rx.recv().await {
                Some(NetworkCommand::RegisterSession(reg)) => {
                    let _ = reg.decision_tx.send(true);
                }
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

    // Clean up: cancel the registered session generation.
    let _ = reg.cancel_tx.send(true);

    // Wait for both tasks to complete (with timeout).
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let _ = tokio::join!(server_handle, client_handle);
    })
    .await;
}

#[tokio::test]
async fn authenticated_peer_local_only_frame_is_rejected_before_router_bridge() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client_identity = DeviceIdentity::generate();
    let server_identity = DeviceIdentity::generate();
    let client_id = client_identity.node_id();
    let server_id = server_identity.node_id();
    let (server_tx, mut server_rx) = mpsc::channel(16);

    let server = tokio::spawn(async move {
        let (stream, peer_addr) = listener.accept().await.unwrap();
        session::run_accepted_session(
            stream,
            peer_addr,
            server_id,
            "Server".to_string(),
            server_identity.public_key_bytes(),
            server_identity.signing_key(),
            server_tx,
        )
        .await;
    });

    let client = tokio::spawn(async move {
        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let framed = tokio_util::codec::Framed::new(stream, session::session_codec());
        let (_, _, _, mut framed) = crate::net::handshake::client_handshake(
            framed,
            client_id,
            "Client",
            client_identity.public_key_bytes(),
            &client_identity.signing_key(),
        )
        .await
        .unwrap();
        session::send_msg(&mut framed, &NetworkMessage::Subscribe)
            .await
            .unwrap();
        session::send_msg(
            &mut framed,
            &NetworkMessage::ConnectionFailed {
                addr,
                reason: "attacker_reason".to_string(),
                target_node_id: Some(server_id),
            },
        )
        .await
        .unwrap();
    });

    let mut saw_unregister = false;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(command) = server_rx.recv().await {
            match command {
                NetworkCommand::RegisterSession(registration) => {
                    registration.decision_tx.send(true).unwrap();
                }
                NetworkCommand::IncomingMessage(_, message) => {
                    panic!("local-only message reached Router bridge: {message:?}");
                }
                NetworkCommand::UnregisterSession { .. } => {
                    saw_unregister = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("malicious local-only frame should close the session promptly");

    client.await.unwrap();
    server.await.unwrap();
    assert!(saw_unregister);
}

#[test]
fn network_manager_new_has_default_state() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let (tx, _rx) = mpsc::channel(16);
    let nm = NetworkManager::new(storage, tx);
    let state = nm.state();
    let state = state.lock().unwrap();
    assert!(state.peers.is_empty());
    assert!(!state.is_connected);
    assert!(state.latency_ms.is_none());
}

#[test]
fn network_manager_public_entry_rejects_non_loopback_before_command_send() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let (ui_tx, _ui_rx) = mpsc::channel(16);
    let mut nm = NetworkManager::new(storage, ui_tx);
    nm.network_mode = crate::app::network_mode::NetworkMode::LoopbackTest;
    let (command_tx, mut command_rx) = mpsc::channel(16);
    nm.command_tx = command_tx;

    assert!(!nm.connect_to_peer("192.168.1.10:42420".parse().unwrap(), None));
    assert!(matches!(
        command_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    assert!(nm.connect_to_peer("127.0.0.1:1234".parse().unwrap(), None));
    assert!(matches!(
        command_rx.try_recv(),
        Ok(NetworkCommand::ConnectToPeer(_))
    ));
    assert!(nm.connect_to_peer("[::1]:1234".parse().unwrap(), None));
    assert!(matches!(
        command_rx.try_recv(),
        Ok(NetworkCommand::ConnectToPeer(_))
    ));
}

#[test]
fn network_manager_does_not_reconnect_peer_with_active_session() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let (ui_tx, _ui_rx) = mpsc::channel(16);
    let mut nm = NetworkManager::new(storage, ui_tx);
    nm.network_mode = crate::app::network_mode::NetworkMode::LoopbackTest;
    let (command_tx, mut command_rx) = mpsc::channel(16);
    nm.command_tx = command_tx;

    let peer_id = NodeId::new_v4();
    let (sender, _receiver) = mpsc::channel(16);
    let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
    nm.sessions.lock().unwrap().insert(
        peer_id,
        session::ActiveSession {
            session_id: SessionId::new_v4(),
            sender,
            direction: ConnectionDirection::Outbound,
            cancel_tx,
        },
    );

    assert!(!nm.connect_to_peer("127.0.0.1:1234".parse().unwrap(), Some(peer_id),));
    assert!(matches!(
        command_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[test]
fn network_manager_shutdown_joins_loopback_runtime_thread() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let (ui_tx, _ui_rx) = mpsc::channel(16);
    let mut nm = NetworkManager::new(storage, ui_tx);

    let _handle = nm
        .start(crate::app::network_mode::NetworkMode::LoopbackTest)
        .unwrap();
    assert!(nm.thread_handle.is_some());
    assert!(nm.shutdown());

    assert!(nm.thread_handle.is_none());
    assert!(nm.runtime_handle.is_none());
    assert!(nm.active_session_ids().is_empty());
    assert_eq!(
        nm.network_mode,
        crate::app::network_mode::NetworkMode::Offline
    );
    assert!(nm.shutdown(), "repeated shutdown must be idempotent");
}

#[test]
fn duplicate_start_returns_already_running_without_stopping_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let (ui_tx, _ui_rx) = mpsc::channel(16);
    let mut nm = NetworkManager::new(storage, ui_tx);

    let first_handle = nm
        .start(crate::app::network_mode::NetworkMode::LoopbackTest)
        .unwrap();
    let original_thread_id = nm.thread_handle.as_ref().unwrap().thread().id();
    assert!(!*nm.shutdown_tx.borrow());

    assert!(matches!(
        nm.start(crate::app::network_mode::NetworkMode::LoopbackTest),
        Err(NetworkStartError::AlreadyRunning)
    ));
    assert_eq!(
        nm.thread_handle.as_ref().unwrap().thread().id(),
        original_thread_id
    );
    assert!(!nm.thread_handle.as_ref().unwrap().is_finished());
    assert!(!*nm.shutdown_tx.borrow());
    assert!(!first_handle.outgoing_sender().is_closed());

    assert!(nm.shutdown());
}

#[test]
fn start_offline_is_rejected_before_any_runtime_state_check() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let (ui_tx, _ui_rx) = mpsc::channel(16);
    let mut nm = NetworkManager::new(storage, ui_tx);
    nm.runtime_shutdown_unconfirmed = true;

    assert!(matches!(
        nm.start(crate::app::network_mode::NetworkMode::Offline),
        Err(NetworkStartError::Offline)
    ));
    assert!(nm.thread_handle.is_none());
}

#[test]
fn unconfirmed_shutdown_blocks_restart_and_remains_visible() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let (ui_tx, _ui_rx) = mpsc::channel(16);
    let mut nm = NetworkManager::new(storage, ui_tx);
    nm.runtime_shutdown_unconfirmed = true;

    assert!(!nm.shutdown());
    let error = match nm.start(crate::app::network_mode::NetworkMode::LoopbackTest) {
        Ok(_) => panic!("unconfirmed shutdown must block restart"),
        Err(error) => error,
    };
    assert_eq!(error, NetworkStartError::ShutdownUnconfirmed);
    assert!(nm.thread_handle.is_none());
}

#[tokio::test]
async fn shutdown_sent_before_wait_is_observed_without_lost_wakeup() {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    shutdown_tx.send(true).unwrap();

    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        crate::net::runtime::wait_for_shutdown(shutdown_rx),
    )
    .await
    .expect("level-triggered shutdown must complete even when sent before polling");
}

#[test]
fn thread_finish_deadline_detaches_instead_of_blocking_join() {
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let (finished_tx, finished_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let thread = std::thread::spawn(move || {
        let _ = release_rx.recv();
        let _ = finished_tx.send(());
    });
    let (_done_tx, done_rx) = std::sync::mpsc::sync_channel::<()>(1);

    assert!(!finish_network_thread(
        thread,
        done_rx,
        std::time::Duration::ZERO,
    ));
    // If finish_network_thread had performed an unbounded join this send could
    // never be reached. Release and observe the now-detached worker cleanly.
    release_tx.send(()).unwrap();
    finished_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
}

#[test]
fn network_manager_uses_provided_node_id() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(crate::app::storage::Storage::open(dir.path()).unwrap());
    let expected_id = storage.identity().node_id();
    let (tx, _rx) = mpsc::channel(16);
    let nm = NetworkManager::new(storage, tx);
    assert_eq!(nm.local_node_id(), expected_id);
}

// ---- Handshake failure-path tests ------------------------------------

mod handshake_failure_tests {
    use super::super::handshake::{client_handshake, server_handshake};
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

    #[tokio::test]
    async fn rejects_hello_with_invalid_display_name() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();
        let server = tokio::spawn(accept_and_handshake(listener, server_identity));

        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
            let client_identity = DeviceIdentity::generate();
            let (hello, _) = build_valid_hello(
                &client_identity,
                &"x".repeat(MAX_DISPLAY_NAME_BYTES + 1),
                PROTOCOL_VERSION,
                APP_ID,
            );
            send_magic_msg(&mut framed, &hello).await;
        });

        let error = server.await.unwrap().unwrap_err();
        assert!(error.contains("Invalid remote Hello display name"));
        client.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_hello_ack_with_invalid_display_name() {
        use futures::StreamExt;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();
        let server_id = server_identity.node_id();
        let server_public_key = server_identity.public_key_bytes();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
            assert!(framed.next().await.is_some()); // Hello
            assert!(framed.next().await.is_some()); // raw HMAC
            send_magic_msg(
                &mut framed,
                &NetworkMessage::HelloAck {
                    node_id: server_id,
                    display_name: "x".repeat(MAX_DISPLAY_NAME_BYTES + 1),
                    protocol_version: PROTOCOL_VERSION,
                    app_id: APP_ID.to_string(),
                    public_key: server_public_key,
                },
            )
            .await;
        });

        let client_identity = DeviceIdentity::generate();
        let stream = TcpStream::connect(addr).await.unwrap();
        let framed = Framed::new(stream, LengthDelimitedCodec::new());
        let error = client_handshake(
            framed,
            client_identity.node_id(),
            "Client",
            client_identity.public_key_bytes(),
            &client_identity.signing_key(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("Invalid remote HelloAck display name"));
        server.await.unwrap();
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
    async fn rejects_zero_public_key_for_protocol_v5() {
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
        assert!(result.is_err(), "server should reject v5 zero public key");
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::app::network_mode::NetworkMode;
    use crate::net::protocol::{DISCOVERY_PORT, LAN_FIXED_PORT, SESSION_TCP_PORT};
    use crate::net::runtime::{
        bind_tcp_listener, connect_tcp_checked, outbound_addr_allowed, session_bind_addr,
    };

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
    fn bind_tcp_listener_errors_cleanly_on_occupied_loopback_port() {
        // Occupy an ephemeral loopback port with a plain std listener first,
        // then try to bind the same address again through the helper used
        // by the network runtime. This must fail cleanly (no panic) rather
        // than silently succeeding or hanging -- SO_REUSEADDR alone does not
        // allow two independent listeners on the same address.
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = occupied.local_addr().unwrap();

        let result = bind_tcp_listener(addr);
        assert!(
            result.is_err(),
            "expected bind_tcp_listener to fail on an already-bound loopback port"
        );

        drop(occupied);
    }

    #[tokio::test]
    async fn production_tcp_listener_helper_is_single_instance() {
        let first = bind_tcp_listener("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = first.local_addr().unwrap();
        let second = bind_tcp_listener(addr);
        assert!(
            second.is_err(),
            "two production listeners must never share the same TCP endpoint"
        );
    }

    #[test]
    fn outbound_address_policy_allows_both_loopback_families_only_in_loopback_mode() {
        let ipv4_loopback = "127.0.0.1:1234".parse().unwrap();
        let ipv6_loopback = "[::1]:1234".parse().unwrap();
        let lan = "192.168.1.10:42420".parse().unwrap();

        assert!(outbound_addr_allowed(
            NetworkMode::LoopbackTest,
            ipv4_loopback
        ));
        assert!(outbound_addr_allowed(
            NetworkMode::LoopbackTest,
            ipv6_loopback
        ));
        assert!(!outbound_addr_allowed(NetworkMode::LoopbackTest, lan));
        assert!(!outbound_addr_allowed(NetworkMode::Offline, ipv4_loopback));
        assert!(outbound_addr_allowed(NetworkMode::Lan, lan));
    }

    #[tokio::test]
    async fn rejected_loopback_test_address_never_invokes_connector() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_connector = calls.clone();
        let addr = "192.168.1.10:42420".parse().unwrap();
        let result = connect_tcp_checked(NetworkMode::LoopbackTest, addr, move |_| async move {
            calls_for_connector.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await;

        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn accepted_ipv4_and_ipv6_loopback_addresses_invoke_connector() {
        for addr in ["127.0.0.1:1", "[::1]:1"] {
            let calls = Arc::new(AtomicUsize::new(0));
            let calls_for_connector = calls.clone();
            connect_tcp_checked(
                NetworkMode::LoopbackTest,
                addr.parse().unwrap(),
                move |_| async move {
                    calls_for_connector.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
            .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
}

mod session_identity_and_generation_tests {
    use std::collections::HashMap;

    use super::*;
    use crate::net::discovery::public_key_fingerprint;
    use crate::net::protocol::ExpectedPeerIdentity;
    use crate::net::session_registry::{remove_session_if_current, should_replace_session};

    fn active_session(session_id: SessionId) -> session::ActiveSession {
        let (sender, _receiver) = mpsc::channel(16);
        let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
        session::ActiveSession {
            session_id,
            sender,
            direction: ConnectionDirection::Outbound,
            cancel_tx,
        }
    }

    #[test]
    fn expected_node_id_and_fingerprint_are_both_enforced() {
        let identity = DeviceIdentity::generate();
        let actual_node_id = identity.node_id();
        let public_key = identity.public_key_bytes();

        let wrong_node = ExpectedPeerIdentity {
            node_id: NodeId::new_v4(),
            public_key_fingerprint: Some(public_key_fingerprint(&public_key)),
        };
        let error = session::validate_expected_peer(actual_node_id, &public_key, Some(&wrong_node))
            .unwrap_err();
        assert!(error.contains("identity_mismatch"));

        let wrong_fingerprint = ExpectedPeerIdentity {
            node_id: actual_node_id,
            public_key_fingerprint: Some("0000000000000000".to_string()),
        };
        let error =
            session::validate_expected_peer(actual_node_id, &public_key, Some(&wrong_fingerprint))
                .unwrap_err();
        assert!(error.contains("fingerprint_mismatch"));

        let exact = ExpectedPeerIdentity {
            node_id: actual_node_id,
            public_key_fingerprint: Some(public_key_fingerprint(&public_key)),
        };
        session::validate_expected_peer(actual_node_id, &public_key, Some(&exact)).unwrap();
    }

    #[tokio::test]
    async fn handshake_identity_mismatch_never_registers_session() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client_identity = DeviceIdentity::generate();
        let server_identity = DeviceIdentity::generate();
        let expected_wrong_server = ExpectedPeerIdentity {
            node_id: NodeId::new_v4(),
            public_key_fingerprint: None,
        };

        let (server_tx, mut server_rx) = mpsc::channel(256);
        let server = tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            session::run_accepted_session(
                stream,
                peer_addr,
                server_identity.node_id(),
                "Server".to_string(),
                server_identity.public_key_bytes(),
                server_identity.signing_key(),
                server_tx,
            )
            .await;
        });

        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (client_tx, mut client_rx) = mpsc::channel(256);
        let result = session::run_connecting_session(
            stream,
            addr,
            client_identity.node_id(),
            "Client".to_string(),
            client_identity.public_key_bytes(),
            client_identity.signing_key(),
            Some(expected_wrong_server),
            client_tx,
        )
        .await;

        assert!(result.unwrap_err().contains("identity_mismatch"));
        assert!(client_rx.try_recv().is_err());
        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .expect("server should close after the client rejects identity")
            .unwrap();
        assert!(server_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn silent_inbound_peer_is_dropped_at_handshake_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();
        let (server_tx, mut server_rx) = mpsc::channel(256);
        let server = tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            session::run_accepted_session(
                stream,
                peer_addr,
                server_identity.node_id(),
                "Server".to_string(),
                server_identity.public_key_bytes(),
                server_identity.signing_key(),
                server_tx,
            )
            .await;
        });

        let _silent_client = tokio::net::TcpStream::connect(addr).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .expect("silent handshake must be dropped at its deadline")
            .unwrap();
        assert!(server_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn peer_that_never_subscribes_is_dropped_at_subscribe_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_identity = DeviceIdentity::generate();
        let client_identity = DeviceIdentity::generate();
        let (server_tx, mut server_rx) = mpsc::channel(256);
        let server = tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            session::run_accepted_session(
                stream,
                peer_addr,
                server_identity.node_id(),
                "Server".to_string(),
                server_identity.public_key_bytes(),
                server_identity.signing_key(),
                server_tx,
            )
            .await;
        });

        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let framed =
            tokio_util::codec::Framed::new(stream, tokio_util::codec::LengthDelimitedCodec::new());
        let (_remote_id, _remote_name, _remote_key, _framed) =
            crate::net::handshake::client_handshake(
                framed,
                client_identity.node_id(),
                "Client",
                client_identity.public_key_bytes(),
                &client_identity.signing_key(),
            )
            .await
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .expect("peer that omits Subscribe must be dropped at its deadline")
            .unwrap();
        assert!(server_rx.try_recv().is_err());
    }

    #[test]
    fn stale_unregister_cannot_remove_new_session_generation() {
        let node_id = NodeId::new_v4();
        let old_id = SessionId::new_v4();
        let new_id = SessionId::new_v4();
        let mut sessions = HashMap::new();
        sessions.insert(node_id, active_session(new_id));

        assert!(!remove_session_if_current(&mut sessions, node_id, old_id));
        assert_eq!(sessions.get(&node_id).unwrap().session_id, new_id);
        assert!(remove_session_if_current(&mut sessions, node_id, new_id));
        assert!(!sessions.contains_key(&node_id));
    }

    #[test]
    fn dedup_preference_is_stable_for_repeated_connections() {
        let lower = NodeId::from_u128(1);
        let higher = NodeId::from_u128(2);
        assert!(should_replace_session(
            lower,
            higher,
            ConnectionDirection::Inbound,
            ConnectionDirection::Outbound,
        ));
        assert!(!should_replace_session(
            lower,
            higher,
            ConnectionDirection::Outbound,
            ConnectionDirection::Inbound,
        ));
        assert!(!should_replace_session(
            lower,
            higher,
            ConnectionDirection::Outbound,
            ConnectionDirection::Outbound,
        ));
    }
}

mod resource_hardening_tests {
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use futures::Sink;
    use tokio_util::bytes::Bytes;

    use super::*;
    use crate::net::runtime::{inbound_session_limiter, should_attempt_discovered_session};

    #[test]
    fn inbound_session_limiter_has_a_hard_cap() {
        let limiter = inbound_session_limiter();
        let mut permits = Vec::new();
        while let Ok(permit) = limiter.clone().try_acquire_owned() {
            permits.push(permit);
        }
        assert_eq!(permits.len(), 16);
        assert!(limiter.try_acquire_owned().is_err());
    }

    #[test]
    fn discovery_attempt_tracking_is_bounded() {
        let mut attempts = HashMap::new();
        for index in 0..400u128 {
            let node_id = NodeId::from_u128(index + 1);
            let addr = std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                10_000 + (index % 50_000) as u16,
            );
            assert!(should_attempt_discovered_session(
                &mut attempts,
                node_id,
                addr,
            ));
        }
        assert_eq!(attempts.len(), 256);
    }

    #[test]
    fn per_session_outgoing_queue_is_bounded() {
        let (sender, _receiver) = session::session_outgoing_channel();
        for _ in 0..256 {
            sender.try_send(NetworkMessage::Ping).unwrap();
        }
        assert!(matches!(
            sender.try_send(NetworkMessage::Ping),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }

    #[test]
    fn runtime_command_queue_is_bounded() {
        let (sender, _receiver) = crate::net::runtime_command_channel();
        for _ in 0..256 {
            sender.try_send(NetworkCommand::Scan).unwrap();
        }
        assert!(matches!(
            sender.try_send(NetworkCommand::Scan),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }

    #[test]
    fn router_to_runtime_queue_is_bounded() {
        let (sender, _receiver) = crate::net::outgoing_message_channel();
        let target = NodeId::new_v4();
        for _ in 0..crate::net::OUTGOING_MESSAGE_CAPACITY {
            sender.try_send((target, NetworkMessage::Ping)).unwrap();
        }
        assert!(matches!(
            sender.try_send((target, NetworkMessage::Ping)),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }

    #[test]
    fn full_ui_ingress_queue_disconnects_message_producer() {
        let (ui_tx, _ui_rx) = mpsc::channel(1);
        ui_tx
            .try_send(crate::ui::events::UiEvent::NetworkStatus {
                kind: crate::net::NetworkStatusKind::Enabled,
                text: "occupied".to_string(),
            })
            .unwrap();
        let peer = NodeId::new_v4();
        let (session_sender, _session_receiver) = mpsc::channel(1);
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let sessions = crate::net::session_registry::SessionRegistry::new();
        sessions.lock().unwrap().insert(
            peer,
            session::ActiveSession {
                session_id: SessionId::new_v4(),
                sender: session_sender,
                direction: ConnectionDirection::Inbound,
                cancel_tx,
            },
        );

        assert!(!crate::net::runtime::forward_incoming_message_to_ui(
            &ui_tx,
            &sessions,
            peer,
            NetworkMessage::PeerNameUpdate {
                display_name: "Peer".to_string(),
            },
        ));
        assert!(*cancel_rx.borrow());
    }

    #[test]
    fn session_frame_allocation_is_bounded() {
        assert_eq!(session::session_codec().max_frame_length(), 4 * 1024);
    }

    #[test]
    fn session_decoder_requires_full_frame_consumption() {
        let mut encoded =
            bincode::serde::encode_to_vec(&NetworkMessage::Ping, bincode::config::standard())
                .unwrap();
        assert!(session::decode_network_message(&encoded).is_ok());
        encoded.push(0xff);
        assert!(session::decode_network_message(&encoded).is_err());
    }

    #[test]
    fn session_decoder_has_an_explicit_bincode_limit() {
        let encoded = bincode::serde::encode_to_vec(
            &NetworkMessage::PeerNameUpdate {
                display_name: "x".repeat(session::MAX_FRAME_LENGTH),
            },
            bincode::config::standard(),
        )
        .unwrap();
        assert!(encoded.len() > session::MAX_FRAME_LENGTH);
        assert!(session::decode_network_message(&encoded).is_err());
    }

    struct NeverReadySink;

    impl Sink<Bytes> for NeverReadySink {
        type Error = std::io::Error;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Pending
        }

        fn start_send(self: Pin<&mut Self>, _item: Bytes) -> Result<(), Self::Error> {
            unreachable!("poll_ready never succeeds")
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Pending
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn relay_writer_has_a_deadline() {
        let mut sink = NeverReadySink;
        let result = session::send_msg(&mut sink, &NetworkMessage::Ping).await;
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }
}
