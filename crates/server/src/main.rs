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
use std::net::SocketAddrV4;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use openshard_config::{Config, DEFAULT_TOML};
use openshard_gateway::{ConnectionId, Event, Server, ServerEvent};
use openshard_login::{DevAccounts, LoginServer, LoginSession, Response};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

/// Where the config lives, relative to the working directory.
const CONFIG_PATH: &str = "openshard.toml";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Printed rather than returned as a `Result`: `main` returning `Err`
            // renders it with `Debug`, which for a config error is a wall of
            // struct fields instead of the sentence that says what to fix.
            error!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config(CONFIG_PATH)?;

    // The 0x8C relay carries four bytes of address. There is no IPv6 form of it,
    // so a v6 `advertise` cannot be honoured — better to say so at startup than
    // to hand clients an address the packet cannot express.
    let advertised = config.advertise_v4().ok_or(
        "server.advertise is IPv6; the UO relay packet has four bytes for an address \
         and no way to carry one",
    )?;

    let (server, events) = Server::bind(config.server.listen).await?;
    info!(
        shard = config.server.name,
        listen = %config.server.listen,
        advertise = %config.server.advertise,
        accounts = config.accounts.len(),
        "OpenShard starting"
    );
    if advertised.ip().is_loopback() {
        warn!(
            "server.advertise is loopback: only clients on this machine can reach the shard. \
             Set it to the address clients dial."
        );
    }

    tokio::spawn(server.run());
    run_login(events, &config, advertised).await;
    Ok(())
}

/// Load the config, writing the shipped default if there is none.
///
/// A fresh checkout should run. Writing the default rather than baking one in
/// means the first thing a new operator sees is the file they need to edit, with
/// the `advertise` warning in it, instead of a shard that works on their laptop
/// and nowhere else for reasons nobody wrote down.
fn load_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    if !Path::new(path).exists() {
        std::fs::write(path, DEFAULT_TOML)?;
        info!(path, "no config found; wrote the default");
    }
    Ok(Config::load(path)?)
}

/// Drive login for every connection until the gateway stops.
async fn run_login(
    mut events: mpsc::UnboundedReceiver<ServerEvent>,
    config: &Config,
    advertised: SocketAddrV4,
) {
    let mut accounts = DevAccounts::new();
    for account in &config.accounts {
        accounts = accounts.with_account(&account.name, &account.password);
        for character in &account.characters {
            accounts = accounts.with_character(&account.name, character);
        }
    }
    let mut login = LoginServer::new(accounts, &config.server.name, advertised);

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
