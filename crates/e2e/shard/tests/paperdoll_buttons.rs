//! The paperdoll's buttons, over a real socket, both ends.
//!
//! # Why this needs both ends
//!
//! Every one of these buttons is a *request*: the client draws the frame, sends
//! a packet, and what appears afterwards is the shard's answer. Neither half
//! means anything alone, and the seam between them is where all four failure
//! modes live — a packet written into an id the shard routes elsewhere, a
//! subcommand word the dispatch does not name, an answer with no decoder on the
//! way back, and a client that never folds what it decoded. Each one leaves a
//! button that looks exactly like a button that works.
//!
//! The war toggle is the one worth the most: it is the only picture on the frame
//! whose *state* comes off the wire, and the last of those four failures is
//! precisely what it had. `ServerPacket::decode` had no arm for `0x72`, so the
//! shard answered every press and the client framed the answer, dropped it, and
//! drew the toggle from a flag byte that could never change.

use std::time::Duration;

use openshard_client_net::connection::Event;
use openshard_client_net::doll;
use openshard_client_net::transport::enter_world;
use openshard_client_net::view::WorldView;
use openshard_e2e_shard::{
    plan,
    shard,
    version,
};
use openshard_protocol::server_packet::ServerPacket;

/// Read packets into `view` until `done` says the answer has arrived, or fail.
///
/// Everything is folded on the way past — the shard sends a good deal that is
/// not what a test is waiting for, and a client that skipped it would be a
/// client that never applied it either.
async fn until<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut openshard_client_net::transport::Socket<S>,
    view: &mut WorldView,
    what: &str,
    mut done: impl FnMut(&WorldView, &ServerPacket) -> bool,
) {
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = socket.next_event().await.expect("the socket stayed up") {
            let Event::Packet(packet) = event else {
                continue;
            };
            view.apply(&packet);
            if done(view, &packet) {
                return;
            }
        }
        panic!("the shard closed the connection before answering {what}");
    })
    .await;
    assert!(waited.is_ok(), "the shard never answered {what}");
}

/// The toggle, both ways round, and the fact the whole feature rests on: what
/// the frame is drawn from is the *server's* stance, not the one the button
/// asked for.
#[tokio::test]
async fn the_war_toggle_asks_for_a_stance_and_the_client_folds_the_answer() {
    let (address, _shard) = shard();
    let entered = tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout")
        .expect("the client reached the world");
    let (mut socket, mut view) = entered;

    assert!(!view.player.war, "a character enters the world at peace");

    socket
        .send(&doll::war_mode(true))
        .await
        .expect("the shard is listening");
    until(&mut socket, &mut view, "the war toggle", |view, _| {
        view.player.war
    })
    .await;

    // And back, because a client that folded "true" from the mere arrival of a
    // `0x72` would pass the half above and fail here.
    socket
        .send(&doll::war_mode(false))
        .await
        .expect("the shard is listening");
    until(&mut socket, &mut view, "leaving war mode", |view, _| {
        !view.player.war
    })
    .await;
}

/// The Quest button, which is the one paperdoll button whose answer this client
/// can already draw: the shard replies with a `0xB0` and the dialog machinery
/// opens it like any other window.
///
/// `0xD7` is an envelope — the id says nothing about what was asked for, the
/// subcommand word does — so this is also the test that the word the client
/// writes is the word the dispatch names.
#[tokio::test]
async fn the_quest_button_opens_the_shards_own_window() {
    let (address, _shard) = shard();
    let entered = tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout")
        .expect("the client reached the world");
    let (mut socket, mut view) = entered;

    let before = view.gumps.len();
    socket
        .send(&doll::quest_log(view.player.serial))
        .await
        .expect("the shard is listening");
    until(&mut socket, &mut view, "the quest log", |view, packet| {
        matches!(packet, ServerPacket::GumpDisplay(_)) && view.gumps.len() > before
    })
    .await;
}

/// "Log Out": the client announces, the shard grants, and the connection is the
/// client's to close from there.
///
/// The `0xD1` is two packets sharing one id — the notification going out and the
/// grant coming back — which is the shape that would look like a working button
/// while the client sat waiting for an answer it had already been sent.
#[tokio::test]
async fn log_out_is_announced_and_granted() {
    let (address, _shard) = shard();
    let entered = tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout")
        .expect("the client reached the world");
    let (mut socket, mut view) = entered;

    socket
        .send(&doll::log_out())
        .await
        .expect("the shard is listening");
    until(&mut socket, &mut view, "the logout", |_, packet| {
        matches!(packet, ServerPacket::LogoutAck(_))
    })
    .await;
}
