//! The shopkeeper: goods in a worn crate, a price on every item, and the
//! classic buy/sell conversation over `0x74`/`0x3B` and `0x9E`/`0x9F`.
//!
//! A vendor's stock is an ordinary container on the vendor's stock layer, so
//! the buy gump is the container machinery the game already has; the vendor
//! packets only add prices and labels alongside. Buying pays gold out of the
//! player's backpack and hands goods into it; selling is the mirror, at half
//! price — the classic margin.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_items as items;
use openshard_map::overlay::Doors;
use openshard_protocol::containers::{ContainedItem, ContainerContents, GridSlot, encode_open_container};
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::item_kind::{ItemKindId, MaterialId};
use openshard_protocol::serial::{RawSerial, Serial, SerialKind};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::vendor::{BuyLine, BuyList, Purchase, Sale, SellLine, SellList};
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::{Graphic, Hue, Layer};
use openshard_state::components::{
    Amount, Contained, Drawn, ItemKind, Material, Name, Position, Price, Restock, StockRecord, Vendor,
};
use openshard_state::sectors::in_range;
use openshard_state::{
    ItemLocation, TooltipMode, WorldState, establish_item_location, kind_from_drawn, presentation_of,
};
use tracing::debug;

use crate::GOLD_GRAPHIC;

/// The layer a vendor's stock container rides on — ServUO's restockable buy
/// layer, `0x1A` (ClassicUO's `ShopBuyRestock`).
pub const STOCK_LAYER: Layer = Layer(0x1A);

/// The second shop layer, `0x1B` (ClassicUO's `ShopBuy`). ClassicUO's buy window
/// scans layers `0x1A` **and** `0x1B` and dereferences the container on each with
/// no null check, so a vendor must wear one on both or the client crashes when
/// the shop opens. This one holds nothing; it exists only to satisfy the scan.
pub const RESALE_LAYER: Layer = Layer(0x1B);

/// The crate the stock lives in, and its gump.
const STOCK_GRAPHIC: Graphic = Graphic(0x0E3F);
const STOCK_GUMP: Graphic = Graphic(0x003E);

/// The vendor buy gump the client opens over the stock container.
const SHOP_GUMP: Graphic = Graphic(0x0030);

/// How near a customer must stand to trade — a few tiles, so a shopper reaches
/// the counter but cannot buy from across the street. Trade also needs line of
/// sight (see [`in_trade_range`]), which is what stops buying through a wall.
const TRADE_RANGE: u32 = 4;

/// One line of stock, as a script supplies it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StockLine {
    /// The goods' graphic.
    pub graphic: Graphic,
    /// Their hue.
    pub hue: Hue,
    /// Explicit semantic type for a new stock script. Omit only for a legacy
    /// line; audited `(graphic, hue)` then upgrades it on stock.
    pub item_kind: Option<ItemKindId>,
    /// Material for `item_kind`, where that kind declares a material family.
    pub material: Option<MaterialId>,
    /// How many the vendor holds.
    pub amount: Amount,
    /// What one unit costs.
    pub price: Price,
    /// The label the client shows.
    pub name: String,
}

/// The stock container a vendor wears, if it is a vendor at all.
fn stock_of(state: &WorldState, vendor: EntityId) -> Option<(EntityId, Serial)> {
    if !state.registry.has::<Vendor>(vendor) {
        return None;
    }
    let vendor_serial = state.registry.serial_of(vendor)?;
    openshard_state::equipped_items(state, vendor_serial)
        .find(|(_, worn)| worn.layer == STOCK_LAYER)
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
    let facet = state.facet_of(player);
    if facet != state.facet_of(vendor) || !in_range(at, vendor_at, TRADE_RANGE) {
        return false;
    }
    // A wall between the shopper and the counter is a wall: the same line-of-sight
    // ServUO and Sphere gate a vendor on, so there is no buying through it. The
    // ray is the one aggro uses (`Terrain::sight_clear`).
    openshard_movement::sight_clear(&state.footing(facet, Doors::AsTheyStand), at, vendor_at)
}

/// How long a bought-out shelf takes to refill, in ticks. ServUO's
/// `BaseVendor.DelayRestock` is an hour of real time, so this is that hour in
/// whatever the tick rate happens to be.
pub const RESTOCK_TICKS: u64 = 60 * 60 * openshard_state::TICKS_PER_SECOND;

/// Fill a vendor's stock from a script's lines. See `Command::StockVendor`.
/// Replaces nothing: lines add to whatever the crate already holds.
///
/// It also records the shelf as it stands *now* on the vendor, so it can be topped
/// back up later — see [`Restock`]. The record is cumulative, like the stocking.
pub fn stock(state: &mut WorldState, vendor_serial: Serial, lines: Vec<StockLine>) {
    let Some(vendor) = state.registry.entity_of(vendor_serial) else {
        return;
    };
    let Some((_, stock_serial)) = stock_of(state, vendor) else {
        return;
    };
    // What "full" means for this shelf, and when it may be filled again. Cumulative,
    // like the stocking itself.
    let mut record = state.registry.get::<Restock>(vendor).cloned().unwrap_or(Restock {
        at: state.ticks + RESTOCK_TICKS,
        lines: Vec::new(),
    });
    for line in lines {
        let Some(line) = semantic_stock_line(line) else {
            debug!("vendor stock line has an invalid semantic projection; skipped");
            continue;
        };
        record.lines.push(StockRecord {
            graphic: line.graphic,
            hue: line.hue,
            item_kind: line.item_kind,
            material: line.material,
            amount: line.amount,
            price: line.price,
            name: line.name.clone(),
        });
        place_stock_line(state, stock_serial, &line);
    }
    state.registry.insert(vendor, record);
    debug!(%stock_serial, "vendor stocked");
}

/// Normalize a compatibility stock line, or project the explicit typed form.
/// Thus once a registered line reaches a vendor crate its identity can never be
/// inferred from its display art during restock or purchase.
fn semantic_stock_line(mut line: StockLine) -> Option<StockLine> {
    match line.item_kind {
        Some(kind) => {
            let drawn = presentation_of(kind, line.material)?;
            line.graphic = drawn.id;
            line.hue = drawn.hue;
        }
        None => {
            if let Some((kind, material)) = kind_from_drawn(Drawn {
                id: line.graphic,
                hue: line.hue,
            }) {
                line.item_kind = Some(kind);
                line.material = material;
            }
        }
    }
    Some(line)
}

/// Put one line of goods on a shelf: the item, its count, its price and its label.
/// Shared by the first stocking and the restock, so a refilled line is the same
/// object a fresh one is.
fn place_stock_line(state: &mut WorldState, stock_serial: Serial, line: &StockLine) {
    let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
        return;
    };
    state.registry.insert(
        entity,
        Drawn {
            id: line.graphic,
            hue: line.hue,
        },
    );
    if let Some(kind) = line.item_kind {
        openshard_items::install_identity(state, entity, kind, line.material);
    }
    let contained = Contained {
        container: stock_serial,
        position: GumpPoint::new(50, 50),
        grid: GridSlot(0),
    };
    establish_item_location(state, entity, ItemLocation::contained(contained))
        .expect("fresh vendor stock has one valid shelf");
    state.registry.insert(entity, line.amount);
    state.registry.insert(entity, line.price);
    state.registry.insert(entity, Name(line.name.clone()));
    // And whatever the graphic implies: a lute's tunes, a bottle's poison. After
    // the name, because which poison a bottle holds is read off its label — all
    // four strengths share one graphic.
    openshard_items::apply_core_defaults(state, entity, line.graphic);
}

/// Top a vendor's shelf back up if its timer has run out — ServUO's
/// `BaseVendor.Restock`, checked when the shop is opened rather than on a tick pass.
/// That is the reference's own choice and it costs nothing when nobody is shopping.
///
/// Each remembered line is brought back to its full amount: a partly bought pile is
/// refilled, a sold-out one is put back. Never *reduced* — a script that added to the
/// shelf outside the record should not be undone by a timer — and anything on the
/// shelf the record does not name is left alone.
fn restock_if_due(state: &mut WorldState, vendor: EntityId, stock_serial: Serial) {
    let Some(record) = state.registry.get::<Restock>(vendor).cloned() else {
        return;
    };
    if state.ticks < record.at {
        return;
    }
    for line in &record.lines {
        let existing = openshard_state::contained_items(state, stock_serial)
            .filter(|(item, _)| match line.item_kind {
                Some(kind) => {
                    state.registry.get::<ItemKind>(*item) == Some(&ItemKind(kind))
                        && state.registry.get::<Material>(*item).map(|material| material.0) == line.material
                }
                None => state
                    .registry
                    .get::<Drawn>(*item)
                    .is_some_and(|g| g.id == line.graphic && g.hue == line.hue),
            })
            .map(|(item, _)| item)
            .next();
        match existing {
            Some(item) => {
                let have = state.registry.get::<Amount>(item).copied().unwrap_or(Amount(0));
                if have.0 < line.amount.0 {
                    state.registry.insert(item, line.amount);
                }
            }
            None => place_stock_line(
                state,
                stock_serial,
                &StockLine {
                    graphic: line.graphic,
                    hue: line.hue,
                    item_kind: line.item_kind,
                    material: line.material,
                    amount: line.amount,
                    price: line.price,
                    name: line.name.clone(),
                },
            ),
        }
    }
    state.registry.insert(
        vendor,
        Restock {
            at: state.ticks + RESTOCK_TICKS,
            lines: record.lines,
        },
    );
    // ServUO says "Restocked!" out loud when a staff member forces one; a shelf that
    // refilled on its own timer is worth the same line, and every visible action in
    // this engine announces itself.
    vendor_says(state, vendor, "I have restocked my wares.");
}

fn build_buy_lines(state: &WorldState, contents: &[ContainedItem]) -> Vec<BuyLine> {
    contents
        .iter()
        .map(|item| {
            let entity = state.registry.entity_of(item.serial);
            let price = entity
                .and_then(|e| state.registry.get::<Price>(e))
                .map_or(1, |p| p.0);
            let name = entity
                .and_then(|e| state.registry.get::<Name>(e))
                .map_or_else(|| format!("item {:#06x}", item.graphic.0), |n| n.0.clone());
            BuyLine { price, name }
        })
        .collect()
}

fn ensure_resale_container(state: &mut WorldState, vendor: EntityId) {
    // ClassicUO's buy window scans shop layers 0x1A and 0x1B and dereferences
    // the container on each with no null check. A vendor restored from a save
    // made before the second crate existed wears only 0x1A, so add 0x1B now or
    // the client crashes when the shop opens.
    if worn_container(state, vendor, RESALE_LAYER).is_none() {
        if let Some(vendor_serial) = state.registry.serial_of(vendor) {
            items::equip_new_container(
                state,
                vendor_serial,
                STOCK_GRAPHIC,
                STOCK_GUMP,
                Hue(0),
                RESALE_LAYER,
            );
        }
    }
}

fn send_shop_containers(state: &mut WorldState, connection: ConnectionId, vendor: EntityId) {
    // ServUO's `SendPacksTo`: tell the client the vendor wears both shop crates
    // (a `0x2E` equip each) *before* opening. The buy window is keyed on the
    // vendor and makes ClassicUO look up the vendor's shop-layer packs — which
    // null-crashes it if the client was never told they exist.
    for layer in [STOCK_LAYER, RESALE_LAYER] {
        let pack = worn_container(state, vendor, layer)
            .and_then(|s| state.registry.entity_of(s))
            .and_then(|entity| items::equip_packet(state, entity));
        if let Some(pack) = pack {
            state.send_packet(connection, &ServerPacket::EquipUpdate(pack));
        }
    }
}

fn send_buy_window(
    state: &mut WorldState,
    connection: ConnectionId,
    vendor_serial: Serial,
    stock_serial: Serial,
    version: ClientVersion,
    contents: &[ContainedItem],
    lines: &[BuyLine],
) {
    // Order and serials from ServUO's `BaseVendor.SendBuyPacket`: contents,
    // then prices, then the display packet **last** — and the display (`0x24`)
    // opens on the **vendor's** serial, not the crate's. This is the crux: the
    // client shows a *buy* interface only when the `0x24` names a mobile; an
    // item serial (the crate) just opens a plain container gump. The crate is
    // worn on the vendor's shop layer, so the client links the crate-keyed
    // contents to the vendor-keyed window itself.
    state.send_packet(
        connection,
        &ServerPacket::ContainerContents(ContainerContents {
            container: Some(stock_serial),
            items: contents.to_vec(),
        }),
    );
    state.send_packet(
        connection,
        &ServerPacket::BuyList(BuyList {
            container: stock_serial,
            lines: lines.to_vec(),
        }),
    );
    state.send(
        connection,
        encode_open_container(vendor_serial, SHOP_GUMP, version),
    );
    // Send each item's tooltip up front, the way ServUO ships the OPLs with the
    // buy packets: a client in OPL mode shows the shop name from the tooltip,
    // so without this the labels read as placeholders until the mouse hovers
    // each row and the client requests the list itself.
    if state.gameplay.tooltip_mode != TooltipMode::Off {
        for item in contents {
            if let Some(entity) = state.registry.entity_of(item.serial) {
                state.send_property_list(connection, entity);
            }
        }
    }
}

/// Open the shop on a double-click, if the clicked mobile is a vendor in
/// range. Returns whether it was — the caller falls through to the ordinary
/// double-click when it was not.
pub fn open_shop(state: &mut WorldState, connection: ConnectionId, serial: Serial) -> bool {
    let Some(&player) = state.players.get(&connection) else {
        return false;
    };
    let Some(vendor) = state.registry.entity_of(serial) else {
        return false;
    };
    let Some((_, stock_serial)) = stock_of(state, vendor) else {
        debug!(
            %serial,
            is_vendor = state.registry.has::<Vendor>(vendor),
            "open_shop: not a stocked vendor"
        );
        return false;
    };
    if !in_trade_range(state, player, vendor) {
        debug!(%serial, "open_shop: out of trade range");
        return false;
    }
    // Whether this customer may trade at all — ServUO checks it here, in `VendorBuy`,
    // and again on each of the three ways a deal can be struck below. It has to be
    // all four: a client that already has the window open can still send a `0x3B`.
    if !crate::speech::check_vendor_access(state, vendor, player) {
        return true;
    }
    let Some(version) = state.version_of(connection) else {
        return false;
    };
    // Before the shelf is read, refill it if its hour is up — ServUO checks the
    // restock delay at exactly this point, in `VendorBuy`.
    restock_if_due(state, vendor, stock_serial);

    // The contents and prices key on the stock crate — the client pairs the 0x74
    // lines with the 0x3C items by order, so the same walk builds both.
    let contents = items::contents_of(state, stock_serial);
    let lines = build_buy_lines(state, &contents);
    ensure_resale_container(state, vendor);
    send_shop_containers(state, connection, vendor);
    send_buy_window(
        state,
        connection,
        serial,
        stock_serial,
        version,
        &contents,
        &lines,
    );
    debug!(%serial, items = lines.len(), "open_shop: sent buy gump");
    true
}

struct BasketLine {
    item: EntityId,
    amount: u16,
    graphic: Graphic,
    hue: Hue,
    item_kind: Option<ItemKindId>,
    material: Option<MaterialId>,
}

struct Basket {
    total: u32,
    lines: Vec<BasketLine>,
}

fn price_basket(state: &WorldState, stock_serial: Serial, list: &[Purchase]) -> Basket {
    let mut total = 0u32;
    let mut lines = Vec::new();
    for purchase in list {
        let Some(item) = purchase
            .serial
            .validate()
            .and_then(|serial| state.registry.entity_of(serial))
        else {
            continue;
        };
        let held_in = match openshard_state::item_location(state, item) {
            Some(ItemLocation::Settled(openshard_state::SettledItemLocation::Contained(c))) => {
                Some(c.container)
            }
            _ => None,
        };
        if held_in != Some(stock_serial) {
            continue;
        }
        let have = state.registry.get::<Amount>(item).map_or(0, |amount| amount.0);
        let amount = have.min(purchase.amount.0);
        if amount == 0 {
            continue;
        }
        let price = state.registry.get::<Price>(item).map_or(1, |price| price.0);
        let Some(&Drawn { id, hue }) = state.registry.get::<Drawn>(item) else {
            continue;
        };
        total = total.saturating_add(price.saturating_mul(u32::from(amount)));
        lines.push(BasketLine {
            item,
            amount,
            graphic: id,
            hue,
            item_kind: state.registry.get::<ItemKind>(item).map(|kind| kind.0),
            material: state.registry.get::<Material>(item).map(|material| material.0),
        });
    }
    Basket { total, lines }
}

struct Payment {
    purse: Serial,
    from_bank: bool,
}

fn payment_for(
    state: &mut WorldState,
    player: EntityId,
    vendor: EntityId,
    backpack: Serial,
    total: u32,
) -> Option<Payment> {
    // ServUO's `BaseVendor` order: the pack whole, then — if the operator
    // allows it — the bank whole. Never split across the two.
    let in_pack = items::count_in_container(state, backpack, GOLD_GRAPHIC);
    let from_bank = if in_pack >= total {
        false
    } else if state.gameplay.vendor_bank_payment && items::banked_gold(state, player) >= total {
        true
    } else {
        vendor_says(state, vendor, "Thou canst not afford that.");
        return None;
    };
    let purse = if from_bank {
        worn_container(state, player, items::BANK_LAYER)?
    } else {
        backpack
    };
    Some(Payment { purse, from_bank })
}

fn commit_purchase(
    state: &mut WorldState,
    stock_serial: Serial,
    backpack: Serial,
    payment: Payment,
    basket: &Basket,
) -> bool {
    items::take_from_container(state, payment.purse, GOLD_GRAPHIC, basket.total);
    let mut complete = true;
    for line in &basket.lines {
        items::remove_from_stack(state, stock_serial, line.item, line.amount);
        complete &= match line.item_kind {
            Some(kind) => items::give_kind(state, backpack, kind, line.material, u32::from(line.amount))
                .is_some_and(|outcome| outcome.is_complete()),
            None => {
                items::give(state, backpack, line.graphic, line.hue, u32::from(line.amount)).is_complete()
            }
        };
    }
    complete
}

/// Settle a purchase: check the gold, take it, hand the goods over. See
/// `Command::Buy`.
pub fn buy(state: &mut WorldState, connection: ConnectionId, vendor_serial: RawSerial, list: &[Purchase]) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(vendor) = vendor_serial.validate().and_then(|s| state.registry.entity_of(s)) else {
        return;
    };
    let Some((_, stock_serial)) = stock_of(state, vendor) else {
        return;
    };
    if !in_trade_range(state, player, vendor) || list.is_empty() {
        return;
    }
    if !crate::speech::check_vendor_access(state, vendor, player) {
        return;
    }
    let Some(backpack) = worn_container(state, player, openshard_items::BACKPACK_LAYER) else {
        return;
    };

    // Price the whole basket first: a purchase is all-or-nothing, so a client
    // that asked for more than it can pay is refused before anything moves.
    let basket = price_basket(state, stock_serial, list);
    if basket.lines.is_empty() {
        return;
    }
    let Some(payment) = payment_for(state, player, vendor, backpack, basket.total) else {
        return;
    };
    let from_bank = payment.from_bank;
    let complete = commit_purchase(state, stock_serial, backpack, payment, &basket);
    vendor_says(
        state,
        vendor,
        &if !complete {
            "I could not place all of thy purchase in thy pack.".to_owned()
        } else if from_bank {
            format!(
                "The total of thy purchase is {} gold, withdrawn from thy bank account.",
                basket.total
            )
        } else {
            format!("The total of thy purchase is {} gold.", basket.total)
        },
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
    // ServUO's `VendorSell` refuses the same customers `VendorBuy` does.
    if !crate::speech::check_vendor_access(state, vendor, actor) {
        return true;
    }
    let Some(backpack) = worn_container(state, actor, openshard_items::BACKPACK_LAYER) else {
        return false;
    };

    // What the vendor stocks, and at what price — the catalogue a sale is
    // judged against.
    let catalogue = stock_prices(state, stock_serial);
    let lines: Vec<SellLine> = openshard_state::contained_items(state, backpack)
        .filter_map(|(entity, _)| {
            let &Drawn { id, hue } = state.registry.get::<Drawn>(entity)?;
            let price = sell_price(stock_price_for(state, entity, &catalogue)?);
            let serial = state.registry.serial_of(entity)?;
            let amount = state.registry.get::<Amount>(entity).map_or(1, |a| a.0);
            let name = state
                .registry
                .get::<Name>(entity)
                .map_or_else(|| format!("item {:#06x}", id.0), |n| n.0.clone());
            Some(SellLine {
                serial,
                graphic: id,
                hue,
                amount: openshard_protocol::items::ItemAmount(amount),
                price,
                name,
            })
        })
        .collect();
    if lines.is_empty() {
        vendor_says(state, vendor, "Thou hast nothing I wouldst buy.");
        return true;
    }
    state.send_packet(
        connection,
        &ServerPacket::SellList(SellList {
            vendor: vendor_serial,
            lines,
        }),
    );
    true
}

/// Settle a sale: goods out of the pack, gold in. See `Command::Sell`.
pub fn sell(state: &mut WorldState, connection: ConnectionId, vendor_serial: RawSerial, list: &[Sale]) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(vendor) = vendor_serial.validate().and_then(|s| state.registry.entity_of(s)) else {
        return;
    };
    let Some((_, stock_serial)) = stock_of(state, vendor) else {
        return;
    };
    if !in_trade_range(state, player, vendor) || list.is_empty() {
        return;
    }
    if !crate::speech::check_vendor_access(state, vendor, player) {
        return;
    }
    let Some(backpack) = worn_container(state, player, openshard_items::BACKPACK_LAYER) else {
        return;
    };
    let catalogue = stock_prices(state, stock_serial);

    let mut earned: u32 = 0;
    for sale in list {
        let Some(item) = sale.serial.validate().and_then(|s| state.registry.entity_of(s)) else {
            continue;
        };
        if !matches!(
            openshard_state::item_location(state, item),
            Some(ItemLocation::Settled(openshard_state::SettledItemLocation::Contained(c)))
                if c.container == backpack
        ) {
            continue;
        }
        let Some(&Drawn { id: _, .. }) = state.registry.get::<Drawn>(item) else {
            continue;
        };
        let Some(price) = stock_price_for(state, item, &catalogue) else {
            continue;
        };
        let taken = items::remove_from_stack(state, backpack, item, sale.amount.0);
        earned = earned.saturating_add(u32::from(sell_price(price)) * u32::from(taken));
    }
    if earned == 0 {
        return;
    }
    // Paid whole, however large: `give` spreads it over as many piles as it
    // needs. Clamping it to one stack's worth here was the same silent loss as
    // clamping a merge.
    let paid = earned;
    let outcome = items::give(state, backpack, GOLD_GRAPHIC, Hue(0), paid);
    vendor_says(
        state,
        vendor,
        &if outcome.is_complete() {
            format!("The total of thy sale is {paid} gold.")
        } else {
            format!(
                "I could place only {} of thy {paid} gold in thy pack.",
                outcome.given
            )
        },
    );
}

/// Half the buy price, never less than one coin.
fn sell_price(buy: u32) -> u16 {
    ((buy / 2).max(1)).min(u32::from(u16::MAX)) as u16
}

/// One stock identity and its unit price. Presentation is only the matching key
/// for still-unmigrated stock; typed rows compare their semantic components.
#[derive(Clone, Copy)]
struct StockPrice {
    graphic: Graphic,
    item_kind: Option<ItemKindId>,
    material: Option<MaterialId>,
    price: u32,
}

/// Every stock identity and unit price the vendor's crate holds.
fn stock_prices(state: &WorldState, stock_serial: Serial) -> Vec<StockPrice> {
    openshard_state::contained_items(state, stock_serial)
        .filter_map(|(entity, _)| {
            let graphic = state.registry.get::<Drawn>(entity)?.id;
            let price = state.registry.get::<Price>(entity).map_or(1, |p| p.0);
            Some(StockPrice {
                graphic,
                item_kind: state.registry.get::<ItemKind>(entity).map(|kind| kind.0),
                material: state.registry.get::<Material>(entity).map(|material| material.0),
                price,
            })
        })
        .collect()
}

/// Resolve what this vendor pays for an item. A typed shelf is intentionally
/// not a graphic-shaped wildcard: another semantic kind using the same classic
/// art cannot be sold as its stock.
fn stock_price_for(state: &WorldState, item: EntityId, catalogue: &[StockPrice]) -> Option<u32> {
    match state.registry.get::<ItemKind>(item) {
        Some(ItemKind(kind)) => {
            let material = state.registry.get::<Material>(item).map(|material| material.0);
            catalogue
                .iter()
                .find(|line| line.item_kind == Some(*kind) && line.material == material)
                .map(|line| line.price)
        }
        None => {
            let graphic = state.registry.get::<Drawn>(item)?.id;
            catalogue
                .iter()
                .find(|line| line.item_kind.is_none() && line.graphic == graphic)
                .map(|line| line.price)
        }
    }
}

/// The nearest vendor within trade range of `actor`, if any.
fn nearest_vendor(state: &WorldState, actor: EntityId) -> Option<EntityId> {
    let &Position(at) = state.registry.get::<Position>(actor)?;
    // The keyword path ("buy"/"sell") answers the closest vendor a shopper could
    // reach — the same range-and-line-of-sight gate a double-click passes, so a
    // shout does not carry through a wall either.
    state
        .registry
        .query::<Vendor>()
        .filter(|(vendor, _)| in_trade_range(state, actor, *vendor))
        .filter_map(|(vendor, _)| {
            let &Position(pos) = state.registry.get::<Position>(vendor)?;
            Some((openshard_state::sectors::distance(at, pos), vendor))
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
    open_shop(state, connection, vendor_serial)
}

/// The vendor's own voice: a conversational line drawn over its head for
/// everyone in earshot, the way any NPC speaks — not a private `0x1C` system
/// line to a single screen. The customer's answer should look like the
/// shopkeeper talking, not the shard.
fn vendor_says(state: &mut WorldState, vendor: EntityId, text: &str) {
    crate::say(state, vendor, text);
}

/// The serial of the container `mobile` wears at `layer`, if any.
fn worn_container(state: &WorldState, mobile: EntityId, layer: Layer) -> Option<Serial> {
    let serial = state.registry.serial_of(mobile)?;
    openshard_state::equipped_items(state, serial)
        .find(|(_, worn)| worn.layer == layer)
        .and_then(|(entity, _)| state.registry.serial_of(entity))
}

/// Dress a fresh townsperson as a vendor: the mark, and the stock crate.
pub(crate) fn make_vendor(state: &mut WorldState, entity: EntityId, serial: Serial) {
    state.registry.insert(entity, Vendor);
    items::equip_new_container(state, serial, STOCK_GRAPHIC, STOCK_GUMP, Hue(0), STOCK_LAYER);
    // The empty second crate ClassicUO's buy scan insists on — see `RESALE_LAYER`.
    items::equip_new_container(state, serial, STOCK_GRAPHIC, STOCK_GUMP, Hue(0), RESALE_LAYER);
}
