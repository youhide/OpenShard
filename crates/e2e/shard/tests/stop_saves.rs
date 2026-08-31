//! A stop puts the world on disk, and `run_shard` does not return before it has.
//!
//! # Why this needs both ends and a real file
//!
//! "`run_shard` returns only once the world is on disk" is the claim the whole
//! shutdown tail is built out of — the last full sweep, the queued writes, the
//! save task awaited rather than detached — and it is what
//! [`openshard_e2e_shard::Running::stop`] promises a caller who waits on it.
//! Nothing asserted it. Every test around it stops a shard with no database at
//! all, where the store is a `MemoryStore` and a save that never happened is
//! indistinguishable from one that did.
//!
//! So: a real SQLite file, a real login, a step that changes something a player
//! would notice, and then the same file opened again by a reader that shares no
//! state with the shard that wrote it. The character has to be there, at the tile
//! it walked to.
//!
//! The reader opens `SqliteStore` directly rather than using `boot::open_store`,
//! which is deliberate: the point is that the bytes are on the disk, and going
//! back in through the shard's own opener would make the assertion partly about
//! the shard's own code path.

use std::path::{
    Path,
    PathBuf,
};
use std::time::Duration;

use openshard_client_net::transport::enter_world_with;
use openshard_client_net::walk::{
    Moved,
    Walk,
};
use openshard_e2e_shard::{
    CHARACTER,
    in_process,
    plan,
    stock_config,
    version,
};
use openshard_persistence::{
    SqliteStore,
    Store,
};
use openshard_protocol::direction::Facing;

/// Generous, and only ever paid by a failure. What it bounds is a hang.
const WAIT: Duration = Duration::from_secs(20);

/// A database file that is removed however the test ends.
///
/// `std::env::temp_dir` and the pid rather than a crate for it: a temporary path
/// is two lines here, and a dependency added for two lines is one more thing in
/// the graph of every build.
///
/// Removed on the way in as well as on the way out. A previous run killed between
/// the two would otherwise leave a file behind, and the next run would open a
/// database that already had this character in it — which is exactly the state
/// that would make the assertion below pass without the shard having written
/// anything.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("openshard-{name}-{}.db", std::process::id()));
        remove(&path);
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        remove(&self.0);
    }
}

/// The database and the two files SQLite may have beside it. Failures are
/// ignored on purpose: this is cleanup, and a temporary file that outlives a
/// crashed test is not worth turning into a second failure over the first.
fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut beside = path.as_os_str().to_owned();
        beside.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(beside));
    }
}

#[tokio::test]
async fn a_stop_leaves_the_world_on_disk_before_it_returns() {
    let scratch = Scratch::new("stop-saves");
    let database = scratch.path().to_string_lossy().into_owned();

    let (dial, shard) = in_process::spawn(
        move |address| {
            let mut config = stock_config(address);
            config.persistence.database = database;
            // The periodic save turned off, so that anything found on the disk
            // afterwards was written by the stop and not by a cadence that happened
            // to fire while the test was walking. Without this the test would pass on
            // a slow machine and prove nothing about shutdown.
            config.persistence.save_seconds = 0;
            config
        },
        Vec::new(),
    );

    let (mut socket, mut view) = tokio::time::timeout(WAIT, enter_world_with(dial, plan(), version()))
        .await
        .expect("the login conversation finished inside the deadline")
        .expect("the client reached the world");

    // One step, acked. The tile it lands on is the thing the disk is asked about
    // afterwards, so it has to be a tile the shard agreed to — an unacked request
    // would leave the test comparing the disk against a move that never happened.
    let start = view.player.position;
    let mut walk = Walk::new(start, view.player.facing);
    let heading = Facing::walking(view.player.facing.direction);
    let step = walk.step(heading, |_, _| None).expect("room on the map to walk");
    socket.send(step.bytes()).await.expect("the shard is listening");

    let stepped = tokio::time::timeout(WAIT, async {
        while let Some(event) = socket.next_event().await.expect("the socket stayed up") {
            let openshard_client_net::connection::Event::Packet(packet) = event else {
                continue;
            };
            match walk.on_packet(&packet).expect("the shard answered in order") {
                Moved::Stepped { position, facing, .. } => {
                    view.player_stepped(position, facing);
                    return position;
                }
                Moved::Snapped { .. } => panic!("the one step was refused; nothing moved"),
                Moved::Turned { .. } | Moved::Idle => {
                    view.apply(&packet);
                }
            }
        }
        panic!("the shard hung up before it acked the step");
    })
    .await
    .expect("the step was acked inside the deadline");

    assert_ne!(
        stepped, start,
        "the body did not move, so finding it at `start` on the disk would prove nothing"
    );

    // Blocking, and the whole point: `stop` joins the shard's thread, which
    // returns from `run_shard`, which returns only once the save task has drained.
    // Everything after this line runs in a world where that claim is either true
    // or the test is about to say so.
    shard.stop();

    let store = Store::sqlite(SqliteStore::open(scratch.path()).expect("the shard left a database behind"));
    let characters = store.characters().await.expect("the database can be read");
    let saved = characters
        .iter()
        .find(|record| record.name == *CHARACTER)
        .expect("the character the client played is not in the database at all");

    assert_eq!(
        (saved.x, saved.y),
        (stepped.x, stepped.y),
        "the stop returned before the step it had already acked reached the disk"
    );
}
