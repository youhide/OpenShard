//! A client walks, and the two ends agree on where it ended up.
//!
//! # What is actually being checked, and why it needs both ends
//!
//! A `0x22` ack says "yes" and nothing else — no position. So a walking client's
//! idea of where it stands is built entirely out of its own predictions: the
//! turn-versus-step rule, the tile a step lands on, and one sequence byte per
//! request. Nothing on the wire ever confirms the result. A client that got any
//! of that wrong walks on happily, a tile out, until something snaps it back.
//!
//! `0x21` is that something, and it is the only packet that carries the
//! server's own answer to "where is this body". So this test provokes one on
//! purpose — by asking for more steps at once than
//! [`WALK_BUFFER`](openshard_movement::WALK_BUFFER) allows — and compares the
//! position it carries against the position the client derived on its own. The
//! refusal is not the thing under test; it is the oracle.

use std::time::Duration;

use openshard_client_net::connection::Event;
use openshard_client_net::transport::enter_world;
use openshard_client_net::walk::{
    InFlightSteps,
    MAX_IN_FLIGHT,
    Moved,
    Walk,
};
use openshard_e2e_shard::{
    plan,
    shard,
    version,
};
use openshard_movement::WALK_BUFFER;
use openshard_protocol::direction::Facing;
use openshard_protocol::world::Point;

#[tokio::test]
async fn a_client_walks_and_the_shard_agrees_on_where_it_ended_up() {
    // Held for the length of the test: dropping the handle stops the shard.
    let (address, _shard) = shard();
    let entered = tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout")
        .expect("the client reached the world");
    let (mut socket, mut view) = entered;

    let start = view.player.position;
    let mut walk = Walk::new(start, view.player.facing);
    // The way the body is already facing, so every request is a step and none
    // of them is a turn — a turn would move nobody and make "advanced by N
    // tiles" below untrue for reasons that have nothing to do with the seam.
    let heading = Facing::walking(view.player.facing.direction);

    // More than the pace budget, so a refusal is certain. Everything up to it is
    // allowed and acked, which is what makes the comparison afterwards worth
    // anything.
    //
    // Sent a few at a time rather than all at once, because the client caps its
    // own unanswered steps at [`MAX_IN_FLIGHT`] — that cap is for a shard that
    // has gone quiet, and this one is answering as fast as a loopback can. The
    // budget is spent on *time* either way: these leave as fast as the round trip
    // allows, which is far faster than a body walks.
    let burst = WALK_BUFFER as usize + 10;
    let mut sent = 0usize;
    let mut acked = 0usize;
    // Where the client believes it is, built only out of its own predictions and
    // the acks that confirmed them.
    let mut confirmed = start;
    let mut refused_at = None;

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            // Keep the wire as full as the client is willing to have it.
            while sent < burst && walk.in_flight() < MAX_IN_FLIGHT {
                // No ground function: this test is about the sequence seam, and
                // the client here has no map of its own to predict a height from
                // — the window is what loads one and hands it to `Walk`.
                let step = walk.step(heading, |_, _| None).expect("room on the map to walk");
                socket.send(step.bytes()).await.expect("the shard is listening");
                sent += 1;
            }
            if sent == burst && walk.in_flight() == InFlightSteps::new(0) {
                // Every step answered and none refused. The assertions below say
                // what that means.
                return;
            }
            let Some(event) = socket.next_event().await.expect("the socket stayed up") else {
                return;
            };
            let Event::Packet(packet) = event else {
                continue;
            };
            match walk.on_packet(&packet).expect("the shard acked steps in order") {
                Moved::Stepped { position, facing, .. } => {
                    view.player_stepped(position, facing);
                    confirmed = position;
                    acked += 1;
                }
                Moved::Snapped { position, .. } => {
                    refused_at = Some(position);
                    return;
                }
                Moved::Turned { .. } | Moved::Idle => {
                    view.apply(&packet);
                }
            }
        }
    })
    .await
    .expect("the shard answered every step inside the timeout");

    assert!(
        acked > 0,
        "not one step was allowed, so the comparison below would hold for a client \
         that never moved and proves nothing"
    );

    let (dx, dy) = heading.direction.step();
    let walked = Point::new(
        (i32::from(start.x) + dx * acked as i32) as u16,
        (i32::from(start.y) + dy * acked as i32) as u16,
        start.z,
    );
    assert_eq!(
        confirmed, walked,
        "{acked} acked steps {} should land {acked} tiles away",
        heading.direction
    );

    let refused_at = refused_at.expect(
        "the burst never outran the pace budget, so nothing carried the shard's own \
         idea of where this body is and there was nothing to compare against",
    );
    assert_eq!(
        refused_at, confirmed,
        "the shard's own position and the client's prediction disagree: \
         the walk handshake is out of step"
    );
}
