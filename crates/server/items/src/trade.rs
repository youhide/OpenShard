//! The secure trade: two players, two escrow containers, and two checkboxes.
//!
//! Handing goods over by dropping them on the ground and trusting the other
//! party is the oldest scam in the genre; this is the window UO answered it
//! with. Dragging an item onto another player opens it. Either side puts things
//! in and takes them back out with the *ordinary* drag machinery — an escrow is
//! a real [`Container`], which is the whole point of making it one — and nothing
//! moves until both boxes are ticked. Every other ending, including one this
//! side decides on, returns each side's offering to its own pack.
//!
//! Ported from ServUO's `Server/SecureTrade.cs` and `SecureTradeContainer.cs`,
//! cross-checked against Sphere's `CClientUse.cpp`/`CItemContainer.cpp`, which
//! reach the same end by deleting the container and letting its bounce run.
//!
//! # The escrow is worn, and that is load-bearing
//!
//! Each container is [`Equipped`] on [`TRADE_LAYER`] — ServUO's `Layer.SecureTrade`
//! — rather than left in limbo with no location at all. That is what makes
//! [`in_reach`] work with nothing written: it already treats a container *you*
//! wear as always in reach, and one somebody else wears as being at their tile.
//! Those are exactly the right answers for your half of the window and theirs.
//! The price is that a worn thing is drawn and saved by default, which is what
//! the [`TradeWindow`] marker exists to undo.
//!
//! # Found, not announced
//!
//! ServUO cancels a trade from `Mobile.Location`'s setter — a call beside every
//! mover. This engine has more movers than it does (the player walk, the
//! creature step, a teleport, a facet change, a resurrection, a login), and a
//! call beside each is five places to forget. [`validate_trades`] runs once a
//! tick over a list that is almost always empty instead, the shape
//! `tick/regions.rs` and `tick/status.rs` use.

use super::*;
use openshard_state::{Trade, TradeSide};

/// How near, in tiles, the two parties must be — to open a trade and to keep
/// one. ServUO's `InRange(Location, 2)`, which is *tighter* than [`ITEM_REACH`]:
/// a trade is a conversation, not a reach.
pub const TRADE_RANGE: u32 = 2;

/// The escrow container's art — ServUO's `SecureTradeContainer`, item `0x1E5E`.
pub const TRADE_CONTAINER_GRAPHIC: Graphic = Graphic(0x1E5E);

/// The layer an escrow is worn on: ServUO's `Layer.SecureTrade`.
///
/// Past the `0x13` equip path's own ceiling, so no client can put anything here
/// or take anything off it. Nothing is drawn on it either — see [`TradeWindow`].
pub const TRADE_LAYER: Layer = Layer(0x1E);

/// The gump an escrow reports as a container.
///
/// Never opened: the client draws the trade window itself off the `0x6F`, and
/// the two container serials only name its halves. It is set because a
/// [`Container`] carries one, not because anything reads it.
const TRADE_CONTAINER_GUMP: Graphic = Graphic(0x003C);

/// "You cannot trade with someone who is dragging something."
const CLILOC_PARTNER_IS_DRAGGING: ClilocId = ClilocId(1_062_727);
/// "That person is already involved in a trade."
const CLILOC_THEY_ARE_TRADING: ClilocId = ClilocId(1_062_779);
/// "You are already trading with someone else!"
const CLILOC_YOU_ARE_TRADING: ClilocId = ClilocId(1_062_781);

/// Whether `entity` is a player: a body with a client behind it.
///
/// ServUO's `from.Player && Player` gate. A creature, a townsperson and a vendor
/// are all bodies, and none of them trades — dropping something on one still
/// bounces, exactly as it did before this existed.
#[must_use]
pub fn is_player(state: &WorldState, entity: EntityId) -> bool {
    state.registry.has::<Body>(entity) && state.registry.has::<Client>(entity)
}

/// Which of `WorldState::trades` a trade is.
///
/// Nothing but that one `Vec` — an entity count, a sector bucket or any other
/// `usize` in scope at a call site here would otherwise typecheck in its place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TradeIndex(pub usize);

/// The trade `player` is in, by index.
fn trade_index(state: &WorldState, player: EntityId) -> Option<TradeIndex> {
    state
        .trades
        .iter()
        .position(|trade| trade.involves(player))
        .map(TradeIndex)
}

/// The trade drawn on `container`, by index.
fn trade_of_container(state: &WorldState, container: Serial) -> Option<TradeIndex> {
    state
        .trades
        .iter()
        .position(|trade| trade.from.container_serial == container || trade.to.container_serial == container)
        .map(TradeIndex)
}

/// The player who owns one half of a live trade window.
///
/// An escrow is a normal container so the regular drag machinery can draw and
/// update it, but it is not a normal shared container: each party may only put
/// goods in and take goods out of *their* half.  Without this gate, looking at
/// the other offer would also grant permission to steal it.
pub(crate) fn trade_container_owner(state: &WorldState, container: Serial) -> Option<EntityId> {
    state.trades.iter().find_map(|trade| {
        if trade.from.container_serial == container {
            Some(trade.from.player)
        } else if trade.to.container_serial == container {
            Some(trade.to.player)
        } else {
            None
        }
    })
}

/// A held item was dropped on another player: open a trade, or add to the one
/// already open with them.
///
/// The refusals are ServUO's `OpenTrade` in its order. A refused offer bounces
/// the item home rather than dropping it, so nothing is ever at risk here.
pub fn offer(state: &mut WorldState, connection: ConnectionId, held: HeldItem, target: EntityId) {
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    // Both parties alive: ServUO refuses a trade with or by the dead outright,
    // and a ghost cannot hold anything anyway.
    if state.registry.has::<Ghost>(player) || state.registry.has::<Ghost>(target) {
        bounce(state, connection, held, DragCancelReason::CannotLift);
        return;
    }
    let Some(&Position(mine)) = state.registry.get::<Position>(player) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let Some(&Position(theirs)) = state.registry.get::<Position>(target) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if state.facet_of(player) != state.facet_of(target) || !in_range(mine, theirs, TRADE_RANGE) {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }

    // A trade already open with *this* partner takes the item rather than
    // opening a second window; both references do this.
    if let Some(index) = trade_index(state, player) {
        if state.trades[index.0].involves(target) {
            let container = state.trades[index.0]
                .sides_for(player)
                .map(|(mine, _)| mine.container_serial);
            match container {
                Some(container) => {
                    // Nobody dragged this onto a gump: the item goes into the
                    // escrow because the trade is already open, so there is no
                    // cursor position to honour and the origin is the honest
                    // answer.
                    drop_into_container(state, connection, held, GumpPoint::new(0, 0), container);
                }
                None => bounce(state, connection, held, DragCancelReason::Other),
            }
            return;
        }
        state.localized_message(player, CLILOC_YOU_ARE_TRADING, "");
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    if trade_index(state, target).is_some() {
        state.localized_message(player, CLILOC_THEY_ARE_TRADING, "");
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    // ServUO's `CheckTrade`: a partner with something on its own cursor cannot be
    // handed anything, because the item it is holding has no settled home yet.
    let Some(&Client {
        connection: partner_connection,
        ..
    }) = state.registry.get::<Client>(target)
    else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if state.held_of(partner_connection).is_some() {
        state.localized_message(player, CLILOC_PARTNER_IS_DRAGGING, "");
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }

    let Some(mine) = new_escrow(state, player) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let Some(theirs) = new_escrow(state, target) else {
        // Half a window is worse than none: undo the one that was made.
        despawn_escrow(state, mine.0);
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    state.trades.push(Trade {
        from: TradeSide {
            player,
            connection,
            container: mine.0,
            container_serial: mine.1,
            accepted: false,
        },
        to: TradeSide {
            player: target,
            connection: partner_connection,
            container: theirs.0,
            container_serial: theirs.1,
            accepted: false,
        },
        witnessed: Vec::new(),
    });
    // Both parties watch both halves, or neither sees what the other offers —
    // which is the whole point of the window.
    for container in [mine.1, theirs.1] {
        let watchers = state.open_containers.entry(container).or_default();
        watchers.insert(connection);
        watchers.insert(partner_connection);
    }

    let index = TradeIndex(state.trades.len() - 1);
    draw_window(state, index);
    // And in goes what was dropped, through the ordinary door — at the origin
    // of the escrow's gump, for the reason above: the drop was onto a person,
    // and the packet's position never meant a point in this window.
    drop_into_container(state, connection, held, GumpPoint::new(0, 0), mine.1);
    debug!("a secure trade opened");
}

/// Make one party's escrow container.
fn new_escrow(state: &mut WorldState, player: EntityId) -> Option<(EntityId, Serial)> {
    let mobile = state.registry.serial_of(player)?;
    let entity = equip_new_container(
        state,
        mobile,
        TRADE_CONTAINER_GRAPHIC,
        TRADE_CONTAINER_GUMP,
        Hue(0),
        TRADE_LAYER,
    )?;
    state.registry.insert(entity, TradeWindow);
    let serial = state.registry.serial_of(entity)?;
    Some((entity, serial))
}

/// Draw the window on both clients, in ServUO's send order.
///
/// The repeated `Update` sends are in the reference; a client that is told its
/// checkboxes only once draws them unset and then never redraws until something
/// else moves, so they are kept.
fn draw_window(state: &mut WorldState, index: TradeIndex) {
    let Some(trade) = state.trades.get(index.0) else {
        return;
    };
    let sides = [
        (trade.from.clone(), trade.to.clone()),
        (trade.to.clone(), trade.from.clone()),
    ];
    for (side, other) in sides {
        let Some(partner_serial) = state.registry.serial_of(other.player) else {
            continue;
        };
        let name = state
            .registry
            .get::<Name>(other.player)
            .map(|name| name.0.clone())
            .unwrap_or_default();
        let updates = encode_trade_update(side.container_serial, false, false);
        state.send(side.connection, updates.clone());
        state.send(
            side.connection,
            encode_trade_open(
                partner_serial,
                side.container_serial,
                other.container_serial,
                &name,
            ),
        );
        state.send(side.connection, updates);
    }
}

/// Send both clients the current pair of checkboxes.
///
/// Each is addressed on *its own* container with its own flag first, which is
/// what makes the same two booleans read correctly on two screens.
fn send_checks(state: &mut WorldState, index: TradeIndex) {
    let Some(trade) = state.trades.get(index.0) else {
        return;
    };
    let (from, to) = (trade.from.clone(), trade.to.clone());
    state.send(
        from.connection,
        encode_trade_update(from.container_serial, from.accepted, to.accepted),
    );
    state.send(
        to.connection,
        encode_trade_update(to.container_serial, to.accepted, from.accepted),
    );
}

/// The client ticked or unticked its checkbox. When both are ticked the goods
/// change hands.
pub fn set_accepted(state: &mut WorldState, connection: ConnectionId, container: RawSerial, accepted: bool) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(container) = container.validate() else {
        return;
    };
    // The reply must name a window this side actually drew, and one this player
    // is a party to — the `Connection::quest_gump` rule, for the same reason.
    let Some(index) = trade_of_container(state, container) else {
        return;
    };
    let trade = &mut state.trades[index.0];
    if trade.from.player == player && trade.from.container_serial == container {
        trade.from.accepted = accepted;
    } else if trade.to.player == player && trade.to.container_serial == container {
        trade.to.accepted = accepted;
    } else {
        return;
    }
    // Remember what was on the table at the moment somebody agreed to it, so a
    // change afterwards can be noticed. See `validate_trades`.
    let both = trade.from.accepted || trade.to.accepted;
    if both {
        let witnessed = escrowed(state, index);
        state.trades[index.0].witnessed = witnessed;
    }
    send_checks(state, index);

    let settled = state
        .trades
        .get(index.0)
        .is_some_and(|trade| trade.from.accepted && trade.to.accepted);
    if settled {
        complete(state, index);
    }
}

/// Every item in either escrow, sorted, as the fingerprint a change is noticed
/// against.
fn escrowed(state: &WorldState, index: TradeIndex) -> Vec<Serial> {
    let Some(trade) = state.trades.get(index.0) else {
        return Vec::new();
    };
    let mut items: Vec<Serial> = contained_items(state, trade.from.container_serial)
        .chain(contained_items(state, trade.to.container_serial))
        .filter_map(|(entity, _)| state.registry.serial_of(entity))
        .collect();
    items.sort_unstable();
    items
}

/// Both boxes are ticked: swap the two offerings and close.
fn complete(state: &mut WorldState, index: TradeIndex) {
    let Some(trade) = state.trades.get(index.0).cloned() else {
        return;
    };
    // Collected first, never iterated live: moving an item mutates the very
    // column the query walks.
    let from_items = contents_of_entity(state, trade.from.container_serial);
    let to_items = contents_of_entity(state, trade.to.container_serial);
    hand_over(state, &from_items, trade.to.player);
    hand_over(state, &to_items, trade.from.player);
    close(state, index);
    debug!("a secure trade settled");
}

/// Move `items` into `receiver`'s backpack, wherever they are now.
///
/// Nothing in `containers` relocates a *live* entity — every "put in" there
/// spawns a fresh one from a graphic — so this is the drag path's own form: drop
/// the old `Contained` and write a new one. A receiver with no pack keeps
/// nothing floating; the items go to their feet instead.
fn hand_over(state: &mut WorldState, items: &[EntityId], receiver: EntityId) {
    let Some(mobile) = state.registry.serial_of(receiver) else {
        return;
    };
    let pack = backpack_of(state, mobile);
    for &item in items {
        if let Some(pack) = pack {
            let grid = item_count(state, pack);
            let contained = Contained {
                container: pack,
                position: GumpPoint::new(0, 0),
                grid: GridSlot(grid),
            };
            relocate_item(state, item, ItemLocation::contained(contained))
                .expect("settled trade goods have one receiver-pack parent");
            tell_watchers_updated(state, pack, item);
        } else {
            // No pack to put it in — a corner ServUO does not have, since
            // every player has one. Better on the floor than nowhere.
            if let Some(&Position(at)) = state.registry.get::<Position>(receiver) {
                place_on_ground(state, item, at, state.facet_of(receiver));
            }
        }
    }
}

/// The entities inside a container, as a list that survives mutating them.
fn contents_of_entity(state: &WorldState, container: Serial) -> Vec<EntityId> {
    contained_items(state, container)
        .map(|(entity, _)| entity)
        .collect()
}

/// End a trade and give each side back what it had offered.
///
/// Every exit runs through here — the client's cancel, a step out of range, a
/// death, a logout, a shutdown — so there is one place that can lose an item and
/// it is tested.
pub fn cancel(state: &mut WorldState, index: TradeIndex) {
    let Some(trade) = state.trades.get(index.0).cloned() else {
        return;
    };
    let from_items = contents_of_entity(state, trade.from.container_serial);
    let to_items = contents_of_entity(state, trade.to.container_serial);
    hand_over(state, &from_items, trade.from.player);
    hand_over(state, &to_items, trade.to.player);
    close(state, index);
    debug!("a secure trade was cancelled");
}

/// Shut both windows and take the escrow containers out of the world.
///
/// Called only once the contents have been dealt with: it despawns, and an item
/// still inside would go with it.
fn close(state: &mut WorldState, index: TradeIndex) {
    if index.0 >= state.trades.len() {
        return;
    }
    let trade = state.trades.remove(index.0);
    // Each client is told about *its own* half — ServUO's `Close`, which sends
    // one packet per side. The client shuts the whole window on it.
    for side in [&trade.from, &trade.to] {
        state.send(side.connection, encode_trade_close(side.container_serial));
        state.open_containers.remove(&side.container_serial);
        despawn_escrow(state, side.container);
    }
}

/// Take one escrow container out of the world.
fn despawn_escrow(state: &mut WorldState, container: EntityId) {
    despawn_item(state, container);
}

/// The client shut its window: end the trade it names.
///
/// The container has to name a window this side drew *and* one this player is a
/// party to — the `Connection::quest_gump` rule, so a `0x6F` naming somebody else's
/// trade cannot end it.
pub fn cancel_by_container(state: &mut WorldState, connection: ConnectionId, container: RawSerial) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(container) = container.validate() else {
        return;
    };
    let Some(index) = trade_of_container(state, container) else {
        return;
    };
    if trade_container_owner(state, container) != Some(player) {
        return;
    }
    cancel(state, index);
}

/// Cancel whatever trade `player` is in. A logout, a death, a facet change.
pub fn cancel_for(state: &mut WorldState, player: EntityId) {
    if let Some(index) = trade_index(state, player) {
        cancel(state, index);
    }
}

/// Cancel every trade in progress — the shutdown path.
///
/// An escrow is deliberately not saved, so a snapshot taken with a trade open
/// would take it without the items in it. Cancelling first puts them back in the
/// two packs, which *are* saved, so a clean stop cannot cost anybody anything.
pub fn cancel_all_trades(state: &mut WorldState) {
    while !state.trades.is_empty() {
        cancel(state, TradeIndex(0));
    }
}

/// Once a tick: end any trade whose two parties are no longer both able to have
/// one, and clear the checkboxes if the goods changed under them.
///
/// ServUO's `NetState.ValidateAllTrades` plus `SecureTradeContainer.ClearChecks`,
/// found rather than announced. See the module note.
pub fn validate_trades(state: &mut WorldState) {
    let mut index = TradeIndex(0);
    while index.0 < state.trades.len() {
        if !still_valid(state, index) {
            cancel(state, index);
            continue;
        }
        clear_checks_if_changed(state, index);
        index = TradeIndex(index.0 + 1);
    }
}

/// Whether both parties are still online, alive, on one facet and within reach.
fn still_valid(state: &WorldState, index: TradeIndex) -> bool {
    let Some(trade) = state.trades.get(index.0) else {
        return false;
    };
    for side in [&trade.from, &trade.to] {
        if state.players.get(&side.connection) != Some(&side.player) {
            return false;
        }
        if state.registry.has::<Ghost>(side.player) {
            return false;
        }
    }
    let (Some(&Position(from)), Some(&Position(to))) = (
        state.registry.get::<Position>(trade.from.player),
        state.registry.get::<Position>(trade.to.player),
    ) else {
        return false;
    };
    state.facet_of(trade.from.player) == state.facet_of(trade.to.player) && in_range(from, to, TRADE_RANGE)
}

/// If either box is ticked and the goods have moved since, untick both.
///
/// The fingerprint is only taken while somebody has agreed to something: an
/// unticked pair has nothing to clear, and [`escrowed`] walks the whole
/// `Contained` column, which is not a thing to do every tick for every trade.
fn clear_checks_if_changed(state: &mut WorldState, index: TradeIndex) {
    let Some(trade) = state.trades.get(index.0) else {
        return;
    };
    if !trade.from.accepted && !trade.to.accepted {
        return;
    }
    let now = escrowed(state, index);
    if now == state.trades[index.0].witnessed {
        return;
    }
    let trade = &mut state.trades[index.0];
    trade.witnessed = now;
    trade.from.accepted = false;
    trade.to.accepted = false;
    send_checks(state, index);
}
