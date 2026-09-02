# The client: where it stands

The canon of the `client` domain — `crates/client/net`, `crates/client/model`,
`crates/client/app`, and the half of `crates/common/uofiles` a renderer needs
that a shard never did. The renderer itself is a domain of its own,
[`render/`](../render/README.md), and the three readers that ask a question about
the map rather than about the picture live in [`world/`](../world/README.md).

**One entry point.** This page answers "what does this client do today" and says
which document holds the reasoning for each line. It used to be one 4,506-line
document that was a plan, a design, a phase record and twenty-one backlogs at
once. Where this page and a design document disagree, the design document is
right and this page is stale.

The work that is still ahead is in [`plans/client/`](../../plans/README.md), not
here.

## The one-line answer

**The shard's own client connects, walks, draws the world and its interface, and
plays the shard's sound — with no reference client anywhere in the loop.** It
speaks the protocol in the direction a client reads it, reads the player's own
installation for every picture and every sample, draws ground stretched over its
corner heights with statics and mobiles ordered by one depth the three passes
share, and owns twelve kinds of window of its own. It logs into a shard through
a transport that is a parameter at both ends, so the same login machine and the
same framing run over a socket and over a pair of in-memory pipes.

**What is not a client yet:** it holds one session, nothing that breathes can be
clicked, and nothing blends — which is one missing pass and five features
waiting behind it.

## Before running anything

```sh
cargo run -p openshard-playground
```

That is a shard in a thread and a window logged into it, with **no port bound and
no socket opened** — the two joined by `tokio::io::duplex`. It reads
`openshard.toml`, so a character remembers where it stood, and it logs in under a
stock development account.

`cargo run -p openshard-client-app` **without `--account` is an offline map
viewer, not a client**: there is no network in it at all. Three symptoms are the
same fact — the body starts at the same tile every time, it is drawn as a
placeholder body in no hue, and nothing survives a restart.

A separate `cargo run -p openshard-server` is only for the shard as a real
network service on port 2593 — under the stock client, or ClassicUO.

## Readiness, by subsystem

| Subsystem | State | What is left | Held by |
|---|---|---|---|
| The protocol read in the client's direction — the server length table, both missing halves, incremental Huffman | ✅ shipping | a decoder gap is silent: `ServerPacket::decode`'s list is shorter than the encode side's and nothing compares them | [`design_net.md`](design_net.md) |
| `client/net`: a sans-io `Connection`, the two-socket login machine, `WorldView`, the walk's acks | ✅ shipping | — | the same |
| The transport is a parameter at both ends — `Dial` for the client, `Gate` for the gateway | ✅ shipping | a third `Dial` (WebSocket, for a browser) is expected and unscheduled | the same |
| One word stops a shard, level-triggered, and `run_shard` returns with the world on disk | ✅ shipping | `SIGTERM`, the outbox, and telling the player why — [`shutdown.md`](../shutdown.md) | the same |
| The client's own data files in `common/uofiles` | 🟡 most of them | `unifont`, `Bodyconv.def`, `Sound.def`, `TexTerr.def`, `anim2`–`anim5` addressing and the UOP animations are each an absence with a visible symptom — rows 3, 7 and 12 below | [`design_picture.md`](design_picture.md) |
| The picture: ground stretched over four corner heights, textured from `texmaps`, statics and mobiles, one CPU ordering all three passes share | ✅ shipping | the ground is not screen-culled; a normal is computed nowhere, so nothing is lit off the terrain | the same |
| A pass that blends | ⬜ | one pass, and five features behind it — row 2 below | the same |
| The camera's geometry: two pixel spaces with a type each, an exact inverse pair, a zoom ladder applied once | ✅ shipping | — | [`design_camera_shell.md`](design_camera_shell.md) |
| The camera rig and the bench it is chosen on | 🟡 C0–C2, C4 and C7 built | the spring, the intent and the anchors are three empty stages of a built pipeline | [`design_camera_rig.md`](design_camera_rig.md), [`plans/client/camera/PLAN.md`](../../plans/client/camera/PLAN.md) |
| The walk: one movement state, a commanded pace, prediction with the ack invisible, resync on a real disagreement | ✅ shipping | the mount's two rates have nothing to select them — row 8 | [`design_walk.md`](design_walk.md) |
| Routing over this end's own reading of the ground, doors named, a refusal with a reason | ✅ shipping | rows 5 and 9 | the same |
| The window layer: twelve kinds, each a pane owning its state and its input, one router | ✅ shipping | rows 1 and 6 | [`design_panes.md`](design_panes.md) |
| Gump art, `0xB0` dialogs, containers, the paperdoll and its buttons, the skill sheet, the status frame | ✅ shipping | `{ checkertrans }`, `{ html }`, four buttons with no packet, and no window remembers where it was — rows 6, 10, 14 | [`design_windows.md`](design_windows.md) |
| Picking: ground items against the picture, the double click, the highlight | ✅ shipping | — | [`design_picking.md`](design_picking.md) |
| Picking a mobile | ⬜ | four of its five decisions are already taken by `items::pick` | [`plans/client/mobile_picking/PLAN.md`](../../plans/client/mobile_picking/PLAN.md) |
| Sound and music out of the player's own archive, two remembered gains | ✅ shipping | rows 11 and 12 | [`design_audio.md`](design_audio.md) |
| The two effect packets | ⬜ | `0x70` and `0xC0` have no decode arm — row 4 | the same |
| Sessions: `[shard → {characters}]`, files loaded once per install | ⬜ | the whole of it; two things it makes blocking are rows 13 and 15 | [`plans/client/sessions/PLAN.md`](../../plans/client/sessions/PLAN.md) |
| Map-block LOD off the block's projected footprint | 🟡 LOD1 rolled out, LOD2 held back | the live soak that gates LOD2 | [`design_lod.md`](design_lod.md), [`plans/client/lod/PLAN.md`](../../plans/client/lod/PLAN.md) |
| The dev HUD, five tabs, written to `client_ui.toml` at exit | ✅ shipping | nothing is written until a clean exit; row 17 | [`design_hud.md`](design_hud.md) |
| Frame pacing | ✅ release is vsync-locked at 60 | the claim wants one run on the operator's own desktop rather than a headless one | [`evidence/2026-08-14-client-jank.md`](evidence/2026-08-14-client-jank.md) |

## What is open, ranked

Every entry below was a bullet in one of the thirty backlog sections the client's
documents each kept for themselves. A finding with a defect behind it is a row
here; a finding with nothing behind it stayed where it was measured, in
[`evidence/2026-08-30-the-client-backlog.md`](evidence/2026-08-30-the-client-backlog.md).

**1. 🚩 egui is painted over this client's own windows and takes their mouse with
it.** The gump pass draws into the surface and the shell loads over it, and
`Shell::on_window_event` claims the click first — so `close_window_under_pointer`
never hears the right button that would have closed it. Not hypothetical: the dev
window opens at `(16, 48)` and is 360×420, `CONTAINER_ORIGIN` is `(120, 80)` and
the skill scroll is 345 wide, so the window a player is most likely to open lands
*entirely* inside the panel that eats its clicks. Escape is a way out, not a fix.
The decision nobody has taken is which of two: the gump pass draws after egui and
the pointer is offered to our windows first (the reference's own order — the
game's interface is on top and the dev shell is a tool), or the windows cascade
into the world's viewport, which is a constant answering a layering question.

**2. Nothing blends, and one pass unlocks five things at once.** Every pass
writes depth and tests it, which is why `cutaway.rs` *cuts* where the reference
*fades*. A fourth pass after the mobiles, reading the depth the other three wrote
and writing none of its own, is the whole of it — and behind it are
`ProcessAlpha` as a ramp rather than a predicate, `IsTranslucent`, foliage as a
union that fades as one, `HasSurfaceOverhead`, and the circle of transparency
that lets a player behind a wall see themselves. It is also what a ghost is
waiting for: the reference draws one translucent and here a ghost and a living
player are the same picture.

**3. Every body that lives only in `anim2`–`anim5` draws nothing at all.**
`Bodyconv.def` is the lookup that says which file holds a body, and it is not
read; on the Felucca spawn set that is bodies 752 and 764–794 among others, tens
of spawn points, each of them a creature that hits a player from an empty tile.
`Body.def` redirects *are* applied. This is a file-reader gap and not a renderer
change; the UOP animations are the same gap on the install people actually have.

**4. A spell is heard and not seen.** `0x6E` and `0xE2` decode and are folded onto
the crowd; `0x70` and `0xC0` have no arm in `ServerPacket::decode` at all, so a
bolt or a sparkle the shard throws is a packet this client drops.

**5. A pier or a bridge draws the body sinking into the water under it.** One bug
with two causes, reported by a player on 2026-08-02. `GroundQuad` builds its four
heights from the land layer only, so a platform static — a pier, a bridge, stairs
— is drawn as a sprite over a ground plane that was never the deck; and the step
onto it predicts its `z` from the same land layer, so the body is placed at the
ravine floor. `App::walk`'s offline path has the identical gap. Both ends of it
have to move together or the walk rubber-bands.

**6. Nothing remembers where a window was.** Four separate backlogs ask for it —
the paperdoll's, the skill sheet's, the container's and Escape's — and the last
one is the argument: with no memory the *only* place a window can open is the
cascade, so a bad cascade is not something a player can work around by putting
the window somewhere sensible once. The skill tree's own state (which headings
are shut, where the list is scrolled) goes with it. `desk.rs` is where it
belongs; a `0xB0` dialog is the hard half, because it is keyed by a serial that
does not survive a logout.

**7. A shard writing anything past Latin-1 gets those glyphs skipped.** Gump text
is drawn in `fonts.mul`'s face 1 and the reference's is `unifont.mul`, for which
there is no reader. The journal wants the same one the day a shard says something
in Cyrillic. Beside it, and separately: `fonts.mul`'s ~6×10 one-bit glyphs do not
survive a fractional scale and nothing here picks an integer one — the cheap
answer (render the atlas at an integer multiple of 1× chosen from the window's
DPI) should be tried and looked at before either of the other two is planned.

**8. A mounted mobile is glided at half the speed it is really moving.**
`WALK_HOLD`/`RUN_HOLD` are the two on-foot rates; the reference's `WalkMount` and
`RunMount` have nothing here to select them, because the mount is not on the
movement packet at all — it is an equipment layer. It wants the same mount
layering that "equipment, mounts and corpses" does and should land with it.

**9. The z-span overlap test exists twice, and it decides whether two ends of a
wire agree.** `Obstructions::blocker_at_z` on the shard and `Clutter::blocked_at`
on the client, with `MOBILE_HEIGHT` written out on both. It is the same
arithmetic on the same units; it belongs in `common/movement` as one function
both call. Two neighbours of it are the same shape: a placed item that is
`Surface` but not `Impassable` blocks nothing on either end where the reference
weighs it, and multis are not items, so the moment multis land this end will walk
into their walls exactly the way it walked into barrels.

**10. Four paperdoll buttons press and send nothing**, and it is four missing
packets and one missing window rather than a gesture: Help (`0x9B`), the profile
scroll (`0xB8`) and the party manifest (`0xBF 0x06`) are not in
`openshard_protocol` at all, and Options is a client window of our own that does
not exist. Four `0xBF` subcommands are undecoded for the same honest reason — the
context menu, the spellbook's contents, the stat locks and the map change each
need a reader on this end that wants them.

**11. A working sound reads as a missing one.** `Sound.def` is the alias table
that redirects an id whose own archive slot is empty; this install's copy carries
437 live redirects beside 351 explicitly dead ones. Without it a legitimate sound
is reported as absent from the install.

**12. Distance attenuation is rodio's, not the reference's, and nothing tests
it.** A `0x54` goes through an inverse-distance law with no cutoff; ClassicUO
takes Chebyshev distance, attenuates linearly and plays nothing past the view
range. What hides the difference today is the shard's own broadcast range — a
different number in a different crate deciding a rule this client owns.

**13. A whole `WorldView` is cloned per changed packet.** One standing character
makes it invisible; a crowded bank clones the map of every mobile each of them
can see to say that one of them turned. The answer is probably a shared snapshot
the window reads rather than a delta protocol between the two threads — and it
wants measuring before the session count multiplies it, not after.

**14. `shell.rs` is 7,401 lines**, and the backlog entry asking for it to be split
was written when it was 1,800. The panels are cleanly separable — one function per
tab, plus the overlays — and the file is four times past the point the entry
called obvious.

**15. The facet is a startup constant, and it is read twice in one process.** The
app loads Felucca and warns when the shard's map size differs rather than
following it; `0xBF 0x08` is what says a session moved. Beside it, the playground
holds two copies of a few hundred megabytes because `WorldMap` is loaded from a
path by each end and neither knows the other is in the room. Both stop being
tidiness and become blocking the moment there is more than one session.

**16. A body the shard sent and this end did not draw is a fact the client knows
and refuses to say.** `mobiles::collect` drops a body whose atlas has no frame,
silently — and from outside that is indistinguishable from the cutaway hiding it,
from the atlas missing a group, and from the mobile never arriving. It cost three
separate hunts, and the third time it *was* the defect. Two live hazards sit
under it: the atlas is grown for one animation group and drawn from another (no
hole opens today, and the ordering is an accident rather than an invariant), and a
mobile whose body frame is missing still draws its equipment, so a floating hat
walks about with no wearer.

**17. The world's markers are egui shapes and the terrain overlay is thousands of
polygons a frame.** A highlight on a tile a body stands on is drawn over the
body, where the ground it lies on is behind it; and the overlay emits a filled
diamond per visible tile — 7,600 at 1600×1000, about 2 ms of frame build. Both
are one instanced quad draw in the world pass with the ground's own depth. Fine
while the overlay is a toggle that is off; the wrong shape the moment anything
wants it on.

**18. `0xC1` decodes and nothing does anything with it.** `LocalizedMessage` is
the packet every gate's refusal travels on — "that cannot be used directly" and
its kin — and it had no decoder at all until the skill window's gate needed one.
It is readable now and still invisible to a player, because nothing in
`WorldView::apply` folds it into the journal or the chat line.

Three debts that are not defects and are worth not rediscovering: the walk's
oracle pins `Turning::Immediate` so the shipped turn delay is covered only by
unit tests; `App::apply_close_window` still writes a window's closure twice, which
is safe *only* because the link thread publishes exactly one snapshot and stops
being safe the day it publishes a second; and three booleans on `Steering`
(`crossing`, `walking`, `turned`) are a state machine written as flags.

## Which document holds what

**Design — how it works today:**

- [`design_net.md`](design_net.md) — the wire in the client's direction, the
  login machine, the transport as a parameter, and the one word that stops a
  shard.
- [`design_picture.md`](design_picture.md) — the data files, the three passes and
  the one depth ordering they share; why a static's zero pixel is absent and a
  land sprite's is black; the half-texel inset.
- [`design_camera_shell.md`](design_camera_shell.md) — the geometry: two pixel
  spaces with a type each, the invertible pair, the zoom ladder applied once in
  the blit, the lock, and the egui shell.
- [`design_camera_rig.md`](design_camera_rig.md) — 🚩 **how the eye follows the
  body, and the bench a camera is chosen on.** Eleven decisions, because several
  cameras are wanted and which one is right is not knowable from a document.
- [`design_walk.md`](design_walk.md) — 🚩 **one movement state, one clock, and
  what a refusal means.** Read it before touching anything that sends a step: the
  input rule, the detour's four tiles, the two leeways, the lean, the two rings,
  and the drain-then-resync cycle.
- [`design_windows.md`](design_windows.md) — what each window *is*: the two
  overlapping index spaces a gump atlas keeps apart, why a window has no size, the
  paperdoll's order tables, and the dialog that stopped being an egui window.
- [`design_panes.md`](design_panes.md) — who *owns* each window: readonly context
  in, mutations out, one router, and no window's state on the manager.
- [`design_picking.md`](design_picking.md) — a hit is an opaque texel of the
  picture that was drawn, and the placement a pick replays is `collect`'s own.
- [`design_audio.md`](design_audio.md) — the archive, the music config's loop
  flag, and the mixer that is an `Option` rather than a trait with a null sink.
- [`design_lod.md`](design_lod.md) — map-block LOD from the block's projected
  physical footprint, not from a zoom rung.
- [`design_hud.md`](design_hud.md) — the five tabs, the desk file, and the rule
  that catches a knob wired to nothing.
- [`design_decisions.md`](design_decisions.md) — the seven standing decisions:
  no engine, the browser as a constraint now, colour never converted, and the
  claimed version belonging to a shard.

**Evidence — measurements, phase records and closed handoffs:**

`evidence/` is dated files and no index; `ls` is the index. The ones a session is
most likely to want:

- [`evidence/2026-08-30-the-client-backlog.md`](evidence/2026-08-30-the-client-backlog.md)
  — thirty backlog sections in the order they were filed. Every measurement in
  this domain that is not in a design document is in it.
- [`evidence/2026-08-27-movement-state-refactor.md`](evidence/2026-08-27-movement-state-refactor.md)
  — the eight places one player position used to live, and the ragged gallop the
  hand-over left behind.
- [`evidence/2026-08-17-the-pane-router.md`](evidence/2026-08-17-the-pane-router.md)
  — S0 to S8, one window kind at a time, and what each move changed for a player.
- [`evidence/2026-08-14-the-camera-rig-record.md`](evidence/2026-08-14-the-camera-rig-record.md)
  — C0 to C7 with the tables each was decided on.
- [`evidence/2026-08-14-client-jank.md`](evidence/2026-08-14-client-jank.md) —
  what release actually costs, and the twenty-frame debug build that was a
  `Cargo.toml` profile rather than an algorithm.
- [`evidence/2026-08-15-tooltips.md`](evidence/2026-08-15-tooltips.md),
  [`evidence/2026-08-15-the-channel-selector.md`](evidence/2026-08-15-the-channel-selector.md),
  [`evidence/2026-08-17-the-amount-picker.md`](evidence/2026-08-17-the-amount-picker.md),
  [`evidence/2026-08-17-a-wheel-notch-out-of-the-shop-list.md`](evidence/2026-08-17-a-wheel-notch-out-of-the-shop-list.md),
  [`evidence/2026-09-01-a-click-that-cannot-be-routed.md`](evidence/2026-09-01-a-click-that-cannot-be-routed.md)
  — five features, each with what is still not right about it.
- [`evidence/2026-08-15-one-owner-for-a-window.md`](evidence/2026-08-15-one-owner-for-a-window.md),
  [`evidence/2026-08-14-lod1-rollout.md`](evidence/2026-08-14-lod1-rollout.md),
  [`evidence/2026-08-12-client-architecture.md`](evidence/2026-08-12-client-architecture.md)
  — three closed handoffs.

**Plans — what is not built:**
[`plans/client/sessions`](../../plans/client/sessions/PLAN.md),
[`mobile_picking`](../../plans/client/mobile_picking/PLAN.md),
[`camera`](../../plans/client/camera/PLAN.md),
[`lod`](../../plans/client/lod/PLAN.md).

**Neighbours.** [`render/README.md`](../render/README.md) is what draws a lit
frame; [`world/README.md`](../world/README.md) owns the map, the search over it,
and the three readers that bake off terrain — the radar raster, the building
flood and the roof cutaway — because what each of them asks is a question about
the map. [`client_versions.md`](../client_versions.md) is which client this one
claims to be, [`findings.md`](../findings.md) is what the reference client
actually does, and [`shutdown.md`](../shutdown.md) is the half of stopping that
is still owed.
