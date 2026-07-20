//! The shopkeeper: goods in a worn crate, a price on every item, and the
//! classic buy/sell conversation over `0x74`/`0x3B` and `0x9E`/`0x9F`.
//!
//! A vendor's stock is an ordinary container on the vendor's stock layer, so
//! the buy gump is the container machinery the game already has; the vendor
//! packets only add prices and labels alongside. Buying pays gold out of the
//! player's backpack and hands goods into it; selling is the mirror, at half
//! price — the classic margin.

use openshard_entities::{EntityId, Serial, SerialKind};
use openshard_gateway::ConnectionId;
use openshard_items as items;
use openshard_protocol::{
    encode_buy_list, encode_container_contents, encode_open_container, encode_sell_list, BuyLine,
    Purchase, Sale, SellLine,
};
use openshard_state::components::{
    Amount, Client, Contained, Equipped, Graphic, Name, Position, Price, Vendor,
};
use openshard_state::sectors::in_range;
use openshard_state::{TooltipMode, WorldState};
use tracing::debug;

use crate::GOLD_GRAPHIC;

/// The layer a vendor's stock container rides on — ServUO's restockable buy
/// layer, `0x1A` (ClassicUO's `ShopBuyRestock`).
pub const STOCK_LAYER: u8 = 0x1A;

/// The second shop layer, `0x1B` (ClassicUO's `ShopBuy`). ClassicUO's buy window
/// scans layers `0x1A` **and** `0x1B` and dereferences the container on each with
/// no null check, so a vendor must wear one on both or the client crashes when
/// the shop opens. This one holds nothing; it exists only to satisfy the scan.
pub const RESALE_LAYER: u8 = 0x1B;

/// The crate the stock lives in, and its gump.
const STOCK_GRAPHIC: u16 = 0x0E3F;
const STOCK_GUMP: u16 = 0x003E;

/// The vendor buy gump the client opens over the stock container.
const SHOP_GUMP: u16 = 0x0030;

/// How near a customer must stand to trade.
const TRADE_RANGE: u32 = 8;

/// One line of stock, as a script supplies it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StockLine {
    /// The goods' graphic.
    pub graphic: u16,
    /// Their hue.
    pub hue: u16,
    /// How many the vendor holds.
    pub amount: u16,
    /// What one unit costs.
    pub price: u32,
    /// The label the client shows.
    pub name: String,
}

/// The stock container a vendor wears, if it is a vendor at all.
fn stock_of(state: &WorldState, vendor: EntityId) -> Option<(EntityId, Serial)> {
    if !state.registry.has::<Vendor>(vendor) {
        return None;
    }
    let vendor_serial = state.registry.serial_of(vendor)?;
    state
        .registry
        .query::<Equipped>()
        .find(|(_, worn)| worn.mobile == vendor_serial && worn.layer == STOCK_LAYER)
        .map(|(entity, _)| entity)
        .and_then(|entity| state.registry.serial_of(entity).map(|s| (entity, s)))
}

/// Whether a player stands near enough to a vendor to trade with it.
fn in_trade_range(state: &WorldState, player: EntityId, vendor: EntityId) -> bool {
    let (Some(&Position(at)), Some(&Position(vendor_at))) = (
        state.registry.get::<Position>(player),
        state.registry.get::<Position>(vendor),
    ) else {
        return false;
    };
    state.facet_of(player) == state.facet_of(vendor) && in_range(at, vendor_at, TRADE_RANGE)
}

/// Fill a vendor's stock from a script's lines. See `Command::StockVendor`.
/// Replaces nothing: lines add to whatever the crate already holds.
pub fn stock(state: &mut WorldState, vendor_serial: u32, lines: Vec<StockLine>) {
    let Some(vendor) = Serial::new(vendor_serial).and_then(|s| state.registry.entity_of(s)) else {
        return;
    };
    let Some((_, stock_serial)) = stock_of(state, vendor) else {
        return;
    };
    for line in lines {
        let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
            return;
        };
        state.registry.insert(
            entity,
            Graphic {
                id: line.graphic,
                hue: line.hue,
            },
        );
        state.registry.insert(
            entity,
            Contained {
                container: stock_serial,
                x: 50,
                y: 50,
                grid: 0,
            },
        );
        state.registry.insert(entity, Amount(line.amount));
        state.registry.insert(entity, Price(line.price));
        state.registry.insert(entity, Name(line.name));
    }
    debug!(%stock_serial, "vendor stocked");
}

/// Open the shop on a double-click, if the clicked mobile is a vendor in
/// range. Returns whether it was — the caller falls through to the ordinary
/// double-click when it was not.
pub fn open_shop(state: &mut WorldState, connection: ConnectionId, serial: u32) -> bool {
    let Some(&player) = state.players.get(&connection) else {
        return false;
    };
    let Some(vendor) = Serial::new(serial).and_then(|s| state.registry.entity_of(s)) else {
        return false;
    };
    let Some((_, stock_serial)) = stock_of(state, vendor) else {
        debug!(
            serial,
            is_vendor = state.registry.has::<Vendor>(vendor),
            "open_shop: not a stocked vendor"
        );
        return false;
    };
    if !in_trade_range(state, player, vendor) {
        debug!(serial, "open_shop: out of trade range");
        return false;
    }
    let Some(&Client { version, .. }) = state.registry.get::<Client>(player) else {
        return false;
    };

    // The contents and prices key on the stock crate — the client pairs the 0x74
    // lines with the 0x3C items by order, so the same walk builds both.
    let contents = items::contents_of(state, stock_serial);
    let lines: Vec<BuyLine> = contents
        .iter()
        .map(|item| {
            let entity = Serial::new(item.serial).and_then(|s| state.registry.entity_of(s));
            let price = entity
                .and_then(|e| state.registry.get::<Price>(e))
                .map_or(1, |p| p.0);
            let name = entity
                .and_then(|e| state.registry.get::<Name>(e))
                .map_or_else(|| format!("item {:#06x}", item.graphic), |n| n.0.clone());
            BuyLine { price, name }
        })
        .collect();
    // ClassicUO's buy window scans shop layers 0x1A and 0x1B and dereferences the
    // container on each with no null check. A vendor restored from a save made
    // before the second crate existed wears only 0x1A, so add 0x1B now or the
    // client crashes when the shop opens.
    if worn_container(state, vendor, RESALE_LAYER).is_none() {
        if let Some(vendor_serial) = state.registry.serial_of(vendor) {
            items::equip_new_container(
                state,
                vendor_serial,
                STOCK_GRAPHIC,
                STOCK_GUMP,
                0,
                RESALE_LAYER,
            );
        }
    }

    // ServUO's `SendPacksTo`: tell the client the vendor wears both shop crates (a
    // `0x2E` equip each) *before* opening. The buy window (`0x24` below) is keyed
    // on the vendor and makes ClassicUO look up the vendor's shop-layer packs —
    // which null-crashes it if the client was never told they exist.
    for layer in [STOCK_LAYER, RESALE_LAYER] {
        let pack = worn_container(state, vendor, layer)
            .and_then(|s| state.registry.entity_of(s))
            .and_then(|entity| items::equip_packet(state, entity));
        if let Some(pack) = pack {
            state.send(connection, pack);
        }
    }

    // Order and serials from ServUO's `BaseVendor.SendBuyPacket`: contents, then
    // prices, then the display packet **last** — and the display (`0x24`) opens on
    // the **vendor's** serial, not the crate's. This is the crux: the client shows
    // a *buy* interface only when the `0x24` names a mobile; an item serial (the
    // crate) just opens a plain container gump, which is why the window never
    // appeared. The crate is worn on the vendor's shop layer, so the client links
    // the crate-keyed contents to the vendor-keyed window itself.
    state.send(
        connection,
        encode_container_contents(stock_serial.raw(), &contents, version),
    );
    state.send(connection, encode_buy_list(stock_serial.raw(), &lines));
    state.send(
        connection,
        encode_open_container(serial, SHOP_GUMP, version),
    );
    // Send each item's tooltip up front, the way ServUO ships the OPLs with the
    // buy packets: a client in OPL mode shows the shop name from the tooltip, so
    // without this the labels read as placeholders until the mouse hovers each row
    // and the client requests the list itself.
    if state.gameplay.tooltip_mode != TooltipMode::Off {
        for item in &contents {
            if let Some(entity) = Serial::new(item.serial).and_then(|s| state.registry.entity_of(s))
            {
                state.send_property_list(connection, entity);
            }
        }
    }
    debug!(serial, items = lines.len(), "open_shop: sent buy gump");
    true
}

/// Settle a purchase: check the gold, take it, hand the goods over. See
/// `Command::Buy`.
pub fn buy(
    state: &mut WorldState,
    connection: ConnectionId,
    vendor_serial: u32,
    list: &[Purchase],
) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(vendor) = Serial::new(vendor_serial).and_then(|s| state.registry.entity_of(s)) else {
        return;
    };
    let Some((_, stock_serial)) = stock_of(state, vendor) else {
        return;
    };
    if !in_trade_range(state, player, vendor) || list.is_empty() {
        return;
    }
    let Some(backpack) = worn_container(state, player, BACKPACK_LAYER) else {
        return;
    };

    // Price the whole basket first: a purchase is all-or-nothing, so a client
    // that asked for more than it can pay is refused before anything moves.
    let mut total: u32 = 0;
    let mut basket: Vec<(EntityId, u16, u16, u16, u32)> = Vec::new();
    for purchase in list {
        let Some(item) = Serial::new(purchase.serial).and_then(|s| state.registry.entity_of(s))
        else {
            continue;
        };
        let held_in = state.registry.get::<Contained>(item).map(|c| c.container);
        if held_in != Some(stock_serial) {
            continue;
        }
        let have = state.registry.get::<Amount>(item).map_or(0, |a| a.0);
        let take = have.min(purchase.amount);
        if take == 0 {
            continue;
        }
        let price = state.registry.get::<Price>(item).map_or(1, |p| p.0);
        let Some(&Graphic { id, hue }) = state.registry.get::<Graphic>(item) else {
            continue;
        };
        total = total.saturating_add(price.saturating_mul(u32::from(take)));
        basket.push((item, take, id, hue, price));
    }
    if basket.is_empty() {
        return;
    }
    let gold = items::count_in_container(state, backpack, GOLD_GRAPHIC);
    if u32::from(u16::MAX) < total || gold < total {
        vendor_says(state, vendor, "Thou canst not afford that.");
        return;
    }
    items::take_from_container(state, backpack, GOLD_GRAPHIC, total as u16);
    for (item, take, graphic, hue, _) in basket {
        items::remove_from_stack(state, stock_serial, item, take);
        items::give(state, backpack, graphic, hue, take);
    }
    vendor_says(
        state,
        vendor,
        &format!("The total of thy purchase is {total} gold."),
    );
}

/// Offer to buy from the player: the sell list, sent when a customer says
/// "sell" near a vendor. The vendor takes only what it also stocks, at half
/// its own price.
pub fn offer_sell_list(state: &mut WorldState, connection: ConnectionId, actor: EntityId) -> bool {
    let Some(vendor) = nearest_vendor(state, actor) else {
        return false;
    };
    let Some(vendor_serial) = state.registry.serial_of(vendor) else {
        return false;
    };
    let Some((_, stock_serial)) = stock_of(state, vendor) else {
        return false;
    };
    let Some(backpack) = worn_container(state, actor, BACKPACK_LAYER) else {
        return false;
    };

    // What the vendor stocks, and at what price — the catalogue a sale is
    // judged against.
    let catalogue = stock_prices(state, stock_serial);
    let lines: Vec<SellLine> = state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == backpack)
        .filter_map(|(entity, _)| {
            let &Graphic { id, hue } = state.registry.get::<Graphic>(entity)?;
            let price = sell_price(*catalogue.iter().find(|(g, _)| *g == id).map(|(_, p)| p)?);
            let serial = state.registry.serial_of(entity)?;
            let amount = state.registry.get::<Amount>(entity).map_or(1, |a| a.0);
            let name = state
                .registry
                .get::<Name>(entity)
                .map_or_else(|| format!("item {id:#06x}"), |n| n.0.clone());
            Some(SellLine {
                serial: serial.raw(),
                graphic: id,
                hue,
                amount,
                price,
                name,
            })
        })
        .collect();
    if lines.is_empty() {
        vendor_says(state, vendor, "Thou hast nothing I wouldst buy.");
        return true;
    }
    state.send(connection, encode_sell_list(vendor_serial.raw(), &lines));
    true
}

/// Settle a sale: goods out of the pack, gold in. See `Command::Sell`.
pub fn sell(state: &mut WorldState, connection: ConnectionId, vendor_serial: u32, list: &[Sale]) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(vendor) = Serial::new(vendor_serial).and_then(|s| state.registry.entity_of(s)) else {
        return;
    };
    let Some((_, stock_serial)) = stock_of(state, vendor) else {
        return;
    };
    if !in_trade_range(state, player, vendor) || list.is_empty() {
        return;
    }
    let Some(backpack) = worn_container(state, player, BACKPACK_LAYER) else {
        return;
    };
    let catalogue = stock_prices(state, stock_serial);

    let mut earned: u32 = 0;
    for sale in list {
        let Some(item) = Serial::new(sale.serial).and_then(|s| state.registry.entity_of(s)) else {
            continue;
        };
        if state.registry.get::<Contained>(item).map(|c| c.container) != Some(backpack) {
            continue;
        }
        let Some(&Graphic { id, .. }) = state.registry.get::<Graphic>(item) else {
            continue;
        };
        let Some(&(_, price)) = catalogue.iter().find(|(g, _)| *g == id) else {
            continue;
        };
        let taken = items::remove_from_stack(state, backpack, item, sale.amount);
        earned = earned.saturating_add(u32::from(sell_price(price)) * u32::from(taken));
    }
    if earned == 0 {
        return;
    }
    let paid = earned.min(u32::from(u16::MAX)) as u16;
    items::give(state, backpack, GOLD_GRAPHIC, 0, paid);
    vendor_says(
        state,
        vendor,
        &format!("The total of thy sale is {paid} gold."),
    );
}

/// Half the buy price, never less than one coin.
fn sell_price(buy: u32) -> u16 {
    ((buy / 2).max(1)).min(u32::from(u16::MAX)) as u16
}

/// Every (graphic, unit price) the vendor's crate holds.
fn stock_prices(state: &WorldState, stock_serial: Serial) -> Vec<(u16, u32)> {
    state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == stock_serial)
        .filter_map(|(entity, _)| {
            let graphic = state.registry.get::<Graphic>(entity)?.id;
            let price = state.registry.get::<Price>(entity).map_or(1, |p| p.0);
            Some((graphic, price))
        })
        .collect()
}

/// The nearest vendor within trade range of `actor`, if any.
fn nearest_vendor(state: &WorldState, actor: EntityId) -> Option<EntityId> {
    let &Position(at) = state.registry.get::<Position>(actor)?;
    let facet = state.facet_of(actor);
    state
        .registry
        .query::<Vendor>()
        .filter(|(vendor, _)| state.facet_of(*vendor) == facet)
        .filter_map(|(vendor, _)| {
            let &Position(pos) = state.registry.get::<Position>(vendor)?;
            in_range(at, pos, TRADE_RANGE)
                .then(|| (openshard_state::sectors::distance(at, pos), vendor))
        })
        .min_by_key(|(d, _)| *d)
        .map(|(_, vendor)| vendor)
}

/// "buy" near a vendor opens its shop — the same buy gump a double-click does,
/// reached by keyword the way "sell" reaches the offer list. Returns whether a
/// vendor was in reach and answered.
pub fn buy_keyword(state: &mut WorldState, connection: ConnectionId, actor: EntityId) -> bool {
    let Some(vendor) = nearest_vendor(state, actor) else {
        return false;
    };
    let Some(vendor_serial) = state.registry.serial_of(vendor) else {
        return false;
    };
    open_shop(state, connection, vendor_serial.raw())
}

/// The vendor's own voice: a conversational line drawn over its head for
/// everyone in earshot, the way any NPC speaks — not a private `0x1C` system
/// line to a single screen. The customer's answer should look like the
/// shopkeeper talking, not the shard.
fn vendor_says(state: &mut WorldState, vendor: EntityId, text: &str) {
    openshard_chat::speak(state, vendor, 0, crate::GREET_HUE, crate::GREET_FONT, text);
}

/// The serial of the container `mobile` wears at `layer`, if any.
fn worn_container(state: &WorldState, mobile: EntityId, layer: u8) -> Option<Serial> {
    let serial = state.registry.serial_of(mobile)?;
    state
        .registry
        .query::<Equipped>()
        .find(|(_, worn)| worn.mobile == serial && worn.layer == layer)
        .and_then(|(entity, _)| state.registry.serial_of(entity))
}

/// The layer a backpack rides on.
const BACKPACK_LAYER: u8 = 0x15;

/// Dress a fresh townsperson as a vendor: the mark, and the stock crate.
pub(crate) fn make_vendor(state: &mut WorldState, entity: EntityId, serial: Serial) {
    state.registry.insert(entity, Vendor);
    items::equip_new_container(state, serial, STOCK_GRAPHIC, STOCK_GUMP, 0, STOCK_LAYER);
    // The empty second crate ClassicUO's buy scan insists on — see `RESALE_LAYER`.
    items::equip_new_container(state, serial, STOCK_GRAPHIC, STOCK_GUMP, 0, RESALE_LAYER);
}
