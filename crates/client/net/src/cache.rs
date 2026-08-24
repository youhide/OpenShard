//! The ground the shard gave us, kept.
//!
//! `docs/map/new_map_representation/to_the_client.md`'s E3, and the whole of it
//! in one sentence: **the 21.3 MiB is paid once**. E2 taught a client to take
//! the world off the wire and it took it again on every start; this is where
//! what arrived is written down, read back, and compared with what the shard
//! says it is holding now.
//!
//! # The file is a base set, and that is not a coincidence
//!
//! What the client writes is `openshard_basemap`'s own format, read back through
//! [`openshard_basemap::load`] — the same call the shard's boot and both bakes
//! resolve a world through. One reader, one format, one revision rule, whether
//! an operator put the file there or a client wrote it. E0's `--base-set` is
//! therefore not a stepping stone that got thrown away: it is this cache's read
//! path, and a cache hit is not a new startup path.
//!
//! # It is filed under the world, not under the shard
//!
//! The obvious key is the address dialled, and it is the wrong one twice. Our
//! own launcher — `openshard-playground` — dials nothing at all, so two runs
//! over two different `openshard.toml` worlds would share one file; and a shard
//! that re-imports its facet serves a different world at the same address, whose
//! first revision is 1 again, so the revision beside it would agree.
//!
//! So the shard names the world, once, in its
//! [`WorldNotice`](openshard_protocol::chunks::WorldNotice) — see
//! `openshard_basemap::identity_of`, which is a hash of the base set's own bytes
//! — and the name goes in the file name. Two worlds are then two files, one
//! world is one file from any address, and a facet the shard *cannot* name is
//! one this module refuses to keep at all: a world of somebody's install has
//! nothing to tell it apart from another install's tomorrow.
//!
//! What the pair does not separate is two shards whose logs forked from one base
//! set at the same revision — a log taken apart by hand, which the append-only
//! rule in `openshard_basemap::patches` exists to make not a thing that happens.
//!
//! # Whole, and not a tail
//!
//! When the world moves, the file is rewritten entire rather than grown by an
//! append-only tail of newer chunks. That was E3's open question and it was
//! **measured** rather than argued: on the shipped Felucca — 7,168 chunks,
//! 102.6 MiB — `openshard_basemap::write` takes 0.10–0.13 s, and the flush
//! behind it is not measurable next to it. A tail would save a tenth of a second
//! per edit and cost a version 2 of the file format, a second read path, and a
//! rule for when to compact. The measurement retires it.

use std::path::{Path, PathBuf};

use openshard_map::snapshot::MapSnapshot;
use openshard_protocol::chunks::WorldNotice;
use openshard_protocol::world::{Facet, WorldId};

/// The extension a kept world takes: it is a base set, so it is a base set's.
const EXTENSION: &str = "osbase";

/// A kept world could not be read or written.
///
/// **None of these is fatal to a client.** A cache that will not read is a facet
/// to fetch again, and one that will not write is a facet to fetch again next
/// time — so every variant here ends in a line on the terminal and a fetch, not
/// in a closed connection. That is the whole difference between this and
/// [`FetchError`](crate::chunks::FetchError), where every variant is terminal.
#[derive(Debug)]
#[non_exhaustive]
pub enum CacheError {
    /// The shard did not name the world, so there is nothing to file it under.
    ///
    /// A facet read out of a UO install. Not an error in the sense of something
    /// having gone wrong — it is the shard saying "this ground is not a world of
    /// mine to name", and the honest answer is to keep no copy of it.
    Unnamed {
        /// Which facet was being talked about.
        facet: Facet,
    },
    /// There is no file for this world yet, which is every first connection.
    Missing {
        /// Where it would be.
        path: PathBuf,
    },
    /// The file is there and is not one world of ours.
    Unreadable {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: openshard_basemap::BaseError,
    },
    /// The file holds a world, and not the one the shard is describing.
    ///
    /// The identity is in the *name*, so this is the second net under it: a file
    /// somebody put there by hand, or one written by a build that filed things
    /// differently.
    NotThisWorld {
        /// Which file.
        path: PathBuf,
        /// What the shard says its facet and size are.
        wanted: WorldNotice,
        /// What the file says it is.
        found: Facet,
    },
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unnamed { facet } => write!(
                f,
                "the shard does not name its world for facet {}, so there is nothing to keep it \
                 under",
                facet.0
            ),
            Self::Missing { path } => write!(f, "no world kept at {}", path.display()),
            Self::Unreadable { path, source } => {
                write!(f, "the world kept at {}: {source}", path.display())
            }
            Self::NotThisWorld { path, wanted, found } => write!(
                f,
                "{} holds facet {} and the shard is describing facet {} of {}x{} blocks",
                path.display(),
                found.0,
                wanted.facet.0,
                wanted.blocks.wide,
                wanted.blocks.down
            ),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreadable { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Where one world's ground is kept.
///
/// The identity first and the facet second, both in the name: a world names the
/// base set the shard imported, and a facet says which of that shard's grounds
/// this is. `dir` is the caller's — the client puts it beside its own state, in
/// the working directory, which is where `client_ui.toml` lives and for the same
/// reason: per-checkout, visible, and deleting it is how you start again.
#[must_use]
pub fn path_of(dir: &Path, world: WorldId, facet: Facet) -> PathBuf {
    dir.join(format!(
        "openshard-world-{:016x}-{}.{EXTENSION}",
        world.0, facet.0
    ))
}

/// Where the world `notice` describes would be kept, if the shard named it.
///
/// # Errors
///
/// [`CacheError::Unnamed`] for a facet the shard cannot name — see this module's
/// header for why such ground is not kept.
pub fn path_for(dir: &Path, notice: WorldNotice) -> Result<PathBuf, CacheError> {
    let world = notice.world.ok_or(CacheError::Unnamed { facet: notice.facet })?;
    Ok(path_of(dir, world, notice.facet))
}

/// The world kept for `notice`, if there is one and it is that world.
///
/// What comes back is at the revision the file recorded, which is the whole
/// input to the decision above it: equal to the notice's is a client that needs
/// no chunks at all, and behind it is a client that asks what moved.
///
/// The facet and the extent are checked against the notice before it is handed
/// over — the identity in the file name is the first net and this is the second,
/// because a world of the wrong *size* would be refused chunk by chunk later,
/// with the reason a long way from the cause.
///
/// # Errors
///
/// [`CacheError`], every variant of which means "fetch the facet instead".
pub fn read(dir: &Path, notice: WorldNotice) -> Result<MapSnapshot, CacheError> {
    let path = path_for(dir, notice)?;
    if !path.exists() {
        return Err(CacheError::Missing { path });
    }
    // `load` and not `read`: a world of ours is a base set plus the log beside
    // it, and going through the one door is what stops a client and a shard
    // arriving at different revisions of one file. A client writes no patches,
    // so in practice there is no log — but if somebody drops one there, the
    // world is what the log makes it, exactly as it is for the shard.
    let loaded = openshard_basemap::load(&path).map_err(|source| CacheError::Unreadable {
        path: path.clone(),
        source,
    })?;
    let held = loaded.snapshot;
    let extent = held.map().extent();
    if held.facet() != notice.facet || extent.wide != notice.blocks.wide || extent.down != notice.blocks.down
    {
        return Err(CacheError::NotThisWorld {
            path,
            wanted: notice,
            found: held.facet(),
        });
    }
    Ok(held)
}

/// Keep this world, so the next connection asks for the difference.
///
/// Written to a neighbouring path and renamed over, so that a client that dies
/// mid-write leaves the world it had rather than half of one. The rename is
/// atomic on every filesystem this runs on; the alternative is a torn file that
/// `read` refuses next time, which is survivable but silently costs the fetch
/// this whole module exists to avoid.
///
/// # Errors
///
/// [`CacheError::Unnamed`] for a facet the shard did not name, and
/// [`CacheError::Unreadable`] carrying the write that failed — the same variant
/// because a caller does the same thing with both: says so, and carries on with
/// the world it is holding.
pub fn write(dir: &Path, notice: WorldNotice, world: &MapSnapshot) -> Result<PathBuf, CacheError> {
    let path = path_for(dir, notice)?;
    let writing = path.with_extension(format!("{EXTENSION}.writing"));
    openshard_basemap::write(&writing, world).map_err(|source| CacheError::Unreadable {
        path: writing.clone(),
        source,
    })?;
    std::fs::rename(&writing, &path).map_err(|source| {
        // The half-written file is left where it is rather than removed: it is
        // named for this world too, so the next run's rename replaces it, and a
        // failed cleanup after a failed write is a second thing to report about
        // one event.
        CacheError::Unreadable {
            path: path.clone(),
            source: openshard_basemap::BaseError::Write {
                path: path.clone(),
                source,
            },
        }
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::BlockExtent;
    use openshard_map::map::{LandCell, WorldMap};
    use openshard_map::snapshot::MapRevision;
    use openshard_protocol::chunks::{FacetBlocks, WorldRevision};
    use openshard_tiles::LandTileId;

    use super::*;

    const FACET: Facet = Facet(0);
    const WORLD: WorldId = WorldId(0x0123_4567_89AB_CDEF);
    const BLOCKS: u32 = 9;

    /// A directory of this test's own, so two of them in one binary do not read
    /// each other's worlds.
    fn dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("openshard-cache-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("a writable temp dir");
        dir
    }

    fn notice() -> WorldNotice {
        WorldNotice {
            facet: FACET,
            blocks: FacetBlocks {
                wide: BLOCKS,
                down: BLOCKS,
            },
            revision: WorldRevision(1),
            world: Some(WORLD),
        }
    }

    fn a_world(revision: MapRevision) -> MapSnapshot {
        let map = WorldMap::from_blocks(
            BlockExtent {
                wide: BLOCKS,
                down: BLOCKS,
            },
            |x, y| LandCell {
                tile: LandTileId(x.wrapping_mul(7).wrapping_add(y)),
                z: (x as i32 - y as i32) as i8,
            },
        );
        MapSnapshot::restored(FACET, revision, map)
    }

    /// The round trip, which is E3 in one assertion: what was kept comes back as
    /// the world it was, at the revision it was at.
    #[test]
    fn a_world_kept_is_the_world_that_comes_back() {
        let dir = dir("round-trip");
        let kept = a_world(MapRevision::decoded(4));
        let path = write(&dir, notice(), &kept).expect("a writable temp dir");
        assert_eq!(path, path_of(&dir, WORLD, FACET));

        let back = read(&dir, notice()).expect("the file just written");
        assert_eq!(back.revision(), MapRevision::decoded(4));
        assert_eq!(back.facet(), FACET);
        for y in 0..u16::try_from(BLOCKS * 8).unwrap() {
            for x in 0..u16::try_from(BLOCKS * 8).unwrap() {
                assert_eq!(back.map().land(x, y), kept.map().land(x, y), "at ({x}, {y})");
            }
        }
        // And nothing is left behind by the write's own temp file.
        let left: Vec<_> = std::fs::read_dir(&dir)
            .expect("the dir")
            .map(|entry| entry.expect("an entry").file_name())
            .collect();
        assert_eq!(left.len(), 1, "one world, one file: {left:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two worlds are two files, and one world is one file whatever else moves.
    ///
    /// The revision is deliberately different in the second write: the world's
    /// *identity* is what names the file, and the revision inside it is what
    /// says how far along that world is.
    #[test]
    fn a_world_is_filed_under_which_world_it_is() {
        let dir = dir("filing");
        let elsewhere = WorldId(WORLD.0 ^ 1);
        assert_ne!(path_of(&dir, WORLD, FACET), path_of(&dir, elsewhere, FACET));
        assert_ne!(path_of(&dir, WORLD, FACET), path_of(&dir, WORLD, Facet(1)));

        write(&dir, notice(), &a_world(MapRevision::decoded(2))).expect("a writable temp dir");
        write(&dir, notice(), &a_world(MapRevision::decoded(3))).expect("a writable temp dir");
        assert_eq!(
            read(&dir, notice()).expect("the file just written").revision(),
            MapRevision::decoded(3),
            "the same world rewritten is the same file"
        );

        let other = WorldNotice {
            world: Some(elsewhere),
            ..notice()
        };
        write(&dir, other, &a_world(MapRevision::decoded(9))).expect("a writable temp dir");
        assert_eq!(
            read(&dir, notice()).expect("still there").revision(),
            MapRevision::decoded(3),
            "another world's file is not this one's"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A shard that does not name its world gets no cache, in either direction.
    ///
    /// This is the ordinary state of a shard running on a UO install, and the
    /// refusal is the point: there is nothing about such a facet that could tell
    /// it from another install's tomorrow.
    #[test]
    fn a_world_the_shard_cannot_name_is_not_kept() {
        let dir = dir("unnamed");
        let unnamed = WorldNotice {
            world: None,
            ..notice()
        };
        assert!(matches!(
            read(&dir, unnamed),
            Err(CacheError::Unnamed { facet: FACET })
        ));
        assert!(matches!(
            write(&dir, unnamed, &a_world(MapRevision::INITIAL)),
            Err(CacheError::Unnamed { facet: FACET })
        ));
        assert_eq!(
            std::fs::read_dir(&dir).expect("the dir").count(),
            0,
            "and nothing was written"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Every way a file that is there is not the world to take.
    #[test]
    fn a_world_that_is_not_this_one_is_refused_rather_than_drawn() {
        let dir = dir("refusals");
        assert!(matches!(read(&dir, notice()), Err(CacheError::Missing { .. })));

        // A file under the right name that is not a base set at all.
        let path = path_of(&dir, WORLD, FACET);
        std::fs::write(&path, b"not a base set, but long enough to have a header").expect("a temp dir");
        assert!(matches!(read(&dir, notice()), Err(CacheError::Unreadable { .. })));

        // And one that is a world of the wrong size: the identity is in the
        // name, so this is the net under a file somebody moved by hand.
        let wider = WorldMap::from_blocks(
            BlockExtent {
                wide: BLOCKS + 8,
                down: BLOCKS,
            },
            |_, _| LandCell {
                tile: LandTileId(3),
                z: 0,
            },
        );
        openshard_basemap::write(&path, &MapSnapshot::new(FACET, wider)).expect("a temp dir");
        assert!(matches!(
            read(&dir, notice()),
            Err(CacheError::NotThisWorld { .. })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
