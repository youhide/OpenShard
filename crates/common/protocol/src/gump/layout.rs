//! Reading a gump layout back: the inverse of [`GumpLayout`](super::GumpLayout).
//!
//! The builder next door writes the client's little command language; this reads
//! it. Both ends of the wire agree on this language — the server writes it and a
//! client renders it — so the two halves live together, and a keyword added to
//! one is a keyword the other is missing in the same file.
//!
//! # Nothing here refuses a layout
//!
//! [`parse`] is total. An element with a keyword this engine has no picture for,
//! or with fewer arguments than that keyword takes, becomes
//! [`Element::Unknown`] and the rest of the window still draws. That is what the
//! reference client does — a layout it cannot read renders as a window with a
//! hole in it, not as a dropped packet — and it is the useful behaviour for a
//! client under construction: a dialog with one unimplemented element is worth
//! more than no dialog.
//!
//! What it deliberately does *not* do is resolve a line index. `{ text 66 56
//! 1153 1 }` names line 1 of the table that travelled beside the layout, and
//! whether that table has a line 1 is the caller's question — see
//! [`GumpDisplay::lines`](super::GumpDisplay::lines).

use super::{GumpButton, RawButtonId, RawSwitchId};

/// One drawable element of a layout, with its arguments in the client's order.
///
/// The variants are the keywords [`GumpLayout`](super::GumpLayout) can write,
/// and nothing else: a keyword no encoder here produces would be a guess about
/// an argument order that has never been checked against a client.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Element {
    /// `{ page N }` — everything after this belongs to page `N` until the next
    /// one. Page `0` is drawn on every page.
    Page(u32),
    /// `{ nomove }`, `{ noclose }`, `{ nodispose }`, `{ noresize }` — how the
    /// window may be handled, carried as the keyword itself since none of the
    /// four takes an argument.
    Flag(Flag),
    /// `{ resizepic }` — a stretched background frame.
    Background {
        /// Top-left corner, from the window's own origin.
        x: i32,
        /// Top-left corner, from the window's own origin.
        y: i32,
        /// How wide to stretch it.
        width: i32,
        /// How tall to stretch it.
        height: i32,
        /// The gump art to stretch.
        gump: u32,
    },
    /// `{ gumppic }` — a picture from the gump art, optionally hued.
    Image {
        /// Where it goes.
        x: i32,
        /// Where it goes.
        y: i32,
        /// Which picture.
        gump: u32,
        /// The `hue=` suffix, or `None` when the element carried none.
        hue: Option<u32>,
    },
    /// `{ gumppictiled }` — one picture repeated to fill a rectangle.
    ImageTiled {
        /// Where the rectangle starts.
        x: i32,
        /// Where the rectangle starts.
        y: i32,
        /// How wide it is.
        width: i32,
        /// How tall it is.
        height: i32,
        /// The picture to repeat.
        gump: u32,
    },
    /// `{ checkertrans }` — a darkened, semi-transparent rectangle.
    AlphaRegion {
        /// Where the rectangle starts.
        x: i32,
        /// Where the rectangle starts.
        y: i32,
        /// How wide it is.
        width: i32,
        /// How tall it is.
        height: i32,
    },
    /// `{ button }` — the only element that answers.
    Button {
        /// Where it goes.
        x: i32,
        /// Where it goes.
        y: i32,
        /// The art drawn while it is up.
        normal: u32,
        /// The art drawn while it is held down.
        pressed: u32,
        /// Whether pressing it replies or flips a page.
        kind: GumpButton,
        /// For a [`GumpButton::Page`], the page to flip to; unread otherwise.
        page: u32,
        /// For a [`GumpButton::Reply`], what to send back. Raw: it is the
        /// server's number and this end only repeats it.
        id: RawButtonId,
    },
    /// `{ checkbox }` — an independent switch.
    Check(Switch),
    /// `{ radio }` — a switch that turns its neighbours off.
    Radio(Switch),
    /// `{ text }` — one line from the text table, in a hue.
    Label {
        /// Where it goes.
        x: i32,
        /// Where it goes.
        y: i32,
        /// The client's 15-bit colour.
        hue: u32,
        /// Which line of the table.
        line: usize,
    },
    /// `{ croppedtext }` — a line clipped to a box rather than overflowing it.
    CroppedLabel {
        /// Where the box starts.
        x: i32,
        /// Where the box starts.
        y: i32,
        /// How wide the box is.
        width: i32,
        /// How tall the box is.
        height: i32,
        /// The client's 15-bit colour.
        hue: u32,
        /// Which line of the table.
        line: usize,
    },
    /// `{ htmlgump }` — a block of HTML text from the table.
    Html {
        /// Where the block starts.
        x: i32,
        /// Where the block starts.
        y: i32,
        /// How wide it is.
        width: i32,
        /// How tall it is.
        height: i32,
        /// Which line of the table.
        line: usize,
        /// Whether it is drawn over a background.
        background: bool,
        /// Whether it scrolls.
        scrollbar: bool,
    },
    /// The `xmfhtml*` family — a string from the client's own `cliloc` file, so
    /// no text travelled and there is nothing here to draw without one.
    Localized {
        /// Where the block starts.
        x: i32,
        /// Where the block starts.
        y: i32,
        /// How wide it is.
        width: i32,
        /// How tall it is.
        height: i32,
        /// The number to look up.
        cliloc: u32,
    },
    /// `{ tilepic }` / `{ tilepichue }` — a picture from the *world's* art, the
    /// same graphic an item on the ground is drawn with.
    Item {
        /// Where it goes.
        x: i32,
        /// Where it goes.
        y: i32,
        /// Which graphic.
        graphic: u32,
        /// Its hue, or `None` for the plain `tilepic` form.
        hue: Option<u32>,
    },
    /// `{ tilepicfit }` — an OpenShard item cell. The item's decoded art is
    /// proportionally fitted and centred in this rectangle rather than drawn
    /// at its natural dimensions.
    ItemFitted {
        /// Where the cell starts.
        x: i32,
        /// Where the cell starts.
        y: i32,
        /// How wide the cell is.
        width: i32,
        /// How tall the cell is.
        height: i32,
        /// Which world-art graphic the cell shows.
        graphic: u32,
        /// Its hue, or `None` when the element carried no hue.
        hue: Option<u32>,
    },
    /// `{ textentry }` — a field the player types into, answered in the `0xB1`.
    TextEntry {
        /// Where the field starts.
        x: i32,
        /// Where the field starts.
        y: i32,
        /// How wide it is.
        width: i32,
        /// How tall it is.
        height: i32,
        /// The client's 15-bit colour.
        hue: u32,
        /// The id its contents come back under.
        entry_id: u16,
        /// Which line of the table it starts out holding.
        line: usize,
    },
    /// An element this engine has no picture for, or one whose arguments did not
    /// fit its keyword. Kept, keyword and all, so a client can say what it did
    /// not draw instead of drawing a window that is silently missing a row.
    Unknown {
        /// The keyword as it arrived.
        keyword: String,
    },
}

/// A `{ checkbox }` or a `{ radio }`: the same six arguments, and the same reply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Switch {
    /// Where it goes.
    pub x: i32,
    /// Where it goes.
    pub y: i32,
    /// The art drawn while it is off.
    pub off: u32,
    /// The art drawn while it is on.
    pub on: u32,
    /// Whether it starts out set.
    pub initial: bool,
    /// What comes back when it is left on. Raw for [`Element::Button`]'s reason.
    pub id: RawSwitchId,
}

/// How the window may be handled — the four argument-less keywords.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flag {
    /// `{ nomove }` — it may not be dragged.
    NoMove,
    /// `{ noclose }` — it has no close box.
    NoClose,
    /// `{ nodispose }` — right-click does not dismiss it.
    NoDispose,
    /// `{ noresize }` — it may not be resized.
    NoResize,
}

/// Read a layout string into its elements, in the order the window draws them.
///
/// Total: see the module docs. Anything outside a `{ ... }` group is skipped —
/// the language has no text between elements, and a stray byte is not a reason
/// to lose the window.
#[must_use]
pub fn parse(layout: &str) -> Vec<Element> {
    let mut elements = Vec::new();
    let mut rest = layout;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break; // an unterminated element: there is nothing further to read
        };
        elements.push(element(&after[..close]));
        rest = &after[close + 1..];
    }
    elements
}

/// One element, from the text between its braces.
fn element(body: &str) -> Element {
    let mut words = body.split_whitespace();
    let Some(keyword) = words.next() else {
        return Element::Unknown {
            keyword: String::new(),
        };
    };
    // The arguments, as the client reads them: positional and numeric. The two
    // suffix forms the builder writes — `hue=N` on a `gumppic` and `@args@` on
    // an `xmfhtmltok` — are not positions, so they are pulled out separately and
    // never shift the ones that are.
    let mut args: Vec<i64> = Vec::new();
    let mut hue_suffix = None;
    for word in words {
        if let Some(value) = word.strip_prefix("hue=") {
            hue_suffix = value.parse().ok();
        } else if let Ok(value) = word.parse::<i64>() {
            args.push(value);
        }
    }

    let unknown = || Element::Unknown {
        keyword: keyword.to_owned(),
    };
    let at = |i: usize| args.get(i).copied();

    match keyword {
        "page" => at(0).map_or_else(unknown, |page| Element::Page(page as u32)),
        "nomove" => Element::Flag(Flag::NoMove),
        "noclose" => Element::Flag(Flag::NoClose),
        "nodispose" => Element::Flag(Flag::NoDispose),
        "noresize" => Element::Flag(Flag::NoResize),
        // Note the order: `resizepic` names the art *before* the size, unlike
        // every other element — see `GumpLayout::background`.
        "resizepic" => match (at(0), at(1), at(2), at(3), at(4)) {
            (Some(x), Some(y), Some(gump), Some(width), Some(height)) => Element::Background {
                x: x as i32,
                y: y as i32,
                width: width as i32,
                height: height as i32,
                gump: gump as u32,
            },
            _ => unknown(),
        },
        "gumppic" => match (at(0), at(1), at(2)) {
            (Some(x), Some(y), Some(gump)) => Element::Image {
                x: x as i32,
                y: y as i32,
                gump: gump as u32,
                hue: hue_suffix,
            },
            _ => unknown(),
        },
        "gumppictiled" => match (at(0), at(1), at(2), at(3), at(4)) {
            (Some(x), Some(y), Some(width), Some(height), Some(gump)) => Element::ImageTiled {
                x: x as i32,
                y: y as i32,
                width: width as i32,
                height: height as i32,
                gump: gump as u32,
            },
            _ => unknown(),
        },
        "checkertrans" => match (at(0), at(1), at(2), at(3)) {
            (Some(x), Some(y), Some(width), Some(height)) => Element::AlphaRegion {
                x: x as i32,
                y: y as i32,
                width: width as i32,
                height: height as i32,
            },
            _ => unknown(),
        },
        "button" => match (at(0), at(1), at(2), at(3), at(4), at(5), at(6)) {
            (Some(x), Some(y), Some(normal), Some(pressed), Some(kind), Some(page), Some(id)) => {
                Element::Button {
                    x: x as i32,
                    y: y as i32,
                    normal: normal as u32,
                    pressed: pressed as u32,
                    // Anything that is not the page code is a reply: the client
                    // reads the field as a boolean, and a button that answered
                    // nothing at all would be a dialog with no way out.
                    kind: if kind == 0 {
                        GumpButton::Page
                    } else {
                        GumpButton::Reply
                    },
                    page: page as u32,
                    id: RawButtonId(id as u32),
                }
            }
            _ => unknown(),
        },
        "checkbox" => switch(&args).map_or_else(unknown, Element::Check),
        "radio" => switch(&args).map_or_else(unknown, Element::Radio),
        "text" => match (at(0), at(1), at(2), at(3)) {
            (Some(x), Some(y), Some(hue), Some(line)) if line >= 0 => Element::Label {
                x: x as i32,
                y: y as i32,
                hue: hue as u32,
                line: line as usize,
            },
            _ => unknown(),
        },
        "croppedtext" => match (at(0), at(1), at(2), at(3), at(4), at(5)) {
            (Some(x), Some(y), Some(width), Some(height), Some(hue), Some(line)) if line >= 0 => {
                Element::CroppedLabel {
                    x: x as i32,
                    y: y as i32,
                    width: width as i32,
                    height: height as i32,
                    hue: hue as u32,
                    line: line as usize,
                }
            }
            _ => unknown(),
        },
        "htmlgump" => match (at(0), at(1), at(2), at(3), at(4), at(5), at(6)) {
            (Some(x), Some(y), Some(width), Some(height), Some(line), Some(back), Some(scroll))
                if line >= 0 =>
            {
                Element::Html {
                    x: x as i32,
                    y: y as i32,
                    width: width as i32,
                    height: height as i32,
                    line: line as usize,
                    background: back != 0,
                    scrollbar: scroll != 0,
                }
            }
            _ => unknown(),
        },
        // The three localized forms differ in what follows the cliloc, and agree
        // on everything up to it — except `xmfhtmltok`, which puts the flags and
        // the colour *first* and the cliloc last. See `html_localized_args`.
        "xmfhtmlgump" | "xmfhtmlgumpcolor" => match (at(0), at(1), at(2), at(3), at(4)) {
            (Some(x), Some(y), Some(width), Some(height), Some(cliloc)) => Element::Localized {
                x: x as i32,
                y: y as i32,
                width: width as i32,
                height: height as i32,
                cliloc: cliloc as u32,
            },
            _ => unknown(),
        },
        "xmfhtmltok" => match (at(0), at(1), at(2), at(3), at(7)) {
            (Some(x), Some(y), Some(width), Some(height), Some(cliloc)) => Element::Localized {
                x: x as i32,
                y: y as i32,
                width: width as i32,
                height: height as i32,
                cliloc: cliloc as u32,
            },
            _ => unknown(),
        },
        "tilepic" => match (at(0), at(1), at(2)) {
            (Some(x), Some(y), Some(graphic)) => Element::Item {
                x: x as i32,
                y: y as i32,
                graphic: graphic as u32,
                hue: None,
            },
            _ => unknown(),
        },
        "tilepichue" => match (at(0), at(1), at(2), at(3)) {
            (Some(x), Some(y), Some(graphic), Some(hue)) => Element::Item {
                x: x as i32,
                y: y as i32,
                graphic: graphic as u32,
                hue: Some(hue as u32),
            },
            _ => unknown(),
        },
        "tilepicfit" => match (at(0), at(1), at(2), at(3), at(4), at(5)) {
            (Some(x), Some(y), Some(width), Some(height), Some(graphic), Some(hue)) => Element::ItemFitted {
                x: x as i32,
                y: y as i32,
                width: width as i32,
                height: height as i32,
                graphic: graphic as u32,
                hue: (hue != 0).then_some(hue as u32),
            },
            _ => unknown(),
        },
        "textentry" => match (at(0), at(1), at(2), at(3), at(4), at(5), at(6)) {
            (Some(x), Some(y), Some(width), Some(height), Some(hue), Some(id), Some(line)) if line >= 0 => {
                Element::TextEntry {
                    x: x as i32,
                    y: y as i32,
                    width: width as i32,
                    height: height as i32,
                    hue: hue as u32,
                    entry_id: id as u16,
                    line: line as usize,
                }
            }
            _ => unknown(),
        },
        _ => unknown(),
    }
}

/// The six arguments a checkbox and a radio button share.
fn switch(args: &[i64]) -> Option<Switch> {
    let &[x, y, off, on, initial, id, ..] = args else {
        return None;
    };
    Some(Switch {
        x: x as i32,
        y: y as i32,
        off: off as u32,
        on: on as u32,
        initial: initial != 0,
        id: RawSwitchId(id as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gump::{ButtonId, GumpLayout, SwitchId};
    use crate::wire::{Graphic, Hue};

    /// The one test that matters, and the reason the parser lives beside the
    /// builder: what this engine *sends* is what it must be able to read. A
    /// keyword renamed on one side and not the other fails here rather than as
    /// an empty window in a client nobody is watching.
    #[test]
    fn everything_the_builder_writes_reads_back_as_itself() {
        let mut layout = GumpLayout::new();
        layout.no_move();
        layout.page(0);
        layout.background(0, 0, 300, 270, 5054);
        layout.label(105, 14, 2100, "Admin");
        layout.button(30, 54, 4005, 4007, GumpButton::Reply, 0, ButtonId(13));
        layout.button(10, 10, 250, 251, GumpButton::Page, 2, ButtonId::UNUSED);
        layout.check(20, 20, 210, 211, true, SwitchId(7));
        layout.radio(20, 40, 208, 209, false, SwitchId(8));
        layout.image(4, 4, 1417);
        layout.image_hued(4, 40, 1417, 1153);
        layout.image_tiled(0, 0, 10, 10, 2624);
        layout.alpha_region(1, 2, 3, 4);
        layout.cropped_label(66, 56, 200, 20, 1153, "Populate");
        layout.item(50, 50, Graphic(0x0EED), Hue::NONE);
        layout.item(50, 70, Graphic(0x0EED), Hue(33));
        layout.fitted_item(80, 50, 24, 18, Graphic(0x0EED), Hue::NONE);
        layout.text_entry(60, 60, 100, 20, 1153, 3, "name");

        let (string, _) = layout.finish();
        assert_eq!(
            parse(string),
            vec![
                Element::Flag(Flag::NoMove),
                Element::Page(0),
                Element::Background {
                    x: 0,
                    y: 0,
                    width: 300,
                    height: 270,
                    gump: 5054,
                },
                Element::Label {
                    x: 105,
                    y: 14,
                    hue: 2100,
                    line: 0,
                },
                Element::Button {
                    x: 30,
                    y: 54,
                    normal: 4005,
                    pressed: 4007,
                    kind: GumpButton::Reply,
                    page: 0,
                    id: RawButtonId(13),
                },
                Element::Button {
                    x: 10,
                    y: 10,
                    normal: 250,
                    pressed: 251,
                    kind: GumpButton::Page,
                    page: 2,
                    id: RawButtonId(0),
                },
                Element::Check(Switch {
                    x: 20,
                    y: 20,
                    off: 210,
                    on: 211,
                    initial: true,
                    id: RawSwitchId(7),
                }),
                Element::Radio(Switch {
                    x: 20,
                    y: 40,
                    off: 208,
                    on: 209,
                    initial: false,
                    id: RawSwitchId(8),
                }),
                Element::Image {
                    x: 4,
                    y: 4,
                    gump: 1417,
                    hue: None,
                },
                Element::Image {
                    x: 4,
                    y: 40,
                    gump: 1417,
                    hue: Some(1153),
                },
                Element::ImageTiled {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                    gump: 2624,
                },
                Element::AlphaRegion {
                    x: 1,
                    y: 2,
                    width: 3,
                    height: 4,
                },
                Element::CroppedLabel {
                    x: 66,
                    y: 56,
                    width: 200,
                    height: 20,
                    hue: 1153,
                    line: 1,
                },
                Element::Item {
                    x: 50,
                    y: 50,
                    graphic: 0x0EED,
                    hue: None,
                },
                Element::Item {
                    x: 50,
                    y: 70,
                    graphic: 0x0EED,
                    hue: Some(33),
                },
                Element::ItemFitted {
                    x: 80,
                    y: 50,
                    width: 24,
                    height: 18,
                    graphic: 0x0EED,
                    hue: None,
                },
                Element::TextEntry {
                    x: 60,
                    y: 60,
                    width: 100,
                    height: 20,
                    hue: 1153,
                    entry_id: 3,
                    line: 2,
                },
            ]
        );
    }

    /// A negative coordinate is legal — the quest frame puts an element at
    /// `x = -16` — and it is the reason the arguments are read as signed.
    #[test]
    fn a_negative_coordinate_survives() {
        assert_eq!(
            parse("{ gumppic -16 -8 1417 }"),
            vec![Element::Image {
                x: -16,
                y: -8,
                gump: 1417,
                hue: None,
            }]
        );
    }

    /// The two ways an element can be unreadable — a keyword with no picture,
    /// and a keyword with too few arguments — and neither costs the window: the
    /// elements around them still parse.
    #[test]
    fn an_unreadable_element_keeps_its_neighbours() {
        let parsed = parse("{ page 1 }{ tooltip 1042971 }{ text 1 2 3 }{ page 2 }");
        assert_eq!(
            parsed,
            vec![
                Element::Page(1),
                Element::Unknown {
                    keyword: "tooltip".to_owned(),
                },
                Element::Unknown {
                    keyword: "text".to_owned(),
                },
                Element::Page(2),
            ]
        );
    }

    /// An unterminated element ends the parse rather than looping or panicking:
    /// a truncated layout is a hostile input like any other byte off the wire.
    #[test]
    fn an_unterminated_element_ends_the_layout() {
        assert_eq!(parse("{ page 1 }{ text 1 2 3 4"), vec![Element::Page(1)]);
        assert!(parse("").is_empty());
        assert!(parse("{}").len() == 1);
    }
}
