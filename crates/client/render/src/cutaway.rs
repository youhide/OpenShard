//! What the frame does *not* draw.
//!
//! [`crate::depth`] answers "which of two things is in front". This module
//! answers the question that comes before it: whether the thing is drawn at
//! all. Two different rules, both ported from ClassicUO, and they are here
//! together because both are decided from the tile the player is standing on:
//!
//! - **The cutaway.** Walk into a building and the roof has to come off, or the
//!   player is a picture of a roof. The client works out three numbers from the
//!   player's own tile and the one diagonally in front of it — `_maxZ`,
//!   `_maxGroundZ`, `_noDrawRoofs` — and every object is tested against them.
//!   `GameScene.UpdateMaxDrawZ`.
//! - **The ceiling.** Nothing whose top reaches past a fixed height above the
//!   ground is drawn, whatever the cutaway says: `GameScene` passes a literal
//!   `150` into `AddTileToRenderList` and `CalculateObjectHeight` is what turns
//!   a static into a top.
//!
//! # The tile's own order
//!
//! Both rules walk a tile's objects in the client's own order, which is the
//! order `Chunk.AddGameObject` keeps its per-tile linked list in: by
//! `PriorityZ`, and on a tie the land tile first. That order is not only this
//! module's — it is the order the passes draw in, and the depth test is what
//! enforces it there (see [`crate::depth`]). Here it is a list, because the
//! rules `break` out of the walk partway and "which came first" is the whole
//! question.
//!
//! # What is deliberately absent
//!
//! The reference client fades an object above `_maxZ` down 25 alpha units per
//! frame, then drops it at zero. This crate's predicates are still that ramp's
//! endpoints — lighting and picking need a binary answer — but the picture is
//! no longer forced to its endpoint in one opaque draw: [`crate::statics`]
//! holds cutaway architecture aside, writes it into a private G-buffer after
//! mobiles, then deferred-lights and blends it without writing the main depth.
//! It also uses the player's screen rectangle to put an ordinary wall that
//! actually covers the body in that layer.
//!
//! The temporal per-object alpha walk is owned by [`Fades`]. Foliage uses the
//! same late primitive with a canopy-tile key, while profile-controlled
//! transparency radii remain separate work.

use std::collections::{
    BTreeMap,
    HashMap,
};

use openshard_map::map::WorldMap;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_tiles::{
    StaticTile,
    TileData,
    TileFlags,
};

use crate::depth;
use crate::geometry::Rect;
use crate::ground::corner_heights;
use crate::items::GroundItem;

/// How tall the client assumes a body is, for the ceiling test.
///
/// `Constants.DEFAULT_CHARACTER_HEIGHT`. Not a mobile's art, which is taller —
/// this is the number the client adds to a mobile's `PriorityZ` before asking
/// whether it is under the ceiling.
pub const CHARACTER_HEIGHT: i32 = 16;

/// Opacity of an architectural sprite while it is in the player's cutaway.
///
/// The deferred cutaway blit uploads this once in its uniform block, after
/// lighting and immediately before premultiplied source-over composition. It
/// stays a product-facing constant rather than becoming a field on every
/// ordinary [`crate::sprite::SpriteQuad`].
pub const TRANSLUCENT_ALPHA: f32 = 0.4;

/// The reference client's `CalculateAlpha` step, in its byte-alpha domain.
pub const ALPHA_STEP: u8 = 25;
/// [`TRANSLUCENT_ALPHA`] in that same domain, rounded to the nearest byte.
///
/// `+ 0.5` and a truncating cast rather than `f32::round`, which is not `const`
/// below 1.90 — the workspace is pinned at 1.88 while `deno_core` is in the tree.
/// Both alphas are positive, which is the case where the two agree exactly.
pub const TRANSLUCENT_ALPHA_U8: u8 = (TRANSLUCENT_ALPHA * 255.0 + 0.5) as u8;

/// Opacity of a foliage canopy while the player's body is underneath it.
pub const FOLIAGE_ALPHA: f32 = 0.4;
/// [`FOLIAGE_ALPHA`] in the byte-alpha domain used by sprite instances. Rounded
/// the same way, and for the same reason, as [`TRANSLUCENT_ALPHA_U8`].
pub const FOLIAGE_ALPHA_U8: u8 = (FOLIAGE_ALPHA * 255.0 + 0.5) as u8;

/// The stable world identity of an object whose opacity is changing.
///
/// Map statics and server items can share a tile and a graphic, so the producer
/// remains part of the key. A static animation deliberately keeps its placed
/// graphic here, rather than whichever frame the atlas is showing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct FadeKey {
    at:      (u16, u16, i8),
    graphic: Graphic,
    item:    bool,
    foliage: bool,
}

impl FadeKey {
    pub const fn static_(at: Point, graphic: Graphic) -> Self {
        Self {
            at: (at.x, at.y, at.z),
            graphic,
            item: false,
            foliage: false,
        }
    }

    pub const fn item(at: Point, graphic: Graphic) -> Self {
        Self {
            at: (at.x, at.y, at.z),
            graphic,
            item: true,
            foliage: false,
        }
    }

    /// A canopy identity is the placed world tile, not an animated frame.
    pub const fn foliage(at: Point) -> Self {
        Self {
            at:      (at.x, at.y, at.z),
            graphic: Graphic(0),
            item:    false,
            foliage: true,
        }
    }
}

/// Persistent per-object cutaway opacity.
#[derive(Default, Debug)]
pub struct Fades(BTreeMap<FadeKey, u8>);

impl Fades {
    /// Whether no object is part-way through a cutaway transition.
    ///
    /// A cached static list is safe only between fade transitions: while this
    /// map is non-empty the collector must keep advancing the affected rows.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Advance one reference-style 25-point step toward `target`.
    ///
    /// Fully opaque objects need no retained state, while a zero-alpha hidden
    /// object remains remembered at zero so that returning it starts a fade-in
    /// rather than popping straight back to opaque.
    pub fn advance(&mut self, key: FadeKey, target: u8) -> u8 {
        let alpha = self.0.get(&key).copied().unwrap_or(u8::MAX);
        let next = if alpha < target {
            alpha.saturating_add(ALPHA_STEP).min(target)
        } else {
            alpha.saturating_sub(ALPHA_STEP).max(target)
        };
        if next == u8::MAX {
            self.0.remove(&key);
        } else {
            self.0.insert(key, next);
        }
        next
    }
}

/// The height nothing is drawn above, whatever the cutaway decides.
///
/// A literal `150` in `GameScene.Update`, passed into `AddTileToRenderList` as
/// its `maxZ` argument and compared against a *top* — an object's `PriorityZ`
/// plus its own height. It is not the world's ceiling (`z` is an `i8` and goes
/// to 127) and not the player's: it is a fixed lid, and what it removes is the
/// mountain tops that would otherwise hang over a valley from ten tiles away.
pub const DRAW_CEILING: i32 = 150;

/// The height the client gives a static that declares none.
///
/// `CalculateObjectHeight`: a tile with a height of zero that is neither
/// background nor surface — so, something standing up that forgot to say how
/// far — is treated as ten. A signpost, not a rug.
const ASSUMED_HEIGHT: u8 = 10;

/// What a frame stops drawing so that the player can be seen.
///
/// Three numbers, exactly the client's, and they are meaningless apart: `max_z`
/// alone would take the walls with the roof, and `no_draw_roofs` alone would
/// leave the floor of the storey above.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cutaway {
    /// A static at or above this height is not drawn. `GameScene._maxZ`.
    pub max_z:         i32,
    /// A land tile above this height is not drawn. `GameScene._maxGroundZ`.
    pub max_ground_z:  i32,
    /// No roof is drawn at all, wherever it stands. `GameScene._noDrawRoofs`.
    pub no_draw_roofs: bool,
}

impl Cutaway {
    /// Nothing cut away: outdoors, with roofs on.
    ///
    /// `127` is the top of the `z` range, so the two tests can be written the
    /// same way whether or not anything was cut — there is no "no limit" case
    /// to branch on, which is what the client's own `127` is for.
    pub const OPEN: Self = Self {
        max_z:         127,
        max_ground_z:  127,
        no_draw_roofs: false,
    };

    /// What is cut away with the player standing here.
    ///
    /// A port of `GameScene.UpdateMaxDrawZ`, which reads two tiles: the
    /// player's own, and the one diagonally down-screen from it. The second is
    /// not a neighbourhood scan — it is where a roof over the player's head
    /// *draws*, because a roof tile stands one tile south-east of the space it
    /// covers on screen.
    ///
    /// `draw_roofs` is the client's `Profile.DrawRoofs`, the setting that turns
    /// roofs off everywhere; passing `true` is the ordinary case.
    ///
    /// Off the map the answer is [`Cutaway::OPEN`], the same as the client's
    /// null chunk.
    pub fn at(
        map: &WorldMap,
        tiledata: &TileData,
        player: openshard_protocol::world::Point,
        draw_roofs: bool,
    ) -> Self {
        Self::at_with_items(map, tiledata, player, draw_roofs, &[])
    }

    /// What is cut away when server items stand over the player too.
    ///
    /// A placed multi reaches the renderer as one [`GroundItem`] per component,
    /// not as map statics. Leaving those pieces out makes a player house an
    /// impossible building: its floor affects movement, but its roof never
    /// reaches the very cutaway that is meant to reveal the body beneath it.
    pub fn at_with_items(
        map: &WorldMap,
        tiledata: &TileData,
        player: openshard_protocol::world::Point,
        draw_roofs: bool,
        items: &[GroundItem],
    ) -> Self {
        let (px, py) = (player.x, player.y);
        if map.land(px, py).is_none() {
            return Self {
                no_draw_roofs: !draw_roofs,
                ..Self::OPEN
            };
        }
        let pz = i32::from(player.z);
        // The client's two thresholds, and they are not the same question:
        // `+14` is "is this above head height", which is what makes a static a
        // candidate for cutting; `+16` is a whole body, which is what the
        // player has to be buried under before the *ground* is cut.
        let pz14 = pz + 14;
        let pz16 = pz + 16;

        // `maxGroundZ`, the local the client keeps beside the field of the same
        // name — and assigns to the field on the very last line of the method,
        // throwing away everything the second tile computed for it. That looks
        // like a bug and is load-bearing: it is what stops a roof over the
        // player from also hiding the ground, and it leaves the ground cut for
        // exactly one case — the player buried under it.
        let mut kept_ground_z = 127;
        let mut max_z = 127;
        let mut no_draw_roofs = !draw_roofs;
        let items = ItemStacks::of(items);

        for piece in stack_with_items(map, tiledata, &items, px, py) {
            let Some(tile) = piece.tile else {
                // The land. Its height is the average of the corners it is
                // stretched over, which is what `Land.AverageZ` is.
                if pz16 <= i32::from(piece.z) {
                    kept_ground_z = pz16;
                    max_z = pz16;
                    break;
                }
                continue;
            };
            let tile_z = i32::from(piece.z);
            if tile_z > pz14 && max_z > tile_z && cuts_from_above(tile) {
                max_z = tile_z;
                no_draw_roofs = true;
            }
        }

        // The tile the roof over this one is *drawn* on. `_maxGroundZ` tracks
        // `_maxZ` from here on, which is why it is a separate variable at all.
        let mut max_ground_z = max_z;
        let mut settled_z = max_z;
        let (nx, ny) = (px.saturating_add(1), py.saturating_add(1));
        if map.land(nx, ny).is_some() {
            for piece in stack_with_items(map, tiledata, &items, nx, ny) {
                let Some(tile) = piece.tile else { continue };
                let tile_z = i32::from(piece.z);
                // A roof, and *only* a roof: a wall standing on the tile in
                // front is not what the player is under.
                //
                // The mask is `0x204` in the client, unnamed there and pinned
                // by a test here: Surface and Transparent, *not* Climbable.
                // A rooftop you can walk up onto carries `Climbable`, and
                // reading that bit into this mask is the difference between
                // the roof coming off and the player being drawn under it —
                // which is exactly what a stair to an upper level walks into.
                if tile_z > pz14
                    && max_z > tile_z
                    && !tile.flags.has(TileFlags::PLATFORM | TileFlags::TRANSPARENT)
                    && tile.flags.is_roof()
                {
                    max_z = tile_z;
                    max_ground_z = i32::from(near_roof_z(map, tiledata, &items, piece.z, nx, ny, piece.z));
                    no_draw_roofs = true;
                }
            }
            settled_z = max_ground_z;
        }

        max_z = max_ground_z;
        // A player standing on something tall enough to be its own storey —
        // a bridge, a house's second floor — is not under anything, and the
        // roof found above would otherwise cut the floor out from under them.
        if settled_z < pz16 {
            max_z = pz16;
        }

        Self {
            max_z,
            max_ground_z: kept_ground_z,
            no_draw_roofs,
        }
    }

    /// Whether a static standing at `z` is drawn.
    ///
    /// `GameScene.ProcessAlpha`, with the fade collapsed to its endpoint: at or
    /// above `max_z` it goes, and a roof goes whatever its height once the
    /// player is under one.
    pub fn shows_static(&self, z: i8, tile: &StaticTile) -> bool {
        self.shows_at(i32::from(z), tile.flags.is_roof())
    }

    /// The same rule with the tiledata row already read: a height and whether the
    /// art calls it a roof, which is the whole of what the test looks at.
    ///
    /// It exists because [`crate::occlusion`] asks this question about a surface
    /// long after the static it came from is gone — `docs/lighting.md`'s decision
    /// 33 moved the cut to where a *frame's* grid is packed — and a second
    /// spelling of the rule there would be a second policy rather than a second
    /// caller.
    pub fn shows_at(&self, z: i32, roof: bool) -> bool {
        if z >= self.max_z {
            return false;
        }
        !(self.no_draw_roofs && roof)
    }

    /// Whether a land tile at `z` is drawn.
    ///
    /// The land's own `z` and not the average of its corners: that is what
    /// `AddTileToRenderList` compares, and the two differ on a slope.
    pub fn shows_land(&self, z: i8) -> bool {
        i32::from(z) <= self.max_ground_z
    }

    /// Whether a mobile standing at `z` is drawn.
    ///
    /// The same `max_z` test the statics get — a mobile on the floor above is
    /// hidden with that floor — and then the ceiling, which a mobile reaches
    /// with a body's height rather than a tile's.
    pub fn shows_mobile(&self, z: i8) -> bool {
        if i32::from(z) >= self.max_z {
            return false;
        }
        depth::mobile_priority_z(z) + CHARACTER_HEIGHT <= DRAW_CEILING
    }
}

/// Whether a static above the player's head cuts everything above it away.
///
/// `UpdateMaxDrawZ`'s first loop, written as the client wrote it: not foliage
/// and not transparent — those are things you see *through*, and a tree is not
/// a ceiling — and either not a roof, or a roof that is also a surface, which
/// is how a flat rooftop you can walk on is spelled.
fn cuts_from_above(tile: &StaticTile) -> bool {
    // `0x20004` in the client, unnamed. Foliage and Transparent.
    !tile.flags.has(TileFlags::FOLIAGE | TileFlags::TRANSPARENT)
        && (!tile.flags.is_roof() || tile.flags.is_platform())
}

/// How low the roof the player is under reaches, over the whole roof.
///
/// A port of `Map.CalculateNearZ`: a flood fill from one roof tile through
/// every neighbouring roof tile within six of it, returning the lowest height
/// any of them stands at. One roof is not one height — a gable steps down as it
/// goes — and cutting at the tile directly in front of the player would leave
/// the low end of the same roof drawn over them two tiles later.
///
/// The visited set is the client's: a 64x64 grid of flags indexed by the low
/// six bits of each coordinate. That is not a bug being carried over so much as
/// the fill's only bound — a roof wider than 64 tiles folds onto itself and the
/// walk stops — and it is what makes an unbounded recursion terminate.
fn near_roof_z(
    map: &WorldMap,
    tiledata: &TileData,
    items: &ItemStacks<'_>,
    default_z: i8,
    x: u16,
    y: u16,
    z: i8,
) -> i8 {
    let mut visited = vec![false; 64 * 64];
    fill(
        map,
        tiledata,
        items,
        default_z,
        (i32::from(x), i32::from(y)),
        z,
        &mut visited,
    )
}

/// One step of [`near_roof_z`]'s fill.
fn fill(
    map: &WorldMap,
    tiledata: &TileData,
    items: &ItemStacks<'_>,
    default_z: i8,
    at: (i32, i32),
    z: i8,
    visited: &mut [bool],
) -> i8 {
    let (x, y) = at;
    let slot = ((x & 0x3F) + ((y & 0x3F) << 6)) as usize;
    if visited[slot] {
        return default_z;
    }
    visited[slot] = true;
    let (Ok(tx), Ok(ty)) = (u16::try_from(x), u16::try_from(y)) else {
        return default_z;
    };
    if map.land(tx, ty).is_none() {
        return default_z;
    }
    // The first roof within six of the height that led here. Not the lowest on
    // the tile: the client breaks out of its walk on the first, and the walk is
    // in `PriorityZ` order.
    let found = stack_with_items(map, tiledata, items, tx, ty)
        .into_iter()
        .find(|piece| {
            piece
                .tile
                .is_some_and(|tile| tile.flags.is_roof() && i32::from(z).abs_diff(i32::from(piece.z)) <= 6)
        });
    let Some(found) = found else {
        return default_z;
    };
    let mut default_z = default_z.min(found.z);
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        default_z = fill(
            map,
            tiledata,
            items,
            default_z,
            (x + dx, y + dy),
            found.z,
            visited,
        );
    }
    default_z
}

/// Server statics grouped by tile for the handful of columns one cutaway reads.
///
/// A roof flood revisits neighbouring columns, so indexing once keeps a placed
/// castle from turning that walk into one full item-list scan per roof tile.
struct ItemStacks<'a>(HashMap<(u16, u16), Vec<&'a GroundItem>>);

impl<'a> ItemStacks<'a> {
    fn of(items: &'a [GroundItem]) -> Self {
        let mut stacks: HashMap<(u16, u16), Vec<&GroundItem>> = HashMap::new();
        for item in items {
            stacks.entry((item.at.x, item.at.y)).or_default().push(item);
        }
        Self(stacks)
    }

    fn at(&self, x: u16, y: u16) -> impl Iterator<Item = &'a GroundItem> + '_ {
        self.0.get(&(x, y)).into_iter().flatten().copied()
    }
}

/// One thing standing on a tile, in the terms the two rules above need.
///
/// Deliberately not "a game object": what a cutaway asks of a tile is its
/// height and its flags, and a `Piece` that also carried a graphic and a hue
/// would be a second, poorer copy of the types the passes already build.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Piece<'a> {
    /// The height the client tests against — for the land, the average of its
    /// four corners, which is what `Land.AverageZ` holds.
    pub z:          i8,
    /// The static's tiledata, or `None` for the tile's own land.
    pub tile:       Option<&'a StaticTile>,
    /// Where it sorts against everything else on the tile. See
    /// [`crate::depth`].
    pub priority_z: i32,
}

/// Everything standing on one tile, in the client's own order.
///
/// `Chunk.AddGameObject` keeps a linked list per tile and inserts into it by
/// `PriorityZ`; on a tie it puts the land first and everything else after
/// whatever is already there — so among statics the order is the order they
/// came out of the file. A stable sort over the same key is the same list.
///
/// Mobiles and dynamic items are not in this bare-map spelling: both rules skip
/// mobiles, while [`Cutaway::at_with_items`] adds the dynamic statics the shard
/// placed. That distinction is what lets diagnostics ask only about a map while
/// a live frame still sees a player house's roof.
pub fn stack<'a>(map: &WorldMap, tiledata: &'a TileData, x: u16, y: u16) -> Vec<Piece<'a>> {
    stack_with_items(map, tiledata, &ItemStacks::of(&[]), x, y)
}

/// [`stack`] with the dynamic statics the shard placed on the same tile.
fn stack_with_items<'a>(
    map: &WorldMap,
    tiledata: &'a TileData,
    items: &ItemStacks<'_>,
    x: u16,
    y: u16,
) -> Vec<Piece<'a>> {
    let mut pieces = Vec::new();
    if let Some(cell) = map.land(x, y) {
        let corners = corner_heights(map, x, y, cell.z).map(|z| z as i32);
        let priority_z = depth::land_priority_z(corners);
        pieces.push(Piece {
            // `land_priority_z` is the average less two, so the average is it
            // plus two — one formula, read in the direction each caller needs.
            z: (priority_z + 2) as i8,
            tile: None,
            priority_z,
        });
    }
    for item in map.statics_at(x, y) {
        let tile = tiledata.static_tile(item.tile.0);
        pieces.push(Piece {
            z:          item.z,
            tile:       Some(tile),
            priority_z: depth::static_priority_z(item.z, tile),
        });
    }
    for item in items.at(x, y) {
        let tile = tiledata.static_tile(item.displayed().0);
        pieces.push(Piece {
            z:          item.at.z,
            tile:       Some(tile),
            priority_z: depth::static_priority_z(item.at.z, tile),
        });
    }
    // Stable, and the land wins a tie: `AddGameObject`'s `state == 0` arm. The
    // stability is what keeps two statics at one height in the order the file
    // has them, which is the order the client draws them in.
    pieces.sort_by_key(|piece| (piece.priority_z, u8::from(piece.tile.is_some())));
    pieces
}

/// How far above its own `z` a static reaches, and the top that puts it at.
///
/// A port of `CalculateObjectHeight`, which is what the ceiling is compared
/// against. Three cases, and the odd one is the middle: a static that declares
/// no height at all is given ten unless it is something that lies flat, because
/// a height of zero on a standing object means "not filled in" rather than
/// "flat" — [`Option`] would say so, but the file has one byte.
///
/// A height of `0xFF` means the client draws it whatever: `CalculateObjectHeight`
/// returns without touching the top, so nothing can push such a tile past the
/// ceiling.
pub fn top_of(priority_z: i32, tile: &StaticTile) -> i32 {
    if tile.height == 0xFF {
        return priority_z;
    }
    let mut height = tile.height;
    if height == 0 && !tile.flags.is_background() && !tile.flags.is_platform() {
        height = ASSUMED_HEIGHT;
    }
    if tile.flags.is_climbable() {
        // A bridge or a stair is walked *up*, so half its art is below the
        // height it puts you at. The same halving Sphere does when it works out
        // where you end up standing.
        height /= 2;
    }
    priority_z + i32::from(height)
}

/// Whether a static is drawn at all, before anything about the player is asked.
///
/// The two unconditional rejects of `AddTileToRenderList`: a tile the client
/// marks internal is never drawn, and nothing whose top reaches past
/// [`DRAW_CEILING`] is either.
pub fn under_ceiling(priority_z: i32, tile: &StaticTile) -> bool {
    !tile.flags.is_internal() && top_of(priority_z, tile) <= DRAW_CEILING
}

/// Everything this module decides about one static, in one call.
///
/// Asked in two places — the map's statics and the server's ground items — for
/// the same reason they already share their placement: they are the same
/// picture standing the same way, and the client reads the same tiledata row
/// for both.
pub fn shows(cutaway: &Cutaway, z: i8, tile: &StaticTile) -> bool {
    drawn_in_any_frame(z, tile) && cutaway.shows_static(z, tile)
}

/// The half of [`shows`] that has nothing to do with where the player stands.
///
/// A static past the [`DRAW_CEILING`], or one the client marks internal, is
/// drawn in no frame from any tile — so a caller that is building something a
/// *frame* is later cut out of asks this and not [`shows`]. That caller is
/// `occlusion::collect`, and `docs/lighting.md`'s decision 33 is why it wants
/// the line drawn exactly here.
pub fn drawn_in_any_frame(z: i8, tile: &StaticTile) -> bool {
    under_ceiling(depth::static_priority_z(z, tile), tile)
}

/// Whether a foliage static's own picture is standing over the player's.
///
/// A tree's leaves in the reference client fade — `ApplyFoliageTransparency`,
/// `GameSceneDrawingSorting.CheckIfBehindATree` — walking a `TreeUnion` table
/// so a multi-tile canopy fades as one piece, at a fixed partial alpha. This
/// crate blends nothing yet (this module's own doc, above), so what lands here
/// is the fade's endpoint with neither refinement: no tree union, and hidden
/// outright rather than dimmed. `screen_rect` is the exact rectangle
/// [`crate::statics::stand_on`] draws the sprite at, in the same space
/// [`crate::mobiles::screen_rect`] answers for the player, so this is asking
/// whether the two pictures share a pixel — the same question the reference
/// answers with `Exstentions.InRect`.
///
/// Only asked of a tile [`TileFlags::is_foliage`] already says yes to: nothing
/// else in a frame hides for standing behind the player.
pub fn hides_foliage_over(player_rect: Rect, screen_rect: Rect) -> bool {
    player_rect.intersects(&screen_rect)
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::BlockExtent;
    use openshard_map::map::LandCell;
    use openshard_protocol::items::ItemAmount;
    use openshard_protocol::wire::Hue;
    use openshard_tiles::TileFlags;

    use super::*;

    /// The two unnamed masks `UpdateMaxDrawZ` tests with, pinned against the
    /// flags this workspace gave names to.
    ///
    /// `0x20004` is the first loop's and `0x204` the second's, and the second
    /// is the one worth a test of its own: `0x4` is Transparent and `0x200` is
    /// Surface, so the bit that is *not* in it is Climbable (`0x400`). Reading
    /// Climbable into that mask stops a walkable rooftop from ever being cut,
    /// which draws the roof over the player who just climbed onto it.
    #[test]
    fn the_two_cutaway_masks_are_the_flags_they_are_named_after() {
        assert_eq!(TileFlags::FOLIAGE | TileFlags::TRANSPARENT, 0x0002_0004);
        assert_eq!(TileFlags::PLATFORM | TileFlags::TRANSPARENT, 0x0000_0204);
        assert_eq!(TileFlags::CLIMBABLE, 0x0000_0400, "not in either mask");
    }

    fn tile(flags: u64, height: u8) -> StaticTile {
        StaticTile {
            flags: TileFlags::new(flags),
            height,
            ..StaticTile::default()
        }
    }

    /// The three arms of `CalculateObjectHeight`, stated as numbers.
    #[test]
    fn a_statics_top_is_its_own_height_unless_it_declares_none() {
        // A wall: 20 tall, standing at 0, so its top is 20.
        assert_eq!(top_of(0, &tile(0, 20)), 20);
        // A rug: flat and background, so flat is what it means.
        assert_eq!(top_of(0, &tile(TileFlags::FLOOR, 0)), 0);
        // A signpost: no height and not flat, so ten is assumed.
        assert_eq!(top_of(0, &tile(0, 0)), ASSUMED_HEIGHT as i32);
        // Stairs: half, because half the art is below where it puts you.
        assert_eq!(top_of(0, &tile(TileFlags::CLIMBABLE, 20)), 10);
        // And 0xFF is the client's "do not ask": the top is where it stands.
        assert_eq!(top_of(7, &tile(0, 0xFF)), 7);
    }

    /// The ceiling is a lid on tops, not on heights: a short thing standing
    /// high is what it removes, and a tall thing standing low is not.
    #[test]
    fn the_ceiling_cuts_by_where_a_thing_reaches() {
        assert!(under_ceiling(0, &tile(0, 20)));
        assert!(
            under_ceiling(140, &tile(0, 10)),
            "150 is the lid, and this is on it"
        );
        assert!(!under_ceiling(145, &tile(0, 10)));
        // Internal is never drawn, wherever it stands.
        assert!(!under_ceiling(0, &tile(TileFlags::INTERNAL, 0)));
    }

    /// An open cutaway hides nothing. The point of asserting it is that the
    /// three fields are `127`/`127`/`false` rather than an `Option`, so every
    /// test below is the same two comparisons with different numbers.
    #[test]
    fn an_open_cutaway_shows_everything() {
        let open = Cutaway::OPEN;
        assert!(open.shows_land(127));
        assert!(open.shows_static(126, &tile(0, 0)));
        assert!(open.shows_static(126, &tile(TileFlags::ROOF, 0)));
        assert!(open.shows_mobile(0));
    }

    /// Under a roof, the roof goes and the walls stay — which is the whole
    /// feature, and the half of it that a single `max_z` gets wrong.
    #[test]
    fn a_roof_goes_and_the_wall_under_it_stays() {
        let inside = Cutaway {
            max_z:         40,
            max_ground_z:  127,
            no_draw_roofs: true,
        };
        // The roof itself, at the height the cutaway was taken from.
        assert!(!inside.shows_static(40, &tile(TileFlags::ROOF, 3)));
        // A roof lower than `max_z` — the eaves — goes too, and that is what
        // `no_draw_roofs` is for on its own.
        assert!(!inside.shows_static(20, &tile(TileFlags::ROOF, 3)));
        // The wall the player is standing beside stays.
        assert!(inside.shows_static(0, &tile(TileFlags::WALL, 20)));
        // And so does the floor: `max_ground_z` was left alone.
        assert!(inside.shows_land(0));
    }

    /// A player house is a server item expanded into dynamic statics, so its
    /// roof has to enter the same two-tile question as a roof from the map.
    #[test]
    fn a_placed_house_roof_is_cut_away() {
        const ROOF: Graphic = Graphic(0x1234);
        let map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell::default());
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(ROOF.0, tile(TileFlags::ROOF, 3));
        let roof = GroundItem {
            // The second tile `UpdateMaxDrawZ` asks: where the roof over the
            // body is painted in the isometric projection.
            at:      Point::new(3, 3, 20),
            graphic: ROOF,
            hue:     Hue::NONE,
            amount:  ItemAmount::ONE,
        };

        assert_eq!(
            Cutaway::at(&map, &tiledata, Point::new(2, 2, 0), true),
            Cutaway::OPEN
        );
        let cutaway = Cutaway::at_with_items(&map, &tiledata, Point::new(2, 2, 0), true, &[roof]);

        assert!(cutaway.no_draw_roofs, "the dynamic roof did not open the cutaway");
        assert!(!cutaway.shows_static(roof.at.z, tiledata.static_tile(ROOF.0)));
        assert!(
            cutaway.shows_mobile(0),
            "the cutaway hid the body it exists to reveal"
        );
    }

    /// The ground is only ever cut when the player is under it, which is the
    /// consequence of the client's last line and worth pinning: a reader who
    /// "fixes" that line makes every roof take the floor with it.
    #[test]
    fn the_ground_is_cut_only_when_the_player_is_buried() {
        let buried = Cutaway {
            max_z:         16,
            max_ground_z:  16,
            no_draw_roofs: true,
        };
        assert!(buried.shows_land(16), "the tile at head height still draws");
        assert!(!buried.shows_land(17), "the hill above it does not");
    }

    /// Britain has buildings, and standing in one has to cut something.
    ///
    /// On the real map rather than a fixture, and the reason is the usual one:
    /// a fixture with a roof tile in it would be a roof this module's own
    /// understanding placed, and the two questions this test is actually asking
    /// — whether a real roof carries the flags `UpdateMaxDrawZ` looks for, and
    /// whether it stands on the tile the client expects to find it on — are
    /// exactly the ones a fixture answers by construction.
    ///
    /// It searches rather than naming a doorway, because a named coordinate is
    /// one more thing to be wrong about; what is asserted is that indoor tiles
    /// exist at all, that outdoor ones do too, and that the two are told apart
    /// by the *roof* and not by anything else.
    ///
    /// Skipped without the client's files.
    #[test]
    fn standing_inside_britain_takes_the_roof_off() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            return;
        };
        let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
        let tiledata =
            openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");

        // A block of Britain wide enough to hold whole buildings and the
        // streets between them.
        let (mut indoors, mut outdoors) = (0, 0);
        for y in 1600..1660u16 {
            for x in 1470..1530u16 {
                let z = map.land(x, y).expect("on the map").z;
                let cutaway = Cutaway::at(
                    &map,
                    &tiledata,
                    openshard_protocol::world::Point::new(x, y, z),
                    true,
                );
                if cutaway == Cutaway::OPEN {
                    outdoors += 1;
                    continue;
                }
                indoors += 1;
                // Whatever was cut, the ground under the player's feet is not
                // it: a cutaway that hid the floor would be a hole.
                assert!(cutaway.shows_land(z), "the floor at {x},{y} was cut away");
            }
        }
        assert!(
            indoors > 100,
            "only {indoors} tiles in Britain are under anything"
        );
        assert!(outdoors > 100, "only {outdoors} tiles in Britain are in the open");

        // And a tile with a real roof drawn over it has that roof cut. This is
        // the second of `UpdateMaxDrawZ`'s two loops — the one that reads the
        // tile diagonally down-screen, where a roof over your head is drawn —
        // and it is a separate assertion because the first loop can set
        // `no_draw_roofs` on its own, from an upper storey standing on the
        // player's own tile.
        let mut roofs_cut = 0;
        for y in 1600..1660u16 {
            for x in 1470..1530u16 {
                let z = map.land(x, y).expect("on the map").z;
                let over = stack(&map, &tiledata, x + 1, y + 1);
                let roof = over.iter().find(|piece| {
                    piece.tile.is_some_and(|tile| tile.flags.is_roof())
                        && i32::from(piece.z) > i32::from(z) + 14
                });
                let Some(roof) = roof else { continue };
                let cutaway = Cutaway::at(
                    &map,
                    &tiledata,
                    openshard_protocol::world::Point::new(x, y, z),
                    true,
                );
                assert!(
                    !cutaway.shows_static(roof.z, roof.tile.expect("a static")),
                    "the roof over {x},{y} at z {} is still drawn",
                    roof.z,
                );
                roofs_cut += 1;
            }
        }
        assert!(
            roofs_cut > 20,
            "only {roofs_cut} tiles in Britain have a roof drawn over them"
        );
    }

    /// The cutaway exists so that the player can be seen, so the one body it
    /// must never hide is the one it was computed from.
    ///
    /// On the real map and at every height a body can stand at, because the
    /// interesting cases are the ones a fixture would not think to build: a
    /// stairwell, a second storey, a tile that is under a roof and on a bridge
    /// at once.
    ///
    /// Skipped without the client's files.
    #[test]
    fn a_cutaway_never_hides_the_player_it_was_taken_from() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            return;
        };
        let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
        let tiledata =
            openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");

        let mut checked = 0;
        for y in 1600..1660u16 {
            for x in 1470..1530u16 {
                // Every surface on the tile is somewhere a body might stand:
                // the ground, a stair tread, the floor of the storey above.
                let heights: Vec<i8> = stack(&map, &tiledata, x, y)
                    .into_iter()
                    .map(|piece| {
                        match piece.tile {
                            None => piece.z,
                            Some(tile) => top_of(i32::from(piece.z), tile).clamp(-128, 127) as i8,
                        }
                    })
                    .collect();
                for z in heights {
                    let cutaway = Cutaway::at(
                        &map,
                        &tiledata,
                        openshard_protocol::world::Point::new(x, y, z),
                        true,
                    );
                    assert!(
                        cutaway.shows_mobile(z),
                        "standing at {x},{y},{z} hides the player: {cutaway:?}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 1000, "only {checked} standing places were tried");
    }

    /// A mobile is cut by the same `max_z` as a static, and by the ceiling with
    /// a body's height rather than a tile's.
    #[test]
    fn a_mobile_upstairs_is_cut_with_the_storey() {
        let inside = Cutaway {
            max_z:         40,
            max_ground_z:  127,
            no_draw_roofs: true,
        };
        assert!(inside.shows_mobile(0));
        assert!(!inside.shows_mobile(40));
        // The ceiling never cuts a mobile: the highest one an open cutaway
        // draws stands at 126, sorts at 127 and reaches 143, which is under the
        // lid of 150. Saying so is the point — the test fails if the body's
        // height is ever added twice.
        assert!(Cutaway::OPEN.shows_mobile(126));
        // And 127 goes, because an open cutaway's `max_z` *is* 127 and the
        // test is `>=`. The client's, exactly, and it costs nothing: nothing
        // stands at the very top of the range.
        assert!(!Cutaway::OPEN.shows_mobile(127));
    }

    #[test]
    fn fades_keep_identity_until_a_hidden_object_returns_to_opaque() {
        let key = FadeKey::static_(Point::new(12, 34, 5), Graphic(0x1234));
        let mut fades = Fades::default();

        assert_eq!(fades.advance(key, 0), 230);
        for _ in 0..9 {
            fades.advance(key, 0);
        }
        assert_eq!(fades.advance(key, 0), 0, "zero is the removal endpoint");
        assert_eq!(fades.advance(key, u8::MAX), 25, "returning art fades in");
        for _ in 0..9 {
            fades.advance(key, u8::MAX);
        }
        assert_eq!(fades.advance(key, u8::MAX), u8::MAX);
    }

    #[test]
    fn fade_keys_keep_their_graphic_typed() {
        let at = Point::new(12, 34, 5);
        let graphic = Graphic(0x1234);

        assert_eq!(FadeKey::static_(at, graphic).graphic, graphic);
        assert_eq!(FadeKey::item(at, graphic).graphic, graphic);
        assert_eq!(FadeKey::foliage(at).graphic, Graphic(0));
    }

    #[test]
    fn foliage_members_share_the_canopys_fade_identity() {
        let at = Point::new(12, 34, 5);
        let key = FadeKey::foliage(at);
        let mut fades = Fades::default();

        assert_eq!(fades.advance(key, FOLIAGE_ALPHA_U8), 230);
        // A second graphic from the same placed canopy sees the same ramp,
        // rather than starting a second fade and producing stripes.
        assert_eq!(fades.advance(key, FOLIAGE_ALPHA_U8), 205);
        assert_eq!(fades.advance(key, u8::MAX), 230);
    }
}
