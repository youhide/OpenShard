//! End-to-end: a fake client walks the whole login conversation over real TCP.
//!
//! Every layer below this has its own tests, and they are better tests — pure
//! state machines, no ports, no timing. What this catches is the one thing they
//! cannot: that the layers are *wired together* correctly. A gateway and a login
//! server that each work perfectly still produce nothing if the events go to the
//! wrong place.
//!
//! So there is one test per thing that can only break at the seam, and no more.
//! Re-testing framing or password rules through a socket would be slower, flakier
//! and would prove less.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Instant;

use openshard_gateway::{ConnectionId, Event, Server, ServerEvent};
use openshard_login::{single_shard, DevAccounts, LoginServer, LoginSession, Response};
use openshard_protocol::{AccountLogin, ClientVersion, GameServerLogin, SelectShard, SEED_COMMAND};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// Stand up a shard on an ephemeral port. Mirrors `main.rs`.
async fn shard() -> SocketAddr {
    let (server, mut events) = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let address = server.local_address().unwrap();
    let advertised = single_shard(Ipv4Addr::new(127, 0, 0, 1), address.port());
    tokio::spawn(server.run());

    tokio::spawn(async move {
        let accounts = DevAccounts::new()
            .with_account("admin", "hunter2")
            .with_character("admin", "Lord British");
        let mut login = LoginServer::new(accounts, "OpenShard", advertised);
        let mut outboxes: HashMap<ConnectionId, mpsc::UnboundedSender<Vec<u8>>> = HashMap::new();
        let mut sessions: HashMap<ConnectionId, LoginSession> = HashMap::new();

        while let Some(event) = events.recv().await {
            match event {
                ServerEvent::Connected { id, outbox, .. } => {
                    outboxes.insert(id, outbox);
                    sessions.insert(id, LoginSession::new());
                }
                ServerEvent::Received { id, event } => {
                    let Some(session) = sessions.get_mut(&id) else {
                        continue;
                    };
                    match event {
                        Event::Seeded(seed) => session.on_seed(seed),
                        Event::Packet(packet) => {
                            let response = login.handle(session, &packet, Instant::now());
                            let outbox = &outboxes[&id];
                            match response {
                                Response::Idle => {}
                                Response::Send(bytes) => {
                                    let _ = outbox.send(bytes);
                                }
                                Response::SendThenClose(bytes) => {
                                    let _ = outbox.send(bytes);
                                    outboxes.remove(&id);
                                    sessions.remove(&id);
                                }
                                Response::Close => {
                                    outboxes.remove(&id);
                                    sessions.remove(&id);
                                }
                            }
                        }
                    }
                }
                ServerEvent::Disconnected { id, .. } => {
                    outboxes.remove(&id);
                    sessions.remove(&id);
                }
            }
        }
    });

    address
}

/// A new-style seed announcing 7.0.45.65, exactly as ClassicUO opens.
fn seed(value: u32) -> Vec<u8> {
    let mut bytes = vec![SEED_COMMAND];
    bytes.extend_from_slice(&value.to_be_bytes());
    for field in [7u32, 0, 45, 65] {
        bytes.extend_from_slice(&field.to_be_bytes());
    }
    bytes
}

/// Read a whole variable-length packet: id, `u16` length, then the rest.
async fn read_variable(stream: &mut TcpStream) -> Vec<u8> {
    let mut header = [0u8; 3];
    stream.read_exact(&mut header).await.unwrap();
    let length = u16::from_be_bytes([header[1], header[2]]) as usize;
    let mut packet = header.to_vec();
    packet.resize(length, 0);
    stream.read_exact(&mut packet[3..]).await.unwrap();
    packet
}

#[tokio::test]
async fn a_client_reaches_the_character_list() {
    let address = shard().await;

    // --- login connection ------------------------------------------------
    let mut client = TcpStream::connect(address).await.unwrap();
    client.write_all(&seed(0x0A00_0001)).await.unwrap();
    client
        .write_all(
            &AccountLogin {
                account: "admin".to_owned(),
                password: "hunter2".to_owned(),
            }
            .encode(),
        )
        .await
        .unwrap();

    let shards = read_variable(&mut client).await;
    assert_eq!(shards[0], 0xA8, "shard list");
    assert_eq!(u16::from_be_bytes([shards[4], shards[5]]), 1, "one shard");
    assert_eq!(&shards[8..17], b"OpenShard");

    client
        .write_all(&SelectShard { index: 1 }.encode())
        .await
        .unwrap();

    let mut relay = [0u8; 11];
    client.read_exact(&mut relay).await.unwrap();
    assert_eq!(relay[0], 0x8C, "relay");
    // The relay carries the octets in order, on every client version — and the
    // opposite way round from the shard list two packets ago. This is the byte
    // order that decides whether anyone ever reaches the shard: the client dials
    // exactly what is here, and if it is wrong it never comes back and this end
    // sees nothing but a tidy disconnect.
    assert_eq!(&relay[1..5], &[127, 0, 0, 1]);
    let port = u16::from_be_bytes([relay[5], relay[6]]);
    let auth_key = u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]]);
    assert_eq!(port, address.port());
    assert_ne!(auth_key, 0);

    // --- the client reconnects to the game server ------------------------
    let mut client = TcpStream::connect(address).await.unwrap();
    client.write_all(&seed(0x0A00_0001)).await.unwrap();
    client
        .write_all(
            &GameServerLogin {
                auth_key,
                account: "admin".to_owned(),
                password: "hunter2".to_owned(),
            }
            .encode(),
        )
        .await
        .unwrap();

    let characters = read_variable(&mut client).await;
    assert_eq!(characters[0], 0xA9, "character list");
    assert_eq!(characters[3], 5, "padded to five slots");
    assert_eq!(&characters[4..16], b"Lord British");
}

#[tokio::test]
async fn a_refused_login_reaches_the_client_and_the_socket_closes() {
    // Two things at the seam: the 0x82 has to arrive *before* the close, and the
    // close has to happen at all. Dropping the outbox is what does it, and it is
    // exactly the kind of thing that works in a unit test and not in the wiring.
    let address = shard().await;
    let mut client = TcpStream::connect(address).await.unwrap();

    client.write_all(&seed(1)).await.unwrap();
    client
        .write_all(
            &AccountLogin {
                account: "admin".to_owned(),
                password: "wrong".to_owned(),
            }
            .encode(),
        )
        .await
        .unwrap();

    let mut denied = [0u8; 2];
    client.read_exact(&mut denied).await.unwrap();
    assert_eq!(denied, [0x82, 0x03], "bad password");

    // Nothing more, and the far end hangs up.
    let mut trailing = Vec::new();
    client.read_to_end(&mut trailing).await.unwrap();
    assert!(trailing.is_empty(), "the server said its piece and left");
}

#[tokio::test]
async fn the_client_version_from_the_seed_shapes_the_reply() {
    // The seed is read by the gateway and the shard list is encoded by login.
    // Proving the version survives that hand-off is the whole point of the test:
    // if it does not, every client gets the oldest dialect and the bug is
    // invisible until someone tries a 2D client from 1999.
    let address = shard().await;
    let mut client = TcpStream::connect(address).await.unwrap();

    // Announce 3.0.7b, which is below the 4.0.0 boundary for IP byte order.
    let mut ancient = vec![SEED_COMMAND];
    ancient.extend_from_slice(&1u32.to_be_bytes());
    for field in [3u32, 0, 7, 2] {
        ancient.extend_from_slice(&field.to_be_bytes());
    }
    client.write_all(&ancient).await.unwrap();
    client
        .write_all(
            &AccountLogin {
                account: "admin".to_owned(),
                password: "hunter2".to_owned(),
            }
            .encode(),
        )
        .await
        .unwrap();

    let shards = read_variable(&mut client).await;
    assert_eq!(
        &shards[42..46],
        &[127, 0, 0, 1],
        "a pre-4.0.0 client wants the shard IP in order"
    );
    assert!(ClientVersion::new(3, 0, 7, 2) < ClientVersion::AOS);
}

#[tokio::test]
async fn a_stolen_auth_key_is_useless_over_a_real_socket() {
    // The security property, end to end rather than in a unit test: someone who
    // reads a key off the wire cannot spend it.
    let address = shard().await;

    let mut client = TcpStream::connect(address).await.unwrap();
    client.write_all(&seed(1)).await.unwrap();
    client
        .write_all(
            &AccountLogin {
                account: "admin".to_owned(),
                password: "hunter2".to_owned(),
            }
            .encode(),
        )
        .await
        .unwrap();
    let _ = read_variable(&mut client).await;
    client
        .write_all(&SelectShard { index: 1 }.encode())
        .await
        .unwrap();
    let mut relay = [0u8; 11];
    client.read_exact(&mut relay).await.unwrap();
    let auth_key = u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]]);

    let game_login = GameServerLogin {
        auth_key,
        account: "admin".to_owned(),
        password: "hunter2".to_owned(),
    };

    // The legitimate client spends it.
    let mut first = TcpStream::connect(address).await.unwrap();
    first.write_all(&seed(1)).await.unwrap();
    first.write_all(&game_login.encode()).await.unwrap();
    assert_eq!(read_variable(&mut first).await[0], 0xA9);

    // The eavesdropper replays it.
    let mut second = TcpStream::connect(address).await.unwrap();
    second.write_all(&seed(1)).await.unwrap();
    second.write_all(&game_login.encode()).await.unwrap();
    let mut denied = [0u8; 2];
    second.read_exact(&mut denied).await.unwrap();
    assert_eq!(denied, [0x82, 0x04], "bad auth id reads as 'other'");
}

#[tokio::test]
async fn a_packet_split_across_tcp_segments_still_arrives() {
    // TCP is a stream and the client is free to flush wherever. This is covered
    // exhaustively in the gateway's unit tests; here it only has to prove the
    // reassembly is actually reached through the real socket path.
    let address = shard().await;
    let mut client = TcpStream::connect(address).await.unwrap();

    let mut stream = seed(1);
    stream.extend_from_slice(
        &AccountLogin {
            account: "admin".to_owned(),
            password: "hunter2".to_owned(),
        }
        .encode(),
    );

    for byte in &stream {
        client.write_all(&[*byte]).await.unwrap();
    }

    let shards = read_variable(&mut client).await;
    assert_eq!(shards[0], 0xA8);
}
