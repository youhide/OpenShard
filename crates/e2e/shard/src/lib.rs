//! A real shard in this process, and the tests that talk to one.
//!
//! # Why this crate exists at all
//!
//! `server/*` and `client/*` never depend on each other — that rule is what
//! keeps the wire the only thing they agree on, and it is worth keeping. But a
//! test that a *client* can log in to a *shard* needs both ends in one process,
//! and putting it on either side would make that side depend on the other,
//! dev-dependency or not.
//!
//! So it lives outside both. This crate is the only place in the workspace
//! allowed to name both ends, and nothing outside `crates/e2e/*` depends on it.
//!
//! # What belongs here
//!
//! In `tests/`: only what cannot be tested on one side alone. The gateway's
//! framing, the client's login machine and the world's tick all have their own
//! tests, and those are better tests — pure state machines, no ports, no timing.
//! What is left for this crate is the seam: that the two ends, each correct,
//! actually agree.
//!
//! In `src/` — here — the shard those tests need, and nothing that is a test
//! itself. It used to be `tests/common/mod.rs`, and it moved out for one reason:
//! `crates/e2e/playground` wants that same shard with a window in front of it
//! rather than a test, and a `tests/` module cannot be shared with a binary.
//! What is left below is *how to start a shard*, with the one thing a caller
//! legitimately differs on — the config — left to it.

pub mod in_process;

use std::net::{SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use openshard_client_net::session::{Pick, Plan};
use openshard_config::{Config, ConfigError, DEFAULT_TOML};
use openshard_gateway::{ClientGatewayServer, Shutdown};
use openshard_protocol::identity::{RawAccountName, RawPlaintextPassword};
use openshard_protocol::version::ClientVersion;

/// The client we claim to be. ClassicUO's own opening version, which is what
/// keeps the shard on the modern packet shapes.
pub fn version() -> ClientVersion {
    ClientVersion::new(7, 0, 45, 65)
}

/// The account the stock config ships with, and which these tests log in as.
pub const ACCOUNT: &str = "admin";
/// Its password, and the one every account here is given: they are all
/// development accounts and none of them is testing a password.
pub const PASSWORD: &str = "hunter2";
/// The character on it.
pub const CHARACTER: &str = "Lord British";

/// A second account, added by [`stock_config`] — see there for why it is not a
/// second character on [`ACCOUNT`].
pub const WITNESS: &str = "witness";
/// The character on it.
pub const NYSTUL: &str = "Nystul";

/// The stock config, pointed at `address`, plus the one account a caller cannot
/// get from it.
///
/// The stock account is not replaced or invented: the shipped config already
/// carries the development account these tests log in as, and a test that made
/// up its own would stop noticing when that changed.
///
/// The port matters twice — the shard listens on it, and the `0x8C` relay tells
/// the client to dial `advertise`. Get the second wrong and the client
/// disconnects politely and never comes back, so both are asserted rather than
/// assumed: they are produced by editing text, and text drifts.
pub fn stock_config(address: SocketAddr) -> Config {
    let port = address.port();
    let text = DEFAULT_TOML
        .replace(
            "listen = \"0.0.0.0:2593\"",
            &format!("listen = \"127.0.0.1:{port}\""),
        )
        .replace(
            "advertise = \"127.0.0.1:2593\"",
            &format!("advertise = \"127.0.0.1:{port}\""),
        );

    // The second account, for the tests that need two players in the world at
    // once. Appended rather than given to `admin` as a second character: two
    // characters on one account would rest on the shard letting one account hold
    // two connections, which nothing states and nothing enforces — an accident
    // to build a fixture on. A whole `[[accounts]]` table at the end of the file
    // is a complete table wherever the sections above it move to.
    let text = format!(
        "{text}\n[[accounts]]\nname = \"{WITNESS}\"\npassword = \"{PASSWORD}\"\ncharacters = [\"{NYSTUL}\"]\n"
    );

    let config: Config = toml::from_str(&text).expect("the stock config parses");
    assert_eq!(
        config.server.listen.port(),
        port,
        "the listen address was not replaced: the stock config's wording changed"
    );
    assert_eq!(
        config.server.advertise.port(),
        port,
        "the advertised address was not replaced: the relay would send the client elsewhere"
    );
    assert!(
        config.accounts.iter().any(|account| {
            account.name == ACCOUNT && account.characters.iter().any(|name| name == CHARACTER)
        }),
        "the stock config no longer ships {ACCOUNT} with {CHARACTER}"
    );
    assert!(
        config.accounts.iter().any(|account| {
            account.name == WITNESS && account.characters.iter().any(|name| name == NYSTUL)
        }),
        "the appended account did not come back out of the parse: the shape of an \
         [[accounts]] table changed"
    );
    config
}

/// The operator's own `openshard.toml`, if there is one to read.
///
/// `Ok(None)` means there is no such file, which is not a failure: a fresh
/// checkout has none, and the caller falls back to [`stock_config`].
///
/// # Only the playground may call this
///
/// A test that read the machine's config would pass or fail on what somebody
/// happened to have configured — a pack loaded, a database pointed at a real
/// file, a gameplay knob turned — and that is the opposite of what these tests
/// are for. They take [`stock_config`], which is the shipped default and the
/// same on every machine. The playground is the exception because it is not a
/// test: it exists to *look* at a world, and the world worth looking at is the
/// one the operator has already laid.
///
/// Nothing here overrides the addresses. The caller does that, because only it
/// knows the address the in-process shard was given.
pub fn operator_config(path: impl AsRef<Path>) -> Result<Option<Config>, ConfigError> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(None);
    }
    Config::load(path).map(Some)
}

/// The base set the *window* should read, given the config the shard will run
/// on — or `None` for the install, which is what a config naming none means.
///
/// Facet 0, because that is the facet the window draws: `openshard_client_app`
/// pins `FACET` and one process opens one facet. A playground whose two ends
/// read two different worlds is the disagreement it exists to make impossible,
/// and after one committed patch a base set *is* a different world from the
/// install it was imported from.
///
/// Here rather than in the playground because the config is here: the playground
/// depends on this crate and on the client, and on nothing else that could
/// answer the question.
#[must_use]
pub fn window_base_set(config: Option<&Config>) -> Option<PathBuf> {
    config?
        .world
        .base_set(openshard_protocol::world::Facet(0))
        .map(Path::to_owned)
}

/// A shard running on a thread of its own, and the way to end it.
///
/// # Hold it for as long as the shard should live
///
/// Dropping one stops the shard and waits for its thread, which is where the
/// last save happens — so `let (address, _) = shard();` starts a shard and
/// immediately ends it, and the test that follows finds nothing listening.
/// Bind it to a name.
///
/// # Why a caller needs one at all
///
/// Because a shard used to have no way to stop: [`spawn`] kept a thread nothing
/// joined, and the gate it held kept the event channel open, so `run_shard`
/// never saw its input close. That is right for a process that ends with the
/// shard and wrong for a test — and wrong in a way that grows, because a fuzzing
/// run wants fifty worlds started and dropped, not fifty threads kept until the
/// process exits.
#[derive(Debug)]
#[must_use = "the shard stops when this is dropped"]
pub struct Running {
    stop: Shutdown,
    /// `Option` only because [`Drop`] has to move the handle out of a `&mut
    /// self` to join it. It is `Some` for the whole life of the value.
    thread: Option<JoinHandle<()>>,
}

impl Running {
    /// Stop the shard and wait until it has stopped.
    ///
    /// The wait is the point: `run_shard` writes the world on its way out, so
    /// this returns once the last save has landed and the thread is gone. A
    /// test that asserts on what a shard persisted has somewhere to put the
    /// assertion.
    pub fn stop(mut self) {
        self.halt();
    }

    /// What both [`Running::stop`] and [`Drop`] do. Idempotent: whichever runs
    /// first takes the handle, and the other finds nothing to join.
    ///
    /// # A shard that died is a failed test, unless something is already failing
    ///
    /// A panic inside [`Drop`] *while another panic is unwinding* aborts the
    /// process, which would replace the failure the test was about to report
    /// with a bare abort. But that is only the case while unwinding, and
    /// `std::thread::panicking()` is exactly the question — so the two cases are
    /// separated rather than conflated: something is already going wrong, so add
    /// a line and let it be reported; nothing is, so re-raise the shard's own
    /// payload and let it reach the harness as the test's failure.
    ///
    /// Re-raising from here is safe against the drop that follows it, because
    /// this is idempotent: `stop` unwinds out of `halt` with the handle already
    /// taken, so the `Drop` that runs on the way out finds `None` and joins
    /// nothing. Nothing proves that second half in a test — a test that panics
    /// inside a panic aborts the runner rather than failing — so the argument is
    /// here instead.
    fn halt(&mut self) {
        self.stop.stop();
        if let Some(thread) = self.thread.take() {
            if let Err(payload) = thread.join() {
                if std::thread::panicking() {
                    eprintln!("the shard thread panicked");
                } else {
                    std::panic::resume_unwind(payload);
                }
            }
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Start a shard on an ephemeral port and hand back where it listens.
///
/// `config` is called with the address the gateway actually bound, because the
/// port is in the config twice — `listen`, and the `advertise` the `0x8C` relay
/// hands out — and neither can be filled in before the bind. That is the whole
/// of what a caller varies: [`shard`] is what the tests want, and
/// `crates/e2e/playground` builds one that reads a client install as well.
///
/// The [`Running`] beside the address is what stops it, and it must be held —
/// see there.
///
/// # Why a thread and not a `tokio::spawn`
///
/// The shard owns a V8 isolate, so its future is not `Send` and cannot be
/// spawned onto a multi-threaded runtime — the binary does not spawn it either,
/// it awaits it in `main`. A thread with its own current-thread runtime is that
/// same arrangement, next door to the caller.
pub fn spawn(config: impl FnOnce(SocketAddr) -> Config + Send + 'static) -> (SocketAddrV4, Running) {
    let (ready, listening) = std::sync::mpsc::channel();

    // Built out here rather than inside the thread, because it is half of what
    // is handed back — and unlike a `Gate`, a `Shutdown` needs no runtime to
    // exist in.
    let shutdown = Shutdown::new();
    let served = shutdown.clone();

    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for the shard");

        runtime.block_on(async move {
            let (gateway, events) = ClientGatewayServer::bind("127.0.0.1:0".parse().unwrap(), served.clone())
                .await
                .expect("a loopback port");
            let address = gateway.local_address().expect("the bound address");

            // A local, not a leak. `run_shard` borrows the config for as long as
            // it runs, and this block is that long — the future is awaited below
            // inside the same `block_on` scope, so the borrow ends with the
            // shard. A fuzzing run that starts and drops fifty worlds leaks
            // nothing.
            let config = config(address);
            let config = &config;
            let world = openshard_server::boot::load_world(config).expect("a world");
            let store = openshard_server::boot::open_store(config).await.expect("a store");

            tokio::spawn(gateway.run());
            ready.send(address).expect("the caller is still waiting");
            // The reins carry a fresh tally nobody reads: what counts unwritten
            // saves is there for the force-exit of `docs/shutdown.md` D2, and a
            // test shard has no signals to force-exit on. `over` rather than
            // `new`, because the stop it is held by already exists — the caller
            // is holding a clone of it in the `Running` below.
            let reins = openshard_server::shard::Reins::over(served);
            openshard_server::shard::run_shard(events, config, world, store, reins, &[]).await;
        });
    });

    let address = match listening.recv().expect("the shard came up") {
        SocketAddr::V4(address) => address,
        SocketAddr::V6(_) => unreachable!("bound to a v4 loopback address"),
    };
    (
        address,
        Running {
            stop: shutdown,
            thread: Some(thread),
        },
    )
}

/// A shard on the stock config: no map, no database, two development accounts.
pub fn shard() -> (SocketAddrV4, Running) {
    spawn(stock_config)
}

/// Log in as the stock account and play its character.
pub fn plan() -> Plan {
    plan_for(ACCOUNT, CHARACTER)
}

/// Log in as `account` and play the character called `character`.
///
/// The password is [`PASSWORD`] whoever the account is — see it for why there is
/// no parameter for one.
pub fn plan_for(account: &str, character: &str) -> Plan {
    Plan {
        account: RawAccountName(account.to_owned()),
        password: RawPlaintextPassword(PASSWORD.to_owned()),
        shard: Pick::First,
        character: Pick::Named(character.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shard whose thread died has to fail the test that was using it, and the
    /// failure has to carry the shard's own panic message — an `eprintln!` here
    /// is a line in the output of a test that passed, which is a way to not
    /// notice for a long time.
    ///
    /// This is a unit test rather than one in `tests/` because [`Running`]'s
    /// fields are private: a thread that panics on purpose cannot be handed to
    /// one from outside the crate, and starting a real shard and killing it is a
    /// far less direct way to ask the same question.
    #[test]
    #[should_panic(expected = "the shard thread died on purpose")]
    fn a_shard_thread_that_panicked_fails_the_test() {
        let running = Running {
            stop: Shutdown::new(),
            thread: Some(std::thread::spawn(|| panic!("the shard thread died on purpose"))),
        };
        running.stop();
    }
}
