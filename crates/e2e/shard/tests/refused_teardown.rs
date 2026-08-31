//! A refused entry tears the whole connection down, and the world lets go.
//!
//! # The chain, link by link
//!
//! Nothing anywhere states this; every link is documented where it lives and no
//! file says they are one thing. That is what this test is: the artefact that
//! knows all six, so the prose belongs here rather than in a seventh file that
//! could go stale on its own. See S3 in `docs/unenforced.md`.
//!
//! 1. The world refuses an entry and says so — `PlayerRefused`, emitted by
//!    `World::enter` for every failure path of `try_enter`.
//! 2. `PhaseSync::apply` reads that event in the next tick, moves the session to
//!    `WorldPhase::Left`, and hands the connection back to `Shard::tick` to be
//!    closed. There is no refusal packet in the protocol for a `0x5D`, so
//!    dropping the socket is the only thing that turns an indefinite hang into a
//!    reconnect.
//! 3. `Sessions::close` removes the session, which drops its `OutboxTx`.
//! 4. The gateway's write task ends when its outbox channel closes, and closes
//!    the socket on the way out. There is no separate "close" call anywhere —
//!    dropping the sender *is* the close.
//! 5. The gateway sees the socket go and emits `ServerEvent::Disconnected`.
//! 6. `Shard::handle_network` queues `Command::Disconnect`, and the tick that
//!    applies it is where the world lets go of the entity, the serial and the
//!    inventory. Never from the shard loop directly: that would be a write to
//!    the world from outside the tick.
//!
//! Break link 3, 4 or 5 and the world still holds a character for a connection
//! that no longer exists — the leak the whole of `docs/connection_state.md` was
//! written against, and the one this cannot become a type for: the links are a
//! socket, a task and a channel, and what joins them is `Drop`.
//!
//! # Why there is a second client
//!
//! The last link is the one that matters and it is the one a refused client
//! cannot see: its own socket closing proves links 1 to 4, and a world that
//! never let go would close it in exactly the same way. So a second client
//! stands in the world as a witness, and what it is asked is the only question
//! whose answer is the world's own: is the refused character still there. It
//! sees the arrival (`0x78`) and then the departure (`0x1D`), and the first of
//! those is not decoration — a "it is gone" assertion about something that was
//! never there is green for the wrong reason. `connection_state.md` S7 learned
//! that one the expensive way.
//!
//! The witness is a *second account* playing a *second character*, which is why
//! `openshard_e2e_shard::stock_config` appends one to the stock config. Two connections
//! playing the one character the stock config ships does work today, and it is
//! not a rule anybody wrote down — nothing refuses a second login on an account,
//! and nothing promises not to. A fixture standing on that is a fixture that
//! dies the day someone adds the check, in a test that has nothing to do with
//! logging in twice.

use std::time::Duration;

use openshard_client_net::connection::Event;
use openshard_client_net::session::Plan;
use openshard_client_net::transport::{
    Socket,
    TransportError,
    enter_world,
};
use openshard_client_net::view::WorldView;
use openshard_e2e_shard::{
    CHARACTER,
    NYSTUL,
    WITNESS,
    plan,
    plan_for,
    shard,
    version,
};
use openshard_protocol::identity::RawCharacterName;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{
    RawCharacterSlot,
    RawClientIp,
};
use openshard_protocol::world::CharacterPlay;
use tokio::net::TcpStream;

/// How long any one step of this may take.
///
/// Generous on purpose, and it bounds a *deadline* rather than pacing anything:
/// every wait below polls a socket and stops the moment its answer arrives, so
/// this number is only ever paid by a failure. A `sleep` long enough to work
/// here would be a test that passes on this machine — see D3 in
/// `docs/unenforced.md`.
const WAIT: Duration = Duration::from_secs(20);

/// Read `socket`, folding everything it says into `view`, until `done` holds.
///
/// Checked before the first read, so a condition that is already true costs
/// nothing. `what` names the thing being waited for, for the panic.
async fn read_until(
    socket: &mut Socket<TcpStream>,
    view: &mut WorldView,
    what: &str,
    done: impl Fn(&WorldView) -> bool,
) {
    tokio::time::timeout(WAIT, async {
        while !done(view) {
            let event = socket.next_event().await.expect("the witness's socket stayed up");
            let Some(event) = event else {
                panic!("the witness's socket closed while waiting for {what}");
            };
            if let Event::Packet(packet) = event {
                view.apply(&packet);
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("waited {WAIT:?} for {what}"));
}

/// Read `socket` until the far end hangs up.
///
/// A clean close reads as zero bytes; a reset reads as an I/O error. Both are
/// the socket going away, and which one a given kernel produces is not what is
/// under test — the gateway drops its socket rather than shutting it down, so
/// the difference is only whether anything was still sitting unread in the
/// server's receive queue at that instant.
///
/// Any other error is a failure rather than a close. A stream that stopped
/// making sense while the connection was still open is a different bug wearing
/// this one's clothes, and `Err(_) => return` would report it as a teardown that
/// worked.
async fn read_until_closed(socket: &mut Socket<TcpStream>, what: &str) {
    tokio::time::timeout(WAIT, async {
        loop {
            match socket.next_event().await {
                Ok(Some(_)) => {}
                Ok(None) | Err(TransportError::Io(_)) => return,
                Err(error) => panic!("the socket failed in a way that is not a close: {error}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("waited {WAIT:?} for {what}"));
}

/// The `0x5D` a client sends to play `CHARACTER`, as ClassicUO encodes it.
///
/// Sent a second time on a connection already in the world, which is the one
/// refusal (`RefusedEntry::AlreadyInWorld`) a real client can provoke over the
/// wire: the other three are a serial collision, an exhausted pool, and a name
/// off no list this account was sent.
fn play_again() -> Vec<u8> {
    CharacterPlay {
        name:      RawCharacterName(CHARACTER.to_owned()),
        slot:      RawCharacterSlot(0),
        client_ip: RawClientIp(0),
    }
    .encode()
}

/// Log in on `plan` and stand in the world, or say which client failed to.
async fn enter(address: std::net::SocketAddrV4, plan: Plan, who: &str) -> (Socket<TcpStream>, WorldView) {
    tokio::time::timeout(WAIT, enter_world(address, plan, version()))
        .await
        .unwrap_or_else(|_| panic!("{who} did not finish logging in inside {WAIT:?}"))
        .unwrap_or_else(|error| panic!("{who} did not reach the world: {error}"))
}

#[tokio::test]
async fn a_refused_entry_closes_the_socket_and_the_world_forgets_the_character() {
    // Held for the length of the test: dropping the handle stops the shard.
    let (address, _shard) = shard();

    // The witness first, so it is already standing there when the second client
    // arrives and is told about it by `0x78` rather than having to have been
    // there all along.
    let (mut witness, mut seen) = enter(address, plan_for(WITNESS, NYSTUL), "the witness").await;
    let (mut doomed, doomed_view) = enter(address, plan(), "the client that will be refused").await;
    let doomed_serial: Serial = doomed_view.player.serial;
    assert_ne!(
        doomed_serial, seen.player.serial,
        "the two clients are standing in the world as one body; \
         a shared serial would make every assertion below meaningless"
    );

    // The state *before*: the world really did put the second character where
    // the witness can see it. Without this, the disappearance below is green on
    // a shard that never spawned anything.
    read_until(
        &mut witness,
        &mut seen,
        "the second character to appear",
        |view| view.mobiles.contains_key(&doomed_serial),
    )
    .await;

    // The provocation. A second `0x5D` on a connection already in the world:
    // `try_enter` refuses it, and link 1 of the chain starts.
    doomed
        .send(&play_again())
        .await
        .expect("the shard is still listening");

    // Links 2 to 4, from the only side that can see them: the socket goes.
    read_until_closed(&mut doomed, "the refused socket to close").await;

    // Links 5 and 6, and the only question whose answer is the world's own.
    read_until(&mut witness, &mut seen, "the refused character to go", |view| {
        !view.mobiles.contains_key(&doomed_serial)
    })
    .await;
}
