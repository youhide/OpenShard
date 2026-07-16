//! The shard binary.
//!
//! Wires the gateway's events to the login server. This is the first thing in
//! the project you can watch work rather than read assertions about: point a
//! ClassicUO at it and it reaches the character list.
//!
//! It is deliberately thin. Everything it does is a few lines of glue over
//! `openshard-gateway` and `openshard-login`, both of which are pure state
//! machines with their own tests. If logic starts collecting here, it belongs in
//! a crate instead.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Instant;

use openshard_gateway::{ConnectionId, Event, Server, ServerEvent};
use openshard_login::{single_shard, DevAccounts, LoginServer, LoginSession, Response};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

/// Where the shard listens. 2593 is the UO game port; 7775 is the login port.
///
/// One port for both: the login server relays the client to this same address,
/// and it reconnects here. Sphere splits them across processes for a shard
/// cluster; a single shard has no reason to.
const LISTEN_PORT: u16 = 2593;

/// What the shard calls itself in the 0xA8 list.
const SHARD_NAME: &str = "OpenShard";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let address = SocketAddr::from(([0, 0, 0, 0], LISTEN_PORT));
    let (server, events) = Server::bind(address).await?;
    info!(%address, "OpenShard starting");

    // The address handed to clients in the 0x8C relay. Loopback is right for a
    // laptop and wrong for anything else: a client on another machine will
    // dutifully try to connect to its own 127.0.0.1 and fail. This is the first
    // thing config has to own.
    let advertised = single_shard(Ipv4Addr::new(127, 0, 0, 1), LISTEN_PORT);

    tokio::spawn(server.run());
    run_login(events, advertised).await;
    Ok(())
}

/// Drive login for every connection until the gateway stops.
async fn run_login(mut events: mpsc::UnboundedReceiver<ServerEvent>, advertised: SocketAddrV4) {
    let accounts = DevAccounts::new()
        .with_account("admin", "hunter2")
        .with_character("admin", "Lord British");
    let mut login = LoginServer::new(accounts, SHARD_NAME, advertised);

    // One session per live connection. Not a global: this task owns it, and the
    // gateway's Disconnected event is what keeps it from growing forever.
    let mut sessions: HashMap<ConnectionId, Session> = HashMap::new();

    while let Some(event) = events.recv().await {
        match event {
            ServerEvent::Connected {
                id,
                address,
                outbox,
            } => {
                info!(%id, %address, "connected");
                sessions.insert(
                    id,
                    Session {
                        login: LoginSession::new(),
                        outbox,
                    },
                );
            }

            ServerEvent::Received { id, event } => {
                let Some(session) = sessions.get_mut(&id) else {
                    // Disconnected arrived first. Possible: the gateway's tasks
                    // and this loop are not synchronised.
                    continue;
                };

                match event {
                    Event::Seeded(seed) => {
                        session.login.on_seed(seed);
                    }
                    Event::Packet(packet) => {
                        let response = login.handle(&mut session.login, &packet, Instant::now());
                        if !session.apply(response, id) {
                            sessions.remove(&id);
                        }
                    }
                }
            }

            ServerEvent::Disconnected { id, reason } => {
                match reason {
                    Some(reason) => warn!(%id, %reason, "disconnected"),
                    None => info!(%id, "disconnected"),
                }
                sessions.remove(&id);
            }
        }

        // Reclaim keys from clients that selected a shard and never came back.
        // Cheap, and this is the only loop that runs often enough to bother.
        login.keys.expire(Instant::now());
    }

    error!("the gateway stopped");
}

/// Per-connection state this loop owns.
struct Session {
    login: LoginSession,
    outbox: mpsc::UnboundedSender<Vec<u8>>,
}

impl Session {
    /// Act on a login response. Returns `false` if the connection should go.
    ///
    /// Dropping the outbox is what closes the socket: the gateway's write task
    /// ends when its channel does. There is no separate "close" to forget to
    /// call.
    fn apply(&self, response: Response, id: ConnectionId) -> bool {
        match response {
            Response::Idle => true,
            Response::Send(bytes) => self.outbox.send(bytes).is_ok(),
            Response::SendThenClose(bytes) => {
                let _ = self.outbox.send(bytes);
                false
            }
            Response::Close => {
                warn!(%id, "closing on a protocol error");
                false
            }
        }
    }
}
