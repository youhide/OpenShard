//! A client logs in to a shard and ends up standing in the world.
//!
//! Both ends are real: the shard is the same `run_shard` the binary runs, and
//! the client is `openshard-client-net` driving two actual sockets. What this
//! catches is the only thing neither side's own tests can — that the two agree.
//! Every layer below has better tests than this one; this is the seam.
//!
//! The shard runs with no client files and no database: the world is a bare
//! grid where every step is allowed, kept in memory. That is enough to be
//! logged into, and it keeps the test runnable on a machine that has no copy of
//! the client's data.

use std::time::Duration;

use openshard_client_net::transport::enter_world;
use openshard_e2e_shard::{
    plan,
    shard,
    version,
};
use openshard_protocol::identity::RawPlaintextPassword;

#[tokio::test]
async fn a_client_logs_in_and_stands_in_the_world() {
    // Held for the length of the test: dropping the handle stops the shard.
    let (address, _shard) = shard();

    // A bound, rather than waiting forever: every step of this conversation is
    // one side waiting for the other, so the failure mode of a mismatch is a
    // hang, not an error.
    let entered = tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout");

    let (_socket, view) = entered.expect("the client reached the world");

    // The shard put a body somewhere on the map and told us which one it is.
    // Not asserted against fixed coordinates: where a new character starts is
    // config, and this test is about the conversation, not the map.
    assert!(
        view.map.width > 0 && view.map.height > 0,
        "the facet has a size: {:?}",
        view.map
    );
    assert_ne!(view.player.body.0, 0, "a body graphic was chosen");

    // Everything between the `0x1B` and the `0x55` is the world being handed
    // over, and it is never sent again. The equipment is what proves it landed:
    // every character wears a backpack — see `tick::enter` — and the only packet
    // that says so is the player's own `0x78`, inside that window.
    assert!(
        !view.player.equipment.is_empty(),
        "the client kept what arrived before it was allowed to draw"
    );
}

#[tokio::test]
async fn a_wrong_password_comes_back_as_a_refusal() {
    // The other end of the same seam: a refusal has to reach the client as the
    // reason it was given, not as a socket that closed. The shard sends `0x82`
    // and hangs up immediately after, so a client that only noticed the close
    // would lose the reason it had already been handed.
    // Held for the length of the test: dropping the handle stops the shard.
    let (address, _shard) = shard();
    let plan = openshard_client_net::session::Plan {
        password: RawPlaintextPassword("wrong".to_owned()),
        ..plan()
    };

    let outcome = tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan, version()))
        .await
        .expect("the shard answered inside the timeout");

    let error = outcome.expect_err("a wrong password cannot reach the world");
    assert!(
        matches!(
            error,
            openshard_client_net::transport::TransportError::Refused(
                openshard_protocol::login::DenyReason::BadPassword
            )
        ),
        "{error}"
    );
}
