//! Direction B's acceptance test: a real facet through our own format.
//!
//! Skips unless `OPENSHARD_CLIENT` points at a UO client directory, like every
//! other test in this crate that needs shipped files. No client files enter this
//! repository.
//!
//! # Why a fixture is not enough here either
//!
//! `openshard-basemap`'s own tests round-trip a nine-block fixture, and they
//! catch a codec that drops a field. They cannot catch the failure this format
//! is actually shaped against: a *transposition*, where every value is right and
//! every position is wrong. A fixture small enough to write by hand is small
//! enough that its block order barely differs from its row order. Felucca is 896
//! blocks by 512, so a column stride read as a row stride puts a coastline in
//! the middle of an ocean — and it parses perfectly.

use std::path::PathBuf;

use openshard_map::grid::BlockCoord;
use openshard_protocol::world::Facet;

/// The client directory, or `None` to skip.
fn client_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    dir.join("tiledata.mul").exists().then_some(dir)
}

fn path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("openshard-import-{tag}-{}.osbase", std::process::id()))
}

/// Whether two files hold the same bytes, without holding both whole.
fn same_bytes(a: &PathBuf, b: &PathBuf) -> bool {
    use std::io::Read;
    let (Ok(a), Ok(b)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let (mut a, mut b) = (std::io::BufReader::new(a), std::io::BufReader::new(b));
    let (mut left, mut right) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let read = |file: &mut std::io::BufReader<std::fs::File>, into: &mut [u8]| {
            let mut filled = 0;
            while filled < into.len() {
                match file.read(&mut into[filled..]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(_) => return None,
                }
            }
            Some(filled)
        };
        let (Some(n), Some(m)) = (read(&mut a, &mut left), read(&mut b, &mut right)) else {
            return false;
        };
        if n != m || left[..n] != right[..m] {
            return false;
        }
        if n == 0 {
            return true;
        }
    }
}

/// Felucca out through the importer, into a base set, and back — the same
/// world, and the same bytes when written again.
#[test]
fn felucca_imports_into_a_base_set_and_comes_back_whole() {
    let Some(dir) = client_dir() else {
        return;
    };
    let facet = Facet(0);
    let original = openshard_uofiles::map::load_facet(&dir, facet).expect("a readable facet 0");

    let first = path("felucca-a");
    let written = openshard_basemap::write(&first, &original, openshard_basemap::Identity::Mint)
        .expect("a writable temp dir");
    assert_eq!(written.statics, original.map().static_count());
    // 7168x4096 is 112 by 64 chunks, and it divides evenly, so every one of
    // them is whole.
    assert_eq!(written.chunks, 112 * 64);

    let back = openshard_basemap::read(&first).expect("the base set we just wrote");
    assert_eq!(back.facet(), facet);
    assert_eq!(back.revision(), original.revision());

    let (was, is) = (original.map(), back.map());
    assert_eq!((was.width(), was.height()), (is.width(), is.height()));
    assert_eq!(was.static_count(), is.static_count());

    // The ground, sampled across the whole facet. A stride of 31 against a
    // block of 8 walks every position within a block as it goes, so a read that
    // was right about blocks and wrong about cells has nowhere to hide either.
    let (width, height) = (is.width() as u16, is.height() as u16);
    let mut tiles = 0u32;
    for y in (0..height).step_by(31) {
        for x in (0..width).step_by(31) {
            assert_eq!(was.land(x, y), is.land(x, y), "the ground at ({x}, {y})");
            tiles += 1;
        }
    }
    assert!(tiles > 30_000, "only {tiles} tiles sampled");

    // The statics, whole blocks at a time rather than sampled tiles: most tiles
    // of Britannia have nothing on them, so a tile sample is mostly a test that
    // two empty answers agree.
    let extent = is.extent();
    let mut items = 0usize;
    // Through `blocks()` rather than a loop counter: a `BlockIndex` is only
    // ever made by the grid, which is what stops a test from spelling the
    // column-major order out for itself and agreeing with a bug.
    for index in extent.blocks().step_by(37) {
        let block = extent.coord_of(index).expect("a block of this facet");
        let (was, is) = (
            was.statics_in_block(block.x, block.y),
            is.statics_in_block(block.x, block.y),
        );
        assert_eq!(was, is, "the statics of block ({}, {})", block.x, block.y);
        items += is.len();
    }
    assert!(items > 50_000, "only {items} statics compared");

    // And byte-identical: the facet read back writes the same file.
    let second = path("felucca-b");
    openshard_basemap::write(&second, &back, openshard_basemap::Identity::Mint).expect("a writable temp dir");
    assert!(same_bytes(&first, &second), "the round trip changed the bytes");

    std::fs::remove_file(&first).ok();
    std::fs::remove_file(&second).ok();
}

/// A static that overhangs a chunk border belongs to the chunk its anchor tile
/// is in, and nothing copies it into the neighbour.
///
/// `mechanics.md` names this as the ownership rule and names copying as what
/// would make removal and hashing ambiguous. It is a property of the cut rather
/// than of the format, so it is checked on a real facet where borders actually
/// have things standing across them.
#[test]
fn a_static_belongs_to_the_chunk_its_anchor_is_in() {
    let Some(dir) = client_dir() else {
        return;
    };
    let snapshot = openshard_uofiles::map::load_facet(&dir, Facet(0)).expect("a readable facet 0");

    let mut seen = 0usize;
    for at in openshard_map::chunk::chunks_of(snapshot.map().extent()).step_by(97) {
        let chunk = openshard_map::chunk::Chunk::of(&snapshot, at).expect("a chunk of this facet");
        let (from_x, from_y) = at.origin();
        for local in chunk.blocks() {
            let block = chunk.world_block(local);
            for item in chunk.statics_in_block(local) {
                assert_eq!(
                    BlockCoord::containing(item.x, item.y),
                    block,
                    "a static at ({}, {}) is filed under block ({}, {})",
                    item.x,
                    item.y,
                    block.x,
                    block.y
                );
                assert!(
                    u32::from(item.x) >= from_x
                        && u32::from(item.x) < from_x + openshard_map::chunk::CHUNK_TILES
                        && u32::from(item.y) >= from_y
                        && u32::from(item.y) < from_y + openshard_map::chunk::CHUNK_TILES,
                    "a static at ({}, {}) is in the chunk at ({from_x}, {from_y})",
                    item.x,
                    item.y,
                );
                seen += 1;
            }
        }
    }
    assert!(seen > 10_000, "only {seen} statics checked");
}
