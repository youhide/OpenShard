//! The skill window: a scroll of headings, the skills under them, and a bar to
//! scroll it by.
//!
//! The second window in this client that is *not* a layout the shard sent — the
//! first being the paperdoll. Nothing about it comes off the wire but the
//! numbers: what the skills are called and which heading each one is filed under
//! are the client's own files ([`openshard_uofiles::skills`] and
//! [`openshard_uofiles::skillgrp`]), the pictures are `gumpartLegacyMUL.uop`,
//! and where any of it goes is here.
//!
//! # The tree, and the two id spaces in it
//!
//! A heading and the skills under it are two different id spaces
//! ([`GroupId`] and [`SkillId`]) and both are small integers, so they are both
//! newtypes and neither is ever an index into the other's table. [`Row`] is what
//! the window is a list of, and it is those two and nothing else: a heading is a
//! row, a skill is a row, and whether a skill's row exists at all is whether its
//! heading is open.
//!
//! # Where the reference's window and this one part company
//!
//! Ported from `StandardSkillsGump` — its frame (`ExpandableScroll` over
//! `0x1F40`), its scrollbar (`ScrollBar`, art `0x00FA`–`0x0101`), its row
//! metrics and its hues. What is deliberately *not* ported, because it is a
//! window's furniture rather than a skill list: the minimise box, the drag
//! handle that resizes the scroll, the checkboxes that swap the value column for
//! the base or the cap, the button that makes a group of your own, and dragging
//! a skill out of the window to leave a button on the desktop. Each is a
//! separate gesture with its own state, and none of them is how a player reads
//! their skills.
//!
//! The reference also opens this window when a `0x3A` of type `0x01` or `0x03`
//! arrives. This client opens it when the player asks — see `docs/client/design_windows.md`,
//! M4 — because the shard sends the whole list at world entry too, and a window
//! that opened on the packet would open itself at every login.

use std::collections::{
    BTreeMap,
    BTreeSet,
};

use openshard_client_model::Skill as SkillLine;
use openshard_protocol::skill::SkillLock;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_uofiles::skillgrp::{
    GroupId,
    SkillGroups,
};
use openshard_uofiles::skills::{
    Skill,
    SkillId,
    Skills,
};

use crate::gump::{
    GumpArt,
    GumpPixel,
    Picture,
    PictureIndex,
    Scissor,
};
use crate::text::GumpLabel;

/// One line of a skill's standing, plus the local lock face drawn over it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Standing {
    /// The wire-derived values, with its id deliberately held by the caller's map.
    pub skill: SkillLine,
    /// Which way the shard is training it.
    pub lock:  SkillLock,
}

/// A row of the window: a heading, or a skill under one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Row {
    /// A heading out of `skillgrp.mul`, open or shut.
    Heading(GroupId),
    /// A skill filed under the heading above it.
    Skill(SkillId),
}

/// What the player has done to the window, which no packet carries: which
/// headings are shut, and how far down the list is scrolled.
///
/// Held by the caller and handed back to [`window`] every frame, the way
/// `Dialogs` holds a dialog's page and switches. Not on the window itself: a
/// window is laid out afresh each frame from the view, and state that lived
/// there would be forgotten the moment a skill changed.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Tree {
    /// The headings that are shut. Absent means open, so a fresh tree shows
    /// everything — which is what a window nobody has touched should do, and it
    /// means a heading the files gain later needs no entry to appear.
    shut:   BTreeSet<GroupId>,
    /// How far down the content the viewport's top edge sits, in gump pixels.
    offset: i32,
    /// Locks the player has clicked since the window last opened, held over
    /// whatever the shard's own line says.
    ///
    /// The one deliberate exception to "wait for the shard's answer", and not
    /// this client's invention: ServUO's own skill window redraws the arrow
    /// the instant it is clicked and never listens for a reply, which is why
    /// `World::set_skill_lock` sends nothing back (see that function's doc in
    /// `crates/server/world/src/tick/skills_wire.rs`). A window that waited
    /// here would show the old face until the skill happened to change some
    /// other way. Lives exactly where `shut`/`offset` do — state the window
    /// has and no packet carries — and is thrown away with the rest of the
    /// tree when the window closes.
    locked: BTreeMap<SkillId, SkillLock>,
}

impl Tree {
    /// Whether a heading is shut.
    #[must_use]
    pub fn is_shut(&self, group: GroupId) -> bool {
        self.shut.contains(&group)
    }

    /// Open a shut heading, or shut an open one.
    pub fn toggle(&mut self, group: GroupId) {
        if !self.shut.remove(&group) {
            self.shut.insert(group);
        }
    }

    /// How far down the list is scrolled, in gump pixels.
    #[must_use]
    pub fn offset(&self) -> i32 {
        self.offset
    }

    /// Scroll to a point, clamped to what there is to scroll.
    ///
    /// `content` is what [`content_height`] measured: the clamp is *here*, at
    /// the one place the offset changes, rather than in the layout — a layout
    /// that silently drew a clamped offset while the tree held an unclamped one
    /// would put the thumb where the rows are not.
    pub fn scroll_to(&mut self, offset: i32, content: i32) {
        self.offset = offset.clamp(0, (content - VIEWPORT_HEIGHT).max(0));
    }

    /// Scroll by a step — a wheel notch, an arrow at the end of the bar.
    pub fn scroll_by(&mut self, delta: i32, content: i32) {
        self.scroll_to(self.offset + delta, content);
    }

    /// A skill's lock, as the window should draw it: the player's own click if
    /// there has been one since the window opened, or `shard` — the shard's
    /// own line — otherwise.
    #[must_use]
    pub fn lock_of(&self, id: SkillId, shard: SkillLock) -> SkillLock {
        self.locked.get(&id).copied().unwrap_or(shard)
    }

    /// Record a click on a skill's lock arrow.
    pub fn set_lock(&mut self, id: SkillId, lock: SkillLock) {
        self.locked.insert(id, lock);
    }
}

/// What one hit on the window means.
///
/// The same shape `paperdoll::DollButton` has and for the same reason:
/// [`crate::gump::pick`] answers an index into the picture list, and this is
/// what turns one into a meaning. Dragging a skill out of the window to leave
/// a button on the desktop is absent on purpose; see the module docs.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Hit {
    /// The arrow beside a heading: open it, or shut it.
    Heading(GroupId),
    /// The arrow at the top of the bar: one row up.
    Up,
    /// The arrow at the bottom: one row down.
    Down,
    /// The thumb itself, which is dragged.
    Thumb,
    /// The bar behind the thumb, which jumps it here.
    Track,
    /// A skill's lock arrow: cycle Up → Down → Locked → Up.
    Lock(SkillId),
    /// A skill's own use button — drawn only for a skill the files mark
    /// [`has_action`](openshard_uofiles::skills::Skill::has_action).
    Use(SkillId),
}

/// A line of text the window writes, already placed.
///
/// Owned rather than borrowed, unlike `paperdoll::title`: a value is *formatted*
/// — 755 becomes "75.5" — so there is no string in the view to point at.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    /// Its top-left corner, in the window's own pixels.
    pub at:      GumpPixel,
    /// The text.
    pub text:    String,
    /// Which face.
    pub font:    Font,
    /// The wire hue to draw it in.
    pub hue:     Hue,
    /// The box it is cut to, or `None` for a line that is part of the frame.
    ///
    /// The same field [`Picture::scissor`] is and for the same reason, arrived
    /// at the same way — by looking at the window. Cutting *every* line to the
    /// viewport is what the caller did first, and the total under the list
    /// vanished: it is written on the frame, below the rule the list stops at,
    /// so the box that keeps a row inside the list is exactly the box that keeps
    /// the total off the screen.
    pub scissor: Option<Scissor>,
}

impl Line {
    /// The label this line is drawn as.
    #[must_use]
    pub fn label(&self) -> GumpLabel<'_> {
        GumpLabel {
            at:   self.at,
            text: &self.text,
            font: self.font,
            hue:  self.hue,
            clip: None,
        }
    }
}

/// A skill window, laid out.
///
/// No `Default`: a sheet without a viewport is not an empty window, it is a
/// window whose rows are cut to a box at the origin. One is always laid out by
/// [`window`], which is where the box comes from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Sheet {
    /// The art, in painter's order: the frame, then the rows, then the bar.
    pub pictures: Vec<Picture>,
    /// Which of those pictures means something, by its index.
    pub hits:     BTreeMap<PictureIndex, Hit>,
    /// Everything written on it.
    pub lines:    Vec<Line>,
    /// The box the rows are cut to.
    ///
    /// Every picture and every line in the list carries it already; it is on the
    /// sheet as well because a caller wants it in one place — to hit-test
    /// against, and to say what a wheel over the window is over.
    pub viewport: Scissor,
    /// Where the scrollbar is, so that a drag can be turned back into an offset
    /// — see [`Sheet::offset_at`].
    bar:          Scissor,
}

// -- the frame ------------------------------------------------------------
//
// `ExpandableScroll`'s four pieces, whose sizes are the file's and are written
// down here because the layout is arithmetic over them: 0x1F40 is 345x37, its
// `+1` and `+2` are both 302x70, and `+3` is 314x34. Checked with
// `cargo run -p openshard-uofiles --example gump_probe`.

/// The top of the scroll, and the picture the window's width comes from.
const SCROLL_TOP: Graphic = Graphic(0x1F40);

/// The body, in two layers the reference draws one over the other: `+1` is the
/// darker edge and `+2` the parchment. Both are tiled down the middle of the
/// window, at the same box, which is what `ExpandableScroll` does.
const SCROLL_EDGE: Graphic = Graphic(0x1F41);

/// The parchment, over the edge.
const SCROLL_BODY: Graphic = Graphic(0x1F42);

/// The bottom, which is narrower than the top and is nudged right by the
/// difference — `off / 2 + off / 4` in the reference, and not `off / 2`.
const SCROLL_BOTTOM: Graphic = Graphic(0x1F43);

/// The title plate: `TitleGumpID = 0x0834`, centred on the top piece.
const TITLE: Graphic = Graphic(0x0834);

/// The rule the reference draws under the title and again above the total.
const RULE: Graphic = Graphic(0x082B);

/// The instruction the reference prints across the bottom of the scroll, and
/// which the total is written to the right of.
///
/// `new GumpPic(25, Height - 85, 0x0836, 0)` — `_bottomComment`. **Not a
/// plate**, which is what this constant used to be called: the graphic is
/// 210×19 pixels of the sentence "Left-click the button before a skill to use
/// the skill. / Skills without buttons are accessed in the world." Nothing is
/// written *on* it and nothing can be — it is already text.
///
/// The wrong name cost a real defect elsewhere. `container.rs` reused this
/// graphic as a blank background for its own captions on the strength of that
/// name and of its size, so every bag in the client drew that sentence under
/// itself. Both readers now name the art for what the artist drew.
const BOTTOM_COMMENT: Graphic = Graphic(0x0836);

/// The arrow beside an open heading — `IsMinimized`'s `value ? 0x0827 : 0x826`.
const HEADING_OPEN: Graphic = Graphic(0x0827);

/// The one beside a shut heading.
const HEADING_SHUT: Graphic = Graphic(0x0826);

/// The strip the reference rules along a heading, from the end of its name to
/// the right-hand edge.
const HEADING_RULE: Graphic = Graphic(0x0835);

/// The three faces of a skill's lock: up, down, and held.
///
/// `GetStatusButtonGraphic`.
const LOCK_UP: Graphic = Graphic(0x0984);
/// Trained down.
const LOCK_DOWN: Graphic = Graphic(0x0986);
/// Held where it is.
const LOCK_HELD: Graphic = Graphic(0x082C);

/// A skill's own use button, drawn only for one the files mark
/// [`has_action`](openshard_uofiles::skills::Skill::has_action).
///
/// `new Button(0, 0x0837, 0x0838, 0x0838)` — normal only: this window draws
/// no pressed art for any of its furniture yet, the arrows and the thumb
/// included, so the reference's `0x0838` pressed face is not ported here
/// either.
const USE_BUTTON: Graphic = Graphic(0x0837);

/// How wide the window is: the top piece's own width.
pub const WIDTH: i32 = 345;

/// How tall. A fixed number, because the drag handle that resizes the
/// reference's scroll is not ported.
pub const HEIGHT: i32 = 320;

/// How tall the top piece is, and where the body starts.
const TOP_HEIGHT: i32 = 37;

/// How tall the bottom piece is.
const BOTTOM_HEIGHT: i32 = 34;

/// How wide the two tiled body pieces are.
const BODY_WIDTH: i32 = 302;

/// How wide the bottom piece is.
const BOTTOM_WIDTH: i32 = 314;

/// Where the rule above the total sits, which is also where the list stops.
const RULE_BOTTOM_Y: i32 = HEIGHT - BOTTOM_HEIGHT - 40;

/// Where that instruction sits, and so where the total beside it starts.
///
/// The reference's own `x` of 25, and a `y` measured on this window's picture
/// rather than taken from its `Height - 85`: that scroll is resizable and this
/// one is not, and its bottom piece is not this one's — see [`HEIGHT`] and
/// [`BOTTOM_HEIGHT`].
const BOTTOM_COMMENT_AT: GumpPixel = GumpPixel::new(25, HEIGHT - BOTTOM_HEIGHT - 31);

/// How wide it is, which is where the total is written from —
/// `_skillsLabelSum.X = _bottomComment.X + _bottomComment.Width + 5`.
const BOTTOM_COMMENT_WIDTH: i32 = 210;

/// The viewport: where the rows are drawn, and the box they are cut to.
///
/// The `x` is not the reference's 22. Its scroll art is rolled at both edges,
/// and a heading's arrow at 22 is drawn on the roll rather than on the
/// parchment — which is what it looked like on the first frame this window was
/// ever drawn on. Measured off that picture, not calculated.
const VIEWPORT_AT: GumpPixel = GumpPixel::new(42, 56);

/// How wide it is — the parchment, less the margin the bar sits in.
const VIEWPORT_WIDTH: i32 = 246;

/// How tall: down to the rule above the total, and not a pixel past it.
///
/// The one number here that was wrong on the first frame and is arithmetic
/// afterwards — `RULE_BOTTOM_Y - VIEWPORT_AT.y`, less a little air. Too tall by
/// forty pixels, the list ran on *under* the rule and under the plate that says
/// what the buttons do, and the row being cut by the viewport's edge was cut
/// somewhere nobody could see it happen.
const VIEWPORT_HEIGHT: i32 = RULE_BOTTOM_Y - VIEWPORT_AT.y - 4;

/// How tall a heading's row is — the reference's `SkillsGroupControl.Height`.
const HEADING_HEIGHT: i32 = 20;

/// How tall a skill's row is: font 9's line, which is what the reference's row
/// grows to.
const ROW_HEIGHT: i32 = 17;

/// The face the rows are written in — `new Label(skill.Name, false, 0x0288,
/// font: 9)`, where the `false` is `isUnicode`.
const ROW_FONT: Font = Font(9);

/// The face a heading is written in — the reference's `StbTextBox(6, ...)`.
const HEADING_FONT: Font = Font(6);

/// The face the total is written in.
const TOTAL_FONT: Font = Font(3);

/// The hue a row is written in — a *wire* hue, as `Label`'s constructor takes
/// one. See `paperdoll::NAME_HUE`, which makes the same point.
const ROW_HUE: Hue = Hue(0x0288);

/// The hue a heading and the total are written in.
const HEADING_HUE: Hue = Hue(0x0386);

/// Where a skill's name starts inside its row — `name.X = 22`.
const NAME_X: i32 = 22;

/// Where the use button sits, left of the name — `buttonUse.X = 8`.
const USE_X: i32 = 8;

/// Where its value's right edge falls — `_value.X = 250 - _value.Width`, taken
/// as a fraction of the viewport since this window is not the reference's width.
const VALUE_RIGHT: i32 = VIEWPORT_WIDTH - 30;

/// Where the lock picture sits: past the value, at the right-hand edge.
const LOCK_X: i32 = VIEWPORT_WIDTH - 22;

/// Where a heading's name starts — `_textbox.X = 16`.
const HEADING_NAME_X: i32 = 16;

// -- the bar --------------------------------------------------------------
//
// `ScrollBar`'s art, and its arithmetic. The pieces are 15 wide; the thumb is
// 13, centred on them.

/// The arrow at the top of the bar.
const BAR_UP: Graphic = Graphic(0x00FB);

/// The arrow at the bottom.
const BAR_DOWN: Graphic = Graphic(0x00FD);

/// The thumb.
const BAR_THUMB: Graphic = Graphic(0x00FE);

/// The three pieces of the track: a cap, a length that tiles, and a cap.
const BAR_TRACK_TOP: Graphic = Graphic(0x0101);
/// The tiled middle.
const BAR_TRACK_MIDDLE: Graphic = Graphic(0x0100);
/// The bottom cap.
const BAR_TRACK_BOTTOM: Graphic = Graphic(0x00FF);

/// How wide the bar's pieces are.
const BAR_WIDTH: i32 = 15;

/// How tall each arrow is.
const ARROW_HEIGHT: i32 = 21;

/// How tall the thumb is.
const THUMB_HEIGHT: i32 = 25;

/// How tall each track cap is.
const TRACK_CAP: i32 = 32;

/// How wide the thumb is, which is narrower than the track it runs in.
const THUMB_WIDTH: i32 = 13;

/// Where the bar sits: to the right of the viewport, the same height.
const BAR_AT: GumpPixel = GumpPixel::new(VIEWPORT_AT.x + VIEWPORT_WIDTH + 6, VIEWPORT_AT.y);

/// How far a wheel notch or an arrow scrolls: one row.
pub const STEP: i32 = ROW_HEIGHT;

/// The rows the window is a list of, in the order it draws them.
///
/// Headings in the file's own order, `Misc` first, and the skills under each in
/// id order — which is very nearly alphabetical, because that is how the skills
/// were numbered when there were forty-nine of them. A shut heading contributes
/// its own row and none of its skills, which is the whole of what shutting one
/// does.
///
/// Every skill the *files* know is a row, whether or not the shard has sent a
/// line for it: the name is a fact about the install and the number is a fact
/// about the character, and a window that hid the rows it had no numbers for
/// would be empty until the first `0x3A` and would never say why.
#[must_use]
pub fn rows(names: &Skills, groups: &SkillGroups, tree: &Tree) -> Vec<Row> {
    let mut rows = Vec::new();
    for (group, _) in groups.groups() {
        rows.push(Row::Heading(group));
        if tree.is_shut(group) {
            continue;
        }
        for (id, _) in names.iter() {
            if groups.group_of(id) == Some(group) {
                rows.push(Row::Skill(id));
            }
        }
    }
    rows
}

/// How tall a row is.
#[must_use]
const fn height_of(row: Row) -> i32 {
    match row {
        Row::Heading(_) => HEADING_HEIGHT,
        Row::Skill(_) => ROW_HEIGHT,
    }
}

/// How tall the whole list is, open headings and all — what the scroll is
/// clamped against and what the thumb's length is a fraction of.
#[must_use]
pub fn content_height(names: &Skills, groups: &SkillGroups, tree: &Tree) -> i32 {
    rows(names, groups, tree).into_iter().map(height_of).sum()
}

/// A skill's value as the window writes it: tenths, with the point put back.
///
/// Integer arithmetic and not a float: 755 is "75.5" exactly, and the wire has
/// no fractions in it that a `f32` would have to round back.
#[must_use]
fn tenths(value: u16) -> String {
    format!("{}.{}", value / 10, value % 10)
}

/// The lock's picture.
#[must_use]
const fn lock_of(lock: SkillLock) -> Graphic {
    match lock {
        SkillLock::Up => LOCK_UP,
        SkillLock::Down => LOCK_DOWN,
        SkillLock::Locked => LOCK_HELD,
    }
}

/// Lay the window out at `at`.
///
/// `standing` answers what the shard has said about a skill, or `None` for one
/// it has not mentioned — a closure rather than a table so that the caller can
/// hand over the view's own map without copying it once a frame.
///
/// `width_of` measures a string in a face, which is [`crate::text::gump_width`]
/// with the caller's font atlas already in hand: a value is written right-
/// aligned and a heading's rule starts where its name ends, and neither can be
/// placed without asking how wide the text came out.
pub fn window(
    names: &Skills,
    groups: &SkillGroups,
    tree: &Tree,
    standing: impl Fn(SkillId) -> Option<Standing>,
    width_of: impl Fn(&str, Font) -> i32,
    at: GumpPixel,
) -> Sheet {
    let viewport = Scissor {
        at:     at.offset(VIEWPORT_AT),
        width:  VIEWPORT_WIDTH,
        height: VIEWPORT_HEIGHT,
    };
    let bar = Scissor {
        at:     at.offset(BAR_AT),
        width:  BAR_WIDTH,
        height: VIEWPORT_HEIGHT,
    };
    let mut sheet = Sheet {
        pictures: Vec::new(),
        hits: BTreeMap::new(),
        lines: Vec::new(),
        viewport,
        bar,
    };
    frame(&mut sheet, at);
    // Over every skill the files name, and not over the rows: a heading that is
    // shut hides its skills from the list and takes nothing away from the
    // character. Summed here rather than as the rows go by, which is what made
    // it the *visible* total the first time it was written.
    let total: u32 = names
        .iter()
        .filter_map(|(id, _)| standing(id))
        .map(|line| u32::from(line.skill.value))
        .sum();
    let mut y = VIEWPORT_AT.y - tree.offset();
    for row in rows(names, groups, tree) {
        match row {
            Row::Heading(group) => {
                let name = groups.name(group).unwrap_or_default();
                heading(&mut sheet, at, y, group, name, tree.is_shut(group), &width_of);
            }
            Row::Skill(id) => {
                let Some(skill) = names.get(id) else {
                    continue;
                };
                skill_row(&mut sheet, at, y, id, skill, standing(id), &width_of);
            }
        }
        y += height_of(row);
    }
    scrollbar(&mut sheet, at, tree.offset(), content_height(names, groups, tree));
    // The total, past the right-hand end of the instruction under the list.
    // Every skill the shard has stated, including the ones whose heading is
    // shut — it is the character's total and not the visible rows'.
    sheet.lines.push(Line {
        // Beside the instruction rather than on it — it is a picture of a
        // sentence, so there is nowhere on it to write. `_bottomComment.X +
        // Width + 5`, five pixels past its right-hand edge.
        at:      at.offset(GumpPixel::new(
            BOTTOM_COMMENT_AT.x + BOTTOM_COMMENT_WIDTH + 5,
            BOTTOM_COMMENT_AT.y + 5,
        )),
        text:    tenths(total.min(u32::from(u16::MAX)) as u16),
        font:    TOTAL_FONT,
        hue:     HEADING_HUE,
        // Not cut: it is written on the frame, below where the list stops.
        scissor: None,
    });
    sheet
}

/// The scroll itself: four pieces, a title and two rules.
fn frame(sheet: &mut Sheet, at: GumpPixel) {
    let body_height = HEIGHT - TOP_HEIGHT - BOTTOM_HEIGHT;
    // `(width - w1) / 2` in the reference: the body is narrower than the top and
    // is centred under it.
    let body_x = (WIDTH - BODY_WIDTH) / 2;
    // `off / 2 + off / 4`, which is not the centre and is written out as the
    // reference wrote it — the art's own margins are not symmetrical.
    let off = WIDTH - BOTTOM_WIDTH;
    for picture in [
        Picture::plain(
            GumpArt::Gump(SCROLL_EDGE),
            at.offset(GumpPixel::new(body_x, TOP_HEIGHT)),
        )
        .tiled(BODY_WIDTH, body_height),
        Picture::plain(
            GumpArt::Gump(SCROLL_BODY),
            at.offset(GumpPixel::new(body_x, TOP_HEIGHT)),
        )
        .tiled(BODY_WIDTH, body_height),
        Picture::plain(GumpArt::Gump(SCROLL_TOP), at),
        Picture::plain(
            GumpArt::Gump(SCROLL_BOTTOM),
            at.offset(GumpPixel::new(off / 2 + off / 4, HEIGHT - BOTTOM_HEIGHT)),
        ),
        // Centred on the top piece, `(_gumpTop.Width - _gumplingTitle.Width) >> 1`.
        Picture::plain(GumpArt::Gump(TITLE), at.offset(GumpPixel::new(140, 12))),
        Picture::plain(GumpArt::Gump(RULE), at.offset(GumpPixel::new(50, 42))),
        Picture::plain(GumpArt::Gump(RULE), at.offset(GumpPixel::new(50, RULE_BOTTOM_Y))),
        Picture::plain(GumpArt::Gump(BOTTOM_COMMENT), at.offset(BOTTOM_COMMENT_AT)),
    ] {
        sheet.pictures.push(picture);
    }
}

/// One heading: its arrow, its name, and the rule that runs off the end of it.
fn heading(
    sheet: &mut Sheet,
    at: GumpPixel,
    y: i32,
    group: GroupId,
    name: &str,
    shut: bool,
    width_of: &impl Fn(&str, Font) -> i32,
) {
    let arrow = match shut {
        true => HEADING_SHUT,
        false => HEADING_OPEN,
    };
    let corner = at.offset(GumpPixel::new(VIEWPORT_AT.x, y + 2));
    sheet
        .pictures
        .push(Picture::plain(GumpArt::Gump(arrow), corner).inside(sheet.viewport));
    sheet
        .hits
        .insert(PictureIndex::new(sheet.pictures.len() - 1), Hit::Heading(group));
    sheet.lines.push(Line {
        at:      at.offset(GumpPixel::new(VIEWPORT_AT.x + HEADING_NAME_X, y)),
        text:    name.to_owned(),
        font:    HEADING_FONT,
        hue:     HEADING_HUE,
        scissor: Some(sheet.viewport),
    });
    // `xx = width + 11 + 16`, ruled to the right-hand edge.
    let ruled = HEADING_NAME_X + width_of(name, HEADING_FONT) + 11;
    if ruled < VIEWPORT_WIDTH {
        sheet.pictures.push(
            Picture::plain(
                GumpArt::Gump(HEADING_RULE),
                at.offset(GumpPixel::new(VIEWPORT_AT.x + ruled, y + 7)),
            )
            .tiled(VIEWPORT_WIDTH - ruled, 5)
            .inside(sheet.viewport),
        );
    }
}

/// One skill: its name, its value right-aligned, and the face of its lock.
///
/// A skill the shard has said nothing about is drawn as its name alone. Not as
/// `0.0`: a zero the shard sent and a zero nobody sent are different facts, and
/// the second one is what a window says by leaving the column empty.
fn skill_row(
    sheet: &mut Sheet,
    at: GumpPixel,
    y: i32,
    id: SkillId,
    skill: &Skill,
    standing: Option<Standing>,
    width_of: &impl Fn(&str, Font) -> i32,
) {
    // The use button, if the files offer one for this skill at all — a fact
    // about the install, not about whether the shard has sent a line yet.
    if skill.has_action {
        sheet.pictures.push(
            Picture::plain(
                GumpArt::Gump(USE_BUTTON),
                at.offset(GumpPixel::new(VIEWPORT_AT.x + USE_X, y + 2)),
            )
            .inside(sheet.viewport),
        );
        sheet
            .hits
            .insert(PictureIndex::new(sheet.pictures.len() - 1), Hit::Use(id));
    }
    sheet.lines.push(Line {
        at:      at.offset(GumpPixel::new(VIEWPORT_AT.x + NAME_X, y)),
        text:    skill.name.clone(),
        font:    ROW_FONT,
        hue:     ROW_HUE,
        scissor: Some(sheet.viewport),
    });
    let Some(standing) = standing else {
        return;
    };
    let value = tenths(standing.skill.value);
    let width = width_of(&value, ROW_FONT);
    sheet.lines.push(Line {
        at:      at.offset(GumpPixel::new(VIEWPORT_AT.x + VALUE_RIGHT - width, y)),
        text:    value,
        font:    ROW_FONT,
        hue:     ROW_HUE,
        scissor: Some(sheet.viewport),
    });
    sheet.pictures.push(
        Picture::plain(
            GumpArt::Gump(lock_of(standing.lock)),
            at.offset(GumpPixel::new(VIEWPORT_AT.x + LOCK_X, y + 2)),
        )
        .inside(sheet.viewport),
    );
    sheet
        .hits
        .insert(PictureIndex::new(sheet.pictures.len() - 1), Hit::Lock(id));
}

/// The bar: two arrows, a track of three pieces, and the thumb on it.
///
/// `ScrollBar.Draw`, whose track is a cap, a tiled length and a cap — and whose
/// thumb is only drawn when there is something to scroll, which is why a list
/// shorter than its window has a bar with nothing running in it rather than a
/// thumb filling the whole track.
fn scrollbar(sheet: &mut Sheet, at: GumpPixel, offset: i32, content: i32) {
    let top = at.offset(BAR_AT);
    let track_top = top.offset(GumpPixel::new(0, ARROW_HEIGHT));
    let middle = VIEWPORT_HEIGHT - 2 * ARROW_HEIGHT - 2 * TRACK_CAP;
    sheet
        .pictures
        .push(Picture::plain(GumpArt::Gump(BAR_TRACK_TOP), track_top));
    if middle > 0 {
        sheet.pictures.push(
            Picture::plain(
                GumpArt::Gump(BAR_TRACK_MIDDLE),
                track_top.offset(GumpPixel::new(0, TRACK_CAP)),
            )
            .tiled(BAR_WIDTH, middle),
        );
    }
    sheet.pictures.push(Picture::plain(
        GumpArt::Gump(BAR_TRACK_BOTTOM),
        top.offset(GumpPixel::new(0, VIEWPORT_HEIGHT - ARROW_HEIGHT - TRACK_CAP)),
    ));
    // The track answers a click by jumping the thumb to it, so all three pieces
    // mean the same thing. Inserted before the arrows and the thumb, which are
    // drawn over it and picked first.
    for index in sheet.pictures.len() - 3..sheet.pictures.len() {
        sheet.hits.insert(PictureIndex::new(index), Hit::Track);
    }
    sheet.pictures.push(Picture::plain(GumpArt::Gump(BAR_UP), top));
    sheet
        .hits
        .insert(PictureIndex::new(sheet.pictures.len() - 1), Hit::Up);
    sheet.pictures.push(Picture::plain(
        GumpArt::Gump(BAR_DOWN),
        top.offset(GumpPixel::new(0, VIEWPORT_HEIGHT - ARROW_HEIGHT)),
    ));
    sheet
        .hits
        .insert(PictureIndex::new(sheet.pictures.len() - 1), Hit::Down);
    let hidden = content - VIEWPORT_HEIGHT;
    if hidden <= 0 {
        return;
    }
    let travel = thumb_travel();
    let thumb = offset.clamp(0, hidden) * travel / hidden;
    sheet.pictures.push(Picture::plain(
        GumpArt::Gump(BAR_THUMB),
        top.offset(GumpPixel::new(
            (BAR_WIDTH - THUMB_WIDTH) / 2,
            ARROW_HEIGHT + thumb,
        )),
    ));
    sheet
        .hits
        .insert(PictureIndex::new(sheet.pictures.len() - 1), Hit::Thumb);
}

/// How far the thumb can travel: the bar less its two arrows and its own
/// height — `GetScrollableArea`.
#[must_use]
const fn thumb_travel() -> i32 {
    VIEWPORT_HEIGHT - 2 * ARROW_HEIGHT - THUMB_HEIGHT
}

impl Sheet {
    /// What offset a pointer at this point means, for a thumb being dragged or a
    /// track being clicked.
    ///
    /// The pointer is taken as the *middle* of the thumb — `y -= _emptySpace.Y +
    /// (_rectSlider.Height >> 1)` in the reference — so a click on the track
    /// puts the thumb under the finger rather than starting it there.
    ///
    /// `content` is [`content_height`]'s answer. A list that fits its window
    /// scrolls to zero and nowhere else, which is the `hidden <= 0` arm and the
    /// reason this cannot divide by it.
    #[must_use]
    pub fn offset_at(&self, pointer: GumpPixel, content: i32) -> i32 {
        let hidden = content - VIEWPORT_HEIGHT;
        if hidden <= 0 {
            return 0;
        }
        let travel = thumb_travel();
        let along = (pointer.y - self.bar.at.y - ARROW_HEIGHT - THUMB_HEIGHT / 2).clamp(0, travel);
        along * hidden / travel
    }

    /// Which picture the pointer is over, and what it means — `None` for a point
    /// on the window that means nothing, which is most of it.
    ///
    /// The walk is [`crate::gump::pick`]'s, over the list this sheet drew, so a
    /// row cut away by the viewport is not pickable where it is not drawn.
    #[must_use]
    pub fn hit(&self, pointer: GumpPixel, atlas: &crate::gump::GumpAtlas) -> Option<Hit> {
        let index = crate::gump::pick(&self.pictures, pointer, atlas)?;
        self.hits.get(&index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two groups and four skills, in the shape the real files have: the names
    /// are one table, the grouping is another, and the second is one-based into
    /// the first's headings.
    fn files() -> (Skills, SkillGroups) {
        let mut records = Vec::new();
        let mut index = Vec::new();
        for (nth, name) in ["Alchemy", "Anatomy", "Magery", "Mining"].iter().enumerate() {
            let offset = records.len() as u32;
            records.push(u8::from(nth % 2 == 0));
            records.extend_from_slice(name.as_bytes());
            records.push(0);
            let length = records.len() as u32 - offset;
            index.extend_from_slice(&offset.to_le_bytes());
            index.extend_from_slice(&length.to_le_bytes());
            index.extend_from_slice(&0u32.to_le_bytes());
        }
        let names = Skills::parse(&index, &records).expect("the fixture is whole entries");

        let mut bytes = 3i32.to_le_bytes().to_vec(); // Misc + two named
        for group in ["Combat", "Magic"] {
            let mut record = vec![0u8; 17];
            record[..group.len()].copy_from_slice(group.as_bytes());
            bytes.extend_from_slice(&record);
        }
        // Alchemy → Misc, Anatomy → Combat, Magery → Magic, Mining → Misc.
        for group in [0i32, 1, 2, 0] {
            bytes.extend_from_slice(&group.to_le_bytes());
        }
        let groups = SkillGroups::parse(&bytes).expect("the fixture is whole");
        (names, groups)
    }

    /// A fixed width per character, so that placement can be tested without a
    /// client's fonts.
    fn width_of(text: &str, _font: Font) -> i32 {
        text.len() as i32 * 6
    }

    fn standing(value: u16) -> Standing {
        Standing {
            skill: SkillLine {
                value,
                base: value,
                lock: SkillLock::Up,
                cap: 1000,
            },
            lock:  SkillLock::Up,
        }
    }

    /// The tree: a heading, then its skills, in the file's order with `Misc`
    /// first.
    #[test]
    fn the_rows_are_a_heading_and_then_what_is_filed_under_it() {
        let (names, groups) = files();
        let tree = Tree::default();
        assert_eq!(
            rows(&names, &groups, &tree),
            [
                Row::Heading(SkillGroups::MISC),
                Row::Skill(SkillId(0)), // Alchemy
                Row::Skill(SkillId(3)), // Mining
                Row::Heading(GroupId(1)),
                Row::Skill(SkillId(1)), // Anatomy
                Row::Heading(GroupId(2)),
                Row::Skill(SkillId(2)), // Magery
            ]
        );
    }

    /// Shutting a heading takes its skills out of the list and leaves the
    /// heading — which is the whole of what shutting one does, and is also what
    /// makes the content shorter and the thumb longer.
    #[test]
    fn a_shut_heading_keeps_its_own_row_and_drops_its_skills() {
        let (names, groups) = files();
        let mut tree = Tree::default();
        let open = content_height(&names, &groups, &tree);
        tree.toggle(SkillGroups::MISC);
        assert!(tree.is_shut(SkillGroups::MISC));
        assert_eq!(
            rows(&names, &groups, &tree),
            [
                Row::Heading(SkillGroups::MISC),
                Row::Heading(GroupId(1)),
                Row::Skill(SkillId(1)),
                Row::Heading(GroupId(2)),
                Row::Skill(SkillId(2)),
            ]
        );
        assert_eq!(
            content_height(&names, &groups, &tree),
            open - 2 * ROW_HEIGHT,
            "two rows shorter, and not one heading shorter"
        );
        tree.toggle(SkillGroups::MISC);
        assert!(!tree.is_shut(SkillGroups::MISC), "and it opens again");
    }

    /// The scroll is clamped where it is *set*, not where it is drawn: a tree
    /// holding an offset the layout would have clamped puts the thumb somewhere
    /// the rows are not.
    #[test]
    fn the_scroll_is_clamped_to_what_there_is_to_scroll() {
        let mut tree = Tree::default();
        tree.scroll_by(-STEP, 1000);
        assert_eq!(tree.offset(), 0, "nothing above the first row");
        tree.scroll_to(10_000, 1000);
        assert_eq!(tree.offset(), 1000 - VIEWPORT_HEIGHT);
        tree.scroll_to(50, VIEWPORT_HEIGHT / 2);
        assert_eq!(tree.offset(), 0, "a list shorter than its window does not scroll");
    }

    /// A value is written in tenths with the point put back, and a skill the
    /// shard has not mentioned is written as its name alone — not as `0.0`,
    /// which is a number nobody sent.
    #[test]
    fn a_skill_with_no_line_is_drawn_without_a_number() {
        let (names, groups) = files();
        let tree = Tree::default();
        let sheet = window(
            &names,
            &groups,
            &tree,
            |id| (id == SkillId(0)).then(|| standing(755)),
            width_of,
            GumpPixel::new(0, 0),
        );
        let written: Vec<&str> = sheet.lines.iter().map(|line| line.text.as_str()).collect();
        assert!(written.contains(&"75.5"), "{written:?}");
        assert_eq!(
            written.iter().filter(|text| text.contains('.')).count(),
            2,
            "the one value, and the total — and nothing for the other three skills"
        );
    }

    /// The total is every line the shard has stated, including the ones under a
    /// shut heading: it is the character's total and not the visible rows'.
    #[test]
    fn the_total_counts_the_skills_a_shut_heading_hides() {
        let (names, groups) = files();
        let mut tree = Tree::default();
        tree.toggle(SkillGroups::MISC);
        let sheet = window(
            &names,
            &groups,
            &tree,
            |_| Some(standing(500)),
            width_of,
            GumpPixel::new(0, 0),
        );
        let total = sheet.lines.last().expect("the total is written last");
        assert_eq!(total.text, "200.0", "four skills at 50.0, two of them hidden");
    }

    /// Everything in the list carries the viewport, and nothing in the frame or
    /// the bar does: a row scrolled out is cut away, and the scroll's own
    /// parchment is not.
    #[test]
    fn the_rows_are_cut_to_the_viewport_and_the_frame_is_not() {
        let (names, groups) = files();
        let tree = Tree::default();
        let sheet = window(
            &names,
            &groups,
            &tree,
            |_| Some(standing(500)),
            width_of,
            GumpPixel::new(100, 100),
        );
        // Named by their own graphics rather than by their place in the list,
        // so that the assertion says which picture it is talking about and
        // cannot be satisfied by counting.
        let cut = |graphic: Graphic| {
            sheet
                .pictures
                .iter()
                .find(|picture| picture.graphic == GumpArt::Gump(graphic))
                .unwrap_or_else(|| panic!("0x{:04X} is not in the window", graphic.0))
                .scissor
        };
        for parchment in [SCROLL_TOP, SCROLL_BOTTOM, TITLE, BOTTOM_COMMENT] {
            assert_eq!(cut(parchment), None, "the frame is not cut to the list's box");
        }
        // No thumb in this list: four skills do not fill the window, which is
        // `a_list_that_fits_has_no_thumb`'s subject.
        for bar in [BAR_UP, BAR_DOWN, BAR_TRACK_TOP] {
            assert_eq!(cut(bar), None, "and neither is the bar beside it");
        }
        for row in [HEADING_OPEN, HEADING_RULE, LOCK_UP] {
            assert_eq!(
                cut(row),
                Some(sheet.viewport),
                "everything in the list is cut to the viewport"
            );
        }
        assert_eq!(
            sheet.viewport.at,
            GumpPixel::new(100 + VIEWPORT_AT.x, 100 + VIEWPORT_AT.y),
            "the window moved with `at`"
        );
    }

    /// Which lines are cut and which are not, which is a difference the window
    /// had to be *looked at* to find: cutting every line to the list's box takes
    /// the total off the screen with them, since it is written on the frame
    /// under the rule the list stops at.
    #[test]
    fn the_rows_are_cut_to_the_list_and_the_total_under_it_is_not() {
        let (names, groups) = files();
        let tree = Tree::default();
        let sheet = window(
            &names,
            &groups,
            &tree,
            |_| Some(standing(500)),
            width_of,
            GumpPixel::new(0, 0),
        );
        let (total, rows) = sheet
            .lines
            .split_last()
            .expect("a window writes at least the total");
        assert_eq!(total.scissor, None, "the total is the frame's, not the list's");
        assert!(
            total.at.y > sheet.viewport.at.y + sheet.viewport.height,
            "and it is written below where the list stops"
        );
        assert!(
            rows.iter().all(|line| line.scissor == Some(sheet.viewport)),
            "every line in the list carries the box the list is cut to"
        );
    }

    /// The thumb runs from the top of its travel to the bottom and no further,
    /// and the point it is dragged to is the offset it means — the same
    /// arithmetic in both directions, which is what keeps a dragged thumb under
    /// the finger.
    #[test]
    fn the_thumb_and_the_offset_are_the_same_arithmetic_both_ways() {
        let (names, groups) = files();
        let tree = Tree::default();
        let sheet = window(
            &names,
            &groups,
            &tree,
            |_| Some(standing(500)),
            width_of,
            GumpPixel::new(0, 0),
        );
        let content = 1000; // taller than the viewport, so there is travel
        let top = sheet.offset_at(GumpPixel::new(0, BAR_AT.y), content);
        assert_eq!(top, 0, "above the track is the top of the list");
        let bottom = sheet.offset_at(GumpPixel::new(0, BAR_AT.y + VIEWPORT_HEIGHT), content);
        assert_eq!(bottom, content - VIEWPORT_HEIGHT, "and below it is the end");
        assert_eq!(
            sheet.offset_at(GumpPixel::new(0, BAR_AT.y + 500), content),
            bottom,
            "past the end is still the end"
        );
    }

    /// A list that fits its window has no thumb — the reference draws one only
    /// when there is something to scroll, and dividing by the nothing there is
    /// to scroll is the arithmetic this avoids.
    #[test]
    fn a_list_that_fits_has_no_thumb() {
        let (names, groups) = files();
        let mut tree = Tree::default();
        for group in [SkillGroups::MISC, GroupId(1), GroupId(2)] {
            tree.toggle(group);
        }
        let sheet = window(
            &names,
            &groups,
            &tree,
            |_| Some(standing(500)),
            width_of,
            GumpPixel::new(0, 0),
        );
        assert!(
            !sheet.hits.values().any(|hit| *hit == Hit::Thumb),
            "three shut headings fit the window"
        );
        assert_eq!(sheet.offset_at(GumpPixel::new(0, BAR_AT.y + 100), 60), 0);
    }

    /// A skill with a line carries a clickable lock; one the files mark
    /// `has_action` carries a clickable use button too — `files()`'s Alchemy
    /// (`nth == 0`, `has_action`) and Anatomy (`nth == 1`, not).
    #[test]
    fn a_row_registers_the_hits_its_skill_actually_has() {
        let (names, groups) = files();
        let tree = Tree::default();
        let sheet = window(
            &names,
            &groups,
            &tree,
            |_| Some(standing(500)),
            width_of,
            GumpPixel::new(0, 0),
        );
        assert!(
            sheet.hits.values().any(|hit| *hit == Hit::Use(SkillId(0))),
            "Alchemy is has_action in the fixture"
        );
        assert!(
            sheet.hits.values().any(|hit| *hit == Hit::Lock(SkillId(0))),
            "every skill with a line has a clickable lock"
        );
        assert!(
            !sheet.hits.values().any(|hit| *hit == Hit::Use(SkillId(1))),
            "Anatomy is not has_action in the fixture"
        );
        assert!(
            sheet.hits.values().any(|hit| *hit == Hit::Lock(SkillId(1))),
            "Anatomy still has a line, so still a lock"
        );
    }

    /// A skill with no line has no lock to click — `skill_row` returns before
    /// pushing one — but a `has_action` skill's use button does not depend on
    /// the shard having said anything.
    #[test]
    fn a_skill_with_no_line_has_no_lock_but_keeps_its_use_button() {
        let (names, groups) = files();
        let tree = Tree::default();
        let sheet = window(
            &names,
            &groups,
            &tree,
            |id| (id != SkillId(0)).then(|| standing(500)),
            width_of,
            GumpPixel::new(0, 0),
        );
        assert!(
            sheet.hits.values().any(|hit| *hit == Hit::Use(SkillId(0))),
            "Alchemy's use button does not wait on a line"
        );
        assert!(
            !sheet.hits.values().any(|hit| *hit == Hit::Lock(SkillId(0))),
            "no line, no lock face to click"
        );
    }

    /// The one deliberate exception to "wait for the shard": a click on the
    /// lock arrow is drawn back before any packet could possibly answer it.
    #[test]
    fn a_tree_draws_the_players_own_click_over_the_shards_line() {
        let mut tree = Tree::default();
        assert_eq!(
            tree.lock_of(SkillId(0), SkillLock::Up),
            SkillLock::Up,
            "nothing clicked yet: the shard's own line stands"
        );
        tree.set_lock(SkillId(0), SkillLock::Locked);
        assert_eq!(tree.lock_of(SkillId(0), SkillLock::Up), SkillLock::Locked);
        assert_eq!(
            tree.lock_of(SkillId(1), SkillLock::Down),
            SkillLock::Down,
            "a click on one skill does not touch another's"
        );
    }
}
