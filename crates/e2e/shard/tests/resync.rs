//! The walk handshake's repair leg, over a real socket, both ends.
//!
//! # Why this needs both ends
//!
//! A client that has lost track of the walk stops walking — `Walk::out_of_step`,
//! and the reference does the same with `WalkerManager.WalkingFailed`. That is
//! only safe because something is guaranteed to start it again, and the only
//! thing that can is the shard: a `0x22` ack carries no position, so there is
//! nothing local a lost client could work the answer out from.
//!
//! So the freeze and its release are one contract across two crates, and neither
//! half means anything alone. A unit test of the client proves it asks; a unit
//! test of the world proves `resync` sends a `0x20`. Only both ends on one wire
//! prove that the packet the client sends is the packet the shard decodes — and
//! that is the interesting part here, because `0x22` is *two different packets*,
//! one per direction, three bytes each, with nothing in the body to tell them
//! apart. A shard that routed the client's `0x22` to its own walk-ack decoder
//! would find a plausible sequence byte in it and answer nothing at all.

use std::time::Duration;

use openshard_client_net::connection::Event;
use openshard_client_net::transport::enter_world;
use openshard_client_net::walk::{Moved, Walk};
use openshard_protocol::direction::Facing;
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::world::{ResyncRequest, StepSequence, WalkAck};

use openshard_e2e_shard::{plan, shard, version};

#[tokio::test]
async fn a_lost_client_asks_where_it_is_and_the_shard_tells_it() {
    let (address, _shard) = shard();
    let entered = tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout")
        .expect("the client reached the world");
    let (mut socket, view) = entered;

    let start = view.player.position;
    let mut walk = Walk::new(start, view.player.facing);

    // Lose track of the handshake without involving the shard at all: an ack for
    // a step that was never sent. Fed straight in, because provoking a real one
    // takes a wall and a slow link and this test is about the *repair*, not about
    // how the disagreement arose.
    let ack = ServerPacket::WalkAck(WalkAck {
        sequence: StepSequence(9),
        notoriety: Notoriety::Innocent,
    });
    assert!(
        walk.on_packet(&ack).is_err(),
        "an ack for a step nobody sent is a disagreement this end cannot repair"
    );
    assert!(walk.out_of_step(), "so the walk stopped");
    assert!(
        walk.step(Facing::walking(view.player.facing.direction), |_, _| None)
            .is_err(),
        "and sends nothing until it is told where it is"
    );

    // The question. This is the packet under test: the shard has to recognise it
    // as a resync request rather than as the walk ack that shares its id.
    socket
        .send(&ResyncRequest.encode())
        .await
        .expect("the shard is listening");

    // The answer: a `0x20` naming the tile the shard has this body on.
    let told = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = socket.next_event().await.expect("the socket stayed up") {
            let Event::Packet(packet) = event else {
                continue;
            };
            if let Moved::Snapped { position, .. } = walk.on_packet(&packet).expect("nothing else desyncs") {
                return Some(position);
            }
        }
        None
    })
    .await
    .expect("the shard answered inside the timeout");

    let told = told.expect("the shard answered the resync with a position");
    assert_eq!(
        told, start,
        "the body has not moved, so the answer is where it already was"
    );
    assert!(
        !walk.out_of_step(),
        "and being told where it is releases the walk"
    );

    // Which is the whole point: it can walk again, from a fresh sequence — the
    // shard reset its own when it answered, and a client that carried on counting
    // would be refused on this very step.
    let heading = Facing::walking(view.player.facing.direction);
    let step = walk.step(heading, |_, _| None).expect("the walk is free again");
    socket.send(step.bytes()).await.expect("the shard is listening");

    let allowed = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = socket.next_event().await.expect("the socket stayed up") {
            let Event::Packet(packet) = event else {
                continue;
            };
            match walk
                .on_packet(&packet)
                .expect("the shard acked the step it was sent")
            {
                Moved::Stepped { .. } => return true,
                Moved::Snapped { .. } => return false,
                Moved::Turned { .. } | Moved::Idle => {}
            }
        }
        false
    })
    .await
    .expect("the shard answered the step inside the timeout");

    assert!(
        allowed,
        "the first step after a resync was refused: the two ends did not both go back to zero"
    );
}
