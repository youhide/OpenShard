//! A facet out to a file and back, and every way a file fails to be one.
//!
//! The fixture is nine blocks square, which is **not** a whole number of
//! chunks: three of its four chunks are edge chunks, so a reader that assumed
//! every chunk was eight blocks by eight would fail here rather than on Tokuno.

use openshard_basemap::{
    BaseError,
    Identity,
    read,
    write,
};
use openshard_map::grid::BlockExtent;
use openshard_map::map::{
    LandCell,
    StaticItem,
    WorldMap,
};
use openshard_map::snapshot::{
    MapRevision,
    MapSnapshot,
};
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_protocol::world::Facet;
use openshard_tiles::LandTileId;

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
    MapSnapshot::new(FACET, a_map())
}

/// The fixture's ground on its own, for the one test that wants to move a tile
/// of it before it is published.
fn a_map() -> WorldMap {
    let mut map = WorldMap::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        |x, y| {
            LandCell {
                tile: LandTileId(u16::try_from(u32::from(x) * TILES + u32::from(y)).unwrap()),
                z:    (i32::from(x) - i32::from(y)) as i8,
            }
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
            x:    20,
            y:    21,
            z:    5,
            hue:  Hue(n),
        });
    }
    map
}

fn written(tag: &str) -> (std::path::PathBuf, MapSnapshot) {
    let path = path(tag);
    let snapshot = snapshot();
    write(&path, &snapshot, Identity::Mint).expect("a writable temp dir");
    (path, snapshot)
}

/// Where the offsets table starts: the header is 34 bytes.
const TABLE: usize = 34;

/// FNV-1a, 64 bits — the file's own spelling, written out again here.
///
/// Duplicated on purpose rather than exported: the format pins this hash, so a
/// test that computes it independently is what holds the file to the algorithm
/// it says it uses, and it is what lets a fixture below build a manifest entry
/// the reader will accept.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Where chunk `at`'s bytes start and end, and what its manifest entry says.
fn entry(bytes: &[u8], count: usize, at: usize) -> (usize, usize, u64, u32) {
    let offset = |which: usize| {
        let start = TABLE + which * 8;
        u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap()) as usize
    };
    let manifest = TABLE + (count + 1) * 8 + at * 12;
    (
        offset(at),
        offset(at + 1),
        u64::from_le_bytes(bytes[manifest..manifest + 8].try_into().unwrap()),
        u32::from_le_bytes(bytes[manifest + 8..manifest + 12].try_into().unwrap()),
    )
}

/// How many chunks the header claims.
fn count_of(bytes: &[u8]) -> usize {
    u32::from_le_bytes(bytes[22..26].try_into().unwrap()) as usize
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
    write(&second, &snapshot, Identity::Mint).expect("a writable temp dir");
    assert_eq!(std::fs::read(&first).unwrap(), std::fs::read(&second).unwrap());

    // And a facet read back and written again is the same file a third time —
    // the round trip is byte-identical, not merely lossless.
    let third = path("canonical-c");
    write(&third, &read(&first).expect("a base set"), Identity::Mint).expect("a writable temp dir");
    assert_eq!(std::fs::read(&first).unwrap(), std::fs::read(&third).unwrap());

    for path in [first, second, third] {
        std::fs::remove_file(path).ok();
    }
}

/// Two worlds that differ by one tile are two identities, and one world written
/// twice is one — which is the whole of what a client files its cache under.
///
/// The second half is the load-bearing one: a shard that minted a fresh identity
/// every time it wrote its world would send a client back to fetching the facet
/// on every restart, and nothing would say why.
#[test]
fn a_world_is_named_by_its_own_content() {
    let (first, snapshot) = written("identity-a");
    let same = path("identity-b");
    write(&same, &snapshot, Identity::Mint).expect("a writable temp dir");
    assert_eq!(
        openshard_basemap::identity_of(&first).expect("a base set"),
        openshard_basemap::identity_of(&same).expect("a base set"),
    );

    let elsewhere = path("identity-c");
    let mut moved = a_map();
    moved.set_land(
        3,
        4,
        LandCell {
            tile: LandTileId(0x3FF),
            z:    12,
        },
    );
    let moved = MapSnapshot::new(FACET, moved);
    write(&elsewhere, &moved, Identity::Mint).expect("a writable temp dir");
    assert_ne!(
        openshard_basemap::identity_of(&first).expect("a base set"),
        openshard_basemap::identity_of(&elsewhere).expect("a base set"),
        "one tile of difference is a different world"
    );

    // And the other half, which is what a squash and a client's cache both
    // stand on: a world written again under the identity it already had keeps
    // it, however much of its content moved in between. Minting from content is
    // what names a *new* world; it is not what a rewrite of an old one does.
    let carried = path("identity-d");
    let known = openshard_basemap::identity_of(&first).expect("a base set");
    write(&carried, &moved, Identity::Keep(known)).expect("a writable temp dir");
    assert_eq!(
        openshard_basemap::identity_of(&carried).expect("a base set"),
        known,
        "a carried identity survives the content moving under it"
    );
    std::fs::remove_file(&carried).ok();

    // And a file that is not a base set has no identity to take: naming a world
    // after somebody else's bytes is worse than not naming it.
    let foreign = path("identity-foreign");
    std::fs::write(
        &foreign,
        b"not a base set at all, but long enough to have a header",
    )
    .unwrap();
    assert!(matches!(
        openshard_basemap::identity_of(&foreign),
        Err(BaseError::NotABaseSet { .. })
    ));

    for path in [first, same, elsewhere, foreign] {
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
    write(&path, &published, Identity::Mint).expect("a writable temp dir");

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
    bytes[4] = 3;
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(read(&path), Err(BaseError::Version { found: 3, .. })));
    // And the identity, which reads the header alone, refuses it there too: a
    // world named out of a layout this build cannot read is a name for nothing.
    assert!(matches!(
        openshard_basemap::identity_of(&path),
        Err(BaseError::Version { found: 3, .. })
    ));
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
    let first = u64::from_le_bytes(bytes[TABLE..TABLE + 8].try_into().unwrap());
    bytes[TABLE + 8..TABLE + 16].copy_from_slice(&(first - 1).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(read(&path), Err(BaseError::BadTable { at: 1, .. })));
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_truncated_table_is_refused() {
    let (path, _) = written("truncated");
    let bytes = std::fs::read(&path).unwrap();
    // Past the header, so the file is a base set that says how many chunks it
    // holds, and short of the table and manifest it says are there.
    std::fs::write(&path, &bytes[..TABLE + 8]).unwrap();
    assert!(matches!(read(&path), Err(BaseError::Truncated { .. })));
    std::fs::remove_file(&path).ok();
}

/// A chunk that is no longer a chunk is reported as which chunk it was, rather
/// than as a facet that would not assemble.
///
/// It takes a hand-built file to get here now, and that is the point of the two
/// tests below it: a byte moved in a real file is caught by the manifest long
/// before a decoder sees it. What is left for this variant is a record that
/// inflates to its declared length, hashes to what the manifest says, and is
/// still not a chunk — which is a file somebody wrote with something other than
/// this crate.
#[test]
fn a_broken_chunk_is_named() {
    let (path, _) = written("broken-chunk");
    let mut bytes = std::fs::read(&path).unwrap();
    let count = count_of(&bytes);
    let (start, end, _, _) = entry(&bytes, count, 0);

    // A record of the right shape for the manifest and the wrong magic for a
    // chunk. The rest of the file's chunks keep their own entries: only the
    // first is replaced, and everything after it shifts by the difference.
    let record = b"OSMX and then some bytes that are not a chunk record".to_vec();
    let blob =
        openshard_protocol::chunks::deflate(&record, openshard_protocol::chunks::DeflateLevel::BASE_SET);
    let manifest = TABLE + (count + 1) * 8;
    bytes[manifest..manifest + 8].copy_from_slice(&fnv1a64(&record).to_le_bytes());
    bytes[manifest + 8..manifest + 12].copy_from_slice(&u32::try_from(record.len()).unwrap().to_le_bytes());
    bytes.splice(start..end, blob.iter().copied());
    // Every offset after the first chunk's start moves by what the blob's
    // length changed by.
    let moved = blob.len() as i64 - (end - start) as i64;
    for at in 1..=count {
        let slot = TABLE + at * 8;
        let was = u64::from_le_bytes(bytes[slot..slot + 8].try_into().unwrap()) as i64;
        bytes[slot..slot + 8].copy_from_slice(&((was + moved) as u64).to_le_bytes());
    }

    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(read(&path), Err(BaseError::Chunk { at: 0, .. })));
    std::fs::remove_file(&path).ok();
}

/// A byte moved inside a chunk's blob is caught by the manifest, not by luck.
///
/// A deflate stream with a byte changed in it usually fails to inflate at all,
/// and sometimes inflates into something else; both are refused, and which one
/// happened is not this test's business — what matters is that neither reaches
/// a decoder that would build a plausible square out of it.
#[test]
fn a_chunk_that_changed_under_the_file_is_refused() {
    let (path, _) = written("changed-chunk");
    let mut bytes = std::fs::read(&path).unwrap();
    let count = count_of(&bytes);
    let (start, end, _, _) = entry(&bytes, count, 0);
    // The last byte of the stream rather than the first: the header of a zlib
    // stream is checked before anything is inflated, and the interesting case
    // is the one that gets past it.
    bytes[end - 1] ^= 0xFF;
    assert!(end - start > 2, "a deflate stream is longer than its header");

    std::fs::write(&path, &bytes).unwrap();
    assert!(
        matches!(
            read(&path),
            Err(BaseError::NotDeflated { at: 0, .. } | BaseError::HashMismatch { at: 0, .. })
        ),
        "a corrupted chunk is refused as chunk 0"
    );
    std::fs::remove_file(&path).ok();
}

/// The manifest is what the chunks hash to, and the chunks are deflated.
///
/// Both halves in one test because they are one claim about the file: the
/// manifest describes the *record*, and what is stored is that record's deflate
/// stream. A reader that hashed the stored bytes instead would pass every other
/// test here and refuse every file written by a build with a different
/// compressor in it.
#[test]
fn the_chunks_are_stored_deflated_and_the_manifest_is_of_the_records() {
    let (path, _) = written("deflated");
    let bytes = std::fs::read(&path).unwrap();
    let count = count_of(&bytes);
    assert_eq!(count, 4, "nine blocks square is four chunks");

    let mut records = 0;
    let mut stored = 0;
    for at in 0..count {
        let (start, end, hash, inflated) = entry(&bytes, count, at);
        let record = openshard_protocol::chunks::inflate(
            &bytes[start..end],
            openshard_protocol::chunks::InflatedLength(inflated),
        )
        .expect("a chunk stored as a deflate stream of its declared length");
        assert_eq!(fnv1a64(&record), hash, "chunk {at}'s manifest entry");
        assert_eq!(&record[..4], b"OSMC", "chunk {at} is a chunk record");
        records += record.len();
        stored += end - start;
    }
    assert!(
        stored < records,
        "the file stores {stored} bytes for {records} of records"
    );
    std::fs::remove_file(&path).ok();
}
