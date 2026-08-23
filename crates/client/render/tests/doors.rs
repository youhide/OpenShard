//! The door table, against the client that has to agree with it.
//!
//! `data/doors.json` is twenty-two rows ported out of ServUO, and a ported table
//! with no oracle is twenty-two chances to have mistyped a hex digit into
//! something that still compiles. The client is the oracle available: every graphic a
//! family claims has to be a door in `tiledata.mul`, which a base off by one or a
//! family length off by eight would break immediately.
//!
//! The second test is what the table is *worth*: how many open leaves the
//! client's own flags would already have let light through, and how many only
//! this table does. If that second number ever collapses, the table has stopped
//! earning its place and should be deleted rather than updated.
//!
//! Ignored and gated on `OPENSHARD_CLIENT`: no client files live in this
//! repository, ever.

use openshard_client_render::{doors, occlusion};
use openshard_protocol::wire::Graphic;
use openshard_tiles::{TileData, TileFlags};

fn tiledata() -> Option<TileData> {
    let dir = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from)?;
    Some(
        openshard_uofiles::tiledata::load(dir.join("tiledata.mul"))
            .expect("tiledata.mul")
            .tiles,
    )
}

/// Every graphic the table claims is a door in the client's own table.
///
/// The check a mistyped base fails: `0x0675` is `MetalDoor` and `0x0765` is not
/// a door at all, and nothing else here would notice the difference.
#[test]
#[ignore]
fn every_graphic_the_table_claims_is_a_door_in_the_client_s() {
    let Some(tiledata) = tiledata() else { return };
    let mut checked = 0usize;
    let mut missing = Vec::new();
    for (base, count) in families() {
        for offset in 0..count {
            let id = base + offset;
            checked += 1;
            if !tiledata.static_tile(id).flags.has(TileFlags::DOOR) {
                missing.push(id);
            }
        }
    }
    // A count, not just a verdict: the families are recovered by walking, and a
    // walk that found nothing would pass every assertion about what it found.
    assert_eq!(
        checked, 328,
        "the table claims a different number of graphics than it did"
    );
    println!(
        "{checked} graphics across 13 families; {} not flagged DOOR: {missing:04X?}",
        missing.len()
    );
    // Not all of them, and that is the client's own doing rather than a mistyped
    // base: `tiledata.mul` leaves a handful of leaves inside otherwise solid
    // families unflagged. What a wrong base looks like is *most* of a family
    // missing, since a door family sits in a run of doors and nothing else does.
    assert!(
        missing.len() * 8 < checked,
        "an eighth of the claimed graphics are not doors — a base is mistyped",
    );
}

/// **What the table is actually worth**, and where the client could have said so
/// on its own.
///
/// Not "the client can never tell", which was the first version of this and is
/// false: over the twenty-two families the answer splits, and the split is worth
/// having written down.
///
/// - The six **secret** doors distinguish the two: a shut leaf is `NO_SHOOT` and
///   an open one is nothing at all, so `occlusion::opacity` would have got those
///   right with no table. Decision 11's original claim was true — for six
///   families out of twenty-two.
/// - The plain doors and the gates do not: their two leaves carry the same
///   stopping flags, so without the table an open one is a wall.
///
/// The assertion is on the *work done*: every open leaf is `CLEAR` now, and a
/// solid share of them would not have been. A table that had stopped matching
/// the client would show up as that share collapsing.
#[test]
#[ignore]
fn the_table_is_what_makes_an_open_leaf_clear() {
    let Some(tiledata) = tiledata() else { return };
    let mut pairs = 0;
    let mut clear_without_the_table = 0;
    for (base, count) in families() {
        for facing in 0..count / 2 {
            let (shut, open) = (base + 2 * facing, base + 2 * facing + 1);
            assert!(!doors::is_open(Graphic(shut)), "{shut:#06X} read as open");
            assert!(doors::is_open(Graphic(open)), "{open:#06X} read as shut");
            pairs += 1;
            // What the band on screen was: the leaf swung out of the way and the
            // grid still holding a whole tile of wall.
            assert_eq!(
                occlusion::opacity(Graphic(open), tiledata.static_tile(open)),
                occlusion::CLEAR,
                "{open:#06X} still stops light",
            );
            let flags = tiledata.static_tile(open).flags;
            if !flags.has(TileFlags::NO_SHOOT) && !flags.has(TileFlags::WINDOW) {
                clear_without_the_table += 1;
            }
        }
    }
    println!(
        "{pairs} open/shut pairs; {clear_without_the_table} open leaves the client's own flags \
         already let light through, {} the table had to",
        pairs - clear_without_the_table,
    );
    assert!(
        pairs - clear_without_the_table > pairs / 4,
        "the flags now answer for nearly every door on their own — check whether this table is \
         still earning its place",
    );
}

/// Every family, recovered rather than restated here.
///
/// Restating them would make the walk circular in the one way that matters — a
/// typo copied into both places. A base is the graphic `doors` calls facing 0,
/// shut, and the count is how far the same family reaches; both are properties
/// of the table and need no second list. Not "the first door after a gap": the
/// eight wooden and metal families are *adjacent*, `0x0675 + 16` being
/// `BarredMetalDoor`'s own base, and looking for gaps found six of the
/// twenty-two.
fn families() -> Vec<(u16, u16)> {
    let mut found: Vec<(u16, u16)> = Vec::new();
    for id in 0..=u16::MAX {
        match doors::family(Graphic(id)) {
            Some((_, 0, false)) => found.push((id, 1)),
            Some(_) => found.last_mut().expect("a family starts before it continues").1 += 1,
            None => {}
        }
    }
    assert_eq!(
        found.len(),
        22,
        "the table has {} families, not twenty-two",
        found.len()
    );
    found
}
