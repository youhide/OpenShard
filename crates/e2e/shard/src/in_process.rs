//! A shard with no listener, and a client that dials it without a socket.
//!
//! The other half of this crate starts a shard on an ephemeral port and talks to
//! it over the loopback, which is what a real client does and what the wire tests
//! must keep doing. This half removes the network entirely: the two ends are
//! joined by [`tokio::io::duplex`], a pair of in-memory pipes, and no port is
//! bound at all.
//!
//! # What is still real, and it is nearly everything
//!
//! The bytes. Both ends run the same code they run against a socket — the
//! client's framing, the login state machine, the relay's second connection,
//! per-write compression, and on the server side `client_session_serve` itself.
//! What is gone is the kernel: a `write` that would have gone to the loopback
//! lands in a buffer the other end reads. So a defect in framing or in the login
//! conversation fails here exactly as it fails over TCP, which is the property
//! that makes this worth having rather than a second implementation to keep in
//! step.
//!
//! What is *not* exercised: everything about a real network. Segment boundaries
//! land where the writes did, `Nagle` does not exist, the connection never
//! resets, and a slow reader blocks the writer rather than filling a kernel
//! queue. The socket tests beside this one are what cover that, and they stay.
//!
//! # Why it exists
//!
//! Two reasons, and only the first is here today. One command that starts both
//! ends of the game — `crates/e2e/playground`. And a world that can be driven
//! by something that is not a person: a virtual player, walking and talking and
//! being fuzzed at, wants a connection per player and no file descriptors, no
//! ports, and no timing that depends on a kernel.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use openshard_client_net::transport::Dial;
use openshard_config::Config;
use openshard_gateway::{Gate, Shutdown};
use tokio::io::DuplexStream;

use crate::Running;

/// The address the config is written with, and which nothing ever listens on.
///
/// A shard with no listener still has a `server.listen` and a `server.advertise`
/// — the config is the same type either way — and `advertise` is what goes out
/// in the `0x8C` relay. [`InProcess::game`] ignores what comes back, because the
/// only place to dial is here. Stating the stock port rather than a made-up one
/// keeps a log line that says `2593` from being a puzzle.
pub const NOMINAL: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 2593));

/// How much either end may run ahead of the other before a write waits.
///
/// A pipe rather than a queue: the reader on both sides drains continuously, so
/// this bounds a burst and not a backlog. 64 KiB is several times the largest
/// packet the protocol has (18,000 bytes), which is the number that matters —
/// anything smaller than one packet would deadlock a writer against a reader
/// waiting for the rest of it.
const PIPE: usize = 64 * 1024;

/// Start a shard that no socket can reach, and hand back the way in.
///
/// `config` is called with [`NOMINAL`], the address the shard *says* it is at.
/// Everything else is [`crate::spawn`]'s arrangement: a thread of its own,
/// because the shard's future holds a V8 isolate and is not `Send`, and a
/// current-thread runtime on it, because that is what the binary does too.
///
/// The returned [`InProcess`] is what a client is given in place of an address.
/// It can be cloned, and each clone opens its own connections — which is what a
/// second character, or a hundred virtual ones, will need.
///
/// The [`Running`] beside it stops the shard, and it must be held for as long as
/// the shard is wanted — see there. Stopping ends the thread and with it the
/// runtime the gate reads on, so an [`InProcess`] that outlives its `Running` is
/// a dialler with nothing behind it: it still dials, and every stream it hands
/// back is closed before the caller sees it. `Gate::serve` is what makes that
/// deliberate rather than a race between the stop and the runtime going away.
pub fn spawn(config: impl FnOnce(SocketAddr) -> Config + Send + 'static) -> (InProcess, Running) {
    let (ready, opened) = std::sync::mpsc::channel();

    // Outside the thread, because it is half of what is handed back. See
    // `crate::spawn`.
    let shutdown = Shutdown::new();
    let served = shutdown.clone();

    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for the shard");

        runtime.block_on(async move {
            // Built inside the runtime on purpose: a `Gate` captures the handle
            // it will read connections on, and the client dials from a thread of
            // its own. See `Gate::new`.
            //
            // The gate is given the same stop as the tick, which is what makes a
            // stop reach a connection here at all: nothing binds a port, so
            // there is no listener to close and the pipes are the whole of what
            // is open.
            let (gate, events) = Gate::new(served.clone());

            // A local, for the same reason as in `crate::spawn`: `run_shard`
            // borrows the config for as long as it runs, and that is this block.
            let config = config(NOMINAL);
            let config = &config;
            let world = openshard_server::boot::load_world(config).expect("a world");
            let store = openshard_server::boot::open_store(config).await.expect("a store");

            ready.send(gate).expect("the caller is still waiting");
            // The tally inside the reins goes unread, as in `crate::spawn` —
            // see there.
            let reins = openshard_server::shard::Reins::over(served);
            openshard_server::shard::run_shard(events, config, world, store, reins, &[]).await;
        });
    });

    let dial = InProcess {
        gate: opened.recv().expect("the shard came up"),
    };
    (
        dial,
        Running {
            stop: shutdown,
            thread: Some(thread),
        },
    )
}

/// A shard in this process, dialled through memory.
///
/// The counterpart of `openshard_client_net::transport::Tcp`, and the reason
/// that type exists: how a client reaches a shard is a choice, and the protocol
/// above it does not get one.
#[derive(Clone, Debug)]
pub struct InProcess {
    gate: Gate,
}

impl InProcess {
    /// A fresh pair of pipes, one end served by the shard.
    ///
    /// Both connections a login needs are this, which is why neither method
    /// below does anything else: the login socket and the game socket differ in
    /// what is *said* on them, and nothing about how they are opened.
    ///
    /// The id `Gate::serve` returns is dropped because nothing here has a use for
    /// it — the world learns it from the `Connected` event — and so is the `None`
    /// it returns once the shard is stopping: the gate has already dropped its end
    /// of the pipe, so the stream handed back is a closed one, and a caller that
    /// reads it finds out immediately. That is a better answer than an error,
    /// because it is the same answer a shard that hung up mid-session gives.
    fn open(&self) -> DuplexStream {
        let (client, server) = tokio::io::duplex(PIPE);
        let _served = self.gate.serve(server, NOMINAL);
        client
    }
}

impl Dial for InProcess {
    type Stream = DuplexStream;

    async fn login(&mut self) -> io::Result<DuplexStream> {
        Ok(self.open())
    }

    /// The relayed address is deliberately ignored: the shard advertised
    /// [`NOMINAL`], nothing is listening there, and the only shard this dialler
    /// can reach is the one it was built from. A client that honoured it would
    /// be dialling a port that does not exist.
    async fn game(&mut self, _address: SocketAddrV4) -> io::Result<DuplexStream> {
        Ok(self.open())
    }
}
