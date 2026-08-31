//! A shop's catalogue as a component: the first window kind to own its own
//! state, lay itself out and answer its own input.
//!
//! Step 1 of `docs/window_components.md`, and the kind the plan was written
//! about — the wheel that became a map zoom over a shop scrolled to its last
//! row was this window. Two maps on `Windows` keyed by the vendor's serial,
//! `vendor_scrolls` and `vendor_amounts`, are now two fields of this struct,
//! and the difference is not tidiness: a map entry outlives the window unless
//! somebody remembers to remove it by hand (`close_window` did, in two lines
//! that are gone), and a field cannot.
//!
//! # What a shop is, on the wire
//!
//! Two packets and two directions. A `0x74` says what the vendor *sells* — its
//! stock lives in a crate of its own, which is why [`Stall::Buy`] carries a
//! container's contents beside the price list — and a `0x9E` says what it will
//! *buy*, which is a list of the player's own items. Either one opens the same
//! window over the same subject, so this pane asks for the first of the two the
//! view holds, and the layout and the answer read the *same* one — see
//! [`Stall::of`].
//!
//! Choosing rows costs nothing on the wire. The one packet this window sends is
//! the order, and the shard remains the authority on stock, money and what ends
//! up in the backpack.

use openshard_client_net::action::Outgoing;
use openshard_client_net::view::WorldView;
use openshard_client_render::gump::{
    GumpArt,
    GumpPixel,
};
use openshard_client_render::vendor;
use openshard_protocol::containers::ContainedItem;
use openshard_protocol::serial::Serial;
use openshard_protocol::vendor::{
    BuyLine,
    SellLine,
};

use crate::panes::{
    Button,
    Effect,
    Input,
    PaneCtx,
    PaneFrame,
    Response,
};
use crate::windows::Drawn;

/// One open shop window.
#[derive(Debug)]
pub struct VendorPane {
    /// The mobile this shop belongs to, which is what the order names.
    ///
    /// The one thing the pane is told when it is built rather than reads out of
    /// its context: a window's *subject* is the manager's, but which vendor a
    /// `0x3B` is addressed to is this window's own business — see
    /// [`AnyPane::of`](crate::panes::AnyPane::of).
    vendor:  Serial,
    /// The first row of the catalogue that is visible.
    ///
    /// Clamped downward as it is used rather than as it is set, the way it
    /// always was: the catalogue can restock between two notches, and a offset
    /// clamped against yesterday's row count would be clamped against a number
    /// nothing on the screen still has.
    scroll:  vendor::CatalogueIndex,
    /// How many of each row the player has chosen, in the row order the
    /// catalogue supplied.
    ///
    /// Shorter than the catalogue is ordinary and means "none chosen" for every
    /// row past its end: both readers ask with `get`, so a restock that grows
    /// the list costs nothing, and one that shrinks it leaves an entry nothing
    /// can reach — the order is built by zipping against the *lines*, never
    /// against this.
    amounts: Vec<u16>,
    /// The action plate the pointer was resting on when the last frame was
    /// drawn, or `None` for neither.
    ///
    /// Remembered for one reason: the tint on ACCEPT and CLEAR is decided in
    /// the layout from the cursor, so it is right whenever a frame is drawn —
    /// and what draws one is the animation clock, not the move that changed
    /// the answer. A pane whose picture depends on the pointer owes a
    /// [`Response::stale`] for that move, and saying it needs knowing what the
    /// picture was. The same shape as the paperdoll's hover, moved in with it.
    tint:    Option<vendor::Action>,
}

/// Which of the two catalogues this window is showing, and everything the rows
/// are read out of.
///
/// Borrowed out of the view for the length of one question. A shop is not
/// remembered by the pane for the reason no window holds a copy of what is in a
/// bag: the view is the authoritative picture and a copy is a way to draw a
/// price that is no longer offered.
enum Stall<'a> {
    /// The vendor sells. `stock` is the crate the `0x74` named, whose items are
    /// what the rows have icons and quantities from — a row is a line and the
    /// item at the same index.
    Buy {
        lines: &'a [BuyLine],
        stock: &'a [ContainedItem],
    },
    /// The vendor buys, out of what this character is carrying.
    Sell { lines: &'a [SellLine] },
}

impl<'a> Stall<'a> {
    /// The catalogue this window is showing, or `None` for a vendor the view no
    /// longer holds either list for.
    ///
    /// **Buy first, because that is what is drawn first.** A serial could in
    /// principle be in both maps — a player who asked to buy and then to sell —
    /// and the three places that used to ask this question did not agree on the
    /// answer: the frame drew the shop's stock while the Confirm button sold the
    /// player's own goods. One reader is the whole point of the pane, and the
    /// one it agrees with has to be the picture.
    fn of(view: &'a WorldView, vendor: Serial) -> Option<Self> {
        if let Some(catalogue) = view.vendor_buys.get(&vendor) {
            return Some(Self::Buy {
                lines: &catalogue.lines,
                stock: view
                    .contents
                    .get(&catalogue.container)
                    .map_or(&[] as &[ContainedItem], Vec::as_slice),
            });
        }
        let catalogue = view.vendor_sells.get(&vendor)?;
        Some(Self::Sell {
            lines: &catalogue.lines,
        })
    }

    /// How many rows the catalogue has, which is what the scroll is bounded by.
    fn rows(&self) -> usize {
        match self {
            Self::Buy { lines, .. } => lines.len(),
            Self::Sell { lines } => lines.len(),
        }
    }

    /// The most of one row that can be chosen: the shop's stock of it, or how
    /// many the player is carrying. Zero for a row the other half of the
    /// catalogue has no entry for, which makes the next click on it choose
    /// nothing — the same answer the shard would give.
    fn limit(&self, row: vendor::CatalogueIndex) -> u16 {
        let row = row.position();
        match self {
            Self::Buy { stock, .. } => stock.get(row).map_or(0, |item| item.amount.0),
            Self::Sell { lines } => lines.get(row).map_or(0, |line| line.amount.0),
        }
    }

    /// The one packet this window sends: what was chosen, by serial.
    ///
    /// Zipped against the catalogue rather than against `amounts`, so a chosen
    /// quantity whose row has since gone simply does not travel.
    fn order(&self, vendor: Serial, amounts: &[u16]) -> Outgoing {
        match self {
            Self::Buy { stock, .. } => {
                Outgoing::Buy {
                    vendor,
                    purchases: stock
                        .iter()
                        .zip(amounts)
                        .filter_map(|(item, amount)| (*amount > 0).then_some((item.serial, *amount)))
                        .collect(),
                }
            }
            Self::Sell { lines } => {
                Outgoing::Sell {
                    vendor,
                    sales: lines
                        .iter()
                        .zip(amounts)
                        .filter_map(|(line, amount)| (*amount > 0).then_some((line.serial, *amount)))
                        .collect(),
                }
            }
        }
    }
}

impl VendorPane {
    /// A shop window as it opens: nothing chosen and the top of the list.
    pub const fn new(vendor: Serial) -> Self {
        Self {
            vendor,
            scroll: vendor::CatalogueIndex::new(0),
            amounts: Vec::new(),
            tint: None,
        }
    }

    /// Keep [`VendorPane::tint`] honest against the pointer, and ask for a
    /// frame exactly when the answer changed.
    ///
    /// The predicate is the layout's own — [`vendor::Window::action_at`] on
    /// the window as it was drawn — so what this remembers and what the next
    /// frame tints cannot disagree. Deliberately not gated on
    /// [`PaneCtx::under_pointer`]: the layout is not either, so a plate under
    /// a covering window still tints, and a memory that said otherwise would
    /// ask for no frame while the picture changed.
    fn hover(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let over = match ctx.drawn {
            Some(Drawn::Vendor(window)) => window.action_at(ctx.frame.cursor),
            _ => None,
        };
        if self.tint == over {
            return Response::ignored();
        }
        self.tint = over;
        Response::stale()
    }

    /// One notch over the catalogue: move the viewport a row, and say what that
    /// was worth.
    ///
    /// **The notch is taken either way, and that is the whole defect.** A
    /// catalogue already at either end still swallows the wheel, the way every
    /// list in every client does — so the answer is [`Response::consumed`] and
    /// not [`Response::ignored`]. Handing the notch on because nothing moved is
    /// what turned a wheel over a shop into a zoom of the map behind it, and it
    /// is why a pane answers two questions instead of one `bool`.
    fn wheel(&mut self, notches: f32, rows: usize) -> Response {
        let was = self.scroll;
        let position = self.scroll.position();
        self.scroll = vendor::CatalogueIndex::new(if notches > 0.0 {
            position.saturating_sub(1)
        } else {
            (position + 1).min(rows.saturating_sub(vendor::VISIBLE_ROWS))
        });
        if self.scroll == was {
            Response::consumed()
        } else {
            Response::changed()
        }
    }

    /// A notch anywhere over the window, once [`AnyPane::handle`](crate::panes::AnyPane::handle) has already
    /// established the pointer is on this window at all.
    ///
    /// **Closes the plan's Backlog entry that pitted this pane against
    /// [`SkillsPane`](crate::panes::skills::SkillsPane): the whole frame is
    /// this window's, matching ClassicUO, not only the catalogue's own
    /// viewport.** `over_catalogue` decides whether there is a list here for
    /// the notch to move — [`vendor::Window::catalogue_contains`] is what the
    /// caller answers it with — but `taken` no longer waits on that answer: a
    /// notch over a button or a plate is [`Response::consumed`] rather than
    /// falling through to the camera behind the window.
    fn wheel_over_window(&mut self, notches: f32, over_catalogue: bool, rows: usize) -> Response {
        if !over_catalogue {
            return Response::consumed();
        }
        self.wheel(notches, rows)
    }

    /// One more of this row, or back to none once the whole of it is chosen.
    ///
    /// A click and not a spinner: the reference client's own catalogue counts up
    /// by one per press and wraps at what is available, which is the only
    /// gesture a row has.
    fn choose(&mut self, row: vendor::CatalogueIndex, limit: u16) {
        let row = row.position();
        if self.amounts.len() <= row {
            self.amounts.resize(row + 1, 0);
        }
        self.amounts[row] = if self.amounts[row] >= limit {
            0
        } else {
            self.amounts[row] + 1
        };
    }

    /// One fewer of a row already in the order panel.
    fn take_back(&mut self, row: vendor::CatalogueIndex) {
        if let Some(amount) = self.amounts.get_mut(row.position()) {
            *amount = amount.saturating_sub(1);
        }
    }

    /// A left press somewhere in this window, against the layout it was drawn
    /// as.
    ///
    /// Every arm raises: a press on a window is what puts it on top, and the
    /// action it landed on does not change that. The press that landed on
    /// neither a row nor a button is the one that moves the window — the art's
    /// header and its empty parchment are deliberately not controls — and that
    /// is [`Effect::Grab`], not a position this pane writes.
    fn press(&mut self, window: &vendor::Window, ctx: &PaneCtx<'_>) -> Response {
        let raised = Response::changed().with(Effect::Raise);
        // Asked per arm rather than once at the top, because the catalogue can
        // go out of the view between the frame this window was drawn on and
        // this press, and the four controls do not mean the same thing without
        // it: a row with no stock chooses nothing, and the order panel's own
        // two still empty and close a window whose shop has gone.
        let stall = Stall::of(ctx.frame.view, self.vendor);
        match window.hit(ctx.frame.cursor) {
            Some(vendor::Hit::Row(row)) => {
                self.choose(row, stall.map_or(0, |stall| stall.limit(row)));
                raised
            }
            Some(vendor::Hit::Remove(row)) => {
                self.take_back(row);
                raised
            }
            Some(vendor::Hit::Clear) => {
                self.amounts.fill(0);
                raised
            }
            // The order, and the window with it: the shard answers a `0x3B`
            // with the goods and a fresh container, not with the shop again.
            Some(vendor::Hit::Confirm) => {
                let answer = match stall {
                    Some(stall) => raised.with(Effect::Net(stall.order(self.vendor, &self.amounts))),
                    None => raised,
                };
                answer.with(Effect::Close)
            }
            // `ctx.frame.cursor` is already this window's own local pixels, so
            // the grab offset — how far into the window the press landed — is
            // that value directly, with no subtraction to do.
            None => raised.with(Effect::Grab),
        }
    }
}

impl VendorPane {
    /// The two parchment panels the window is drawn on, both halves of both
    /// catalogues.
    ///
    /// The row icons are not here: they are items, and every picture of every
    /// window is offered to the atlas once the frame has been laid out — see
    /// the sweep at the end of `render_passes::draw_gump_windows`. Naming them
    /// here as well would be a second answer to "what is in this shop", worked
    /// out before the layout that decides which four rows are showing.
    pub(super) fn art(&self, _frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        vendor::art_of().collect()
    }

    pub(super) fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let stall = Stall::of(frame.view, self.vendor)?;
        // Window-local: `at` is the origin, exactly as if the manager had put
        // this window at `(0, 0)` — see `PaneFrame::cursor`'s doc. The draw
        // pass and the pointer pick are the two places that add this
        // window's real, absolute position back.
        let window = match stall {
            Stall::Buy { lines, stock } => {
                vendor::buy(
                    self.vendor,
                    lines,
                    stock,
                    &self.amounts,
                    self.scroll,
                    GumpPixel::new(0, 0),
                    frame.cursor,
                    frame.files.gump_atlas,
                )
            }
            Stall::Sell { lines } => {
                vendor::sell(
                    self.vendor,
                    lines,
                    &self.amounts,
                    self.scroll,
                    GumpPixel::new(0, 0),
                    frame.cursor,
                    frame.files.gump_atlas,
                )
            }
        };
        Some(Drawn::Vendor(window))
    }

    /// A notch anywhere over the window, and a left press anywhere in it.
    ///
    /// Both are located, so both begin by asking whether this window is the one
    /// the pointer is on — see [`PaneCtx::under_pointer`], and note that a
    /// window *below* one that is being pointed at is never offered either of
    /// them at all. Neither is answered against a layout this pane works out
    /// now: [`PaneCtx::drawn`] is the picture the player is pointing at.
    ///
    /// Everything else is somebody's: the right button is the manager's close,
    /// and a release finishes a gesture this window does not have. A move is
    /// the tint's — offered to every window, ahead of the located gate,
    /// because the tint follows the raw cursor the way the layout reads it.
    ///
    /// **A notch anywhere over the frame is this window's**, matching
    /// [`SkillsPane`](crate::panes::skills::SkillsPane) and ClassicUO — see
    /// `wheel_over_window`.
    pub(super) fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        if let Input::Move = input {
            return self.hover(ctx);
        }
        if !ctx.under_pointer {
            return Response::ignored();
        }
        let Some(Drawn::Vendor(window)) = ctx.drawn else {
            // Never drawn, so there is nothing to hit-test against and no
            // pixels the player can have meant. The window is still raised and
            // moved by the manager's own path.
            return Response::ignored();
        };
        match input {
            // Anywhere over the window's frame, not only the catalogue's own
            // viewport — matching the skill sheet's whole-frame claim and
            // ClassicUO's own behaviour. See `wheel_over_window`.
            Input::Wheel(notches) => {
                let rows = Stall::of(ctx.frame.view, self.vendor).map_or(0, |stall| stall.rows());
                self.wheel_over_window(notches, window.catalogue_contains(ctx.frame.cursor), rows)
            }
            Input::Press(Button::Left) => self.press(window, ctx),
            // A keystroke among them for the skill sheet's reason: a shop has
            // nothing to type into, so it never holds the keyboard.
            Input::Press(Button::Right)
            | Input::Release(_)
            | Input::Move
            | Input::Key(_)
            | Input::Answered(_) => Response::ignored(),
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_client_net::view::VendorSell;
    use openshard_protocol::items::ItemAmount;
    use openshard_protocol::wire::{
        Graphic,
        Hue,
    };

    use super::*;
    use crate::panes::fixture;

    fn pane() -> VendorPane {
        VendorPane::new(Serial::new(0x0000_002A).unwrap())
    }

    const fn catalogue(position: usize) -> vendor::CatalogueIndex {
        vendor::CatalogueIndex::new(position)
    }

    /// A catalogue of `rows` lines the shard is willing to buy, in a world with
    /// nothing else in it.
    fn shop(rows: usize) -> WorldView {
        let vendor = Serial::new(0x0000_002A).unwrap();
        let mut view = fixture::world(Serial::new(0x0000_0001).unwrap());
        view.vendor_sells.insert(
            vendor,
            VendorSell {
                lines: (0..rows)
                    .map(|row| {
                        SellLine {
                            // Serials from a range of their own, so a row's identity
                            // cannot be confused with the vendor's or the player's.
                            serial:  Serial::new(0x4000_0001 + row as u32).unwrap(),
                            graphic: Graphic(0x0EED),
                            hue:     Hue::NONE,
                            amount:  ItemAmount(9),
                            price:   3,
                            name:    format!("row {row}"),
                        }
                    })
                    .collect(),
            },
        );
        view
    }

    /// The two parchment panels a shop is drawn on, at whatever size — the
    /// window's own arithmetic is in boxes, so what matters is that the atlas
    /// has an entry for each and not how big it is.
    fn install() -> fixture::Install {
        fixture::Install::shipping(vendor::art_of().map(|art| (art, (256, 400))))
    }

    /// **Step 8, and the defect this plan grew out of, asserted through
    /// [`AnyPane::handle`](crate::panes::AnyPane::handle).**
    ///
    /// The three tests below it ask `VendorPane::wheel` directly, which pins the
    /// *rule*; this one goes in the front door — the located gate, the `drawn`
    /// lookup, the catalogue-or-frame split and the rule behind them — and so it
    /// is the one that proves the pointer tests in front of the rule agree with
    /// it. That was the last thing blocking this step, and what unblocked it is
    /// [`PaneFiles`](crate::panes::PaneFiles): the context used to need a whole
    /// `Resources`, which needs a client install on disk.
    #[test]
    fn a_notch_through_handle_is_taken_at_the_last_row_and_asks_for_no_frame() {
        let files = install();
        let view = shop(9);
        let mut pane = pane();
        // Inside the catalogue's viewport, which is what makes this a notch the
        // list itself could move — asserted below rather than assumed, because
        // a cursor that had missed the viewport would take the notch for the
        // *frame* and pass this test for the wrong reason.
        let cursor = GumpPixel::new(100, 70);

        for step in 0..5 {
            let laid_out = pane
                .layout(&files.ctx(&view, None, cursor, true).frame)
                .expect("a shop with a catalogue in the view lays itself out");
            let Drawn::Vendor(window) = &laid_out else {
                panic!("a shop lays itself out as a shop");
            };
            assert!(
                window.catalogue_contains(cursor),
                "the cursor is over the list, not the buttons"
            );
            let ctx = files.ctx(&view, Some(&laid_out), cursor, true);
            let answer = pane.handle(Input::Wheel(-1.0), &ctx);
            assert!(answer.taken, "notch {step} is the window's");
            assert!(answer.redraw, "notch {step} moved the list");
        }
        assert_eq!(pane.scroll, catalogue(5));

        let laid_out = pane
            .layout(&files.ctx(&view, None, cursor, true).frame)
            .expect("a shop with a catalogue in the view lays itself out");
        let ctx = files.ctx(&view, Some(&laid_out), cursor, true);
        let answer = pane.handle(Input::Wheel(-1.0), &ctx);
        assert!(
            answer.taken,
            "the notch is still the window's at the last row — the camera never hears it"
        );
        assert!(!answer.redraw, "and there is nothing new to draw");
        assert_eq!(pane.scroll, catalogue(5));
    }

    /// The gate in front of the rule, asserted the same way: a notch offered to
    /// a window the pointer is *not* on is not that window's, however scrollable
    /// its list is.
    ///
    /// This is the half a test of `wheel` alone cannot see, and the half that
    /// decides whether a bag drawn over a shop can be scrolled through.
    #[test]
    fn a_notch_on_a_window_below_the_pointer_is_not_taken() {
        let files = install();
        let view = shop(9);
        let mut pane = pane();
        let cursor = GumpPixel::new(100, 70);

        let laid_out = pane
            .layout(&files.ctx(&view, None, cursor, true).frame)
            .expect("a shop with a catalogue in the view lays itself out");
        let ctx = files.ctx(&view, Some(&laid_out), cursor, false);
        let answer = pane.handle(Input::Wheel(-1.0), &ctx);
        assert!(!answer.taken, "a covered window does not answer a notch");
        assert!(!answer.redraw);
        assert_eq!(pane.scroll, catalogue(0), "and its list did not move");
    }

    /// A window that has never been drawn has no pixels the player can have
    /// meant, so a press on it is nobody's — the arm that would otherwise
    /// hit-test a layout worked out at press time, which is the second answer
    /// [`PaneCtx::drawn`](crate::panes::PaneCtx::drawn) exists to prevent.
    #[test]
    fn a_press_on_a_window_that_was_never_drawn_is_ignored() {
        let files = install();
        let view = shop(9);
        let mut pane = pane();
        let ctx = files.ctx(&view, None, GumpPixel::new(100, 70), true);
        let answer = pane.handle(Input::Press(Button::Left), &ctx);
        assert!(!answer.taken);
        assert!(!answer.redraw);
    }

    /// **The defect this plan grew out of, as an assertion.** A catalogue at
    /// its last row has nothing new to draw and the notch is still its own.
    ///
    /// The old chain answered both questions with one `bool`, so the shop said
    /// "nothing moved", the `||` fell through, and the wheel zoomed the map
    /// under a pointer that had never left the window.
    #[test]
    fn a_catalogue_at_its_last_row_takes_the_notch_and_draws_nothing() {
        let mut pane = pane();
        // Nine rows, four visible: five notches reach the bottom and the sixth
        // has nowhere to go.
        for step in 0..5 {
            let answer = pane.wheel(-1.0, 9);
            assert!(answer.taken, "notch {step} is the window's");
            assert!(answer.redraw, "notch {step} moved the list");
        }
        assert_eq!(pane.scroll, catalogue(5));

        let answer = pane.wheel(-1.0, 9);
        assert!(answer.taken, "the notch is still the window's at the last row");
        assert!(!answer.redraw, "and there is nothing new to draw");
        assert_eq!(pane.scroll, catalogue(5));
    }

    /// The top is the other end of the same rule.
    #[test]
    fn a_catalogue_at_its_first_row_takes_the_notch_too() {
        let mut pane = pane();
        let answer = pane.wheel(1.0, 9);
        assert!(answer.taken, "already at the top, and still the window's");
        assert!(!answer.redraw);
        assert_eq!(pane.scroll, catalogue(0));

        assert!(pane.wheel(-1.0, 9).redraw);
        assert!(pane.wheel(1.0, 9).redraw);
        assert_eq!(pane.scroll, catalogue(0));
    }

    /// **The Backlog's decision, as an assertion.** A notch over a button or a
    /// plate — anywhere in the window that is not the catalogue's own
    /// viewport — is still this window's, matching [`SkillsPane`]'s whole-frame
    /// claim and ClassicUO. Before this, `catalogue_contains` gated `taken`
    /// itself, and a notch over the CLEAR or ACCEPT plates reached the camera
    /// behind the shop.
    #[test]
    fn a_notch_off_the_catalogue_is_still_the_windows_and_does_not_scroll() {
        let mut pane = pane();
        let answer = pane.wheel_over_window(-1.0, false, 9);
        assert!(answer.taken, "the whole frame is the window's, not only the list");
        assert!(!answer.redraw, "there is no list here for it to move");
        assert_eq!(pane.scroll, catalogue(0), "the catalogue itself did not move");
    }

    /// The same notch over the catalogue's own viewport still scrolls, so the
    /// widened claim above did not cost the list its own behaviour.
    #[test]
    fn a_notch_on_the_catalogue_still_scrolls_it() {
        let mut pane = pane();
        let answer = pane.wheel_over_window(-1.0, true, 9);
        assert!(answer.taken);
        assert!(answer.redraw);
        assert_eq!(pane.scroll, catalogue(1));
    }

    /// A catalogue that fits in its viewport has no scrolling to do, and the
    /// notch is *still* not the camera's.
    #[test]
    fn a_short_catalogue_does_not_scroll_and_does_not_let_the_notch_past() {
        let mut pane = pane();
        let answer = pane.wheel(-1.0, vendor::VISIBLE_ROWS);
        assert!(answer.taken);
        assert!(!answer.redraw);
        assert_eq!(pane.scroll, catalogue(0));
    }

    /// Choosing counts up by one and wraps at what is available, and it does so
    /// for a row past the end of the list it has chosen so far.
    #[test]
    fn a_row_counts_up_to_its_limit_and_then_back_to_none() {
        let mut pane = pane();
        pane.choose(catalogue(2), 2);
        assert_eq!(pane.amounts, vec![0, 0, 1], "the rows before it are untouched");
        pane.choose(catalogue(2), 2);
        assert_eq!(pane.amounts[2], 2);
        pane.choose(catalogue(2), 2);
        assert_eq!(pane.amounts[2], 0, "the whole of it was chosen, so none of it is");
    }

    /// A row with nothing behind it cannot be chosen — an empty shelf, or a
    /// line the other half of the catalogue has no item for.
    #[test]
    fn a_row_with_no_stock_chooses_nothing() {
        let mut pane = pane();
        pane.choose(catalogue(0), 0);
        assert_eq!(pane.amounts, vec![0]);
    }

    /// The order panel's click takes one back, and stops at none rather than
    /// wrapping the way the row does.
    #[test]
    fn taking_back_stops_at_none() {
        let mut pane = pane();
        pane.choose(catalogue(0), 5);
        pane.choose(catalogue(0), 5);
        pane.take_back(catalogue(0));
        assert_eq!(pane.amounts[0], 1);
        pane.take_back(catalogue(0));
        pane.take_back(catalogue(0));
        assert_eq!(pane.amounts[0], 0);
        pane.take_back(catalogue(7));
        assert_eq!(
            pane.amounts,
            vec![0],
            "a row nothing was chosen for is not created"
        );
    }

    /// Only what was chosen travels, and it travels named by the *catalogue's*
    /// serials rather than by where it sat in this pane's list.
    #[test]
    fn the_order_carries_the_chosen_rows_and_nothing_else() {
        let vendor = Serial::new(0x0000_002A).unwrap();
        let lines = [
            SellLine {
                serial:  Serial::new(0x0000_0100).unwrap(),
                graphic: openshard_protocol::wire::Graphic(0x0EED),
                hue:     openshard_protocol::wire::Hue::NONE,
                amount:  openshard_protocol::items::ItemAmount(9),
                price:   3,
                name:    "gold".into(),
            },
            SellLine {
                serial:  Serial::new(0x0000_0200).unwrap(),
                graphic: openshard_protocol::wire::Graphic(0x0EED),
                hue:     openshard_protocol::wire::Hue::NONE,
                amount:  openshard_protocol::items::ItemAmount(9),
                price:   4,
                name:    "silver".into(),
            },
        ];
        let stall = Stall::Sell { lines: &lines };
        let Outgoing::Sell { vendor: named, sales } = stall.order(vendor, &[0, 2]) else {
            panic!("a sell catalogue answers with a sale");
        };
        assert_eq!(named, vendor);
        assert_eq!(sales, vec![(Serial::new(0x0000_0200).unwrap(), 2)]);
    }

    /// An amount left over from a catalogue that has since shrunk does not
    /// travel: the zip is against the lines.
    #[test]
    fn a_chosen_row_that_no_longer_exists_does_not_travel() {
        let vendor = Serial::new(0x0000_002A).unwrap();
        let stall = Stall::Sell { lines: &[] };
        let Outgoing::Sell { sales, .. } = stall.order(vendor, &[3, 4]) else {
            panic!("a sell catalogue answers with a sale");
        };
        assert!(sales.is_empty());
    }
}
