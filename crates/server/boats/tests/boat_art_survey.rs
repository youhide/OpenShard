//! **What a ship is made of, read twice.**
//!
//! `roadmap.md`'s pier-and-bridge report has three suspects left after the
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
//! OPENSHARD_CLIENT=... cargo test -p openshard-boats --test boat_art_survey -- --nocapture --ignored
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use openshard_entities::{EntityId, Registry};
use openshard_map::grid::Tile;
use openshard_map::overlay::{Cover, Overlay};
use openshard_movement::MAX_STEP_UP;
use openshard_state::boat::Plank;
use openshard_tiles::TileData;
use openshard_uofiles::multi::Multis;

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
    tiles: TileData,
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
    graphic: u16,
    /// Its own z within the ship, and how tall the table says it is.
    z: i8,
    height: u8,
}

/// Everything one ship's art says, under both readings.
struct Ship {
    id: u16,
    /// Drawn components — the signature tile every multi opens with is not one.
    drawn: usize,
    /// What each disagreement costs this ship, by count.
    disagreements: BTreeMap<Disagreement, usize>,
    /// The surfaces the shared reading puts on the ship — the deck, as every
    /// other reader of this art sees it.
    real_surfaces: BTreeSet<i32>,
    /// The art the ship turns into a floor by itself, and how many tiles each
    /// piece of it covers.
    invented_art: BTreeMap<Invented, usize>,
    /// The two overlays, holding the same ship read two ways. `aboard`'s own
    /// choice function is asked of both rather than reimplemented here.
    shard: Overlay,
    shared: Overlay,
    /// The ship's tiles, keyed by the pair rather than by [`Tile`] — which is a
    /// grid coordinate and deliberately not ordered.
    tiles: BTreeSet<(u16, u16)>,
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
        let art = install.tiles.static_tile(component.graphic);
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
    lower: usize,
    /// The worst of those: tile, the feet it happened from, and how far below.
    worst: Option<((u16, u16), i32, i32)>,
    /// Feet heights where the shared reading refuses and the ship answers —
    /// footing conjured where the map has open water and the art has a rope.
    conjured: usize,
    /// Which feet heights those were, so a reader can see whether a real pier
    /// stands at one of them.
    conjured_from: BTreeSet<i32>,
    /// Tiles carrying no real surface at all and an invented one — a stretch of
    /// ship a body can walk on that the rest of the engine says is not there.
    phantom_tiles: usize,
}

fn board(ship: &Ship) -> Boarding {
    let mut out = Boarding {
        lower: 0,
        worst: None,
        conjured: 0,
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
                piece.graphic, piece.z, piece.height
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
            piece.graphic, piece.z, piece.height
        );
    }
    if totals.is_empty() {
        println!("  the two readings agree on every component of every ship");
    }
}
