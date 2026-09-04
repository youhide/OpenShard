//! Splitting and merging a stack, over a real shard and a real socket.
//!
//! # Why this needs both ends
//!
//! Two bugs prompted this file, and both were agreement failures between the
//! shard and this client's own model of the world rather than a defect either
//! side's unit tests could see alone:
//!
//! - Answering the amount prompt on a split used to synthesize a drop back
//!   into the same bag instead of leaving the taken amount on the cursor —
//!   `crates/client/app/src/panes/container.rs`'s own doc comment argued for
//!   the old behaviour, and its unit tests never drove a `SplitPrompt` answer
//!   through `handle()` to catch it.
//! - Merging one pile onto another consumed the dragged-away pile without
//!   ever telling the *dragging* connection it was gone — every other watcher
//!   gets a `Remove`, but a held item is on nobody's screen but its holder's
//!   own cursor, so that connection was skipped. `stack.rs`'s own proptest
//!   suite covers the merge arithmetic exhaustively and never once looked at
//!   what packet went out afterwards.
//!
//! Both gaps are exactly the seam `crates/e2e/shard` exists for: the only
//! place in the workspace allowed to name both ends of the wire at once (see
//! this crate's own `src/lib.rs`). The scenarios below drive raw
//! [`Outgoing`] actions — `PickUp`/`DropInto`/`DropOnGround` — the same
//! encoding `crates/client/app` sends once a pane has decided what to ask
//! for, and read the answer back into a [`WorldView`], the same projection
//! the app draws from. Nothing here goes through a pane or a gesture: the
//! bugs were never in deciding what to ask for, they were in what came back.

use std::collections::HashSet;
use std::time::Duration;

use openshard_client_net::action::Outgoing;
use openshard_client_net::connection::Event;
use openshard_client_net::talk;
use openshard_client_net::transport::{
    Socket,
    enter_world,
};
use openshard_client_net::view::WorldView;
use openshard_config::RawAccessLevel;
use openshard_e2e_shard::{
    ACCOUNT,
    Running,
    plan,
    spawn,
    stock_config,
    version,
};
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::items::{
    ItemAmount,
    WorldItemPayload,
};
use openshard_protocol::serial::Serial;
use openshard_protocol::speech::TalkMode;
use openshard_protocol::wire::{
    Graphic,
    Layer,
};
use tokio::net::TcpStream;

/// A stackable graphic with no amount-dependent art variant.
///
/// Gold and its two neighbours swap in a fuller-pile picture past five items
/// (`openshard_client_render::items::stacks::displayed_graphic`) — a
/// rendering concern this suite has no atlas and no interest in. Ore draws
/// the same icon at any amount, so the entity under test is exactly the
/// serial and the number the shard tracks, nothing about how it looks.
const ORE: Graphic = Graphic(0x19B9);

/// Read packets into `view` until `done` says the shard has told this client
/// everything the scenario is waiting for, or fail loudly.
///
/// The predicate is deliberately the *whole* completion condition — a merge's
/// `Remove` for the consumed pile and its target's `AddToContainer` are two
/// packets, sent one call apart, and stopping as soon as the first looks
/// right would race the second still sitting unread in the socket: a fixed
/// bug would pass here for the same reason it used to fail — by never reading
/// far enough to see it.
async fn until(
    socket: &mut Socket<TcpStream>,
    view: &mut WorldView,
    what: &str,
    mut done: impl FnMut(&WorldView) -> bool,
) {
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = socket.next_event().await.expect("the socket stayed up") {
            let Event::Packet(packet) = event else {
                continue;
            };
            view.apply(&packet);
            if done(view) {
                return;
            }
        }
        panic!("the shard closed the connection before {what}");
    })
    .await;
    assert!(waited.is_ok(), "the shard never {what}");
}

/// A shard with `ACCOUNT` promoted to staff — `.add` is a command rather than
/// speech only for an account with authority — logged in and standing with
/// its backpack already open, so every scenario starts from the same known
/// state: nothing in view but the pack itself, empty.
async fn seeded() -> (Socket<TcpStream>, WorldView, Serial, Serial, Running) {
    let (address, shard) = spawn(|address| {
        let mut config = stock_config(address);
        for account in &mut config.accounts {
            if account.name == ACCOUNT {
                account.access = RawAccessLevel("administrator".to_owned());
            }
        }
        config
    });
    let (mut socket, view) =
        tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan(), version()))
            .await
            .expect("the login conversation finished inside the timeout")
            .expect("the client reached the world");

    let player = view.player.serial;
    let backpack = view
        .player
        .equipment
        .iter()
        .find(|item| item.layer == Layer::BACKPACK)
        .map(|item| item.serial)
        .expect("the stock character carries a backpack");

    // A fresh backpack is empty, and an empty `0x3C` carries no container
    // field to wait on — it names its subject off the first item in the
    // listing, and there is none (`ContainerContents::container`). Sent and
    // not awaited: what matters is that opening it registers this connection
    // as a watcher before anything else goes out on the same socket, and the
    // shard processes one connection's packets in the order they arrive.
    socket
        .send(&Outgoing::Use(backpack).encode(player, version()))
        .await
        .expect("the shard is listening");

    (socket, view, player, backpack, shard)
}

/// `.add <graphic> <amount>` and the serial of the pile that landed at the
/// actor's feet — the one entity carrying that graphic `view.items` did not
/// have a moment ago.
async fn add_ground_pile(
    socket: &mut Socket<TcpStream>,
    view: &mut WorldView,
    graphic: Graphic,
    amount: u16,
) -> Serial {
    let before: HashSet<Serial> = view.items.keys().copied().collect();
    socket
        .send(&talk::say(
            &format!(".add {:#06x} {amount}", graphic.0),
            TalkMode::Regular,
        ))
        .await
        .expect("the shard is listening");
    until(socket, view, "the spawned pile to appear on the ground", |view| {
        view.items.keys().any(|serial| !before.contains(serial))
    })
    .await;
    *view
        .items
        .iter()
        .find(|(serial, item)| !before.contains(serial) && item.graphic == graphic)
        .map(|(serial, _)| serial)
        .expect(".add spawned the graphic asked for")
}

/// Lift a whole pile and drop it into a container at a free gump position —
/// an ordinary relocation, not a merge, because `at` names empty space rather
/// than another item's serial.
async fn stow(
    socket: &mut Socket<TcpStream>,
    view: &mut WorldView,
    player: Serial,
    item: Serial,
    amount: u16,
    container: Serial,
    at: GumpPoint,
) {
    socket
        .send(
            &Outgoing::PickUp {
                item,
                amount: ItemAmount(amount),
            }
            .encode(player, version()),
        )
        .await
        .expect("the shard is listening");
    socket
        .send(&Outgoing::DropInto { item, container, at }.encode(player, version()))
        .await
        .expect("the shard is listening");
    until(socket, view, "the stowed pile to land in its container", |view| {
        view.contents
            .get(&container)
            .is_some_and(|items| items.iter().any(|i| i.serial == item && i.amount.0 == amount))
    })
    .await;
}

/// Lift `amount` off `source` and drop it onto `target`'s own serial — the
/// wire's one gesture for "merge these", dispatched server-side by what
/// `target` turns out to be rather than by a separate action kind. Does not
/// itself wait: a full merge, a partial one that bounces, and one that
/// overflows settle on different packets, and the caller is the one who
/// knows which.
async fn merge_onto(
    socket: &mut Socket<TcpStream>,
    player: Serial,
    source: Serial,
    amount: u16,
    target: Serial,
) {
    socket
        .send(
            &Outgoing::PickUp {
                item:   source,
                amount: ItemAmount(amount),
            }
            .encode(player, version()),
        )
        .await
        .expect("the shard is listening");
    socket
        .send(
            &Outgoing::DropInto {
                item:      source,
                container: target,
                at:        GumpPoint::new(0, 0),
            }
            .encode(player, version()),
        )
        .await
        .expect("the shard is listening");
}

/// **The direct repro of the ghost-pile bug.** Two piles land in the same
/// backpack at different gump positions; the second is dragged onto the
/// first. The shard merges the quantities correctly on its own — that half
/// was never in question, `stack::tests::merge_conserves_quantity_at_every_stack_boundary`
/// already pins it — what this proves is that the *dragging client's own
/// model* ends up agreeing: one pile, the right total, and the consumed
/// serial gone rather than lingering beside it.
#[tokio::test]
async fn merging_onto_a_matching_pile_in_a_backpack_leaves_no_ghost() {
    let (mut socket, mut view, player, backpack, _shard) = seeded().await;

    let first = add_ground_pile(&mut socket, &mut view, ORE, 20).await;
    stow(
        &mut socket,
        &mut view,
        player,
        first,
        20,
        backpack,
        GumpPoint::new(40, 40),
    )
    .await;
    let second = add_ground_pile(&mut socket, &mut view, ORE, 15).await;
    stow(
        &mut socket,
        &mut view,
        player,
        second,
        15,
        backpack,
        GumpPoint::new(90, 40),
    )
    .await;

    merge_onto(&mut socket, player, second, 15, first).await;
    until(
        &mut socket,
        &mut view,
        "the merge to land in the backpack with no ghost of the pile it consumed",
        |view| {
            let items = view
                .contents
                .get(&backpack)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let merged = items
                .iter()
                .any(|item| item.serial == first && item.amount.0 == 35);
            let no_ghost = !items.iter().any(|item| item.serial == second);
            merged && no_ghost
        },
    )
    .await;
}

/// The merge's other arm — `merge_onto`'s ground-target branch
/// (`crates/server/items/src/stack.rs`), left untouched by the scenario
/// above. Both piles stay on the ground this time; the fix is the same
/// helper either way, so this is the proof it actually covers both call
/// sites and not just the one the bug report named.
#[tokio::test]
async fn merging_onto_a_matching_pile_on_the_ground_leaves_no_ghost() {
    let (mut socket, mut view, player, _backpack, _shard) = seeded().await;

    let first = add_ground_pile(&mut socket, &mut view, ORE, 20).await;
    let second = add_ground_pile(&mut socket, &mut view, ORE, 15).await;

    merge_onto(&mut socket, player, second, 15, first).await;
    until(
        &mut socket,
        &mut view,
        "the ground pile to absorb the merge with no ghost left standing",
        |view| {
            let merged = matches!(
                view.items.get(&first).map(|item| item.payload),
                Some(WorldItemPayload::Stack(amount)) if amount.0 == 35
            );
            let no_ghost = !view.items.contains_key(&second);
            merged && no_ghost
        },
    )
    .await;
}

/// **The split half of the same story, at the wire level.** A partial
/// `PickUp` keeps the taken amount separable from the remainder rather than
/// silently reappearing whole — the claim `ContainerPane::answered` now
/// relies on by leaving the amount `Hand::Held` instead of dropping it back.
/// Placed on the ground once lifted, the taken pile turns out to genuinely be
/// six, and the backpack settles to exactly the remainder — proving the split
/// conserves the total across the wire, not merely inside one process's
/// `WorldState`.
#[tokio::test]
async fn a_partial_pickup_holds_exactly_the_amount_asked_and_leaves_the_rest_behind() {
    let (mut socket, mut view, player, backpack, _shard) = seeded().await;
    let whole = add_ground_pile(&mut socket, &mut view, ORE, 20).await;
    stow(
        &mut socket,
        &mut view,
        player,
        whole,
        20,
        backpack,
        GumpPoint::new(40, 40),
    )
    .await;

    socket
        .send(
            &Outgoing::PickUp {
                item:   whole,
                amount: ItemAmount(6),
            }
            .encode(player, version()),
        )
        .await
        .expect("the shard is listening");
    until(
        &mut socket,
        &mut view,
        "the split remainder to land in the backpack",
        |view| {
            view.contents.get(&backpack).is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.serial != whole && item.amount.0 == 14)
            })
        },
    )
    .await;

    let feet = view.player.position;
    socket
        .send(
            &Outgoing::DropOnGround {
                item: whole,
                at:   feet,
            }
            .encode(player, version()),
        )
        .await
        .expect("the shard is listening");
    until(
        &mut socket,
        &mut view,
        "the taken six to land on the ground and the backpack to settle to just the remainder",
        |view| {
            let landed = matches!(
                view.items.get(&whole).map(|item| item.payload),
                Some(WorldItemPayload::Stack(amount)) if amount.0 == 6
            );
            let settled = view
                .contents
                .get(&backpack)
                .is_some_and(|items| items.len() == 1 && items[0].serial != whole && items[0].amount.0 == 14);
            landed && settled
        },
    )
    .await;
}

/// **A short sequence, not one isolated gesture.** Split, merge, split the
/// merged pile again, merge it back once more — the total is checked after
/// every step, and no step is allowed to leave a serial the shard has already
/// disposed of sitting in this client's own model of the backpack. Both
/// fixed bugs were exactly this shape: correct in isolation, wrong once a
/// second operation followed the first before anything re-synced the view.
/// This is the harness's actual answer to "too many problems like this" —
/// the four helpers above compose into a scripted walk, and any future
/// split/merge defect that only shows up two operations deep has a place to
/// be caught rather than a fresh fixture to build.
#[tokio::test]
async fn a_split_and_merge_sequence_conserves_the_total_at_every_step() {
    let (mut socket, mut view, player, backpack, _shard) = seeded().await;
    let total = 40;

    let whole = add_ground_pile(&mut socket, &mut view, ORE, total).await;
    stow(
        &mut socket,
        &mut view,
        player,
        whole,
        total,
        backpack,
        GumpPoint::new(40, 40),
    )
    .await;

    // Split it in two, both halves in the pack.
    socket
        .send(
            &Outgoing::PickUp {
                item:   whole,
                amount: ItemAmount(15),
            }
            .encode(player, version()),
        )
        .await
        .expect("the shard is listening");
    socket
        .send(
            &Outgoing::DropInto {
                item:      whole,
                container: backpack,
                at:        GumpPoint::new(90, 40),
            }
            .encode(player, version()),
        )
        .await
        .expect("the shard is listening");
    until(
        &mut socket,
        &mut view,
        "the split to leave two piles summing to the total",
        |view| sum_of(view, backpack) == u32::from(total) && count_of(view, backpack) == 2,
    )
    .await;
    let (remainder, taken) = two_piles(&view, backpack);

    // Merge them straight back together.
    merge_onto(&mut socket, player, taken, 15, remainder).await;
    until(
        &mut socket,
        &mut view,
        "the re-merge to leave one pile with the whole total and no ghost of the other",
        |view| {
            sum_of(view, backpack) == u32::from(total)
                && count_of(view, backpack) == 1
                && !contains_serial(view, backpack, taken)
        },
    )
    .await;

    // And split it once more, a different way, to be sure the pile that
    // absorbed a merge a moment ago still divides correctly afterwards.
    socket
        .send(
            &Outgoing::PickUp {
                item:   remainder,
                amount: ItemAmount(9),
            }
            .encode(player, version()),
        )
        .await
        .expect("the shard is listening");
    socket
        .send(
            &Outgoing::DropInto {
                item:      remainder,
                container: backpack,
                at:        GumpPoint::new(90, 40),
            }
            .encode(player, version()),
        )
        .await
        .expect("the shard is listening");
    until(
        &mut socket,
        &mut view,
        "the final split to still sum to the original total",
        |view| sum_of(view, backpack) == u32::from(total) && count_of(view, backpack) == 2,
    )
    .await;
}

/// The total of every ore pile a container currently shows — the
/// quantity-conservation half of the oracle every scenario in this file
/// checks against.
fn sum_of(view: &WorldView, container: Serial) -> u32 {
    view.contents
        .get(&container)
        .into_iter()
        .flatten()
        .filter(|item| item.graphic == ORE)
        .map(|item| u32::from(item.amount.0))
        .sum()
}

/// How many separate piles a container currently shows.
fn count_of(view: &WorldView, container: Serial) -> usize {
    view.contents.get(&container).map_or(0, Vec::len)
}

/// Whether a serial the shard is done with still turns up in a container —
/// the ghost-pile half of the oracle.
fn contains_serial(view: &WorldView, container: Serial, serial: Serial) -> bool {
    view.contents
        .get(&container)
        .into_iter()
        .flatten()
        .any(|item| item.serial == serial)
}

/// The two piles a container holds, ordered by amount — the smaller taken
/// half of a split first, so a caller does not have to guess which serial
/// the shard picked for which.
fn two_piles(view: &WorldView, container: Serial) -> (Serial, Serial) {
    let mut items: Vec<_> = view.contents.get(&container).cloned().unwrap_or_default();
    items.sort_by_key(|item| item.amount.0);
    match items.as_slice() {
        [smaller, larger] => (larger.serial, smaller.serial),
        other => panic!("expected exactly two piles, found {other:?}"),
    }
}
