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
//!
//! # How many worlds a client keeps
//!
//! Filing under the world is what makes an *orphan* possible: a shard that
//! re-imports its facet serves a world with a new identity, so the copy of the
//! old one stays on the disk under the old name and nothing will ever ask for it
//! again. On a Felucca that is 102 MiB, and a checkout somebody is re-importing
//! while they work collects one per import.
//!
//! What is missing is a rule rather than a mechanism, because the names of every
//! world a client has kept are in one directory. The rule is
//! [`KEPT_PER_FACET`]: on each write, every facet's worlds are ranked by when
//! they were last *used* and the tail goes. Used, not written — [`read`] stamps
//! the file it read, because a world that is always current is never rewritten
//! and would otherwise be the first one evicted for being old.
//!
//! A world goes with everything named after it. `bake::artifact_path` names a
//! navigation graph after the world's file stem, and a torn write leaves
//! `.osbase.writing` beside it, so "the world and every file whose name begins
//! with its stem" is one rule that covers all three without this module having
//! to know what a bake is.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use openshard_map::snapshot::MapSnapshot;
use openshard_protocol::chunks::WorldNotice;
use openshard_protocol::world::{Facet, WorldId};

/// The extension a kept world takes: it is a base set, so it is a base set's.
const EXTENSION: &str = "osbase";

/// What every kept world's name begins with, and nothing else in the directory
/// does.
const PREFIX: &str = "openshard-world-";

/// How many worlds of one facet a client keeps.
///
/// **Two, and the second one is the point.** One would be a rule that a client
/// may hold the ground of exactly one shard per facet, so a person who plays two
/// would re-fetch a facet on every start and this whole module would be worth
/// nothing to them. Three would be a shard they play rarely, and fetching that
/// one again costs seconds *once*.
///
/// The number bounds the directory at twice a facet — about 205 MiB for a
/// Felucca-sized one — per facet visited, which is the size of the thing being
/// traded against and is why it is a small number rather than a generous one.
const KEPT_PER_FACET: usize = 2;

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
/// the working directory, which is where `client_ui.ron` lives and for the same
/// reason: per-checkout, visible, and deleting it is how you start again.
#[must_use]
pub fn path_of(dir: &Path, world: WorldId, facet: Facet) -> PathBuf {
    dir.join(format!("{}.{EXTENSION}", stem_of(world, facet)))
}

/// A kept world's file name without its extension.
///
/// Everything derived from that world is named after this — a navigation graph
/// by `bake::artifact_path`, and a torn write by [`write`] — which is what makes
/// [`sweep`] able to let go of a world and its belongings in one rule.
fn stem_of(world: WorldId, facet: Facet) -> String {
    format!("{PREFIX}{:016x}-{}", world.0, facet.0)
}

/// Which world a file name in the cache directory names, if it names one.
///
/// The inverse of [`stem_of`], and deliberately strict: a name that is not
/// exactly the prefix, sixteen hex digits, a dash and a facet number is not a
/// file this module wrote, and [`sweep`] must not delete somebody else's.
fn world_named_by(name: &str) -> Option<(WorldId, Facet)> {
    let stem = name.strip_suffix(&format!(".{EXTENSION}"))?;
    let (identity, facet) = stem.strip_prefix(PREFIX)?.split_once('-')?;
    if identity.len() != 16 {
        return None;
    }
    Some((
        WorldId(u64::from_str_radix(identity, 16).ok()?),
        Facet(facet.parse().ok()?),
    ))
}

/// Whether `name` is `stem`'s own file or something named after it.
///
/// The character after the stem has to be a separator: a facet number is written
/// out in decimal, so the stem of facet 1's world is a prefix of the *string*
/// naming facet 10's, and a plain `starts_with` would sweep a world nobody asked
/// about.
fn belongs_to(name: &str, stem: &str) -> bool {
    name.strip_prefix(stem)
        .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('-'))
}

/// Let go of every world this client keeps beyond [`KEPT_PER_FACET`] of each
/// facet, and of everything named after those worlds.
///
/// Ranked by when each was last used — [`read`] stamps what it reads and
/// [`write`] has just stamped `keeping`, so the file's own modified time is that
/// clock. `keeping` is named explicitly rather than trusted to be the newest,
/// because a caller must never be able to lose the world it has this second.
///
/// Best effort in both halves: a directory that cannot be read sweeps nothing,
/// and a file that cannot be removed stays. Neither is worth a failed write —
/// what a sweep that did not happen costs is disk, and it will be attempted
/// again on the next one. What comes back is the worlds that really went, each
/// named by its base set.
#[must_use]
pub fn sweep(dir: &Path, keeping: &Path) -> Vec<PathBuf> {
    // Every world in the directory, by facet, newest use first.
    let mut by_facet: std::collections::BTreeMap<Facet, Vec<(SystemTime, PathBuf)>> =
        std::collections::BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((_, facet)) = world_named_by(name) else {
            continue;
        };
        // A world whose time cannot be read is the oldest there is: it is the
        // one this rule knows least about, so it is the one to let go of first.
        let used = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        by_facet.entry(facet).or_default().push((used, entry.path()));
    }

    let mut swept = Vec::new();
    for mut worlds in by_facet.into_values() {
        worlds.sort_unstable_by_key(|(used, _)| std::cmp::Reverse(*used));
        let mut places = KEPT_PER_FACET;
        for (_, world) in worlds {
            // `keeping` takes a place whatever its time says. It was written a
            // moment ago so it is the newest here anyway, but a caller must not
            // be able to lose the world it is holding to a clock.
            if world == keeping || places > 0 {
                places = places.saturating_sub(1);
                continue;
            }
            if forget(dir, &world) {
                swept.push(world);
            }
        }
    }
    swept
}

/// Remove one kept world and everything named after it, and say whether the
/// world itself went.
fn forget(dir: &Path, world: &Path) -> bool {
    let stem = world.file_stem().map(|stem| stem.to_string_lossy().into_owned());
    let Some(stem) = stem else { return false };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if belongs_to(name, &stem) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
    !world.exists()
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
    used(&path);
    Ok(held)
}

/// Say that this world is one somebody still plays on.
///
/// [`sweep`] ranks by the file's modified time, and a world that is *already
/// current* is never rewritten — so without this the best cache a client has
/// would be the first one it let go of. Reading is using, and this is where that
/// is written down.
///
/// Silent on failure, and deliberately: a read-only checkout can hand back a
/// world perfectly well, and what a stamp that did not happen costs is a place
/// in a ranking. Whether the file opens for writing is not this call's question
/// — [`write`] asks it, about the file it is actually writing.
fn used(path: &Path) {
    let now = std::fs::FileTimes::new()
        .set_accessed(SystemTime::now())
        .set_modified(SystemTime::now());
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        file.set_times(now).ok();
    }
}

/// What keeping a world did.
///
/// Two facts because a caller reports both: where the world now is, and which
/// worlds it let go of to hold it. [`sweep`] is not something a caller can be
/// left to remember — a client that wrote and never swept would collect a facet
/// per re-import — so it is part of the write and its result comes back with it.
#[derive(Debug)]
pub struct Kept {
    /// Where this world is now kept.
    pub path: PathBuf,
    /// The worlds let go of to make room, each named by its base set.
    ///
    /// Empty on nearly every write: it takes a *new* world of a facet this
    /// client already keeps two of, which is a shard that re-imported its facet
    /// or a third shard on one facet.
    pub swept: Vec<PathBuf>,
}

/// Keep this world, so the next connection asks for the difference.
///
/// Written to a neighbouring path and renamed over, so that a client that dies
/// mid-write leaves the world it had rather than half of one. The rename is
/// atomic on every filesystem this runs on; the alternative is a torn file that
/// `read` refuses next time, which is survivable but silently costs the fetch
/// this whole module exists to avoid.
///
/// The [`sweep`] afterwards is what stops a directory from collecting a facet
/// per re-import — see this module's header for the rule, and [`Kept`] for why
/// it is part of the write rather than a second call a caller could forget.
///
/// # Errors
///
/// [`CacheError::Unnamed`] for a facet the shard did not name, and
/// [`CacheError::Unreadable`] carrying the write that failed — the same variant
/// because a caller does the same thing with both: says so, and carries on with
/// the world it is holding.
pub fn write(dir: &Path, notice: WorldNotice, world: &MapSnapshot) -> Result<Kept, CacheError> {
    let path = path_for(dir, notice)?;
    let identity = notice.world.ok_or(CacheError::Unnamed { facet: notice.facet })?;
    let writing = path.with_extension(format!("{EXTENSION}.writing"));
    // The shard's name for its world, carried into our copy of it rather than a
    // fresh one minted here: this file is somebody else's world, and a world
    // that changed identity by being cached is a world every later comparison
    // is about the wrong thing.
    openshard_basemap::write(&writing, world, openshard_basemap::Identity::Keep(identity)).map_err(
        |source| CacheError::Unreadable {
            path: writing.clone(),
            source,
        },
    )?;
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
    let swept = sweep(dir, &path);
    Ok(Kept { path, swept })
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
        world_notice(WORLD, FACET)
    }

    fn world_notice(world: WorldId, facet: Facet) -> WorldNotice {
        WorldNotice {
            facet,
            blocks: FacetBlocks {
                wide: BLOCKS,
                down: BLOCKS,
            },
            revision: WorldRevision(1),
            world: Some(world),
        }
    }

    /// Say a world was last used this many seconds ago.
    ///
    /// The ranking [`sweep`] makes is over a clock with a resolution, and three
    /// writes in one test share a millisecond — so the tests about *order* set
    /// the order rather than hoping the filesystem noticed one.
    fn used_at(path: &Path, seconds_ago: u64) {
        let when = SystemTime::now() - std::time::Duration::from_secs(seconds_ago);
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("a world this test just wrote");
        file.set_times(std::fs::FileTimes::new().set_accessed(when).set_modified(when))
            .expect("a writable temp dir");
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
        let written = write(&dir, notice(), &kept).expect("a writable temp dir");
        assert_eq!(written.path, path_of(&dir, WORLD, FACET));
        assert!(written.swept.is_empty(), "the first world lets go of nothing");

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

    /// A world of a facet this client already keeps two of pushes one out.
    ///
    /// Three worlds of one facet is a shard that re-imported its Felucca, and
    /// the copy of the one before it is a hundred megabytes nothing will ever ask
    /// for again — see [`KEPT_PER_FACET`], which is where the number two is
    /// argued.
    #[test]
    fn a_third_world_of_one_facet_lets_go_of_the_least_recently_used() {
        let dir = dir("sweep");
        let (first, second, third) = (WorldId(1), WorldId(2), WorldId(3));
        for world in [first, second] {
            write(&dir, world_notice(world, FACET), &a_world(MapRevision::INITIAL))
                .expect("a writable temp dir");
        }
        // An explicit clock: three writes in one millisecond can share a modified
        // time, and the ranking is the whole thing under test.
        used_at(&path_of(&dir, first, FACET), 300);
        used_at(&path_of(&dir, second, FACET), 200);

        let written = write(&dir, world_notice(third, FACET), &a_world(MapRevision::INITIAL))
            .expect("a writable temp dir");
        assert_eq!(written.swept, vec![path_of(&dir, first, FACET)]);
        assert!(!path_of(&dir, first, FACET).exists());
        assert!(path_of(&dir, second, FACET).exists());
        assert!(path_of(&dir, third, FACET).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Reading a world is using it, and a world in use is not the one to drop.
    ///
    /// The case the modified time alone gets exactly backwards: a world that is
    /// already at the shard's revision is never *written* again, so ranking by
    /// when each was last written would let go of the one cache that is paying
    /// for itself on every connection.
    #[test]
    fn a_world_read_is_a_world_used() {
        let dir = dir("used");
        let (older, newer, arriving) = (WorldId(1), WorldId(2), WorldId(3));
        for world in [older, newer] {
            write(&dir, world_notice(world, FACET), &a_world(MapRevision::INITIAL))
                .expect("a writable temp dir");
        }
        used_at(&path_of(&dir, older, FACET), 300);
        used_at(&path_of(&dir, newer, FACET), 200);

        // The older world is the one this client actually plays on.
        read(&dir, world_notice(older, FACET)).expect("the file just written");

        let written = write(
            &dir,
            world_notice(arriving, FACET),
            &a_world(MapRevision::INITIAL),
        )
        .expect("a writable temp dir");
        assert_eq!(
            written.swept,
            vec![path_of(&dir, newer, FACET)],
            "the one that was written later and used never"
        );
        assert!(path_of(&dir, older, FACET).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A world takes its belongings with it, and leaves everything else alone.
    ///
    /// The graph baked beside it is named after its stem by
    /// `bake::artifact_path`, and a torn write leaves `.osbase.writing` there —
    /// so one rule about names covers both without this module knowing what a
    /// bake is. What the same rule must *not* touch is a world of another facet
    /// whose number happens to start with this one's: facet 1's stem is a string
    /// prefix of facet 10's.
    #[test]
    fn everything_named_after_a_world_goes_with_it_and_nothing_else_does() {
        let dir = dir("belongings");
        let (going, staying) = (WorldId(1), WorldId(2));
        for world in [going, staying] {
            write(
                &dir,
                world_notice(world, Facet(1)),
                &a_world(MapRevision::INITIAL),
            )
            .expect("a writable temp dir");
        }
        used_at(&path_of(&dir, going, Facet(1)), 300);
        used_at(&path_of(&dir, staying, Facet(1)), 200);

        let belongings = [
            dir.join(format!("{}-navigation-1.bin", stem_of(going, Facet(1)))),
            dir.join(format!("{}.osbase.writing", stem_of(going, Facet(1)))),
        ];
        // A world of facet 10, whose name begins with facet 1's stem, and a file
        // this module did not write.
        let neighbour = path_of(&dir, going, Facet(10));
        let stranger = dir.join("client_ui.toml");
        for path in belongings.iter().chain([&neighbour, &stranger]) {
            std::fs::write(path, b"beside a world").expect("a writable temp dir");
        }

        let written = write(
            &dir,
            world_notice(WorldId(3), Facet(1)),
            &a_world(MapRevision::INITIAL),
        )
        .expect("a writable temp dir");
        assert_eq!(written.swept, vec![path_of(&dir, going, Facet(1))]);
        for gone in &belongings {
            assert!(!gone.exists(), "{} went with its world", gone.display());
        }
        assert!(neighbour.exists(), "facet 10 is not facet 1");
        assert!(stranger.exists(), "and this module wrote none of that");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Facets are counted separately, because a person walks between them.
    ///
    /// A shard has up to six, each kept in a file of its own, so one budget over
    /// the directory would make travelling across a shard evict that shard's own
    /// ground.
    #[test]
    fn each_facet_is_counted_on_its_own() {
        let dir = dir("facets");
        for facet in 0..4u8 {
            write(
                &dir,
                world_notice(WORLD, Facet(facet)),
                &a_world(MapRevision::INITIAL),
            )
            .expect("a writable temp dir");
        }
        for facet in 0..4u8 {
            assert!(
                path_of(&dir, WORLD, Facet(facet)).exists(),
                "one shard's four facets are four worlds and all of them are its own"
            );
        }
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
        openshard_basemap::write(
            &path,
            &MapSnapshot::new(FACET, wider),
            openshard_basemap::Identity::Keep(WORLD),
        )
        .expect("a temp dir");
        assert!(matches!(
            read(&dir, notice()),
            Err(CacheError::NotThisWorld { .. })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
