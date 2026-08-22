//! The same login, with no network under it.
//!
//! `enter_world` over TCP is covered next door. What this file is about is that
//! the transport is genuinely a parameter: the client's `Dial` and the gateway's
//! stream are the only two things that changed, and the whole conversation —
//! seed, `0xA8`, the relay, a second connection, the character list, `0x1B` and
//! the `0x55` that ends it — comes out the same over a pair of in-memory pipes.
//!
//! It is also the test that would catch the one thing this arrangement can get
//! wrong on its own: a hang. Nothing here binds a port, so there is no
//! "connection refused" to fail with — a gate that captured the wrong runtime,
//! or a write half that never shut down, would simply wait forever. Hence a
//! deadline on every step, and a walk at the end that needs bytes to travel in
//! both directions.

use std::time::Duration;

use openshard_client_net::connection::Event;
use openshard_client_net::transport::enter_world_with;
use openshard_client_net::walk::{Moved, Walk};
use openshard_protocol::direction::Facing;
use openshard_protocol::server_packet::ServerPacket;
use openshard_server::shard::SHUTDOWN_NOTICE;

use openshard_e2e_shard::{in_process, plan, stock_config, version};

/// Generous, and only ever paid by a failure: every wait below finishes the
/// moment its answer arrives. What it bounds is a hang, which is this
/// arrangement's characteristic way of being broken.
const WAIT: Duration = Duration::from_secs(20);

#[tokio::test]
async fn a_client_enters_the_world_with_no_socket_anywhere() {
    // The handle is held for the length of the test: dropping it stops the
    // shard, and `stopping_a_shard_ends_its_thread_and_hangs_up` below is where
    // that is the subject rather than the housekeeping.
    let (dial, _shard) = in_process::spawn(stock_config, Vec::new());

    let (mut socket, mut view) = tokio::time::timeout(WAIT, enter_world_with(dial, plan(), version()))
        .await
        .expect("the login conversation finished inside the deadline")
        .expect("the client reached the world");

    // The shard's own answer about who we are: a `0x1B` was decoded, which means
    // both connections carried real packets and the second one was compressed.
    assert!(
        view.player.serial.raw() != 0,
        "the world sent a body, so the whole login conversation happened"
    );

    // And bytes travel in both directions afterwards: one step, one ack, and the
    // client's own prediction of where it landed.
    let start = view.player.position;
    let mut walk = Walk::new(start, view.player.facing);
    let heading = Facing::walking(view.player.facing.direction);
    let step = walk.step(heading, |_, _| None).expect("room on the map to walk");
    socket.send(step.bytes()).await.expect("the shard is listening");

    let stepped = tokio::time::timeout(WAIT, async {
        while let Some(event) = socket.next_event().await.expect("the pipe stayed up") {
            let Event::Packet(packet) = event else {
                continue;
            };
            match walk
                .on_packet(&packet)
                .expect("the shard acked the step in order")
            {
                Moved::Stepped { position, facing, .. } => {
                    view.player_stepped(position, facing);
                    return position;
                }
                // A refusal here is a failure, not an oracle: one step is well
                // inside the pace budget, so a `0x21` would mean the two ends
                // disagree about a walk nothing has stressed yet.
                Moved::Snapped { position, .. } => panic!("the first step was refused, back to {position:?}"),
                Moved::Turned { .. } | Moved::Idle => {
                    view.apply(&packet);
                }
            }
        }
        panic!("the shard closed the connection without acking the step");
    })
    .await
    .expect("the shard answered the step inside the deadline");

    assert_ne!(stepped, start, "an acked step moves the body");
    assert_eq!(view.player.position, stepped, "and the view follows it");
}

#[tokio::test]
async fn stopping_a_shard_ends_its_thread_and_hangs_up() {
    // The shard used to have no way out at all: the thread was kept, nothing
    // joined it, and the gate it held kept the event channel open, so the tick
    // never saw its input close. Right for a playground that ends with the
    // process; wrong for anything that wants a second world — a fuzzing run
    // starting fifty of them would leak fifty threads.
    //
    // Both halves of a stop are asserted here, because either alone would pass
    // for the wrong reason: `stop` returning proves the tick left its loop and
    // saved (`run_shard` returns after the last write, and `stop` joins), and
    // the client's zero read proves the same word reached the connection task,
    // which is a different loop on the other side of the gate.
    let (dial, shard) = in_process::spawn(stock_config, Vec::new());

    let (mut socket, _view) = tokio::time::timeout(WAIT, enter_world_with(dial, plan(), version()))
        .await
        .expect("the login conversation finished inside the deadline")
        .expect("the client reached the world");

    // Blocking, on purpose and safely: the shard runs on a thread of its own, so
    // nothing this runtime is holding is needed for it to finish.
    shard.stop();

    // And the client, which asked for nothing and was never told, finds the
    // connection closed under it rather than waiting for a shard that has gone.
    let ended = tokio::time::timeout(WAIT, async {
        loop {
            match socket.next_event().await {
                Ok(Some(_)) => continue, // whatever was in flight when it stopped
                Ok(None) => return,      // hung up on: what a stop looks like from here
                Err(error) => panic!("the pipe failed rather than closing: {error}"),
            }
        }
    })
    .await;
    assert!(
        ended.is_ok(),
        "the client was left waiting on a shard that had stopped"
    );
}

#[tokio::test]
async fn a_stop_tells_the_player_before_it_hangs_up() {
    // The manners, end to end. A clean stop used to be indistinguishable from
    // the shard crashing — the screen freezes, the connection dies — and this is
    // the one event in the engine that had nothing to say for itself.
    //
    // What is asserted is the *order*, and that is not pedantry: the notice is
    // queued by the world and only reaches the wire when the outbound queue is
    // drained into the sessions, and the sessions are what the hang-up drops.
    // Anything inserted between the announcement and the flush — or the sessions
    // let go one line too early — swallows the line without failing anything
    // else. Checking only that the text arrived would pass on a machine fast
    // enough for the bytes to win the race, and fail on someone else's.
    let (dial, shard) = in_process::spawn(stock_config, Vec::new());

    let (mut socket, _view) = tokio::time::timeout(WAIT, enter_world_with(dial, plan(), version()))
        .await
        .expect("the login conversation finished inside the deadline")
        .expect("the client reached the world");

    shard.stop();

    let heard = tokio::time::timeout(WAIT, async {
        let mut said = None;
        loop {
            match socket.next_event().await {
                Ok(Some(Event::Packet(ServerPacket::SpokenMessage(line)))) => said = Some(line.text),
                Ok(Some(_)) => continue, // whatever else was in flight
                // The hang-up, and the end of the ordering assertion: anything
                // still unheard at this point was never sent.
                Ok(None) => return said,
                Err(error) => panic!("the pipe failed rather than closing: {error}"),
            }
        }
    })
    .await
    .expect("the shard hung up inside the deadline");

    assert_eq!(
        heard.as_deref(),
        Some(SHUTDOWN_NOTICE),
        "a stopping shard says why, and says it before it goes"
    );
}

#[tokio::test]
async fn dialling_a_shard_that_has_stopped_gets_a_closed_pipe() {
    // An `InProcess` is `Clone` and nothing ties a clone's life to the `Running`,
    // so a dial after the stop is not a misuse anybody can be told off for — a
    // virtual player holding one has no way to know the shard went. What it used
    // to get was a connection spawned onto the shard's runtime just as that
    // runtime was being dropped: no panic, but the task is dropped unpolled, so
    // the login conversation simply never begins and the client waits on a pipe
    // whose other end nobody is holding.
    //
    // Now the gate says so. What the caller gets is a stream that is already
    // closed, which is the same thing it gets when a shard hangs up mid-session —
    // one way for a shard to be gone, not two.
    //
    // This test passed before the gate learned to refuse, and it is kept knowing
    // that. A dropped runtime cancels the task that owned the server end, which
    // closes the pipe by accident and arrives at the same visible answer — so
    // what fails without the refusal is `a_gate_that_is_stopping_serves_nobody`
    // next to the code, which asks the question this one cannot see: whether an
    // id was minted and the world told about a session that will never speak.
    // What this one is for is the whole path — that a dial after a stop *ends*,
    // by whichever mechanism, rather than hanging until a deadline.
    let (dial, shard) = in_process::spawn(stock_config, Vec::new());

    // Cloned before the stop, because that is the case: something took a dialler
    // while the shard was up and still holds it afterwards.
    let after = dial.clone();
    shard.stop();

    let entered = tokio::time::timeout(WAIT, enter_world_with(after, plan(), version())).await;
    let ended = entered.expect("the login gave up rather than hanging on a dead shard");
    assert!(
        ended.is_err(),
        "a shard that has stopped let a client in through a gate that was closed"
    );
}
