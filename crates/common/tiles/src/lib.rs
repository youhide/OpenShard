//! What every tile in the game *is*.
//!
//! Two tables. Land tiles are the ground itself, 0x4000 of them. Static tiles
//! are everything sitting on it — walls, trees, doors — 0x10000 of them. Both
//! carry a flag word saying whether you can walk on it, stand on it, swim in it
//! or climb it, and statics carry a height.
//!
//! # A table is not a file
//!
//! These declarations come off `tiledata.mul`, and that is the last thing this
//! crate has to say about a file: it opens none, names no byte offset and has
//! no dependencies at all. `openshard_uofiles::tiledata` is the reader that
//! fills a [`TileData`] and hands it back, and it depends on this crate rather
//! than the other way round — which is what lets a world made of tiles name the
//! tiles it is made of without linking a parser.
//!
//! # The ids live here too
//!
//! [`LandTileId`] indexes the land table, and `Graphic` — which is on the wire,
//! so it lives in `openshard-protocol` — indexes the static one. Two more are ids into *other* clients' tables that these entries name:
//! [`TextureId`], which [`LandTile::texture`] points at, and [`AnimId`], which
//! [`StaticTile::anim_id`] does. They are here because the entry that carries
//! one is here; the readers of those two files take them as arguments.

use std::fmt;

/// How many land tiles a client knows about.
pub const LAND_TILE_COUNT: usize = 0x4000;
/// How many static tiles a client knows about.
pub const STATIC_TILE_COUNT: usize = 0x10000;

/// What a tile can do, straight from `tiledata.mul`.
///
/// The bits are Sphere's `UFLAG*` in `game/uo_files/uofiles_macros.h`. Only the
/// ones movement needs are named; the rest are on the wire and not our business.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct TileFlags(u64);

impl TileFlags {
    /// UFLAG1_FLOOR: walkable at its base.
    pub const FLOOR: u64 = 0x0000_0001;
    /// UFLAG1_WALL: wall, door or fireplace.
    pub const WALL: u64 = 0x0000_0010;
    /// UFLAG2_WALL2: the second wall bit. ServUO calls it `NoShoot` and uses it
    /// for exactly that — a straight line an arrow or a look does not cross.
    ///
    /// The value is `0x2000`, not `0x20`: `0x20` is `UFLAG1_DAMAGE` (a fire, a
    /// spike), and there is no `UFLAG1_NOSHOOT` in Sphere's header at all. Naming
    /// the damage bit "no shoot" made every brazier opaque and every portcullis
    /// transparent, which is the wrong answer in both directions at once.
    pub const NO_SHOOT: u64 = 0x0000_2000;
    /// UFLAG1_BLOCK: too big and heavy to walk through.
    pub const BLOCK: u64 = 0x0000_0040;
    /// UFLAG1_WATER: water or wet.
    pub const WATER: u64 = 0x0000_0080;
    /// UFLAG2_PLATFORM: you can stand on top of it.
    pub const PLATFORM: u64 = 0x0000_0200;
    /// UFLAG2_CLIMBABLE: stairs. Sphere halves the height of these.
    pub const CLIMBABLE: u64 = 0x0000_0400;
    /// UFLAG2_WINDOW: an arch or doorway you can walk through.
    pub const WINDOW: u64 = 0x0000_1000;
    /// UFLAG4_DOOR.
    pub const DOOR: u64 = 0x2000_0000;
    /// ClassicUO's `TileFlag.Transparent`. Only the renderer reads it, and only
    /// as one half of the pair that keeps a tile from cutting the roof away
    /// above the player — see `openshard-render`'s `cutaway`.
    pub const TRANSPARENT: u64 = 0x0000_0004;
    /// Drawn at partial alpha whatever else is decided about it: a window pane,
    /// a force field. ClassicUO's `TileFlag.Translucent`.
    pub const TRANSLUCENT: u64 = 0x0000_0008;
    /// Never drawn and never walked on: the client's own marker for a graphic
    /// that exists in the tables and nowhere in the world. ClassicUO drops these
    /// in `AddTileToRenderList` before anything else is asked about them.
    pub const INTERNAL: u64 = 0x0001_0000;
    /// A tree's leaves, a boat's mast — the things that fade when a body walks
    /// behind them. ClassicUO's `TileFlag.Foliage`.
    pub const FOLIAGE: u64 = 0x0002_0000;
    /// A roof tile. This is what makes a building's inside visible at all: the
    /// client stops drawing these once the player is under one.
    ///
    /// `0x1000_0000` — ClassicUO's `TileFlag.Roof`. Sphere's header has no name
    /// for this bit, so ClassicUO is the only reference for it and the value is
    /// pinned in a test beside the constant.
    pub const ROOF: u64 = 0x1000_0000;
    /// The static gives off light: a torch, a candle, a brazier, a lantern.
    ///
    /// `0x0080_0000` — ClassicUO's `TileFlag.LightSource`, read in
    /// `TileDataLoader`'s `IsLight`, and ServUO's `TileFlag.LightSource` at the
    /// same value. It says *that* a graphic burns and nothing about how big or
    /// what colour: the client takes those from `light.mul`, keyed by an id this
    /// reader does not carry yet. See `openshard-client-render`'s `light`, which
    /// picks a flame by graphic until that file is read.
    ///
    /// Pinned in a test beside the constant, because a flag means what the
    /// engine *reads* it for.
    pub const LIGHT_SOURCE: u64 = 0x0080_0000;
    /// The static cycles through graphics on its own: a fire, a torch, a water
    /// wheel. What it cycles through is `animdata.mul`, read by
    /// `openshard_uofiles::animdata` — and this bit is the only thing that says a graphic
    /// animates at all, since that file has a zeroed entry for everything else.
    ///
    /// `0x0100_0000` in both references that name it: ClassicUO's
    /// `TileFlag.Animation` and ServUO's. Pinned in a test beside the constant,
    /// because a flag means what the engine *reads* it for.
    pub const ANIMATION: u64 = 0x0100_0000;
    /// The graphic piles up: several of it are one item with an amount, rather
    /// than one entity each.
    ///
    /// `0x0000_0800` — ClassicUO calls the bit `Generic` and reads it as
    /// exactly this (`TileDataLoader`'s `IsStackable`), and ServUO's
    /// `TileFlag.Generic` has the same value. Sphere's header has no name for
    /// it. Pinned in a test beside the constant, because a flag means what the
    /// engine *reads* it for — and what this one is read for is whether a pile
    /// is drawn with its count written on it. See
    /// `openshard-client-render`'s `items::stack_label`.
    pub const STACKABLE: u64 = 0x0000_0800;

    /// Wrap a raw flag word.
    pub const fn new(bits: u64) -> Self {
        Self(bits)
    }

    /// The raw word.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Whether *any* bit in `mask` is set.
    ///
    /// Any and not all, which is what a caller passing a pair of alternatives
    /// wants — `has(WINDOW | NO_SHOOT)` is "does this stop an arrow", the pair
    /// ServUO's `Map.LineOfSight` tests together, and no tile carries both.
    pub const fn has(self, mask: u64) -> bool {
        self.0 & mask != 0
    }

    /// Whether this is water.
    pub const fn is_water(self) -> bool {
        self.has(Self::WATER)
    }

    /// Whether this blocks a walking human.
    pub const fn is_blocking(self) -> bool {
        self.has(Self::BLOCK)
    }

    /// Whether a mobile can stand on top of this.
    pub const fn is_platform(self) -> bool {
        self.has(Self::PLATFORM)
    }

    /// Whether this is stairs.
    pub const fn is_climbable(self) -> bool {
        self.has(Self::CLIMBABLE)
    }

    /// Whether this static plays a cycle of its own. See [`Self::ANIMATION`].
    pub const fn is_animated(self) -> bool {
        self.has(Self::ANIMATION)
    }

    /// Whether this burns, glows or otherwise lights its surroundings. See
    /// [`Self::LIGHT_SOURCE`].
    pub const fn is_light_source(self) -> bool {
        self.has(Self::LIGHT_SOURCE)
    }

    /// Whether several of this graphic pile into one item with a count. See
    /// [`Self::STACKABLE`].
    pub const fn is_stackable(self) -> bool {
        self.has(Self::STACKABLE)
    }

    /// Whether this is a roof. See [`Self::ROOF`].
    pub const fn is_roof(self) -> bool {
        self.has(Self::ROOF)
    }

    /// Whether the client never draws this. See [`Self::INTERNAL`].
    pub const fn is_internal(self) -> bool {
        self.has(Self::INTERNAL)
    }

    /// Whether this fades when a body walks behind it. See [`Self::FOLIAGE`].
    pub const fn is_foliage(self) -> bool {
        self.has(Self::FOLIAGE)
    }

    /// Whether this lies flat under whatever stands on it — a floor, a rug.
    ///
    /// ClassicUO calls the bit `Background`; this workspace named it after
    /// Sphere's `UFLAG1_FLOOR`. One bit, two names.
    pub const fn is_background(self) -> bool {
        self.has(Self::FLOOR)
    }
}

impl fmt::Debug for TileFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names = Vec::new();
        for (mask, name) in [
            (Self::FLOOR, "FLOOR"),
            (Self::WALL, "WALL"),
            (Self::NO_SHOOT, "NO_SHOOT"),
            (Self::BLOCK, "BLOCK"),
            (Self::WATER, "WATER"),
            (Self::PLATFORM, "PLATFORM"),
            (Self::CLIMBABLE, "CLIMBABLE"),
            (Self::WINDOW, "WINDOW"),
            (Self::DOOR, "DOOR"),
            (Self::ANIMATION, "ANIMATION"),
            (Self::LIGHT_SOURCE, "LIGHT_SOURCE"),
            (Self::TRANSPARENT, "TRANSPARENT"),
            (Self::TRANSLUCENT, "TRANSLUCENT"),
            (Self::INTERNAL, "INTERNAL"),
            (Self::FOLIAGE, "FOLIAGE"),
            (Self::ROOF, "ROOF"),
            (Self::STACKABLE, "STACKABLE"),
        ] {
            if self.has(mask) {
                names.push(name);
            }
        }
        write!(f, "TileFlags(0x{:X}", self.0)?;
        if !names.is_empty() {
            write!(f, " {}", names.join("|"))?;
        }
        f.write_str(")")
    }
}

/// One land tile: the ground.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct LandTile {
    /// What it can do.
    pub flags: TileFlags,
    /// Which square texture the ground is stretched over where it slopes.
    ///
    /// Its own index space — [`TextureId`] — and unrelated to the tile's
    /// art graphic. [`TextureId(0)`](TextureId) is the ordinary "none": entry 0
    /// of `texidx.mul` is empty, and the client draws such a tile flat however
    /// the ground around it stands.
    pub texture: TextureId,
    /// Its name, for logs and tools. Often "NoName".
    pub name: String,
}

/// One static tile: anything standing on the ground.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StaticTile {
    /// What it can do.
    pub flags: TileFlags,
    /// How tall it is.
    ///
    /// For climbable tiles this is the *full* height; Sphere halves it when
    /// working out where you end up standing. See `MapTerrain`.
    pub height: u8,
    /// 255 means immovable.
    pub weight: u8,
    /// Which paperdoll layer a wearable copy of it sits on.
    ///
    /// UO's file documentation calls this field *quality*, and for a piece of
    /// equipment the value is its layer — ServUO reads it exactly that way
    /// (`BaseWeapon`: `Layer = (Layer)ItemData.Quality`), which is how a halberd
    /// knows to take both hands. It was read past for most of this reader's life
    /// because nothing asked; Arms Lore does.
    pub layer: u8,
    /// What a worn copy of it draws as, in the body-animation index space —
    /// a different space from this tile's own art graphic, and read from
    /// `anim.mul`/`AnimAtlas` rather than `art.mul`.
    ///
    /// This is the *default* a worn item draws with — `EquipConv` only
    /// overrides it for the pairs where a body needs a different picture
    /// (a race or gender variant); an ordinary shirt has no such entry and
    /// draws from this field directly. Read past for most of this reader's
    /// life, the same way `layer` was, because nothing asked for it either.
    pub anim_id: AnimId,
    /// Its name.
    pub name: String,
}

/// A worn item's picture, in the body-animation index space `anim.mul` reads —
/// [`StaticTile::anim_id`]'s own space, and unrelated to that tile's art
/// graphic. See `openshard_uofiles::equipconv` for the table that overrides
/// it per body.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct AnimId(pub u16);

/// An index into the land table.
///
/// Land and static entries both look like `u16` in the files, but are indexed
/// into different halves of tiledata. Keeping them distinct prevents a static
/// art graphic from quietly becoming a mountain, or the reverse — the static
/// side's id is `Graphic`, which is on the wire and so lives in
/// `openshard-protocol`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct LandTileId(pub u16);

/// Which texture a land tile is stretched over.
///
/// Its own index space: [`LandTile::texture`] holds one of these, and it has
/// nothing to do with the art graphic of the same tile. Here rather than beside
/// the reader of `texmaps.mul` because the entry that names one is here, which
/// is [`AnimId`]'s reason as well.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TextureId(pub u16);

/// Every tile definition the client has.
///
/// `Clone` because it is shared across facets: `tiledata.mul` describes tiles,
/// not a map, so one copy is read and each facet's terrain gets its own.
#[derive(Clone)]
pub struct TileData {
    land: Vec<LandTile>,
    statics: Vec<StaticTile>,
}

impl fmt::Debug for TileData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TileData")
            .field("land", &self.land.len())
            .field("statics", &self.statics.len())
            .finish()
    }
}

impl TileData {
    /// The two tables, as a reader finished them.
    ///
    /// The one way to build a populated table, and it takes both halves at
    /// once so a half-filled one cannot exist. Its only caller is
    /// `openshard_uofiles::tiledata`; the lengths are the caller's contract —
    /// [`LAND_TILE_COUNT`] and [`STATIC_TILE_COUNT`] — because every lookup
    /// here is total and a short table would turn that into a panic.
    ///
    /// # Panics
    ///
    /// If either table is not the length its id space needs.
    #[must_use]
    pub fn from_tables(land: Vec<LandTile>, statics: Vec<StaticTile>) -> Self {
        assert_eq!(land.len(), LAND_TILE_COUNT, "the land table is a fixed size");
        assert_eq!(
            statics.len(),
            STATIC_TILE_COUNT,
            "the static table is a fixed size"
        );
        Self { land, statics }
    }

    /// Every tile, defined and unremarkable: no flags, no height, no name.
    ///
    /// For a caller that needs the *shape* of a tiledata and not the client's —
    /// a renderer test about where a sprite lands, which is decided by the
    /// sprite's own size and not by a flag. It is honest here in a way it would
    /// not be for a test about flags: nothing in it is a guess at what the file
    /// says, because it says nothing at all. Anything asserting on real flags
    /// reads a real install, the way `uofiles`' `tests/client_files.rs` does.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            land: vec![LandTile::default(); LAND_TILE_COUNT],
            statics: vec![StaticTile::default(); STATIC_TILE_COUNT],
        }
    }

    /// A land tile. Total: the index is masked into range.
    ///
    /// Masking rather than returning `Option` because the caller is the map,
    /// every id in it came off disk, and a `None` there would mean an unwalkable
    /// hole rather than an error anyone can act on.
    #[must_use]
    pub fn land(&self, id: LandTileId) -> &LandTile {
        &self.land[(id.0 as usize) & (LAND_TILE_COUNT - 1)]
    }

    /// A static tile. Total: every `u16` is a valid index.
    #[must_use]
    pub fn static_tile(&self, id: u16) -> &StaticTile {
        &self.statics[id as usize]
    }

    /// What a copy of static art `id` weighs, in stones.
    ///
    /// [`StaticTile::weight`] read through the one convention the file has about
    /// it: `255` is tiledata's *immovable* sentinel — a wall, a tree, a signpost
    /// — and not a quarter-ton object. Nothing immovable is ever in a pack, so it
    /// weighs nothing rather than instantly overloading whoever picked it up.
    ///
    /// Here rather than in a gameplay crate because the sentinel is the file
    /// talking, and every reader of that byte has to know it. The choice it
    /// leaves — what a shard with no tiledata at all does about encumbrance — is
    /// the caller's, and is made where the table is looked up.
    #[must_use]
    pub fn item_weight(&self, id: u16) -> u8 {
        match self.static_tile(id).weight {
            255 => 0,
            weight => weight,
        }
    }

    /// The name of static art `id`, or `None` where the file has not given it
    /// one.
    ///
    /// [`StaticTile::name`] read through the file's two ways of saying nothing:
    /// the placeholder `"NoName"`, and the empty string a table shorter than the
    /// id space pads with. Neither is worth drawing over a clicked item.
    #[must_use]
    pub fn item_name(&self, id: u16) -> Option<&str> {
        let name = self.static_tile(id).name.as_str();
        (!name.is_empty() && name != "NoName").then_some(name)
    }

    /// Put one entry into the table, replacing whatever was there.
    ///
    /// For tests that need a tiledata saying one specific thing — a graphic that
    /// is a light source, a roof, a wall — the way [`TileData::empty`] is for
    /// tests that need it to say nothing. It is `pub` and not `#[cfg(test)]`
    /// because the tests that want it are in other crates: a renderer's test
    /// about what a flag makes it draw cannot read a real install, since this
    /// repository ships no client files.
    ///
    /// Nothing in the engine calls it, and nothing should: what a graphic can do
    /// is the client's file talking, and an entry written over at runtime is a
    /// disagreement between the two ends of the wire about the same graphic.
    pub fn set_static_tile(&mut self, id: u16, tile: StaticTile) {
        self.statics[id as usize] = tile;
    }

    /// Put one land entry into the table, replacing whatever was there.
    ///
    /// [`set_static_tile`](Self::set_static_tile)'s other half, and it exists for
    /// the same callers with one more reason: what makes a tile *water* or
    /// *impassable ground* is a flag on this row and nowhere else, so a fixture
    /// that cannot write one has to fake water by overriding a movement rule —
    /// which is a fixture agreeing with itself. `openshard_movement::scene` is
    /// the caller, and `land_is_water` is the question.
    ///
    /// The id is masked into range exactly as [`land`](Self::land) masks it, so
    /// the two cannot disagree about which row an id names.
    pub fn set_land_tile(&mut self, id: LandTileId, tile: LandTile) {
        self.land[(id.0 as usize) & (LAND_TILE_COUNT - 1)] = tile;
    }
}

/// Resolve the pluralization markers in a tiledata name, given whether the pile
/// is plural (more than one).
///
/// UO item names carry `%...%` blocks the client normally interprets and the
/// server has to as well when it draws the name itself (a single-click label):
/// left raw, `"bolt%s% of cloth"` reaches the client verbatim. Inside a block a
/// `/` splits the plural form (before it) from the singular (after it), so
/// `%s%` adds an "s" when plural and nothing when singular, and `%ves/f%` gives
/// "…ves" / "…f". Text outside a block is always kept. Ported from Sphere's
/// `CItemBase::GetNamePluralize`.
#[must_use]
pub fn pluralize_name(name: &str, plural: bool) -> String {
    let mut out = String::with_capacity(name.len());
    let mut inside = false;
    // Within a block, the part before a `/` is the plural form. A block with no
    // `/` is a pure plural suffix (`%s%`), kept only when pluralizing.
    let mut is_plural_part = true;
    for ch in name.chars() {
        match ch {
            '%' => {
                inside = !inside;
                is_plural_part = true;
            }
            '/' if inside => is_plural_part = false,
            _ if inside && (plural != is_plural_part) => {}
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bits the renderer reads, pinned against ClassicUO's `TileFlag`.
    ///
    /// None of these appear in Sphere's `uofiles_macros.h` under a name that
    /// says what the client does with them, so ClassicUO is the only reference
    /// and a wrong bit here is silent: the roof simply never lifts, or every
    /// wall is treated as one. The values are `TileDataLoader.cs`'s enum.
    #[test]
    fn the_drawing_flags_are_classicuos_bits() {
        assert_eq!(TileFlags::TRANSPARENT, 0x0000_0004);
        assert_eq!(TileFlags::TRANSLUCENT, 0x0000_0008);
        assert_eq!(TileFlags::INTERNAL, 0x0001_0000);
        assert_eq!(TileFlags::FOLIAGE, 0x0002_0000);
        assert_eq!(TileFlags::ROOF, 0x1000_0000);
        // ClassicUO's `Surface` and `Bridge` are bits this workspace already
        // named after Sphere. Asserted here rather than trusted, because the
        // renderer is about to read them under the client's names: a surface is
        // what a roof may still be drawn as, and a bridge is what
        // `CalculateObjectHeight` halves.
        assert_eq!(TileFlags::PLATFORM, 0x0000_0200, "ClassicUO's Surface");
        assert_eq!(TileFlags::CLIMBABLE, 0x0000_0400, "ClassicUO's Bridge");
        // `TileFlag.LightSource`, which `TileDataLoader.IsLight` reads and
        // ServUO's `TileData` gives the same value. One bit off is a torch that
        // lights nothing and a bookshelf that burns: `0x0040_0000` next door is
        // `Wearable` and `0x0100_0000` above it is `Animation`, and both are
        // set on plenty of graphics that are not on fire.
        assert_eq!(TileFlags::LIGHT_SOURCE, 0x0080_0000);
        assert!(TileFlags::new(TileFlags::LIGHT_SOURCE).is_light_source());
        assert!(!TileFlags::new(TileFlags::ANIMATION).is_light_source());
        // `TileFlag.Generic`, which ClassicUO reads as `IsStackable` and ServUO
        // gives the same value. One bit off and every arrow in the pack would
        // be counted or no reagent would be: `0x0000_0400` below it is the
        // bridge a stair is and `0x0000_1000` above it is the archway a body
        // walks through.
        assert_eq!(TileFlags::STACKABLE, 0x0000_0800, "ClassicUO's Generic");
        assert!(TileFlags::new(TileFlags::STACKABLE).is_stackable());
        assert!(!TileFlags::new(TileFlags::CLIMBABLE).is_stackable());
    }

    #[test]
    fn pluralize_resolves_the_tiledata_markers() {
        // The reported bug: "bolt%s% of cloth" reaching the client verbatim.
        assert_eq!(pluralize_name("bolt%s% of cloth", false), "bolt of cloth");
        assert_eq!(pluralize_name("bolt%s% of cloth", true), "bolts of cloth");
        // A block with a slash: plural before, singular after.
        assert_eq!(pluralize_name("loa%ves/f%", true), "loaves");
        assert_eq!(pluralize_name("loa%ves/f%", false), "loaf");
        // A name with no markers is untouched either way.
        assert_eq!(pluralize_name("a torch", true), "a torch");
    }

    #[test]
    fn flags_name_the_bits_sphere_names() {
        // Pinned to uofiles_macros.h. These are not ours to renumber.
        assert_eq!(TileFlags::FLOOR, 0x0000_0001);
        assert_eq!(TileFlags::WALL, 0x0000_0010);
        assert_eq!(TileFlags::BLOCK, 0x0000_0040);
        assert_eq!(TileFlags::WATER, 0x0000_0080);
        assert_eq!(TileFlags::PLATFORM, 0x0000_0200);
        assert_eq!(TileFlags::CLIMBABLE, 0x0000_0400);
        assert_eq!(TileFlags::WINDOW, 0x0000_1000);
        assert_eq!(TileFlags::NO_SHOOT, 0x0000_2000);
        assert_eq!(TileFlags::DOOR, 0x2000_0000);
        // The bit next door, and the reason NO_SHOOT is pinned here: 0x20 is
        // UFLAG1_DAMAGE, and naming it "no shoot" is a one-character mistake
        // that silently moves every line-of-sight test one flag to the left.
        assert_ne!(TileFlags::NO_SHOOT, 0x0000_0020);
    }

    #[test]
    fn flags_read_the_way_the_real_files_do() {
        // A water land tile is 0xC0 = BLOCK|WATER.
        let water = TileFlags::new(0xC0);
        assert!(water.is_water());
        assert!(water.is_blocking());
        assert!(!water.is_platform());

        // Grass is zero: no flags at all, and perfectly walkable.
        let grass = TileFlags::new(0);
        assert!(!grass.is_water());
        assert!(!grass.is_blocking());
    }

    #[test]
    fn land_lookups_are_total() {
        // Every id in a map block came off disk and may be anything. A panic
        // here would mean one bad tile takes the shard down.
        let data = TileData::empty();
        for id in [0u16, 1, 0x3FFF, 0x4000, 0xFFFF] {
            let _ = data.land(LandTileId(id));
        }
    }
}
