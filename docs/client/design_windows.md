# The window layer: gumps, containers, the paperdoll and the sheet

Everything this client draws that is not the world. Nine numbered decisions,
each taken and each with the reason it was taken that way — the two overlapping
index spaces a gump atlas has to keep apart, why a window has no size and is
picked by the pictures it drew, the paperdoll's order tables, and the `0xB0`
dialog that stopped being an egui window. Who *owns* each window's state is
[`design_panes.md`](design_panes.md).

Status and what is left are [`README.md`](README.md); the findings this work
turned up are
[`evidence/2026-08-30-the-client-backlog.md`](evidence/2026-08-30-the-client-backlog.md).

## M4 — the gump layer

The journal and the speech line, the status bar, the paperdoll, containers, and
generic gumps.

**Partly built, ahead of the rest of M4**, because the shard's staff commands
are unreachable without it: they are `.`-prefixed *speech*, and `.admin` is
answered with a gump. What landed is the whole path and none of the art —

- `0xAD` encodes (`UnicodeTalkRequest::encode`) and `0xB0` decodes; `0xB1`
  encodes. Each is the direction the engine had never needed until it grew a
  client of its own.
- `protocol::gump::layout::parse` reads the layout language the builder next
  door writes, and is tested against it element by element. Total: an unknown
  keyword is an `Element::Unknown`, never a lost window.
- `WorldView::gumps` holds what is open, and `gump_closed` is the one thing the
  wire never says — a reply button closes a window client-side.
- `client/app/src/gump.rs` draws a layout with egui's own widgets — still the
  *dev HUD's* rendering, in the same spirit as the rest of that file.
- **The speech line and journal are no longer egui's.** `App::chat` holds what
  is typed and whether the keyboard is listening for it; `App::window_event`
  gives it the keyboard ahead of every hotkey and walk key once focused
  (opened by Enter, the reference client's own gesture); and `App::draw` reads
  `WorldView::journal` directly and lays both out through
  `openshard_client_render::text::{GumpLabel, collect_gump}` — a top-left
  anchored, screen-space glyph layout bound to a second `GumpRenderer`
  (`Screen::gump_text_pass`), over the picture and under egui, the same corner
  the old `egui::Panel::bottom` claimed. `collect_gump` is written to be
  reusable rather than single-purpose: it is where a gump dialog's own
  `{ text }` / `{ croppedtext }` captions are meant to draw through too, once
  that lands — see the next bullet.

What is still M4 proper: the gump art (`gumpart.mul`), which is why a
`{ gumppic }` is drawn as a placeholder naming its graphic; hue lookup for gump
text; and the dialogs themselves, still egui's widgets over the client's own
gump art.

### Containers — the wire and the memory, not yet the window

A chest was **write-only**: the shard encodes `0x24`, `0x25` and `0x3C` and
nothing in the tree could read one back, so a client double-clicking a container
heard three `Undecoded` ids and drew nothing. Two of the three pieces are now in.

- **The protocol reads back.** `OpenContainer` (`0x24`), `AddToContainer`
  (`0x25`) and `ContainerContents` (`0x3C`) decode, and the first two became
  `ServerPacket` variants on the way. They had been kept out because
  `EncodePacket::LENGTH` is a `const` that cannot ask its payload's version and
  both change size across a `Feature` seam — an excuse the *enum* never had, so
  `ServerPacket::length` takes the version now and the two hand-written encoders
  stay as thin wrappers for the shard, which sends them as bytes with the value
  already in hand.
- **`ContainerContents::container` is an `Option`, and that is the wire's shape.**
  A `0x3C` has no header field naming the container — every item record names its
  own — so a listing with no items has named nothing at all, and opening an empty
  chest sends exactly that. The client learns which container it was from the
  `0x24` before it.
- **`WorldView` holds two tables, not one.** `containers` is which art each open
  window uses; `contents` is what each container holds. A **vendor** is what
  separates them: the shop window is a `0x24` naming the *vendor* and the goods
  are a `0x3C` naming the crate worn on its shop layer, so a listing whose
  container has no window is a shape the shard sends on purpose.
- **Three things the wire never says**, each one a line in `view.rs`: taking an
  item out of a bag is a plain `0x1D` and nothing else, so a `Remove` that does
  not reach the contents leaves the icon in the window forever; closing a window
  is a click, so `container_closed` is `gump_closed`'s twin and drops the
  contents with it; and a `0x25` for an item already listed is a stack that grew,
  not a second item.

**The window draws.** It stopped at the same place `{ tilepic }` did — the
icons inside a container come from the *world's* art and the gump pass bound one
atlas, which held `gumpart` — and the fix was the key rather than a second pass:
`GumpArt::Gump` versus `GumpArt::Item`. Not cosmetic. Gump ids and art ids are
separate 16-bit index spaces and they overlap, `0x003C` being a chest's gump
*and* an item's graphic, so a shared `Graphic` key answers one with the other and
draws the wrong picture with no error anywhere to notice it by. The packer under
the wrapper stays keyed by a `Graphic` and what reaches it is a slot the atlas
hands out, private to two fields. Growing it now needs both readers, so it takes
an `ArtFiles` and not a `Gumps`.

`{ tilepic }` closed in the same move, which is what it was always going to be: a
container is the question a layout had been asking since the gump reader was
written.

`crates/client/render/container.rs` is the layout, and it is deliberately *not* a
layout at all — a `0xB0` is a program with pages and buttons; a container is one
picture with statics laid on it at the coordinates the `0x3C` carried. Its size
is its background's size, because there is no rectangle on the wire to
nine-slice. Its order is the shard's, because icons in a bag overlap and
re-sorting them would put a different one on top than the reference shows.
Picking is against the picture and not the box, for `items::pick`'s reason.

**And the window is not an egui window**, unlike the dialogs beside it: a
container has no widget in it — no button, no field, nothing that needs egui's
hit test — so its position, its drag and its z-order are the client's, in gump
pixels. Where it goes is the one thing a `0x24` never says, so the client
cascades them and the player's drag wins after that. Left raises and holds;
right closes, which is the reference's own gesture and does not fight the
right-hold that steers, because a press over a window is not a press into the
world.

Still open, in rough order:

- **Dragging an item out** — `0x07`/`0x08`/`0x09` and the drop back. Picking
  inside a window is built (`container::pick`); nothing sends yet.
- **A double-click inside a window.** `use_under_cursor` asks the world's
  `items::pick` and knows nothing about windows, so a bag inside a bag cannot be
  opened from the one that holds it.
- **Window positions are not remembered**, per container or at all: the cascade
  is recomputed from scratch every session.
- **The gump pass has no blending**, so a `{ checkertrans }` is skipped and a
  container's own art is drawn opaque. Fine for a bag; not for a paperdoll.

### The paperdoll — drawn

A `0x88` is the shard's answer to double-clicking a body, and it *was* the one
packet this engine sends that its own client could not read: `OpenPaperdoll`
encoded and was a `ServerPacket` variant, with no `DecodePacket` behind it, so a
client was handed `Ok(None)` and read on. It decodes, `WorldView` holds what is
open, and the window draws: a body picture with its equipment over it, in the
reference's order, in a window that drags, raises and closes like a bag's.

All five decisions are taken. What is *not* built is listed under decision 3 and
in the backlog below — chiefly `IsCovered`, which needs a graphic the client
does not carry yet.

1. **A `0x88` is not a listing, and `WorldView::paperdolls` is built on that.**
   It carries a serial, a 60-byte name and a flag byte (`PaperdollFlags`:
   warmode, can-lift) and nothing about equipment, because the client already
   has it: a `0x78` carries the wearer's layers, and `WorldView` folds them into
   the mobile (and into `Player` for our own, which is the only `0x78` a shard
   ever sends us about ourselves). So the table holds what is *open* — name and
   flags, keyed by the mobile — and reads the equipment out of the mobile it
   names. The same shape `containers` has beside `contents`, and for the same
   reason: the window and what is in it arrive apart, and one can change without
   the other. It also means taking a hat off reaches an open paperdoll with no
   packet of its own.

   Two things the wire does not say, each a line in `view.rs`: closing the
   window is a click, so `paperdoll_closed` is `container_closed`'s twin — and
   it drops nothing else, because the equipment belongs to the body; and a `0x1D`
   for the mobile closes it, which is `Mobile.Destroy` in the reference and the
   only thing that ever would.
2. **A paperdoll's art is gump art, keyed by the item's `anim_id`, and this is
   a third index space.** The body is a gump — `0x000C` male, `0x000D` female,
   `0x03DB` drawn as `0x000C` in hue `0x03EA` — and each worn layer's picture is
   `StaticTile::anim_id + 50000` for a male body and `+ 60000` for a female one,
   falling back to the male gump when the female one is missing
   (ClassicUO's `IsAnimExistsInGump`; `PaperDollInteractable.GetAnimID` is the
   whole function). `Equipconv.def`'s fourth column is read now
   (`EquipConvEntry::gump`) and overrides the `anim_id` before the offset for
   bodies that need it — a *different* override from the third column, which is
   the animation's; a row may send the two apart, and the client-file test found
   one that does. The column's two shorthands (`0` is the item's own `AnimID`,
   `-1`/`0xFFFF` the animation override) are resolved by the reader, where
   `ProcessEquipConvDef` resolves them, so nothing downstream has to guess.
   `Gumps::has` answers "does the client ship this picture" without decoding one,
   which is what the female fallback asks twice per layer.
   **`worn_graphic` cannot be reused**: it answers the same
   question into `anim.mul`'s body-animation space, and `+ 50000` into
   `gumpart` is a different picture of the same shirt. Two resolvers, one table
   each, and the `anim_id == 0` guard is the only line they share — with
   opposite meaning, since a backpack draws here and never on a walking body.
3. **The draw order is not the layer numbering.** ClassicUO builds the order per
   body (`PaperdollOrder.Build`) — arms and torso swap for a female or gargoyle
   body — draws the backpack last and outside that pass, and skips hair and
   beard on the dead. Every one of those is a question about which *layer* a
   piece was worn on, and
   [`EquipmentLayer`](../../crates/client/render/src/mobiles.rs) carried a graphic
   and a hue and not the layer. It carries `Layer` now, through `crowd::worn`
   from the wire's own byte — one field, and the ordering table has something to
   be written against. It paid for itself immediately: **a ghost is bald**,
   which is the backlog entry below, and `worn_graphic` is the one place that
   decides it, for the atlas and the quad alike.

   The table is written: `paperdoll::order`, the whole of `PaperdollOrder.Build`
   — three base tables chosen by the arms and torso *graphics* (not by the body:
   an arms match locks the arms-late table and the torso is never asked), then
   the per-graphic exceptions, which are a list of garments whose art was drawn
   expecting a different neighbour. The layer names it is written against live on
   `wire::Layer` now, because a table of hex bytes cannot be checked against the
   reference it came from.

   The walking body reads the same table now — `paperdoll::world_order`, which
   is `PaperdollOrder.BuildInWorld`: `Build` plus the one cloak rule keyed on
   facing. "As the shard listed them" was *not* tolerable after all, and the
   case that showed it is the commonest one on screen: a cloak listed after a
   tunic, on a character facing the camera, was painted over the front of the
   tunic when it belongs behind the whole body. The rule has three arms and they
   are `LayerOrder.UsedLayers`'s three distinct rows — away, the cloak is
   painted last; facing the camera, first; edge-on, immediately under the
   helmet. `mobiles::drawn_layers` is the one list the three passes that walk a
   mobile's equipment share, so what is packed, what is drawn and what is
   pickable cannot disagree; it also drops the backpack from a walking body, as
   the reference does (`includeBackpack: false`).

   Not built: `MobileView.IsCovered`, which *hides* a layer an outer garment
   fully occludes — shoes under plate legs, arms under a closed robe. Every arm
   of it keys on the item's **wire graphic**, and that is the one graphic
   `EquipmentLayer` does not carry: it holds the `AnimID` `crowd::worn` resolved
   out of tiledata. Its absence draws a garment poking out from under a robe,
   which is visible and is not a hole.
4. **Female is the body graphic, not a flag on the wire.** Nothing in `0x78` or
   `0x88` says it; `mobile.IsFemale` is a fact about `0x0191`/`0x025E` and their
   kin. So the offset in decision 2 is chosen from the body the client is
   already drawing, and a mobile whose body is unknown is drawn male the way the
   reference does rather than not at all.
5. **The window is the container's, not egui's, and its background is the
   frame.** Position, drag, z-order, right-click close and picking against the
   picture were already the client's (`crates/client/render/container.rs`,
   `client/app`), in gump pixels, and a paperdoll is that machinery's second
   caller — which is the point of having written it there.

   What that machinery asks each window for is the one picture it *is*, and for
   a while a paperdoll answered with the body: there was no frame at all, and a
   doll floated on the world. The frame is `0x07D0` for our own character and
   `0x07D1` for anyone else's — `PaperDollGump.BuildGump`'s `0x07d0 +
   (LocalSerial == World.Player ? 0 : 1)`, written out as two named pictures
   because the arithmetic is how the file happens to be laid out and not a rule
   — and the difference between them is room down the right-hand side for the
   buttons a player gets over their own doll and nobody gets over a stranger's.
   The doll sits at `(8, 19)` inside it (`new PaperDollInteractable(8, 19,
   ...)`), one offset applied to the whole stack rather than per garment, since
   every layer already shares one origin.

   Drawing the frame first is also what lets a window outlive not knowing what
   is in it: `paperdoll::window` takes the wearer as an `Option`, the frame is
   drawn either way, and a doll the client has not been told the body of is a
   window that still picks up the pointer and still closes — the backlog entry
   that used to say it drew nothing.

   The machinery is shared by having the *subject* of a window say what kind it
   is: `App::own_windows` is one list of `WindowSubject::Container` and
   `WindowSubject::Paperdoll`, so a bag dragged over a paperdoll stays over it,
   and the two kinds differ in exactly two `match`es — what is laid out for it,
   and what closing one means to the `WorldView`.

   **A window has no size, and is picked by every picture it drew.** That is the
   third `match` gone, and the backlog entry that said a paperdoll was picked by
   its frame alone. `gump::pick` is one walk over a laid-out window — last
   picture first, because last is what was drawn on top, each against its own
   opaque texels — and it answers an *index*, because what a hit means differs
   per window kind and the walk does not. `container::pick` is that walk with
   the background dropped from the answer, and `App::window_under_pointer` is
   the same walk keeping only whether there was one. Nothing asks how big a
   window is any more, which is why `container::size` has no caller and
   `paperdoll` never grew one: a hat drawn past the edge of the frame belongs to
   the window and a click through the frame's transparent corner falls to the
   world, and neither of those is expressible as a rectangle.

   What the walk is picked over is the list the *last frame drew*
   (`App::drawn_windows`), not one laid out again at the press. A paperdoll's
   layout is not a function of the window alone — it reads the view, the
   tiledata and `gumpart` to decide which picture a worn item is — so a second
   walk asking those questions again is a second answer waiting to disagree with
   what is on the screen; it is `items::place`'s rule one layer up. The cost is
   that a window just opened is not pickable until it has been drawn once, which
   is the same frame its art is packed on and so the first frame it has any
   pixels to be picked by.

   What a paperdoll adds is *buttons* over its own art, which a container has
   none of. The hit test they want is now written — a button is a picture in the
   list and `gump::pick`'s index names it — and the layout is written with it.

6. **The frame's furniture is a table, and the name is a line of `fonts.mul`.**
   `PaperDollGump.BuildGump` puts every one of its buttons at `x = 185`, `y = 44
   + 27 * n`, and which button is on which row is the whole of the layout:
   help, options, log out, quests, skills, guild and the peace/war toggle down
   our own frame, then the status button on row seven — which is the only one a
   *stranger's* frame carries, because `0x07D1` has no column to put the rest
   in. Three pictures are not buttons at all: the profile scroll at `(25, 196)`,
   the party manifest fourteen pixels along it, and the virtue menu at `(80,
   4)`; the reference answers a **double** click on those and a single one on
   the buttons, which is why `paperdoll::DollButton` names windows rather than
   actions and leaves that difference to the caller.

   The peace/war toggle is the one picture the frame is drawn differently for,
   so `Whose::Own { war }` carries it: the flag comes off the `0x88`'s own
   `PaperdollFlags`, and the toggle only exists on our own doll, which is why
   the field is on that variant rather than beside it.

   The name is `new Label("", false, 0x0386, 185, font: 1)` at `(39, 262)` —
   and the `false` is `isUnicode`, which makes it the one string in this
   client's interface whose face the reference states outright: `fonts.mul`'s
   face 1, cropped to 185 pixels. It is drawn through `text::collect_gump` like
   every other line of interface text, so the plate is a `GumpLabel` and not a
   window kind of its own.

7. **A `0xB0` dialog is a window of ours now, not an egui one.** This is the
   layout fix, and it is worth writing down what was wrong: a dialog used to be
   an egui window with the shard's own background art drawn underneath it. That
   is two frames — egui's title bar and close box around the picture of a frame
   the shard sent — and, worse, two opinions about where everything in it is.
   egui needs a widget's size *before* the art is packed, so `client/app`'s
   gump module had invented one: a 26 by 20 button, a 220-point label, and a
   content rectangle measured from those. The clickable rectangle, the picture
   under it and the window's own extent were three different rectangles.

   All three are one list now. `gump::window` answers a `Window` — pictures,
   captions, `hits` and `fields` — and every question is asked of it:
   `gump::pick` for what was clicked, `Window::hits` for what that means,
   `gump::field` for the one thing in a window that is a box rather than a
   picture. Position, drag, z-order and closing are `App::own_windows`', the
   same machinery a container and a paperdoll have used since decision 5, so
   `WindowSubject` grew a third variant and nothing else about it changed.

   Four things fall out of it, and each was a defect before:

   - **A dialog opens where the shard put it.** A `0xB0` carries a coordinate,
     unlike a `0x24`, so dialogs are not cascaded — the client no longer
     second-guesses a layout it was handed.
   - **A button presses on the way down and answers on the way up**, with the
     pointer still on it. The layout has carried two pictures per button all
     along and nothing said when to draw the second; it is the mouse, and
     `Dialogs::held` is where the mouse lives between the two events. It is a
     `Hit` rather than a button id so that what is drawn pressed and what the
     release acts on cannot be two values.
   - **Text is the client's own.** A caption is drawn from `fonts.mul` through
     the gump pass, tinted by the same hue ramp every picture is, at the
     coordinate the layout named — and the layout's text hue is **one less**
     than the wire hue it means (`Label`'s and `CroppedText`'s constructors both
     add one), which the egui path had never done. `{ croppedtext }` crops
     character by character, which is what the reference's `FontStyle.Cropped`
     does and what a wrapping label did not.
   - **`{ textentry }` works without a widget.** A field is a box with an id;
     clicking one takes the keyboard, `winit`'s own `KeyEvent::text` fills it,
     and a caret is drawn at `text::gump_width` past the last character. The
     keyboard is given back on Escape, on Enter and on a press outside the
     window, so a letter is a letter typed only while a field is asking for one.

   What is still egui's is the dev HUD and the panels around the world. Nothing
   the shard sends is drawn by it any more.

8. **A paperdoll's buttons are requests, and the frame is drawn from the
   answer.** The layout landed in decision 6 with nothing behind it. What is
   behind it now is one rule stated three ways: *nothing is done locally on the
   way out.* The toggle does not flip its own picture, the Log Out button does
   not close the connection, and no window is opened by pressing anything — each
   button sends a packet and what is drawn follows the shard's answer to it.
   It is `App::use_under_cursor`'s rule for the interface, and it is why the
   client's own war stance is not a field the button writes.

   **The gesture is a dialog button's, with no dialog.** `App::held_doll` is
   `Dialogs::holding`'s counterpart: the press takes hold of a button, the frame
   draws its pressed face while the finger is down, and the release acts *only*
   if the pointer is still on the same button. It is keyed by
   `(WindowSubject, DollButton)` and not by picture index, because the doll is
   laid out afresh every frame and a hat coming off renumbers the list. Taking
   the press away from the drag is the other half: the column runs down the
   middle of the frame, and without this, pressing a button picked the doll up.

   **The three scrolls want a pair.** `scroll_pairs` is `App::last_click`'s rule
   applied to a picture — ClassicUO's 350ms, no distance test — and it compares
   the window *and* the picture, because the profile scroll and the party
   manifest sit fourteen pixels apart and a rule that only looked at the clock
   would open one because the hand slipped.

   **War mode has one home, and it is the player.** It arrives two ways — the
   `0x88`'s `PaperdollFlags::WARMODE` when a doll opens on us, and every `0x72`
   after that — and both fold to `WorldView::player.war`. The flag byte is *not*
   kept beside the window: `view::Paperdoll` carries `can_lift` and nothing
   else, because a stance stored on the packet that opened the window is a
   stance that goes stale the moment the next `0x72` lands, and the toggle would
   draw the older of the two answers. That the `0x72` was not being read at all
   is the defect this found: `ServerPacket::decode` had no arm for it, so the
   shard's answer was framed, dropped, and the toggle could never move.

   **What each button sends, and what four of them do not.** The wire half is
   `openshard_client_net::doll`, one function per button, each tested against
   this engine's own `ClientPacket::decode` — a button whose packet decoded as
   `Unknown` would look, from the window, exactly like one that works:

   | Button | Packet | What answers it |
   |---|---|---|
   | Peace/War | `0x72` | the shard's own `0x72`, which is what the toggle is drawn from |
   | Log Out | `0xD1` | the shard's `0xD1`, and the client closes the socket on it |
   | Quests | `0xD7` `0x32` | a `0xB0` — the quest log opens as a dialog, and that path was already built |
   | Guild | `0xD7` `0x28` | nothing yet: `guilds` is a stub, and the dispatch says so where it names the subcommand |
   | Status | `0x34` `0x04` | a `0x11` nothing draws yet |
   | Skills | `0x34` `0x05` | a `0x3A` nothing draws yet |
   | Profile scroll (double) | — | `0xB8` is not in `openshard_protocol` |
   | Party manifest (double) | — | `0xBF 0x06` is not either |
   | Virtue menu (double) | `0xB1` | a `GumpAnswered` the script pack can act on |
   | Help | — | `0x9B` is not in `openshard_protocol` |
   | Options | — | a window of the client's own that does not exist |

   Two of those deserve their reasons written down. **Status and Skills send
   only from our own doll**, because that is all this shard answers: the request
   is keyed on the connection and the serial in the packet is ignored
   (`StatusQuery::serial`), so pressing Status on a stranger's frame would fetch
   *our* status and open nothing about them. A health bar over somebody else is
   a window of its own. And the **virtue menu is a `0xB1` for a dialog nobody
   opened** — `ReplyGump(player, 0x1CD, 1, [subject])` is the reference's own
   convention, ServUO registers the same id — which reaches the script pack as
   an ordinary `GumpAnswered`, so a Community Pack can draw a virtue menu
   without an engine change.

   The four that send nothing press and come back up, and they are in the
   backlog rather than papered over: a packet invented here so that a button
   "did something" would be a shard logging an unknown id for a window that is
   never going to open.

   **The gate is both ends on one wire**:
   `crates/e2e/shard/tests/paperdoll_buttons.rs` logs a client into a real shard
   and presses three of them — the toggle in *both* directions (a client that
   folded "at war" from the mere arrival of a `0x72` passes half of that and
   fails the other), the Quest button until a `0xB0` opens, and Log Out until
   the grant comes back. Nothing else could have caught the missing decode arm:
   every unit test on either side of it passed while the toggle was dead.

9. **The skill window is a tree, and both of its tables are the client's own
   files.** The second window here that is nobody's layout: the shard sends
   numbers and nothing else. What a skill is *called* is `skills.mul` and which
   heading it is filed under is `skillgrp.mul`, and neither had a reader —
   `openshard_uofiles::skills` and `openshard_uofiles::skillgrp` are new. Two id
   spaces, two newtypes: `SkillId` indexes the names, `GroupId` the headings,
   and a window that mixed them would draw the right names under the wrong
   headings while never being out of range.

   `skillgrp.mul` is a file nothing but UOFiddler has ever read, and its shape
   is worth writing down: a count, then `count - 1` fixed-width names, then one
   `int32` per skill numbering **from one** into those names — zero means the
   group the file never names, which the reference tooling calls *Misc* and
   which holds eight of the fifty-eight. Read as zero-based, every skill comes
   out one heading early and nothing looks broken.

   **The `0x3A` is two packets sharing an id**, told apart by the byte after the
   length: `0x00`–`0x03` is the whole list (ids one-based, zero-terminated),
   `0xFF`/`0xDF` is one line (zero-based), `0xFE` is the shard sending its own
   skill *names*. That byte is also what says whether the rows carry a cap, and
   **it is the byte that is believed and not the version** — the version says
   what a shard should send, and a decoder that asked it would read every field
   of every row two bytes out of place the day the two disagreed. Two forms are
   refused by name rather than read as the form we know: the `0xFE` table, and
   the capless rows of a pre-4.0.0a client, which the reference hands a cap of
   1000 that nobody sent.

   **The window opens on the button and the packet only fills it.** The shard
   sends the whole list at world entry too, so a window that opened when a
   `0x3A` arrived would open itself at every login. `App::skills` is `Some` when
   the window is up *and* holds the tree — one field on purpose, since a
   `skills_open: bool` beside it could say the window was shut while its scroll
   position stood. It is the one window kind whose existence `WorldView` cannot
   answer for.

   **A list that scrolls needed a scissor, and there was none.** `Scissor` is a
   box applied on the processor: a quad straddling its edge is cut, rectangle
   shortened and region moved by the same fraction, which is exact for a sprite
   that is never rotated and needs no second pass. It rides on the `Picture`,
   because a picture is *picked* as well as drawn and a hit test reading a
   different box answers for a row scrolled out of the window. It is **not**
   `GumpLabel::clip`, which is `{ croppedtext }` — that one drops whole
   characters off the end of a line, ellipsis and all, and was checked against
   `FontsLoader.GetTextByWidthASCII` rather than assumed.

   **What the picture said that no test did.** Drawn and looked at, three
   things were wrong at once. Every window's text was being overwritten by the
   chat — one `GumpRenderer`, one instance buffer, and a queued write that lands
   before the encoder is submitted, so two `render` calls a frame draw the
   second call's quads twice and lose the first's; a defect the paperdoll's name
   plate has had for as long as there has been a journal line to overwrite it.
   The viewport was forty pixels too tall, so the list ran under the frame's own
   rule. And cutting *every* line to the list's box took the total with them,
   which is written on the frame below where the list stops — a line carries its
   own box now, exactly as a picture does.

   `crates/e2e/shard/tests/skill_window.rs` is the gate, and it is on the whole
   path rather than the decoder: the list a character is sent for entering the
   world, and the button asking for it again with the table emptied first —
   because with the login's own list still standing, every assertion in it
   passes on a shard that ignores the request. Checked by mutation: with the
   `0x3A` arm out of `ServerPacket::decode` both tests fail, and every unit test
   on either side stays green. The picture is `gumpshot`'s `skills` scene.

Done: double-clicking a mobile — or ourselves — opens a framed window that draws
its body and its equipment in the reference's order and hues, with the backpack
last; the window drags, raises and closes like a container's; a unit test says a
female body's order differs from a male one where the reference says it does;
and the client-file tests say a layer with `anim_id == 0` draws nothing, that
every gump a dressed body asks for is one the client ships, and that the female
fallback is exercised rather than merely available. The frame carries its own
buttons and its name plate, and the client-file tests say every one of those
pictures — up and pressed, on both frames — is one the client ships. Seven of
those eleven pictures now send something: the toggle asks for a stance and is
drawn from the answer, Log Out leaves the world, Quests opens the shard's own
dialog, and the rest are in decision 8's table.

Done for dialogs: a `0xB0` opens where the shard asked, drags by its own
background, closes on the right button with an answer of button zero (unless
`{ noclose }`), presses its buttons in their pressed art, flips its pages, keeps
its switches, takes typing into its fields, and draws its own text — with no
egui window anywhere in it.

**Seeing a window's layout without running the client**:
`crates/client/render/tests/gumpshot.rs` composites a laid-out window out of the
same quads the GPU pass draws and writes it to `target/gumps/`. It is
`artshot.rs` one layer up — that one answers what the artist drew, this one
answers where we put it — and it is how the paperdoll's buttons and the
nine-slice's seams were argued rather than guessed:

```sh
OPENSHARD_CLIENT=… cargo test -p openshard-client-render --test gumpshot \
    -- --ignored --nocapture
```

