//! **What a ship is made of, read twice.**
//!
//! The pier-and-bridge report
//! (`docs/world/evidence/2026-08-24-the-movement-surface-investigation.md`) has
//! three suspects left after the
//! `landCheck` mechanism was refuted, and this is inside the first of them: *a
//! boat moored at a pier*, which is the one shape that puts a live surface
//! beside a body standing on map terrain. Both surveys that did the refuting
//! walk the bare map with **no overlay at all**, so nothing has yet looked at
//! what a real ship lays over the water.
//!
//! This asks something narrower than a walk, and it comes before one: **a boat
//! was the only placement on this shard that did not read its art through
//! [`Cover::of_static`].** `planks_of` split a component on `is_blocking()`
//! alone — hull if it stops a body, deck if it does not — where housing,
//! decoration, the persistence reload and the client all go through the shared
//! reading, which splits on `is_platform()` instead: ServUO's `(flags &
//! ImpassableSurface) == TileFlag.Surface`
//! (`Scripts/Services/Pathing/Movement.cs:211`).
//!
//! **The retired rule is written out below**, as [`the_reading_that_was`], and
//! this survey is what it cost. Spelling it out rather than deleting it is what
//! keeps the number reproducible — and what would show a regression back to it,
//! since a survey that compared the current reading against itself would print
//! zeroes whatever either end did.
//!
//! The two disagree in four ways, and each of them is a statement about where a
//! body's feet may go:
//!
//! - **An invented floor.** Art that is neither a platform nor a blocker — a
//!   rope, a sail, a rudder — lays nothing at all under the shared reading and
//!   becomes a surface at `z + height` under the ship's own.
//! - **A raised floor.** A climbable platform stands at *half* its art under
//!   the shared reading (`platform_surface`, which is Sphere's rule and the
//!   map's); the ship's reading gives it the full height.
//! - **A hollow deck.** A platform lays a blocking half as thick as its own
//!   rise, so a body cannot stand inside the planking. A ship's deck lays no
//!   such half.
//! - **A lost floor.** Art that is a platform *and* blocks is a floor to the
//!   shared reading and a hull to the ship's.
//!
//! The first of those is the one with a fall in it, because `walk::aboard`
//! takes the **nearest** surface to the body's feet and
//! [`Overlay::surface_at`] bounds only the climb — every surface below a body
//! qualifies, at any depth. An invented floor under the deck is therefore not a
//! curiosity: it is where a player stepping off a pier ends up, with the deck
//! over their head.
//!
//! The reading is [`Plank::of_art`]'s now, and the survey is kept as the
//! record of what that changed rather than as the argument for changing it.
//!
//! A survey and not an assertion, for `land_check_survey`'s reason one crate
//! over: an assertion over a shipped multi table is an assertion about the art.
//!
//! ```sh
//! OPENSHARD_CLIENT=... cargo test -p openshard-boats --test moored_boat boat_art_survey -- --nocapture --ignored
//! ```

use std::collections::{
    BTreeMap,
    BTreeSet,
    HashSet,
};
use std::path::PathBuf;

use openshard_entities::{
    EntityId,
    Registry,
};
use openshard_map::grid::Tile;
use openshard_map::map::WorldMap;
use openshard_map::overlay::{
    Cover,
    Doors,
    Overlay,
};
use openshard_movement::spans::SpanIndex;
use openshard_movement::{
    Footing,
    MAX_STEP_UP,
    MapTerrain,
    step_allowed,
};
use openshard_protocol::direction::Direction;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_state::boat::Plank;
use openshard_tiles::TileData;
use openshard_uofiles::multi::{
    Component,
    Multis,
};

/// The multi ids a ship can have.
///
/// ServUO's, from the deeds: `SmallBoatDeed` is `0x0`, `MediumBoatDeed` `0x8`,
/// `LargeBoatDeed` `0x10`, with the dragon-prowed variants at `0x4`, `0xC` and
/// `0x14` — four facings each, so the ships occupy `0x00..=0x17` with no gaps.
/// Everything above is a house.
const BOAT_IDS: std::ops::RangeInclusive<u16> = 0x00..=0x17;

/// Where a body standing beside a ship might have its feet, relative to the
/// ship's own z.
///
/// A pier's deck is a handful of units over the water and a mast top is a good
/// way above it, so this brackets both ends generously rather than guessing at
/// a real dock. The comparison below is z-invariant — both readings are based
/// at the same component z, so shifting the ship shifts both answers together —
/// which is why the ship itself is surveyed at zero.
const FEET: std::ops::RangeInclusive<i32> = -12..=24;

/// Where the ship is laid on the tile grid.
///
/// A component's offsets are signed and a [`Tile`] is not, so the ship needs an
/// origin far enough in for the widest galleon's negative half to stay
/// positive. Nothing else depends on it: the survey prints offsets, so this is
/// subtracted back out at the one place a tile is named.
const ORIGIN: i32 = 1000;

/// The client files, or nothing.
///
/// `terrain.rs`'s `client_dir`, and the same bargain: a survey over shipped art
/// needs an install, and there is no path that is correct for two people.
fn client_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    dir.join("tiledata.mul").exists().then_some(dir)
}

/// The two tables a ship is read out of: what it is made of, and how tall each
/// piece of it is.
struct Install {
    multis: Multis,
    tiles:  TileData,
}

fn real_install() -> Option<Install> {
    let dir = client_dir()?;
    let multis = Multis::load(&dir).expect("the client's multi table should load");
    let tiles =
        openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata should load");
    Some(Install { multis, tiles })
}

/// How the two readings of one component differ, if they do.
///
/// Named rather than counted inline because the interesting question is *which*
/// disagreement a ship has: three of the four are tidiness and the first is a
/// player under a deck.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Disagreement {
    /// The ship lays a surface where the shared reading lays none.
    InventedFloor,
    /// Both lay one, at different heights — the halved climbable.
    RaisedFloor,
    /// The shared reading lays a solid half and the ship lays none.
    HollowDeck,
    /// The shared reading lays a surface and the ship lays none.
    LostFloor,
}

impl Disagreement {
    const fn describe(self) -> &'static str {
        match self {
            Self::InventedFloor => "a floor the ship invents (art that is neither platform nor blocker)",
            Self::RaisedFloor => "a floor at a different height (a climbable, unhalved)",
            Self::HollowDeck => "a deck with no thickness (the platform's blocking half)",
            Self::LostFloor => "a floor the ship loses (a platform that also blocks)",
        }
    }
}

/// **The rule a ship's art used to be read by, kept so the survey has something
/// to compare against.**
///
/// `Plank` held a `(z, height, blocks)` triple and turned it into exactly one
/// cover: blocking if the tiledata said `is_blocking()`, and a *floor*
/// otherwise. One cover and never two, which is where the hollow deck came
/// from; and "otherwise" rather than `is_platform()`, which is where the
/// invented floors came from.
///
/// Deliberately a free function in a test rather than a comment in the history:
/// a number this survey prints is a number about a rule, and the rule has to be
/// somewhere the reader can check it against what it says.
fn the_reading_that_was(art: &openshard_tiles::StaticTile, z: i8) -> Cover {
    match art.flags.is_blocking() {
        true => Cover::blocking(z, art.height),
        false => Cover::standing(z, art.height),
    }
}

/// One piece of art the ship turns into a floor and nothing else does.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Invented {
    graphic: Graphic,
    /// Its own z within the ship, and how tall the table says it is.
    z:       i8,
    height:  u8,
}

/// Everything one ship's art says, under both readings.
struct Ship {
    id:            u16,
    /// Drawn components — the signature tile every multi opens with is not one.
    drawn:         usize,
    /// What each disagreement costs this ship, by count.
    disagreements: BTreeMap<Disagreement, usize>,
    /// The surfaces the shared reading puts on the ship — the deck, as every
    /// other reader of this art sees it.
    real_surfaces: BTreeSet<i32>,
    /// The art the ship turns into a floor by itself, and how many tiles each
    /// piece of it covers.
    invented_art:  BTreeMap<Invented, usize>,
    /// The two overlays, holding the same ship read two ways. `aboard`'s own
    /// choice function is asked of both rather than reimplemented here.
    shard:         Overlay,
    shared:        Overlay,
    /// The ship's tiles, keyed by the pair rather than by [`Tile`] — which is a
    /// grid coordinate and deliberately not ordered.
    tiles:         BTreeSet<(u16, u16)>,
}

/// Read one ship both ways.
fn read(install: &Install, id: u16, boat: EntityId) -> Option<Ship> {
    let multi = install.multis.get(id)?;

    let mut ship = Ship {
        id,
        drawn: 0,
        disagreements: BTreeMap::new(),
        real_surfaces: BTreeSet::new(),
        invented_art: BTreeMap::new(),
        shard: Overlay::default(),
        shared: Overlay::default(),
        tiles: BTreeSet::new(),
    };
    // Per tile, because that is how an overlay is written: whole-tile, and a
    // ship puts several components on one tile all the time — a deck plank with
    // a mast standing in it.
    let mut shard_at: BTreeMap<(u16, u16), Vec<Cover>> = BTreeMap::new();
    let mut shared_at: BTreeMap<(u16, u16), Vec<Cover>> = BTreeMap::new();

    for component in multi.drawn() {
        ship.drawn += 1;
        let art = install.tiles.static_tile(component.graphic.0);
        // The ship floats at zero here, so a component's own `dz` is its z. A
        // real placement adds the ship's, and both readings take that addition
        // identically.
        let Ok(z) = i8::try_from(component.dz) else {
            continue;
        };
        let (Ok(x), Ok(y)) = (
            u16::try_from(ORIGIN + i32::from(component.dx)),
            u16::try_from(ORIGIN + i32::from(component.dy)),
        ) else {
            continue;
        };
        ship.tiles.insert((x, y));

        // The rule this shard retired, and the one it kept — the second read
        // through `Plank` itself, so what is compared is the value the shard
        // actually moors with rather than a restatement of it.
        let old = the_reading_that_was(art, z);
        let shared = Plank::of_art(boat, art, z).covers();

        shard_at.entry((x, y)).or_default().push(old);
        shared_at.entry((x, y)).or_default().extend(shared);

        // Surfaces, which is the half a fall is made of.
        let shard_surface = old.is_surface().then(|| old.surface());
        let shared_surface = shared.stands().map(Cover::surface);
        match (shard_surface, shared_surface) {
            (Some(_), None) => {
                *ship.disagreements.entry(Disagreement::InventedFloor).or_default() += 1;
                *ship
                    .invented_art
                    .entry(Invented {
                        graphic: component.graphic,
                        z,
                        height: art.height,
                    })
                    .or_default() += 1;
            }
            (Some(ours), Some(theirs)) if ours != theirs => {
                *ship.disagreements.entry(Disagreement::RaisedFloor).or_default() += 1;
                ship.real_surfaces.insert(theirs);
            }
            (Some(_), Some(theirs)) => {
                ship.real_surfaces.insert(theirs);
            }
            (None, Some(theirs)) => {
                *ship.disagreements.entry(Disagreement::LostFloor).or_default() += 1;
                ship.real_surfaces.insert(theirs);
            }
            (None, None) => {}
        }
        // And the solid half, which is what keeps a body out of the planking.
        if shared.blocks().is_some() && !old.is_blocker() {
            *ship.disagreements.entry(Disagreement::HollowDeck).or_default() += 1;
        }
    }

    for ((x, y), covers) in shard_at {
        ship.shard.set(Tile::new(x, y), covers);
    }
    for ((x, y), covers) in shared_at {
        ship.shared.set(Tile::new(x, y), covers);
    }
    Some(ship)
}

/// **Where a body stepping aboard ends up, under each reading.**
///
/// [`Overlay::surface_at`] is `aboard`'s own choice — the nearest surface to
/// the body's feet among those within reach — so this borrows the rule rather
/// than carrying a second copy of it, which is the mistake `step_cost`'s
/// `expand` made one plan over. `reach` is what `aboard` passes on flat ground:
/// the top of what the body stands on plus [`MAX_STEP_UP`].
struct Boarding {
    /// Feet heights, over any tile, where both readings answer and the ship's
    /// is the lower — a body put under the deck every other reader sees.
    lower:         usize,
    /// The worst of those: tile, the feet it happened from, and how far below.
    worst:         Option<((u16, u16), i32, i32)>,
    /// Feet heights where the shared reading refuses and the ship answers —
    /// footing conjured where the map has open water and the art has a rope.
    conjured:      usize,
    /// Which feet heights those were, so a reader can see whether a real pier
    /// stands at one of them.
    conjured_from: BTreeSet<i32>,
    /// Tiles carrying no real surface at all and an invented one — a stretch of
    /// ship a body can walk on that the rest of the engine says is not there.
    phantom_tiles: usize,
}

fn board(ship: &Ship) -> Boarding {
    let mut out = Boarding {
        lower:         0,
        worst:         None,
        conjured:      0,
        conjured_from: BTreeSet::new(),
        phantom_tiles: 0,
    };
    for &(x, y) in &ship.tiles {
        let tile = Tile::new(x, y);
        if ship.shared.surfaces_at(tile).next().is_none() && ship.shard.surfaces_at(tile).next().is_some() {
            out.phantom_tiles += 1;
        }
        for feet in FEET {
            let reach = feet + MAX_STEP_UP;
            let ours = ship.shard.surface_at(tile, feet, reach);
            let theirs = ship.shared.surface_at(tile, feet, reach);
            match (ours, theirs) {
                (Some(ours), Some(theirs)) if ours < theirs => {
                    out.lower += 1;
                    let drop = theirs - ours;
                    if out.worst.is_none_or(|(_, _, seen)| drop > seen) {
                        out.worst = Some(((x, y), feet, drop));
                    }
                }
                (Some(_), None) => {
                    out.conjured += 1;
                    out.conjured_from.insert(feet);
                }
                _ => {}
            }
        }
    }
    out
}

#[test]
#[ignore = "a survey of a shipped multi table, not an assertion — see the module doc"]
fn boat_art_survey() {
    let Some(install) = real_install() else {
        eprintln!("OPENSHARD_CLIENT is unset — nothing to survey");
        return;
    };
    let mut registry = Registry::new();
    let boat = registry.spawn();

    println!("boat art survey — {} multis in the table", install.multis.len());
    let mut totals: BTreeMap<Disagreement, usize> = BTreeMap::new();
    let mut art: BTreeMap<Invented, usize> = BTreeMap::new();
    let mut ships = 0_usize;
    let mut with_phantoms = 0_usize;

    for id in BOAT_IDS {
        let Some(ship) = read(&install, id, boat) else {
            continue;
        };
        ships += 1;
        for (&kind, &count) in &ship.disagreements {
            *totals.entry(kind).or_default() += count;
        }
        for (&piece, &count) in &ship.invented_art {
            *art.entry(piece).or_default() += count;
        }
        let boarding = board(&ship);
        with_phantoms += usize::from(boarding.phantom_tiles > 0);

        println!(
            "\n  multi 0x{:02X}: {} drawn components over {} tiles",
            ship.id,
            ship.drawn,
            ship.tiles.len()
        );
        println!(
            "    the deck, as every other reader sees it: {:?}",
            ship.real_surfaces
        );
        for (piece, count) in &ship.invented_art {
            println!(
                "    a floor invented from 0x{:04X} at z {} ({} tall), on {count} tile(s)",
                piece.graphic.0, piece.z, piece.height
            );
        }
        for (kind, count) in ship
            .disagreements
            .iter()
            .filter(|(&k, _)| k != Disagreement::InventedFloor)
        {
            println!("    {count:>3} × {}", kind.describe());
        }
        println!(
            "    boarding: {} heights land lower than the shared reading, {} conjure footing from nothing{}",
            boarding.lower,
            boarding.conjured,
            match boarding.conjured_from.iter().next() {
                Some(from) => format!(" (feet from {from} up)"),
                None => String::new(),
            }
        );
        if let Some(((x, y), feet, drop)) = boarding.worst {
            println!(
                "    ⚠ worst: feet at {feet} over ({dx},{dy}) land {drop} under the deck",
                dx = i32::from(x) - ORIGIN,
                dy = i32::from(y) - ORIGIN,
            );
        }
        if boarding.phantom_tiles > 0 {
            println!(
                "    ⚠ {} tile(s) a body can walk on that no other reader believes in",
                boarding.phantom_tiles
            );
        }
    }

    println!("\n  {ships} ships surveyed, {with_phantoms} of them with a tile only this reading carries");
    for (kind, count) in &totals {
        println!("  {count:>4} × {}", kind.describe());
    }
    println!("  the art the fleet invents floors out of:");
    for (piece, count) in &art {
        println!(
            "    0x{:04X} at z {}, {} tall — {count} placements",
            piece.graphic.0, piece.z, piece.height
        );
    }
    if totals.is_empty() {
        println!("  the two readings agree on every component of every ship");
    }
}

// ---------------------------------------------------------------------------
// The walk: a real pier, a real ship beside it, and the shard's own step rule.
// ---------------------------------------------------------------------------

/// The ship this walk moors.
///
/// One hull and not the fleet: [`boat_art_survey`] has already established that
/// all twenty-four lay the same two readings at the same heights, so a second
/// ship would measure the same arithmetic against more piers.
const SMALL_BOAT: u16 = 0x00;

/// How far from a pier a berth is looked for.
///
/// A ship is a handful of tiles across, so an origin further out than this puts
/// even its nearest gunwale beyond a step of the pier and leaves the walk with
/// nothing to measure.
const BERTH_RADIUS: i32 = 4;

/// How many piers to walk.
///
/// A cap, and the survey says what it dropped: silent truncation reads as "the
/// whole facet" when it is not. The piers are taken at an even stride so the
/// sample is not one harbour.
const PIERS_WALKED: usize = 20_000;

/// One facet, and the three tables a step over it is decided by.
struct Harbour {
    map:    WorldMap,
    tiles:  TileData,
    spans:  SpanIndex,
    multis: Multis,
}

fn real_harbour() -> Option<Harbour> {
    let dir = client_dir()?;
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("the client's map0 should load");
    let tiles =
        openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata should load");
    let multis = Multis::load(&dir).expect("the client's multi table should load");
    let spans = SpanIndex::build(&map, &tiles);
    Some(Harbour {
        map,
        tiles,
        spans,
        multis,
    })
}

/// Every tile a ship whose origin is `(x, y)` would put something on.
fn footprint(components: &[Component], x: u16, y: u16) -> Option<Vec<(u16, u16)>> {
    components
        .iter()
        .filter(|c| c.drawn())
        .map(|component| {
            let (Ok(cx), Ok(cy)) = (
                u16::try_from(i32::from(x) + i32::from(component.dx)),
                u16::try_from(i32::from(y) + i32::from(component.dy)),
            ) else {
                return None;
            };
            Some((cx, cy))
        })
        .collect()
}

/// The nearest berth to `(x, y)` a ship could actually be moored at — all sea,
/// which is `boats::check_berth`'s first judgement.
///
/// Its second judgement, *nothing else already there*, is met by construction:
/// **one ship is moored at a time, in an overlay of its own**. A harbour-wide
/// pass would have to arbitrate between piers competing for the same water, and
/// whichever pier lost would go unmeasured — which is a cap on coverage
/// disguised as a fixture. Every pier gets its ship here.
///
/// Nearest rather than first, so the ship ends up alongside the pier rather
/// than wherever the scan happened to reach it from.
fn berth_near(terrain: &MapTerrain<'_>, components: &[Component], x: u16, y: u16) -> Option<(u16, u16)> {
    let mut best: Option<((u16, u16), i32)> = None;
    for dy in -BERTH_RADIUS..=BERTH_RADIUS {
        for dx in -BERTH_RADIUS..=BERTH_RADIUS {
            let (Ok(ox), Ok(oy)) = (u16::try_from(i32::from(x) + dx), u16::try_from(i32::from(y) + dy))
            else {
                continue;
            };
            let Some(berth) = footprint(components, ox, oy) else {
                continue;
            };
            if !berth
                .iter()
                .all(|&(cx, cy)| terrain.land_is_water(Tile::new(cx, cy)))
            {
                continue;
            }
            let distance = dx * dx + dy * dy;
            if best.is_none_or(|(_, seen)| distance < seen) {
                best = Some(((ox, oy), distance));
            }
        }
    }
    best.map(|(origin, _)| origin)
}

/// Lay a ship into two overlays at once: the reading the shard has, and the one
/// it retired.
///
/// Both from the same components at the same z, so a difference between the two
/// walks below is the *rule* and nothing else.
fn moor_both(
    harbour: &Harbour,
    boat: EntityId,
    origin: (u16, u16),
    z: i8,
    now: &mut Overlay,
    then: &mut Overlay,
) {
    let mut now_at: BTreeMap<(u16, u16), Vec<Cover>> = BTreeMap::new();
    let mut then_at: BTreeMap<(u16, u16), Vec<Cover>> = BTreeMap::new();
    for component in harbour.multis.components(SMALL_BOAT).iter().filter(|c| c.drawn()) {
        let (Ok(x), Ok(y)) = (
            u16::try_from(i32::from(origin.0) + i32::from(component.dx)),
            u16::try_from(i32::from(origin.1) + i32::from(component.dy)),
        ) else {
            continue;
        };
        let Ok(at) = i8::try_from(i32::from(z) + i32::from(component.dz)) else {
            continue;
        };
        let art = harbour.tiles.static_tile(component.graphic.0);
        now_at
            .entry((x, y))
            .or_default()
            .extend(Plank::of_art(boat, art, at).covers());
        then_at
            .entry((x, y))
            .or_default()
            .push(the_reading_that_was(art, at));
    }
    for ((x, y), covers) in now_at {
        now.set(Tile::new(x, y), covers);
    }
    for ((x, y), covers) in then_at {
        then.set(Tile::new(x, y), covers);
    }
}

/// One step, spelled out, so a count can be checked against a case.
struct Example {
    /// The pier it was taken from.
    pier:  (u16, u16),
    /// The tile it landed on.
    onto:  (u16, u16),
    /// The height the body arrived at.
    at:    i32,
    /// Every surface the ship lays on that tile.
    decks: Vec<i32>,
}

/// What one reading of one moored ship does to the steps off a pier.
///
/// **The reference for every verdict is the deck as the shard reads it now**,
/// for both walks: "under the deck" has to mean under the floor the engine
/// agrees is there, and a retired reading asked about its own invented floors
/// would report itself correct.
struct Walk {
    /// Steps the bare map refused and the ship made legal — a boarding.
    boarded:        usize,
    /// Of those, the ones that put the body exactly on the ship's deck. A
    /// boarding, however far down it was: a pier stands over the water and a
    /// hull floats in it, so stepping aboard is a step *down* by construction.
    onto_deck:      usize,
    /// **The fall.** The body lands below the ship's own deck at that tile —
    /// inside the hull, with the planking over its head.
    under_deck:     usize,
    /// The worst of those: the pier it was walked off, and how far under.
    worst_under:    Option<((u16, u16), i32)>,
    /// Steps the ship made legal **without supplying the landing**: the height
    /// the body arrived at is the map's own — a piling, a pier plank over
    /// water, the shore — and what the ship changed was a *diagonal's flank*,
    /// which `step_allowed` refuses when either cardinal beside it has no
    /// footing. Not a boarding, and counted apart so it cannot be read as one.
    not_a_boarding: usize,
    /// One of those, spelled out. A count alone cannot be told apart from a
    /// defect in this survey's own arithmetic — which is how the first two
    /// versions of this classification were caught.
    an_example:     Option<Example>,
    /// How far a pier deck stands above the deck a body boards onto.
    ///
    /// `boats.md`'s open question, which its own fixture names: *what a real
    /// sloop's deck actually stands at over real water*.
    drops:          Vec<i32>,
    /// Steps the bare map already allowed whose answer the ship *lowered*.
    ///
    /// Should be nothing: where the map answers, only `climbed` speaks, and it
    /// takes surfaces strictly above the ground. A landing that went down here
    /// would be a defect in the rule rather than in the art.
    lowered:        usize,
}

impl Walk {
    fn nothing_yet() -> Self {
        Self {
            boarded:        0,
            onto_deck:      0,
            under_deck:     0,
            worst_under:    None,
            not_a_boarding: 0,
            an_example:     None,
            drops:          Vec::new(),
            lowered:        0,
        }
    }
}

/// Every step off one pier, with one ship beside it, under one reading.
fn walk_off(
    terrain: MapTerrain<'_>,
    overlay: &Overlay,
    bare: &Overlay,
    truth: &Overlay,
    pier: (u16, u16, i8),
    out: &mut Walk,
) {
    let (x, y, deck) = pier;
    let footing = Footing::new(Some(terrain), overlay, Doors::AsTheyStand);
    let bare_footing = Footing::new(Some(terrain), bare, Doors::AsTheyStand);
    let from = Point::new(x, y, deck);
    for direction in Direction::ALL {
        let with = step_allowed(&footing, from, direction);
        let without = step_allowed(&bare_footing, from, direction);
        match (with, without) {
            (Some(landed), None) => {
                out.boarded += 1;
                let at = i32::from(landed.z);
                let tile = Tile::new(landed.x, landed.y);
                // The verdict rests on what the ship lays *at the destination*,
                // and the first arm is why: a diagonal is refused when either
                // flanking cardinal has no footing, so a ship can make a step
                // legal without the body ever leaving the map's own ground.
                let decks: Vec<i32> = truth.surfaces_at(tile).map(Cover::surface).collect();
                let highest = decks.iter().copied().max();
                // **Did the ship supply the height the body arrived at?** That
                // is the whole classification, and nothing else is: a hull may
                // cover the destination and a pier plank may stand on the same
                // water, so neither "the ship is here" nor "the map refused"
                // is the question.
                match highest {
                    Some(_) if decks.contains(&at) => {
                        out.onto_deck += 1;
                        out.drops.push(i32::from(deck) - at);
                    }
                    Some(highest) if at < highest => {
                        out.under_deck += 1;
                        let under = highest - at;
                        if out.worst_under.is_none_or(|(_, seen)| under > seen) {
                            out.worst_under = Some(((x, y), under));
                        }
                    }
                    _ => {
                        out.not_a_boarding += 1;
                        if out.an_example.is_none() {
                            out.an_example = Some(Example {
                                pier: (x, y),
                                onto: (landed.x, landed.y),
                                at,
                                decks,
                            });
                        }
                    }
                }
            }
            (Some(landed), Some(bare_landed)) if landed.z < bare_landed.z => out.lowered += 1,
            _ => {}
        }
    }
}

/// The same pier walked by a body water is ground to.
///
/// **`boats.md`'s open question, and the overlay it says the earlier survey did
/// not have.** That document keeps `MapTerrain::swimming` off, and since
/// 2026-08-23 its recorded reason is an open one: with the flag on, `check`
/// stops refusing water and answers with the water's own height, so `aboard`
/// never fires and the deck is left to `climbed` — which bounds the climb by
/// `MAX_STEP_UP`. A deck more than two above the water would then leave a body
/// standing on the sea *under its own ship*. It says so, and says it has not
/// been measured. This measures it.
///
/// There is no bare comparison here, because the question is not which steps
/// the ship makes legal — with swimming on the water is already walkable. It is
/// where a body ends up on a tile the ship covers.
///
/// **And it is walked from the water rather than from the pier**, which is the
/// whole of what makes it the right question. A body stepping off a pier
/// reaches from the top of the pier's own art, which clears a deck easily; the
/// body the prediction is about is *in the sea beside the hull*, whose reach is
/// the waterline plus two.
struct Swim {
    /// Tiles of open water beside a hull where a swimmer can float at all. A
    /// zero anywhere below means nothing was tried unless this one is large.
    alongside:   usize,
    /// Steps from one of those toward a tile the ship covers.
    tried:       usize,
    /// Refused: the tile is a wall to a swimmer, whatever is on it.
    refused:     usize,
    /// Allowed, and the body arrives on the deck.
    onto_deck:   usize,
    /// **Allowed, and the body arrives below the deck** — floating in the sea
    /// with its own ship over its head, which is the prediction.
    under_deck:  usize,
    /// The worst of those: the water it swam from, and how far under.
    worst_under: Option<((u16, u16), i32)>,
}

impl Swim {
    const fn nothing_yet() -> Self {
        Self {
            alongside:   0,
            tried:       0,
            refused:     0,
            onto_deck:   0,
            under_deck:  0,
            worst_under: None,
        }
    }
}

fn swim_off(
    terrain: MapTerrain<'_>,
    map: &WorldMap,
    overlay: &Overlay,
    berth: &[(u16, u16)],
    out: &mut Swim,
) {
    let footing = Footing::new(Some(terrain), overlay, Doors::AsTheyStand);
    let aboard: HashSet<(u16, u16)> = berth.iter().copied().collect();
    // The ring of open water round the hull, each tile once however many of the
    // ship's tiles it touches.
    let mut alongside: HashSet<(u16, u16)> = HashSet::new();
    for &(bx, by) in berth {
        for direction in Direction::ALL {
            let (dx, dy) = direction.step();
            let (Ok(wx), Ok(wy)) = (
                u16::try_from(i32::from(bx) + dx),
                u16::try_from(i32::from(by) + dy),
            ) else {
                continue;
            };
            if !aboard.contains(&(wx, wy)) && terrain.land_is_water(Tile::new(wx, wy)) {
                alongside.insert((wx, wy));
            }
        }
    }

    for (wx, wy) in alongside {
        let Some(cell) = map.land(wx, wy) else {
            continue;
        };
        let waterline = i32::from(cell.z);
        // Where a swimmer actually floats there, by the map's own rule rather
        // than by this survey's guess at it.
        let Some(stand) = terrain.check(wx, wy, waterline, waterline) else {
            continue;
        };
        let Ok(from_z) = i8::try_from(stand) else {
            continue;
        };
        out.alongside += 1;
        let from = Point::new(wx, wy, from_z);
        for direction in Direction::ALL {
            let (dx, dy) = direction.step();
            let (Ok(tx), Ok(ty)) = (
                u16::try_from(i32::from(wx) + dx),
                u16::try_from(i32::from(wy) + dy),
            ) else {
                continue;
            };
            if !aboard.contains(&(tx, ty)) {
                continue;
            }
            out.tried += 1;
            let Some(landed) = step_allowed(&footing, from, direction) else {
                out.refused += 1;
                continue;
            };
            let decks: Vec<i32> = overlay
                .surfaces_at(Tile::new(landed.x, landed.y))
                .map(Cover::surface)
                .collect();
            let Some(highest) = decks.iter().copied().max() else {
                out.refused += 1;
                continue;
            };
            let at = i32::from(landed.z);
            if decks.contains(&at) {
                out.onto_deck += 1;
            } else if at < highest {
                out.under_deck += 1;
                let under = highest - at;
                if out.worst_under.is_none_or(|(_, seen)| under > seen) {
                    out.worst_under = Some(((wx, wy), under));
                }
            }
        }
    }
}

/// **A ship moored at a real pier, and every step off that pier.**
///
/// The first remaining suspect for the 2026-08-02 report
/// (`docs/world/evidence/2026-08-24-the-movement-surface-investigation.md`), measured
/// rather than reasoned about. The two surveys that refuted the `landCheck`
/// mechanism both walk the bare map with no overlay at all, and this is the
/// overlay they could not see: over every pier on facet 0 with sea beside it, a
/// small boat is moored at the nearest berth it would actually float in, and
/// every step off the pier is asked of the shard's own `step_allowed`.
///
/// Asked twice — of the reading the shard has and of the one it retired — so
/// the fix is priced on real ground rather than on the multi table alone.
///
/// ```sh
/// OPENSHARD_CLIENT=... cargo test --release -p openshard-boats --test moored_boat moored_pier_survey -- --nocapture --ignored
/// ```
#[test]
#[ignore = "a survey of a whole facet, not an assertion — see the doc comment"]
fn moored_pier_survey() {
    let Some(harbour) = real_harbour() else {
        eprintln!("OPENSHARD_CLIENT is unset — nothing to survey");
        return;
    };
    let terrain = MapTerrain::new(&harbour.map, &harbour.tiles, &harbour.spans);
    let components = harbour.multis.components(SMALL_BOAT).to_vec();
    assert!(!components.is_empty(), "this install has no small boat to moor");
    let mut registry = Registry::new();
    let boat = registry.spawn();

    // Every pier and bridge deck with open water beside it — the only shape a
    // ship can be moored against.
    let (width, height) = (harbour.map.width() as u16, harbour.map.height() as u16);
    let mut piers: Vec<(u16, u16, i8)> = Vec::new();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            if harbour.map.land(x, y).is_none() {
                continue;
            }
            let beside_water = Direction::ALL.iter().any(|direction| {
                let (dx, dy) = direction.step();
                let (Ok(nx), Ok(ny)) = (u16::try_from(i32::from(x) + dx), u16::try_from(i32::from(y) + dy))
                else {
                    return false;
                };
                terrain.land_is_water(Tile::new(nx, ny))
            });
            if !beside_water {
                continue;
            }
            for item in harbour.map.statics_at(x, y) {
                let art = harbour.tiles.static_tile(item.tile.0);
                if !(art.flags.is_platform() && art.flags.is_climbable()) {
                    continue;
                }
                let Some(stands) = Cover::of_static(art).based_at(item.z).stands() else {
                    continue;
                };
                let Ok(deck) = i8::try_from(stands.surface()) else {
                    continue;
                };
                piers.push((x, y, deck));
            }
        }
    }

    let stride = piers.len().div_ceil(PIERS_WALKED).max(1);
    let walked: Vec<(u16, u16, i8)> = piers.iter().copied().step_by(stride).collect();
    println!(
        "moored-pier survey over facet 0, {width}x{height}\n  \
         pier and bridge decks with sea beside them: {}\n  \
         walked: {} (every {stride}{})",
        piers.len(),
        walked.len(),
        match stride {
            1 => String::new(),
            _ => format!(", so {} were not walked", piers.len() - walked.len()),
        }
    );

    // One pier at a time, each with its own ship in its own overlay — see
    // `berth_near` for why a harbour-wide pass would have been a cap on
    // coverage rather than a fixture.
    let bare = Overlay::default();
    let swimmer = MapTerrain::new(&harbour.map, &harbour.tiles, &harbour.spans).swimming(true);
    let (mut with_now, mut with_then) = (Walk::nothing_yet(), Walk::nothing_yet());
    let (mut swim_now, mut swim_then) = (Swim::nothing_yet(), Swim::nothing_yet());
    let mut moored = 0_usize;
    for &pier in &walked {
        let (x, y, _) = pier;
        let Some(origin) = berth_near(&terrain, &components, x, y) else {
            continue;
        };
        let Some(water) = harbour.map.land(origin.0, origin.1) else {
            continue;
        };
        let Some(berth) = footprint(&components, origin.0, origin.1) else {
            continue;
        };
        moored += 1;
        let (mut now, mut then) = (Overlay::default(), Overlay::default());
        moor_both(&harbour, boat, origin, water.z, &mut now, &mut then);
        // Both walks are judged against the deck as the shard reads it *now*:
        // what a fall is cannot be defined by the reading under test.
        walk_off(terrain, &now, &bare, &now, pier, &mut with_now);
        walk_off(terrain, &then, &bare, &now, pier, &mut with_then);
        swim_off(swimmer, &harbour.map, &now, &berth, &mut swim_now);
        swim_off(swimmer, &harbour.map, &then, &berth, &mut swim_then);
    }
    println!("  of those, {moored} have room for a small boat within {BERTH_RADIUS} tiles");

    for (who, walk) in [
        ("the reading now", &with_now),
        ("the reading retired", &with_then),
    ] {
        println!(
            "\n  {who}:\n    \
             steps the ship makes legal:        {}\n    \
             of those, onto the ship's deck:    {}\n    \
             ⚠ UNDER the ship's own deck:       {}\n    \
             the ship did not supply the floor: {}\n    \
             already-legal steps it lowers:     {}",
            walk.boarded, walk.onto_deck, walk.under_deck, walk.not_a_boarding, walk.lowered
        );
        if let Some(((x, y), under)) = walk.worst_under {
            println!("    ⚠ worst: off the pier at ({x},{y}), {under} under the deck");
        }
        if let Some(example) = &walk.an_example {
            println!(
                "    one that is not a boarding: off ({},{}) onto ({},{}) at z {}, where the ship lays {:?}",
                example.pier.0, example.pier.1, example.onto.0, example.onto.1, example.at, example.decks
            );
        }
        let mut drops = walk.drops.clone();
        drops.sort_unstable();
        if let (Some(&least), Some(&most)) = (drops.first(), drops.last()) {
            println!(
                "    a pier stands {least}..{most} above the deck a body boards onto (median {})",
                drops[drops.len() / 2]
            );
        }
    }

    // And `boats.md`'s open question, which is about a swimmer rather than a
    // walker: with `MapTerrain::swimming` on — a flag this shard keeps off —
    // does a body that cannot climb to the deck end up in the sea under it?
    println!("\n  with MapTerrain::swimming on, which this shard keeps off:");
    for (who, swim) in [
        ("the reading now", &swim_now),
        ("the reading retired", &swim_then),
    ] {
        println!(
            "    {who}: {} tiles of water alongside a hull, {} steps toward the ship\n      \
             refused outright:              {}\n      \
             arrive on the deck:            {}\n      \
             ⚠ arrive UNDER the deck:       {}",
            swim.alongside, swim.tried, swim.refused, swim.onto_deck, swim.under_deck
        );
        if let Some(((x, y), under)) = swim.worst_under {
            println!("      ⚠ worst: from the water at ({x},{y}), {under} under the deck");
        }
    }
}
