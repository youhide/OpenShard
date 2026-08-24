//! Ground-item pile presentation: artwork and count labels.

use openshard_protocol::items::ItemAmount;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_tiles::TileData;

/// One thing lying on the ground, as the client has been told about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GroundItem {
    /// Where it lies.
    pub at: Point,
    /// The base graphic the shard sent; pile artwork is selected at draw time.
    pub graphic: Graphic,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue: Hue,
    /// The amount in this ground pile.
    pub amount: ItemAmount,
}

impl GroundItem {
    /// The graphic actually shown for this amount.
    #[must_use]
    pub const fn displayed(&self) -> Graphic {
        displayed_graphic(self.graphic, self.amount)
    }
}

/// Choose the classic small-stack or pile artwork for currency.
#[must_use]
pub const fn displayed_graphic(graphic: Graphic, amount: ItemAmount) -> Graphic {
    match graphic.0 {
        0x0EEA | 0x0EED | 0x0EF0 if amount.0 > 5 => Graphic(graphic.0 + 2),
        0x0EEA | 0x0EED | 0x0EF0 if amount.0 > 1 => Graphic(graphic.0 + 1),
        _ => graphic,
    }
}

/// The compact face used for pile counts.
pub const STACK_COUNT_FONT: Font = Font(9);

/// The count to draw for a stackable ground item, if it has more than one item.
#[must_use]
pub fn stack_label(graphic: Graphic, amount: ItemAmount, tiledata: &TileData) -> Option<String> {
    let stacks = tiledata.static_tile(graphic.0).flags.is_stackable();
    (stacks && amount.0 > 1).then(|| abbreviated(amount))
}

/// Render an item amount in the compact form that fits on an icon.
#[must_use]
pub fn abbreviated(amount: ItemAmount) -> String {
    let count = u32::from(amount.0);
    match count {
        0..1_000 => count.to_string(),
        1_000..10_000 => format!("{}.{}k", count / 1_000, (count % 1_000) / 100),
        _ => format!("{}k", count / 1_000),
    }
}
