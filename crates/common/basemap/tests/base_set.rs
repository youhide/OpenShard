//! A facet out to a file and back, and every way a file fails to be one.
//!
//! The fixture is nine blocks square, which is **not** a whole number of
//! chunks: three of its four chunks are edge chunks, so a reader that assumed
//! every chunk was eight blocks by eight would fail here rather than on Tokuno.

use openshard_basemap::{BaseError, read, write};
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, LandTile, StaticItem, WorldMap};
use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Facet;

const FACET: Facet = Facet(0);
const BLOCKS: u32 = 9;
const TILES: u32 = BLOCKS * 8;

/// A path under `temp_dir` this run will not share with another.
fn path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("openshard-basemap-{tag}-{}.osbase", std::process::id()))
}

/// Land that names its own tile, so a transposed read comes back holding a cell
/// that says where it should have been, and statics on the seams.
fn snapshot() -> MapSnapshot {
    let mut map = WorldMap::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        |x, y| LandCell {
            tile: LandTile(u16::try_from(u32::from(x) * TILES + u32::from(y)).unwrap()),
            z: (i32::from(x) - i32::from(y)) as i8,
        },
    );
    let last = u16::try_from(TILES).unwrap() - 1;
    for (n, (x, y)) in [(0, 0), (63, 30), (64, 30), (30, 63), (30, 64), (last, last)]
        .into_iter()
        .enumerate()
    {
        map.place_static(StaticItem {
            tile: Graphic(0x100 + u16::try_from(n).unwrap()),
            x,
            y,
            z: 0,
            hue: Hue(0),
        });
    }
    // Two on one tile: what the stable sort is for, and what a client draws one
    // of on top of the other.
    for n in 0..2u16 {
        map.place_static(StaticItem {
            tile: Graphic(0x200 + n),
            x: 20,
            y: 21,
            z: 5,
            hue: Hue(n),
        });
    }
    MapSnapshot::new(FACET, map)
}

fn written(tag: &str) -> (std::path::PathBuf, MapSnapshot) {
    let path = path(tag);
    let snapshot = snapshot();
    write(&path, &snapshot).expect("a writable temp dir");
    (path, snapshot)
}

/// The acceptance test in miniature: every tile of the file is every tile of
/// the facet, and it comes back at the revision it went in at.
#[test]
fn a_facet_written_and_read_back_is_the_same_facet() {
    let (path, original) = written("round-trip");
    let back = read(&path).expect("the file we just wrote");

    assert_eq!(back.facet(), original.facet());
    assert_eq!(back.revision(), MapRevision::INITIAL);
    let (was, is) = (original.map(), back.map());
    assert_eq!((was.width(), was.height()), (is.width(), is.height()));
    assert_eq!(was.static_count(), is.static_count());
    for y in 0..u16::try_from(TILES).unwrap() {
        for x in 0..u16::try_from(TILES).unwrap() {
            assert_eq!(was.land(x, y), is.land(x, y), "the ground at ({x}, {y})");
            let was: Vec<_> = was.statics_at(x, y).collect();
            let is: Vec<_> = is.statics_at(x, y).collect();
            assert_eq!(was, is, "the statics at ({x}, {y})");
        }
    }
    std::fs::remove_file(&path).ok();
}

/// Written twice, the same bytes. Which is what a content hash would be about,
/// and what lets an import be checked by comparing files.
#[test]
fn writing_the_same_facet_twice_writes_the_same_bytes() {
    let (first, snapshot) = written("canonical-a");
    let second = path("canonical-b");
    write(&second, &snapshot).expect("a writable temp dir");
    assert_eq!(std::fs::read(&first).unwrap(), std::fs::read(&second).unwrap());

    // And a facet read back and written again is the same file a third time —
    // the round trip is byte-identical, not merely lossless.
    let third = path("canonical-c");
    write(&third, &read(&first).expect("a base set")).expect("a writable temp dir");
    assert_eq!(std::fs::read(&first).unwrap(), std::fs::read(&third).unwrap());

    for path in [first, second, third] {
        std::fs::remove_file(path).ok();
    }
}

/// A base set records its revision, and reading one back does not mint a fresh
/// one — a bake stamped against that revision has to stay valid across the
/// round trip, which is the point of writing one at all.
#[test]
fn a_recorded_revision_survives_the_round_trip() {
    let path = path("revision");
    let map = WorldMap::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        |_, _| LandCell::default(),
    );
    let published = MapSnapshot::restored(Facet(3), MapRevision::decoded(97), map);
    write(&path, &published).expect("a writable temp dir");

    let back = read(&path).expect("a base set");
    assert_eq!(back.facet(), Facet(3));
    assert_eq!(back.revision(), MapRevision::decoded(97));
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_file_that_is_not_a_base_set_is_refused() {
    let path = path("not-a-base-set");
    std::fs::write(&path, b"OSBX").unwrap();
    assert!(matches!(read(&path), Err(BaseError::NotABaseSet { .. })));

    std::fs::write(&path, b"").unwrap();
    assert!(matches!(read(&path), Err(BaseError::NotABaseSet { .. })));
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_later_layout_is_refused_rather_than_guessed_at() {
    let (path, _) = written("version");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[4] = 2;
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(read(&path), Err(BaseError::Version { found: 2, .. })));
    std::fs::remove_file(&path).ok();
}

/// A header whose facet size does not imply the chunk count it claims is a file
/// disagreeing with itself, and it is checked before anything goes looking
/// inside.
#[test]
fn a_header_that_disagrees_with_itself_is_refused() {
    let (path, _) = written("count");
    let mut bytes = std::fs::read(&path).unwrap();
    // Say the facet is one block column wider, which is one chunk column more.
    bytes[14..18].copy_from_slice(&(BLOCKS + 8).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(read(&path), Err(BaseError::CountMismatch { .. })));
    std::fs::remove_file(&path).ok();
}

/// An offset that runs backwards would hand the decoder the bytes of the chunk
/// before it — which decode perfectly, into the wrong chunk.
#[test]
fn a_table_that_does_not_rise_is_refused() {
    let (path, _) = written("table");
    let mut bytes = std::fs::read(&path).unwrap();
    // The second entry, pointed back at the start of the first chunk's bytes.
    let first = u64::from_le_bytes(bytes[26..34].try_into().unwrap());
    bytes[34..42].copy_from_slice(&(first - 1).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(read(&path), Err(BaseError::BadTable { at: 1, .. })));
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_truncated_table_is_refused() {
    let (path, _) = written("truncated");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..30]).unwrap();
    assert!(matches!(read(&path), Err(BaseError::Truncated { .. })));
    std::fs::remove_file(&path).ok();
}

/// A chunk that is no longer a chunk is reported as which chunk it was, rather
/// than as a facet that would not assemble.
#[test]
fn a_broken_chunk_is_named() {
    let (path, _) = written("broken-chunk");
    let mut bytes = std::fs::read(&path).unwrap();
    let first = u64::from_le_bytes(bytes[26..34].try_into().unwrap()) as usize;
    bytes[first] = b'X';
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(read(&path), Err(BaseError::Chunk { at: 0, .. })));
    std::fs::remove_file(&path).ok();
}
