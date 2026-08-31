//! A game master says `.admin`, and the menu the shard draws comes back.
//!
//! # What needs both ends
//!
//! Nothing about this feature lives in one place. The client encodes speech
//! (`0xAD`), the server recognises the leading `.` as a staff command, checks
//! the account's authority, and answers with a window (`0xB0`) written in a
//! layout language the client then has to read. Four seams, and every one of
//! them fails *quietly*: a mis-encoded header shifts the text so the command is
//! never recognised, an authority gate that says no answers with a line rather
//! than a window, and a layout the client cannot parse renders as an empty box.
//! None of that is visible from either side alone.
//!
//! So this drives the whole path and then presses a button, because the reply
//! (`0xB1`) is the other half nothing else exercises: the shard's answer to it
//! is a line addressed to the actor, which is the proof the button meant what
//! the layout said it meant.

use std::time::Duration;

use openshard_client_net::connection::Event;
use openshard_client_net::talk;
use openshard_client_net::transport::enter_world;
use openshard_client_net::view::OpenGump;
use openshard_e2e_shard::{
    ACCOUNT,
    plan,
    spawn,
    stock_config,
    version,
};
use openshard_protocol::gump::layout::Element;
use openshard_protocol::gump::{
    GumpButton,
    RawButtonId,
    RawGumpId,
    RawGumpKey,
};
use openshard_protocol::server_packet::ServerPacket;

#[tokio::test]
async fn saying_dot_admin_opens_the_staff_menu_and_its_buttons_answer() {
    // Held for the length of the test: dropping the handle stops the shard.
    //
    // Not the stock shard: the shipped development account is an ordinary
    // player, and `.admin` from one is speech — the server says the words over
    // their head and draws nothing, which is the *right* behaviour and not the
    // one under test. So the authority is granted here, where the grant is
    // visible, rather than by an assumption about what the config ships.
    let (address, _shard) = spawn(|address| {
        let mut config = stock_config(address);
        for account in &mut config.accounts {
            if account.name == ACCOUNT {
                account.access = openshard_config::RawAccessLevel("administrator".to_owned());
            }
        }
        config
    });
    let entered = tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout")
        .expect("the client reached the world");
    let (mut socket, mut view) = entered;

    // The account has the authority granted above, so the gate passes. A shard
    // that answered with speech instead of a window would time out below rather
    // than fail here, which is why the journal is printed with the failure.
    socket
        .send(&talk::say(
            ".admin",
            openshard_protocol::speech::TalkMode::Regular,
        ))
        .await
        .expect("the shard is listening");

    let menu: OpenGump = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = socket.next_event().await.expect("the socket stayed up") {
            let Event::Packet(packet) = event else {
                continue;
            };
            let was_gump = matches!(packet.as_ref(), ServerPacket::GumpDisplay(_));
            view.apply(&packet);
            if was_gump {
                return view.gumps.last().cloned().expect("a 0xB0 opened a window");
            }
        }
        panic!("the shard closed the connection without drawing the menu");
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "no window arrived. What the shard said instead: {:?}",
            view.journal.iter().map(|line| &line.text).collect::<Vec<_>>()
        )
    });

    // The layout parsed into something usable — not merely into a window. An
    // empty box is what a client that could not read the layout would show.
    let heading = menu.elements.iter().find_map(|element| {
        match element {
            Element::Label { line, .. } => menu.line(*line),
            _ => None,
        }
    });
    assert_eq!(heading, Some("Admin"), "the menu draws its own name");

    let (button, label) = menu
        .elements
        .windows(2)
        .find_map(|pair| {
            match pair {
                // Every row of this menu is a reply button followed by its label —
                // the label is what the button *means*, and pairing them is what
                // says the parse kept the layout's order.
                [
                    Element::Button {
                        kind: GumpButton::Reply,
                        id,
                        ..
                    },
                    Element::Label { line, .. },
                ] => Some((*id, menu.line(*line).unwrap_or_default().to_owned())),
                _ => None,
            }
        })
        .expect("the menu has at least one labelled reply button");
    assert!(
        !label.is_empty(),
        "a button nobody can read is a button nobody presses"
    );

    // And the way back. The shard answers a button it accepts with a line to the
    // actor — see `World::handle_gump_response` — so a `0xB1` that was routed,
    // authorised and matched against the drawn layout is one that produces
    // speech, and one that was not produces silence.
    socket
        .send(&talk::answer_gump(
            RawGumpKey(menu.key.0),
            RawGumpId(menu.gump_id.0),
            button,
            Vec::new(),
            Vec::new(),
        ))
        .await
        .expect("the shard is listening");

    let answered = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = socket.next_event().await.expect("the socket stayed up") {
            let Event::Packet(packet) = event else {
                continue;
            };
            view.apply(&packet);
            if let Some(line) = view.journal.back() {
                if line.text.starts_with("Admin:") {
                    return line.text.clone();
                }
            }
        }
        panic!("the shard closed the connection without answering the button");
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "button {button:?} ({label}) was pressed and the shard said nothing about it. \
             What it did say: {:?}",
            view.journal.iter().map(|line| &line.text).collect::<Vec<_>>()
        )
    });
    assert_ne!(
        button,
        RawButtonId(0),
        "the close box is not one of the menu's own buttons"
    );
    assert!(
        answered.len() > "Admin:".len(),
        "the shard named the verb it ran: {answered}"
    );
}
