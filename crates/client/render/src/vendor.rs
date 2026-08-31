//! The client-owned NPC vendor window.
//!
//! The shop packets carry rows, not a `0xB0` layout, so this is deliberately a
//! small native gump: it gives the otherwise wire-only catalogue prices,
//! quantities and controls while keeping all payment decisions on the shard.

use openshard_protocol::containers::ContainedItem;
use openshard_protocol::serial::Serial;
use openshard_protocol::speech::Font;
use openshard_protocol::vendor::{
    BuyLine,
    SellLine,
};
use openshard_protocol::wire::Hue;

use crate::gump::{
    self,
    GumpArt,
    GumpAtlas,
    GumpPixel,
    Picture,
};
use crate::text::GumpLabel;

/// ClassicUO's two overlapping shop panels are 283 pixels wide, with the
/// right panel beginning 251 pixels into the left one.
pub const WIDTH: i32 = 534;
const HEIGHT: i32 = 369;
/// ClassicUO gives each item a 50-pixel square; static art is not a table
/// glyph and needs that room to keep its original proportions.
pub const ROW_HEIGHT: i32 = 50;
const ROW_TOP: i32 = 68;
pub const VISIBLE_ROWS: usize = 4;
const BUY_LEFT: u16 = 0x0870;
const BUY_RIGHT: u16 = 0x0871;
const SELL_LEFT: u16 = 0x0872;
const SELL_RIGHT: u16 = 0x0873;
const RIGHT_AT: GumpPixel = GumpPixel::new(251, 121);
const ICON_CELL_WIDTH: i32 = 50;
const ROW_FONT: Font = Font(9);
// ClassicUO's shop labels use this dark, high-contrast hue over the parchment
// panels rather than the white used by the old improvised list.
const ROW_HUE: Hue = Hue(0x0219);

/// A position in the vendor catalogue, never a position in the shorter order
/// panel drawn beside it.
///
/// The renderer produces one from hit-testing and the app uses it to update an
/// order, so the type crosses that crate boundary with the meaning intact.
/// [`position`](Self::position) opens it only where a catalogue slice or the
/// parallel amount list is actually indexed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct CatalogueIndex(usize);

impl CatalogueIndex {
    pub const fn new(position: usize) -> Self {
        Self(position)
    }

    pub const fn position(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Row(CatalogueIndex),
    /// A row already present in the order panel; clicking it removes one.
    Remove(CatalogueIndex),
    Confirm,
    Clear,
}

/// A button plate the pointer is currently over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Confirm,
    Clear,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    pub at:   GumpPixel,
    pub text: String,
    pub font: Font,
    pub hue:  Hue,
    pub clip: Option<(i32, i32)>,
}

impl Line {
    pub fn label(&self) -> GumpLabel<'_> {
        GumpLabel {
            at:   self.at,
            text: &self.text,
            font: self.font,
            hue:  self.hue,
            clip: self.clip,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    pub vendor:   Serial,
    pub sell:     bool,
    pub at:       GumpPixel,
    pub scroll:   CatalogueIndex,
    pub rows:     usize,
    selected:     Vec<CatalogueIndex>,
    pub pictures: Vec<Picture>,
    pub lines:    Vec<Line>,
}

impl Window {
    pub fn height(&self) -> i32 {
        HEIGHT
    }

    /// The complete bitmap window, including its decorative header.  A vendor
    /// gump may be dragged from that header even though it is not an action.
    pub fn contains(&self, cursor: GumpPixel) -> bool {
        let x = cursor.x - self.at.x;
        let y = cursor.y - self.at.y;
        (0..WIDTH).contains(&x) && (0..self.height()).contains(&y)
    }

    /// The left catalogue viewport.  Wheel input over the order panel must not
    /// unexpectedly scroll stock behind a confirmation click.
    pub fn catalogue_contains(&self, cursor: GumpPixel) -> bool {
        let x = cursor.x - self.at.x;
        let y = cursor.y - self.at.y;
        (32..251).contains(&x) && (ROW_TOP..ROW_TOP + VISIBLE_ROWS as i32 * ROW_HEIGHT).contains(&y)
    }

    pub fn hit(&self, cursor: GumpPixel) -> Option<Hit> {
        let x = cursor.x - self.at.x;
        let y = cursor.y - self.at.y;
        if !self.contains(cursor) {
            return None;
        }
        if self.catalogue_contains(cursor) && (ROW_TOP..ROW_TOP + self.rows as i32 * ROW_HEIGHT).contains(&y)
        {
            return Some(Hit::Row(CatalogueIndex::new(
                self.scroll.position() + ((y - ROW_TOP) / ROW_HEIGHT) as usize,
            )));
        }
        if (267..447).contains(&x) && (185..297).contains(&y) {
            let selected = ((y - 185) / 28) as usize;
            if let Some(row) = self.selected.get(selected) {
                return Some(Hit::Remove(*row));
            }
        }
        self.action_at(cursor).map(|action| {
            match action {
                Action::Confirm => Hit::Confirm,
                Action::Clear => Hit::Clear,
            }
        })
    }

    /// The action plate at a cursor position, if any.  This is separate from
    /// [`hit`](Self::hit) because drawing needs the same answer for hover.
    pub fn action_at(&self, cursor: GumpPixel) -> Option<Action> {
        action_at(self.at, cursor)
    }
}

fn action_at(at: GumpPixel, cursor: GumpPixel) -> Option<Action> {
    let x = cursor.x - at.x;
    let y = cursor.y - at.y;
    // ClassicUO leaves these controls as bitmap art with transparent hit
    // boxes.  Use the full visible button plates, not a narrow 34-pixel
    // slice, so their label and bevel are both clickable.
    if (267..440).contains(&x) && (306..365).contains(&y) {
        Some(Action::Confirm)
    } else if (440..530).contains(&x) && (306..365).contains(&y) {
        Some(Action::Clear)
    } else {
        None
    }
}

pub fn art_of() -> impl Iterator<Item = GumpArt> {
    [BUY_LEFT, BUY_RIGHT, SELL_LEFT, SELL_RIGHT]
        .into_iter()
        .map(|graphic| GumpArt::Gump(openshard_protocol::wire::Graphic(graphic)))
}

struct Row {
    name:     String,
    price:    u32,
    amount:   u16,
    quantity: String,
    graphic:  Option<openshard_protocol::wire::Graphic>,
    hue:      Hue,
}

// `sell` below takes seven of these eight, and for the same reason as
// `paperdoll::window`: they are the frame's inputs — the catalogue, what the
// pointer is doing, and where to draw — not one value arriving in parts.
#[allow(clippy::too_many_arguments)]
pub fn buy(
    vendor: Serial,
    lines: &[BuyLine],
    items: &[ContainedItem],
    amounts: &[u16],
    scroll: CatalogueIndex,
    at: GumpPixel,
    cursor: GumpPixel,
    atlas: &GumpAtlas,
) -> Window {
    window(
        vendor,
        false,
        lines.iter().enumerate().map(|(i, line)| {
            Row {
                name:     line.name.clone(),
                price:    line.price,
                amount:   amounts.get(i).copied().unwrap_or(0),
                quantity: format!("x{}", amounts.get(i).copied().unwrap_or(0)),
                graphic:  items.get(i).map(|item| item.graphic),
                hue:      items.get(i).map_or(Hue::NONE, |item| item.hue),
            }
        }),
        scroll,
        at,
        cursor,
        atlas,
    )
}

pub fn sell(
    vendor: Serial,
    lines: &[SellLine],
    amounts: &[u16],
    scroll: CatalogueIndex,
    at: GumpPixel,
    cursor: GumpPixel,
    atlas: &GumpAtlas,
) -> Window {
    window(
        vendor,
        true,
        lines.iter().enumerate().map(|(i, line)| {
            Row {
                name:     line.name.clone(),
                // A real widening: `SellLine::price` is a `u16` where `BuyLine`'s is
                // already a `u32`.
                price:    u32::from(line.price),
                amount:   amounts.get(i).copied().unwrap_or(0),
                quantity: format!("{}/{}", amounts.get(i).copied().unwrap_or(0), line.amount.0),
                graphic:  Some(line.graphic),
                hue:      line.hue,
            }
        }),
        scroll,
        at,
        cursor,
        atlas,
    )
}

fn window(
    vendor: Serial,
    sell: bool,
    rows: impl Iterator<Item = Row>,
    scroll: CatalogueIndex,
    at: GumpPixel,
    cursor: GumpPixel,
    atlas: &GumpAtlas,
) -> Window {
    let rows: Vec<Row> = rows.collect();
    let scroll = CatalogueIndex::new(scroll.position().min(rows.len().saturating_sub(VISIBLE_ROWS)));
    let scroll_position = scroll.position();
    let mut lines = catalogue_lines(&rows, scroll_position, at);
    // The right-hand panel is the ClassicUO transaction list.  The catalogue
    // stays on the left; only chosen lines travel across, so confirmation is
    // legible before it commits the one packet to the shard.
    let (selected, order_lines) = order_panel(&rows, at, action_at(at, cursor));
    lines.extend(order_lines);
    Window {
        vendor,
        sell,
        at,
        scroll,
        rows: rows.len().saturating_sub(scroll_position).min(VISIBLE_ROWS),
        selected,
        pictures: pictures(&rows, scroll_position, sell, at, atlas),
        lines,
    }
}

fn catalogue_lines(rows: &[Row], scroll: usize, at: GumpPixel) -> Vec<Line> {
    rows.iter()
        .skip(scroll)
        .take(VISIBLE_ROWS)
        .enumerate()
        .flat_map(|(i, row)| {
            let y = ROW_TOP + i as i32 * ROW_HEIGHT;
            [
                Line {
                    at:   at.offset(GumpPixel::new(92, y + 15)),
                    text: format!("{}: {} gp", row.name, row.price),
                    font: ROW_FONT,
                    hue:  ROW_HUE,
                    clip: Some((108, ROW_HEIGHT)),
                },
                Line {
                    at:   at.offset(GumpPixel::new(200, y + 15)),
                    text: row.quantity.clone(),
                    font: ROW_FONT,
                    hue:  ROW_HUE,
                    clip: Some((35, ROW_HEIGHT)),
                },
            ]
        })
        .collect()
}

fn order_panel(rows: &[Row], at: GumpPixel, hover: Option<Action>) -> (Vec<CatalogueIndex>, Vec<Line>) {
    let selected: Vec<CatalogueIndex> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| (row.amount > 0).then_some(CatalogueIndex::new(index)))
        .collect();
    let mut lines: Vec<Line> = selected
        .iter()
        .filter_map(|index| rows.get(index.position()))
        .take(4)
        .enumerate()
        .flat_map(|(i, row)| {
            let y = 185 + i as i32 * 28;
            [
                Line {
                    at:   at.offset(GumpPixel::new(277, y)),
                    text: row.amount.to_string(),
                    font: ROW_FONT,
                    hue:  ROW_HUE,
                    clip: Some((22, 22)),
                },
                Line {
                    at:   at.offset(GumpPixel::new(302, y)),
                    text: row.name.clone(),
                    font: ROW_FONT,
                    hue:  ROW_HUE,
                    clip: Some((145, 22)),
                },
            ]
        })
        .collect();
    let total: u32 = rows
        .iter()
        .map(|row| row.price.saturating_mul(u32::from(row.amount)))
        .sum();
    lines.push(Line {
        at:   at.offset(GumpPixel::new(287, 302)),
        text: format!("TOTAL: {total} gp"),
        font: ROW_FONT,
        hue:  ROW_HUE,
        clip: Some((135, 22)),
    });
    for (action, x, text) in [(Action::Confirm, 302, "ACCEPT"), (Action::Clear, 456, "CLEAR")] {
        lines.push(Line {
            at:   at.offset(GumpPixel::new(x, 328)),
            text: text.to_owned(),
            font: ROW_FONT,
            hue:  if hover == Some(action) {
                Hue(0x0035)
            } else {
                ROW_HUE
            },
            clip: Some((80, 22)),
        });
    }
    (selected, lines)
}

fn pictures(rows: &[Row], scroll: usize, sell: bool, at: GumpPixel, atlas: &GumpAtlas) -> Vec<Picture> {
    let (left, right) = if sell {
        (SELL_LEFT, SELL_RIGHT)
    } else {
        (BUY_LEFT, BUY_RIGHT)
    };
    let mut pictures = vec![
        Picture::plain(GumpArt::Gump(openshard_protocol::wire::Graphic(left)), at),
        Picture::plain(
            GumpArt::Gump(openshard_protocol::wire::Graphic(right)),
            at.offset(RIGHT_AT),
        ),
    ];
    pictures.extend(
        rows.iter()
            .skip(scroll)
            .take(VISIBLE_ROWS)
            .enumerate()
            .filter_map(|(i, row)| {
                row.graphic.map(|graphic| {
                    let cell_at = at.offset(GumpPixel::new(37, ROW_TOP + i as i32 * ROW_HEIGHT));
                    let (width, height) = atlas
                        .sprite(GumpArt::Item(graphic))
                        .map_or((ICON_CELL_WIDTH, ROW_HEIGHT), |sprite| {
                            (i32::from(sprite.width), i32::from(sprite.height))
                        });
                    let icon_at = cell_at.offset(GumpPixel::new(
                        (ICON_CELL_WIDTH - width) / 2,
                        (ROW_HEIGHT - height) / 2,
                    ));
                    Picture::plain(GumpArt::Item(graphic), icon_at)
                        .hued(row.hue)
                        .inside(gump::Scissor {
                            at:     cell_at,
                            width:  ICON_CELL_WIDTH,
                            height: ROW_HEIGHT,
                        })
                })
            }),
    );
    pictures
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rows_and_footer_have_disjoint_hits() {
        let window = buy(
            Serial::new(42).unwrap(),
            &[BuyLine {
                price: 5,
                name:  "apple".into(),
            }],
            &[],
            &[1],
            CatalogueIndex::new(0),
            GumpPixel::new(10, 20),
            GumpPixel::new(0, 0),
            &GumpAtlas::empty(),
        );
        assert_eq!(
            window.hit(GumpPixel::new(50, 90)),
            Some(Hit::Row(CatalogueIndex::new(0)))
        );
        assert_eq!(
            window.hit(GumpPixel::new(290, 205)),
            Some(Hit::Remove(CatalogueIndex::new(0)))
        );
        assert_eq!(window.hit(GumpPixel::new(300, 345)), Some(Hit::Confirm));
        assert_eq!(window.hit(GumpPixel::new(480, 345)), Some(Hit::Clear));
        assert!(window.contains(GumpPixel::new(12, 25)));
        assert!(!window.contains(GumpPixel::new(550, 25)));
        assert!(window.catalogue_contains(GumpPixel::new(50, 90)));
        assert!(!window.catalogue_contains(GumpPixel::new(300, 90)));
    }

    #[test]
    fn a_long_catalogue_has_a_fixed_viewport_and_offsets_its_row_hits() {
        let lines: Vec<BuyLine> = (0..13)
            .map(|price| {
                BuyLine {
                    price,
                    name: format!("item {price}"),
                }
            })
            .collect();
        let window = buy(
            Serial::new(42).unwrap(),
            &lines,
            &[],
            &vec![0; lines.len()],
            CatalogueIndex::new(1),
            GumpPixel::new(10, 20),
            GumpPixel::new(0, 0),
            &GumpAtlas::empty(),
        );

        assert_eq!(window.rows, VISIBLE_ROWS);
        assert_eq!(window.lines.len(), VISIBLE_ROWS * 2 + 3);
        assert!(window.lines.iter().any(|line| line.text.starts_with("item 1:")));
        assert_eq!(
            window.hit(GumpPixel::new(50, 89)),
            Some(Hit::Row(CatalogueIndex::new(1)))
        );
    }

    #[test]
    fn the_order_panel_contains_only_chosen_rows_and_their_total() {
        let window = buy(
            Serial::new(42).unwrap(),
            &[
                BuyLine {
                    price: 5,
                    name:  "apple".into(),
                },
                BuyLine {
                    price: 7,
                    name:  "pear".into(),
                },
            ],
            &[],
            &[2, 0],
            CatalogueIndex::new(0),
            GumpPixel::new(10, 20),
            GumpPixel::new(0, 0),
            &GumpAtlas::empty(),
        );

        assert_eq!(window.selected, vec![CatalogueIndex::new(0)]);
        assert!(window.lines.iter().any(|line| line.text == "2"));
        assert!(window.lines.iter().any(|line| line.text == "apple"));
        assert!(!window.lines.iter().any(|line| line.text == "pear"));
        assert!(window.lines.iter().any(|line| line.text == "TOTAL: 10 gp"));
    }
}
