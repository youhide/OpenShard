//! The Tokio adapter.
//!
//! Everything interesting lives in [`Connection`]. This module only moves bytes
//! between a socket and that state machine, and it is kept small enough to read
//! in one sitting on purpose — code that cannot be unit tested should not be
//! where the thinking happens.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use openshard_protocol::version::ClientVersion;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::connection::{Connection, ConnectionError, Event};
use crate::shutdown::Shutdown;

/// Identifies a connection for the lifetime of the process.
///
/// Not an entity `Serial` and not an account: a client has one of
/// these before it has said anything at all.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// The raw value, for logging.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Name a connection that no gateway handed out.
    ///
    /// Only the accept loop should mint these in a running server — an id is
    /// meaningless unless a socket is behind it. But every crate downstream
    /// addresses clients by one, and their tests need to say "this connection"
    /// without standing up a listener.
    ///
    /// Not `Default`: there is no sensible default connection, and deriving one
    /// would let `..Default::default()` quietly address whatever `#0` turns out
    /// to be.
    pub const fn from_raw(id: u64) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Sender half of a connection's outbound-byte channel.
///
/// A newtype rather than the bare `UnboundedSender<Vec<u8>>` so that a
/// connection's send side cannot be passed where some other `Vec<u8>` channel
/// was meant — the two crates on either side of it (this one and the world
/// server that holds it in a `Session`) share only this name for it.
#[derive(Debug, Clone)]
pub struct OutboxTx(mpsc::UnboundedSender<Vec<u8>>);

impl OutboxTx {
    /// Queue bytes for the writer task. An error means the writer half is
    /// gone, which for a caller across the boundary means the socket already
    /// closed.
    pub fn send(&self, bytes: Vec<u8>) -> Result<(), mpsc::error::SendError<Vec<u8>>> {
        self.0.send(bytes)
    }
}

/// Receiver half of [`OutboxTx`], held by the writer task alone.
#[derive(Debug)]
pub struct OutboxRx(mpsc::UnboundedReceiver<Vec<u8>>);

impl OutboxRx {
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        self.0.recv().await
    }

    pub fn try_recv(&mut self) -> Result<Vec<u8>, mpsc::error::TryRecvError> {
        self.0.try_recv()
    }
}

/// A fresh outbox channel, for a new connection or a test double for one.
pub fn outbox_channel() -> (OutboxTx, OutboxRx) {
    let (tx, rx) = mpsc::unbounded_channel();
    (OutboxTx(tx), OutboxRx(rx))
}

/// Sender half of the channel that carries a connection's resolved
/// [`ClientVersion`] to the framer, once the server has it (a game connection
/// carries none of its own). Needed for the handful of client packets whose
/// length changed across eras — the drop packet, today. See
/// [`Connection::set_version`].
#[derive(Debug, Clone)]
pub struct VersionTx(mpsc::UnboundedSender<ClientVersion>);

impl VersionTx {
    pub fn send(&self, version: ClientVersion) -> Result<(), mpsc::error::SendError<ClientVersion>> {
        self.0.send(version)
    }
}

/// Receiver half of [`VersionTx`], read by the gateway's read loop.
#[derive(Debug)]
pub struct VersionRx(mpsc::UnboundedReceiver<ClientVersion>);

impl VersionRx {
    pub async fn recv(&mut self) -> Option<ClientVersion> {
        self.0.recv().await
    }
}

/// A fresh version channel, for a new connection or a test double for one.
pub fn version_channel() -> (VersionTx, VersionRx) {
    let (tx, rx) = mpsc::unbounded_channel();
    (VersionTx(tx), VersionRx(rx))
}

/// Something that happened on a connection, addressed.
#[derive(Debug)]
pub enum ServerEvent {
    /// A client connected. Nothing has been read yet.
    Connected {
        /// Who.
        id: ConnectionId,
        /// From where.
        address: SocketAddr,
        /// Send bytes back through this.
        outbox: OutboxTx,
        /// Tell the framer the client's version through this. See [`VersionTx`].
        control: VersionTx,
    },
    /// The connection produced something.
    Received {
        /// Who.
        id: ConnectionId,
        /// What.
        event: Event,
    },
    /// The connection is gone. No further events will carry this id.
    Disconnected {
        /// Who.
        id: ConnectionId,
        /// Why, or `None` for a clean close.
        reason: Option<String>,
    },
}

/// Sender half of the gateway's event channel — one clone per connection
/// task, all feeding the single [`ServerEventRx`] the world server drains.
///
/// Never leaves this crate: only [`ServerEventRx`] crosses to the world
/// server, so this stays private rather than adding API surface nothing uses.
#[derive(Debug, Clone)]
struct ServerEventTx(mpsc::UnboundedSender<ServerEvent>);

impl ServerEventTx {
    fn send(&self, event: ServerEvent) -> Result<(), mpsc::error::SendError<ServerEvent>> {
        self.0.send(event)
    }
}

/// Receiver half of [`ServerEventTx`], handed to the world server by
/// [`ClientGatewayServer::bind`].
#[derive(Debug)]
pub struct ServerEventRx(mpsc::UnboundedReceiver<ServerEvent>);

impl ServerEventRx {
    pub async fn recv(&mut self) -> Option<ServerEvent> {
        self.0.recv().await
    }
}

fn server_event_channel() -> (ServerEventTx, ServerEventRx) {
    let (tx, rx) = mpsc::unbounded_channel();
    (ServerEventTx(tx), ServerEventRx(rx))
}

/// Hands out monotonically increasing [`ConnectionId`]s.
///
/// Wraps the `Arc<AtomicU64>` counter so the only way to touch it is
/// [`SessionIdFabric::next`] — nothing can bump the counter without minting an
/// id, or read the counter without going through a `ConnectionId`. Cloning
/// shares the same underlying counter, which is what lets [`ClientGatewayServer::run`]
/// hand one to every connection task.
#[derive(Clone, Debug)]
struct SessionIdFabric(Arc<AtomicU64>);

impl SessionIdFabric {
    /// Start a fresh sequence at `1`.
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(1)))
    }

    /// Mint the next id in the sequence.
    fn next(&self) -> ConnectionId {
        ConnectionId(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

/// The door a connection comes through, whoever opened it.
///
/// A [`ClientGatewayServer`] is this plus a listener, and the split is what lets
/// a connection arrive from somewhere other than a socket: a test, or a client
/// in this same process. What a `Gate` needs is an id to mint, somewhere to send
/// the events, and a runtime to read on — nothing about *where* the bytes came
/// from, which is the whole point.
///
/// # The runtime is captured, not borrowed from the caller
///
/// [`Gate::serve`] may be called from another runtime entirely — an in-process
/// client dials from its own thread — and the connection must be read by the
/// shard's, because that is the one that outlives the caller and the one whose
/// events the world drains. So the handle is taken when the gate is built, which
/// means [`Gate::new`] must be called inside the runtime that will serve.
#[derive(Clone, Debug)]
pub struct Gate {
    events: ServerEventTx,
    session_ids: SessionIdFabric,
    /// Where connection tasks are spawned. See the note above.
    reader: tokio::runtime::Handle,
    /// Handed to every connection this gate serves, so that a stop reaches a
    /// socket nobody is speaking on. See [`Shutdown`].
    shutdown: Shutdown,
}

impl Gate {
    /// A gate with nothing in front of it, and the channel its events arrive on.
    ///
    /// `shutdown` is the shard's, not one made here: a gate that owned its own
    /// stop would be a second thing to remember to stop, and the whole point of
    /// the type is that there is one.
    ///
    /// # Panics
    ///
    /// If called outside a Tokio runtime: a gate with no runtime to read on
    /// could accept a connection and never poll it, which is a hang rather than
    /// an error — better said here, at the one line that can say it.
    pub fn new(shutdown: Shutdown) -> (Self, ServerEventRx) {
        let (events, receiver) = server_event_channel();
        (
            Self {
                events,
                session_ids: SessionIdFabric::new(),
                reader: tokio::runtime::Handle::current(),
                shutdown,
            },
            receiver,
        )
    }

    /// Serve `stream` as a client that arrived from `address`.
    ///
    /// Returns immediately with the id the connection will be known by; the
    /// reading happens on the gate's own runtime. `address` is what the world is
    /// told the client's address is — a real peer for an accepted socket, and a
    /// stated one for anything else.
    ///
    /// # A gate that has been asked to stop serves nobody
    ///
    /// `None` means the shard is stopping and this stream was not served: it is
    /// dropped here, so the caller's end closes rather than waiting. A gate
    /// outlives the loop that feeds it — `ClientGatewayServer::run` returns on the
    /// stop but a cloned `Gate` in an in-process dialler does not — so this is
    /// reachable in two ways, and both end badly without it.
    ///
    /// While the shard's runtime is still alive, spawning here would hand a client
    /// a login conversation whose events go onto a channel the tick has stopped
    /// draining: a session that appears to connect and then answers nothing.
    ///
    /// Once that runtime is *gone*, `Handle::spawn` is not a panic and not a hang
    /// — checked, not assumed: the future is dropped without ever being polled and
    /// the `JoinHandle` resolves to `JoinError::Cancelled`. Which happens to close
    /// the stream too, by dropping the task that owns it, but silently and by
    /// accident. Saying it here makes the same outcome deliberate and gives the
    /// caller an answer it can read.
    pub fn serve<S>(&self, stream: S, address: SocketAddr) -> Option<ConnectionId>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        if self.shutdown.is_stopping() {
            debug!(%address, "a connection arrived at a gate that is stopping; not serving it");
            return None;
        }

        let id = self.session_ids.next();
        let events = self.events.clone();
        let shutdown = self.shutdown.clone();
        self.reader.spawn(async move {
            // A panic in here takes this connection down and nothing else.
            // That is why the release profile does not set panic = "abort".
            if let Err(error) = client_session_serve(id, address, stream, events, shutdown).await {
                debug!(%id, %error, "connection ended");
            }
        });
        Some(id)
    }
}

/// Accepts connections and drives a [`Connection`] for each.
///
/// Events go onto a channel rather than through a callback: the world server
/// consumes them on its own tick, and a callback would run world code inside a
/// network task on an arbitrary thread. The channel is the boundary between
/// "async everywhere" and "the deterministic simulation".
#[derive(Debug)]
pub struct ClientGatewayServer {
    listener: TcpListener,
    gate: Gate,
}

impl ClientGatewayServer {
    /// Bind to `address`.
    ///
    /// Returns the server and the channel its events arrive on. `shutdown` is
    /// what ends [`run`](Self::run) and every connection it accepts — see
    /// [`Shutdown`].
    pub async fn bind(address: SocketAddr, shutdown: Shutdown) -> io::Result<(Self, ServerEventRx)> {
        let listener = TcpListener::bind(address).await?;
        let (gate, receiver) = Gate::new(shutdown);
        Ok((Self { listener, gate }, receiver))
    }

    /// The address actually bound, which matters when port 0 was requested.
    pub fn local_address(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// The door this listener feeds, for a caller with a connection of its own.
    ///
    /// Cloned rather than borrowed because it outlives [`run`](Self::run), which
    /// takes `self`: a shard that serves both a port and an in-process client
    /// hands the gate to the second and the whole server to the first.
    pub fn gate(&self) -> Gate {
        self.gate.clone()
    }

    /// Accept until the shard stops, spawning a task per connection.
    ///
    /// Returns `Ok(())` on a stop and an error if accepting itself fails, which
    /// means the listener is gone. Returning drops the listener, so the port is
    /// free the moment this ends — a client that dials afterwards is refused
    /// rather than accepted into a world that is saving.
    pub async fn run(self) -> io::Result<()> {
        info!(address = ?self.local_address()?, "gateway listening");
        loop {
            let accepted = tokio::select! {
                // Biased towards the stop: a connection that arrives in the same
                // moment is one nobody is left to serve, and accepting it would
                // hand a client a session on a world that has already begun its
                // last save.
                biased;

                () = self.gate.shutdown.requested() => {
                    info!("gateway stopping; not accepting any more connections");
                    return Ok(());
                }

                accepted = self.listener.accept() => accepted?,
            };
            let (stream, address) = accepted;
            // Nagle batches small writes, and nearly everything a UO server
            // sends is a small write that the client is waiting on. Latency
            // beats packet count. Here rather than in `serve`, which is the one
            // place a `TcpStream` is still a `TcpStream`.
            stream.set_nodelay(true)?;
            if self.gate.serve(stream, address).is_none() {
                // The stop landed between the select above and this line. The
                // select is biased, so it wins every race it can see; this is the
                // one it cannot, and it means the same thing.
                info!("gateway stopping; not accepting any more connections");
                return Ok(());
            }
        }
    }
}

/// How long a stopping connection waits for what is already queued to reach the
/// wire before it hangs up regardless.
///
/// A constant and not a setting: it is a bound on the shard's own teardown, not
/// on anything an operator tunes, and a number nobody can vary is a number, not a
/// configuration. Two seconds is generous for a handful of queued packets on a
/// live socket and short enough that a shard whose tick has wedged still stops
/// while somebody is watching.
const DRAIN_ON_STOP: std::time::Duration = std::time::Duration::from_secs(2);

/// Which side ended a connection and therefore whether queued output is still
/// entitled to a bounded drain.
enum SessionEnding {
    Reader(Option<String>),
    Writer,
    Shutdown,
}

impl SessionEnding {
    fn reason(self) -> Option<String> {
        match self {
            Self::Reader(reason) => reason,
            Self::Writer | Self::Shutdown => None,
        }
    }
}

/// Empty one connection's outbox into its write half, then close that half.
///
/// The shutdown is explicit rather than left to a drop. An `OwnedWriteHalf`
/// closes its direction when dropped and `tokio::io::split`'s half does not, so
/// a generic stream otherwise behaves differently from a socket at teardown.
async fn write_loop<W: AsyncWrite + Unpin>(mut writer: W, mut outbox: OutboxRx) {
    while let Some(bytes) = outbox.recv().await {
        if writer.write_all(&bytes).await.is_err() {
            break;
        }
    }
    let _ = writer.shutdown().await;
}

/// Finish the write half according to why the session ended.
///
/// A shard stop is the only ending where already queued output is worth waiting
/// for: the shutdown notice is a decision the world made while it was still the
/// authority. The wait is bounded so a wedged world cannot prevent shutdown.
async fn finish_writes(id: ConnectionId, writes: &mut tokio::task::JoinHandle<()>, ending: &SessionEnding) {
    if matches!(ending, SessionEnding::Shutdown)
        && tokio::time::timeout(DRAIN_ON_STOP, &mut *writes).await.is_err()
    {
        warn!(%id, ?DRAIN_ON_STOP, "the outbox did not drain before the deadline; hanging up anyway");
    }
    // The reader ended while the writer was waiting, the writer already ended,
    // or the bounded drain finished. Abort is harmless for a completed task and
    // makes every unfinished case close immediately.
    writes.abort();
}

/// Drive one connection until it closes.
///
/// Generic over the stream, and not for the sake of a test double: an
/// in-process client is a `DuplexStream` and a fuzzing one is whatever it likes,
/// and every one of them must go through this same function or it is not the
/// gateway that is being exercised.
async fn client_session_serve<S>(
    id: ConnectionId,
    address: SocketAddr,
    stream: S,
    events: ServerEventTx,
    shutdown: Shutdown,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut client_tcp_reader, client_tcp_writer) = tokio::io::split(stream);
    let (to_client_tx, to_client_rx) = outbox_channel();
    let (control_tx, control_rx) = version_channel();

    if events
        .send(ServerEvent::Connected {
            id,
            address,
            outbox: to_client_tx,
            control: control_tx,
        })
        .is_err()
    {
        return Ok(()); // The world server is gone; nothing to serve.
    }

    let mut writes = tokio::spawn(write_loop(client_tcp_writer, to_client_rx));

    // client -> server loop, raced against the write half ending.
    //
    // # Why the race, and what it cost to leave out
    //
    // Dropping the outbox shuts down the *write* half — that is `OwnedWriteHalf`'s
    // `Drop`, and it is enough for the client to read zero bytes and know it has
    // been hung up on. It is not enough for this end: a socket whose peer has not
    // closed its own half is still open, so `read_loop` went on awaiting a read
    // that would never come, and no `Disconnected` was ever emitted.
    //
    // Which made the server's own hang-up depend on the client answering it. A
    // real client closes, so the chain completed and everything looked right. One
    // that does not — a hung process, a dropped route, anything holding the socket
    // half-open — left the world holding the character of a connection the shard
    // had already forgotten: visible to everyone, standing there, undeletable,
    // for as long as the socket lingered. Found by
    // `crates/e2e/shard/tests/refused_teardown.rs`, which walks the whole chain
    // and is where its six links are written down.
    //
    // The third arm is the shard stopping. Without it a connection outlives the
    // tick that was serving it: the world has saved and gone, and this task is
    // still reading a socket to queue events onto a channel nobody drains. The
    // client is hung up on instead, which is what it would see from a process
    // that had exited — and it sees it while the shard is still saving rather
    // than at whatever later moment the runtime happened to be torn down.
    //
    // `None` is the right reason for writer/shutdown endings: this end decided,
    // and that is a clean close.
    let ending = tokio::select! {
        reason = read_loop(id, &mut client_tcp_reader, &events, control_rx) => SessionEnding::Reader(reason),
        _ = &mut writes => SessionEnding::Writer,
        () = shutdown.requested() => {
            debug!(%id, "the shard is stopping; draining the outbox, then hanging up");
            SessionEnding::Shutdown
        }
    };
    finish_writes(id, &mut writes, &ending).await;
    let _ = events.send(ServerEvent::Disconnected {
        id,
        reason: ending.reason(),
    });
    Ok(())
}

/// Read until the socket closes or the client breaks the protocol.
///
/// Also listens on `control` for the client version the server resolves out of
/// band (a game connection sends none itself), so the framer can size the packets
/// whose length changed across eras. The two are raced: a version and a read are
/// both things that can happen next, and neither may block the other.
async fn read_loop<R: AsyncRead + Unpin>(
    id: ConnectionId,
    reader: &mut R,
    events: &ServerEventTx,
    mut control: VersionRx,
) -> Option<String> {
    let mut connection = Connection::new();
    let mut buffer = [0u8; 4096];
    // Once the server drops the control end, stop selecting on it: a closed
    // channel's `recv` is ready instantly and forever, which would spin the loop
    // and never read the socket. The guard disables that branch for good.
    let mut control_open = true;

    loop {
        let count = tokio::select! {
            // A version update only sets state; it yields no event, so loop back
            // to keep waiting on the socket.
            version = control.recv(), if control_open => {
                match version {
                    Some(version) => connection.set_version(version),
                    None => control_open = false,
                }
                continue;
            }
            read = reader.read(&mut buffer) => match read {
                Ok(0) => return None, // clean close
                Ok(count) => count,
                Err(error) => return Some(error.to_string()),
            },
        };
        connection.receive(&buffer[..count]);

        // Drain every event this read produced. Stopping at the first would
        // strand the rest until more bytes happened to arrive.
        loop {
            match connection.poll() {
                Ok(Some(event)) => {
                    if events.send(ServerEvent::Received { id, event }).is_err() {
                        return None;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    // Every ConnectionError is fatal: a UO stream has no frame
                    // markers, so there is nothing to resynchronise to.
                    warn!(%id, %error, "protocol violation, dropping");
                    return Some(error.to_string());
                }
            }
        }
    }
}

/// Convenience for callers that only have a `ConnectionError`.
impl From<ConnectionError> for io::Error {
    fn from(error: ConnectionError) -> Self {
        Self::new(io::ErrorKind::InvalidData, error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use openshard_protocol::seed::SEED_COMMAND;
    use tokio::net::TcpStream;

    use super::*;

    fn modern_seed() -> Vec<u8> {
        let mut bytes = vec![SEED_COMMAND];
        bytes.extend_from_slice(&0x0A00_0001u32.to_be_bytes());
        for field in [7u32, 0, 45, 65] {
            bytes.extend_from_slice(&field.to_be_bytes());
        }
        bytes
    }

    /// Bind to an ephemeral port and start accepting.
    ///
    /// The stop is a parameter rather than made here because most of these tests
    /// end by dropping the runtime and only two are about stopping on purpose —
    /// and those two need the handle the server was built with.
    async fn start(shutdown: Shutdown) -> (SocketAddr, ServerEventRx) {
        let (server, events) = ClientGatewayServer::bind("127.0.0.1:0".parse().unwrap(), shutdown)
            .await
            .unwrap();
        let address = server.local_address().unwrap();
        tokio::spawn(server.run());
        (address, events)
    }

    #[tokio::test]
    async fn the_writer_drains_the_outbox_in_order_and_closes_its_half() {
        let (server, mut client) = tokio::io::duplex(64);
        let (outbox, receiver) = outbox_channel();
        let writer = tokio::spawn(write_loop(server, receiver));

        outbox.send(vec![1, 2, 3]).unwrap();
        outbox.send(vec![4, 5]).unwrap();
        drop(outbox);

        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut received))
            .await
            .expect("the writer closed its half")
            .expect("the in-memory stream read");
        writer.await.expect("the writer task finished");
        assert_eq!(received, [1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn a_client_can_connect_and_be_heard() {
        let (address, mut events) = start(Shutdown::new()).await;
        let mut client = TcpStream::connect(address).await.unwrap();

        // The outbox is held for as long as the connection is meant to live.
        // Dropping it is how a server hangs up — see
        // [`dropping_the_outbox_ends_the_connection_without_the_client_answering`]
        // — so a test double that let it go would be closing the socket it is
        // about to read from.
        let ServerEvent::Connected { id, outbox, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected first");
        };
        let _outbox = outbox;

        let mut stream = modern_seed();
        stream.extend_from_slice(&[0x73, 0x00]);
        client.write_all(&stream).await.unwrap();

        let ServerEvent::Received { event, id: got } = events.recv().await.unwrap() else {
            panic!("expected the seed");
        };
        assert_eq!(got, id);
        assert!(matches!(event, Event::Seeded(_)));

        let ServerEvent::Received { event, .. } = events.recv().await.unwrap() else {
            panic!("expected the ping");
        };
        assert!(matches!(event, Event::Packet(_)));
    }

    #[tokio::test]
    async fn the_server_can_write_back() {
        let (address, mut events) = start(Shutdown::new()).await;
        let mut client = TcpStream::connect(address).await.unwrap();

        let ServerEvent::Connected { outbox, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected");
        };

        outbox.send(vec![0x82, 0x03]).unwrap(); // login denied
        let mut received = [0u8; 2];
        client.read_exact(&mut received).await.unwrap();
        assert_eq!(received, [0x82, 0x03]);
    }

    #[tokio::test]
    async fn a_clean_close_reports_no_reason() {
        let (address, mut events) = start(Shutdown::new()).await;
        let client = TcpStream::connect(address).await.unwrap();
        // Held, so that the close under test is unambiguously the client's:
        // dropping this would hang up from the other end and the assertion below
        // would hold for the wrong reason.
        let ServerEvent::Connected { outbox, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected");
        };
        drop(client);

        let ServerEvent::Disconnected { reason, .. } = events.recv().await.unwrap() else {
            panic!("expected Disconnected");
        };
        assert_eq!(reason, None, "hanging up is not an error");
        drop(outbox);
    }

    #[tokio::test]
    async fn dropping_the_outbox_ends_the_connection_without_the_client_answering() {
        // The server's own hang-up, and the half of it that used to be missing.
        // Dropping the outbox shuts the write half, so the client reads zero
        // bytes and knows — but this end went on awaiting a read from a socket
        // the client had every right to keep open, and never said `Disconnected`.
        // Which meant the shard's teardown chain stopped one link short and the
        // world kept the character of a connection nobody was holding any more.
        //
        // So the client here deliberately does *not* close: it reads its zero
        // bytes and keeps its own half open, which is exactly the case a
        // well-behaved client hid.
        let (address, mut events) = start(Shutdown::new()).await;
        let mut client = TcpStream::connect(address).await.unwrap();
        let ServerEvent::Connected { outbox, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected");
        };

        outbox.send(vec![0x82, 0x03]).unwrap(); // login denied, then the close
        drop(outbox);

        let mut denied = [0u8; 2];
        client.read_exact(&mut denied).await.unwrap();
        assert_eq!(denied, [0x82, 0x03], "the last packet lands before the close");
        let mut trailing = Vec::new();
        client.read_to_end(&mut trailing).await.unwrap();
        assert!(trailing.is_empty(), "and the write half is shut");

        let ServerEvent::Disconnected { reason, .. } = events.recv().await.unwrap() else {
            panic!("the gateway never said the connection was gone");
        };
        assert_eq!(reason, None, "this end decided; that is a clean close");
    }

    #[tokio::test]
    async fn a_protocol_violation_drops_the_connection() {
        let (address, mut events) = start(Shutdown::new()).await;
        let mut client = TcpStream::connect(address).await.unwrap();
        // Held: the reason under test is the protocol violation, and a dropped
        // outbox would close the connection first and report none.
        let ServerEvent::Connected { outbox, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected");
        };
        let _outbox = outbox;

        let mut stream = modern_seed();
        stream.extend_from_slice(&[0x01]); // no such client packet
        client.write_all(&stream).await.unwrap();

        // Seed, then the drop.
        assert!(matches!(
            events.recv().await.unwrap(),
            ServerEvent::Received {
                event: Event::Seeded(_),
                ..
            }
        ));
        let ServerEvent::Disconnected { reason, .. } = events.recv().await.unwrap() else {
            panic!("expected Disconnected");
        };
        assert!(reason.unwrap().contains("unknown packet"));
    }

    /// A stream that never went near a socket is served the same way.
    ///
    /// The point of the generic: an in-process client and, later, a fuzzing one
    /// go through `client_session_serve` rather than around it, so what they
    /// exercise is the gateway and not a second implementation of it.
    #[tokio::test]
    async fn a_gate_serves_a_stream_that_is_not_a_socket() {
        let (gate, mut events) = Gate::new(Shutdown::new());
        let (mut client, server) = tokio::io::duplex(4096);
        let served = gate
            .serve(server, "127.0.0.1:0".parse().unwrap())
            .expect("nothing has asked this gate to stop");

        let ServerEvent::Connected { id, outbox, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected first");
        };
        assert_eq!(id, served, "the id `serve` returned is the one the world hears");

        let mut stream = modern_seed();
        stream.extend_from_slice(&[0x73, 0x00]);
        client.write_all(&stream).await.unwrap();

        let ServerEvent::Received { event, .. } = events.recv().await.unwrap() else {
            panic!("expected the seed");
        };
        assert!(matches!(event, Event::Seeded(_)));

        outbox.send(vec![0x82, 0x03]).unwrap();
        let mut received = [0u8; 2];
        client.read_exact(&mut received).await.unwrap();
        assert_eq!(received, [0x82, 0x03], "and the answer comes back");
    }

    /// The hang-up reaches a client that is not a socket either.
    ///
    /// This is the assertion the explicit `shutdown` in the write loop exists
    /// for: an `OwnedWriteHalf` closes its direction when dropped and a split
    /// half does not, so without that line the zero read below never arrives and
    /// an in-process client waits for a server that has already gone.
    #[tokio::test]
    async fn dropping_the_outbox_ends_a_stream_connection_too() {
        let (gate, mut events) = Gate::new(Shutdown::new());
        let (mut client, server) = tokio::io::duplex(4096);
        gate.serve(server, "127.0.0.1:0".parse().unwrap())
            .expect("nothing has asked this gate to stop yet");

        let ServerEvent::Connected { outbox, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected");
        };
        outbox.send(vec![0x82, 0x03]).unwrap();
        drop(outbox);

        let mut denied = [0u8; 2];
        client.read_exact(&mut denied).await.unwrap();
        assert_eq!(denied, [0x82, 0x03], "the last packet lands before the close");
        let mut trailing = Vec::new();
        client.read_to_end(&mut trailing).await.unwrap();
        assert!(trailing.is_empty(), "and the write half is shut");

        let ServerEvent::Disconnected { reason, .. } = events.recv().await.unwrap() else {
            panic!("the gateway never said the connection was gone");
        };
        assert_eq!(reason, None, "this end decided; that is a clean close");
    }

    /// What the world said on its way out reaches the client before the hang-up.
    ///
    /// The order below is the order a shutdown actually happens in, and it is why
    /// aborting the write task was wrong: the world *hears* the stop first and
    /// only then says its last word, so everything worth delivering is queued
    /// after `stop()` — on the losing side of a race the connection task used to
    /// win. Without the drain the client reads zero bytes here, which is
    /// indistinguishable from the shard having crashed.
    #[tokio::test]
    async fn a_stop_drains_what_the_world_queued_before_hanging_up() {
        let shutdown = Shutdown::new();
        let (gate, mut events) = Gate::new(shutdown.clone());
        let (mut client, server) = tokio::io::duplex(4096);
        gate.serve(server, "127.0.0.1:0".parse().unwrap())
            .expect("nothing has asked this gate to stop yet");

        let ServerEvent::Connected { outbox, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected");
        };

        shutdown.stop();
        outbox.send(vec![0x82, 0x03]).unwrap();
        // The tick letting go of its sessions, which is what ends the write task
        // and what the drain is waiting for.
        drop(outbox);

        let mut parting = [0u8; 2];
        tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut parting))
            .await
            .expect("the queued bytes were not dropped on the way out")
            .unwrap();
        assert_eq!(parting, [0x82, 0x03]);

        let mut trailing = Vec::new();
        client.read_to_end(&mut trailing).await.unwrap();
        assert!(trailing.is_empty(), "and then the hang-up, in that order");
    }

    /// A gate outlives the loop that feeds it, so it has to refuse on its own.
    ///
    /// `ClientGatewayServer::run` returns on the stop and its listener goes with
    /// it, which covers the socket. A cloned `Gate` covers nothing: an in-process
    /// dialler holds one, and dialling after the stop used to spawn a login
    /// conversation onto a runtime that was either about to die or already gone.
    ///
    /// Two things are asserted, and the second is the one that matters to the
    /// caller: no id was minted, so nothing thinks a session exists; and the
    /// stream this end kept is closed rather than silently unread.
    #[tokio::test]
    async fn a_gate_that_is_stopping_serves_nobody() {
        let shutdown = Shutdown::new();
        let (gate, mut events) = Gate::new(shutdown.clone());
        shutdown.stop();

        let (mut client, server) = tokio::io::duplex(4096);
        assert!(
            gate.serve(server, "127.0.0.1:0".parse().unwrap()).is_none(),
            "a stopping gate handed out a connection id"
        );

        let mut trailing = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut trailing))
            .await
            .expect("the caller was left holding a pipe nobody closed")
            .unwrap();
        assert!(trailing.is_empty(), "and nothing was said on it");

        // Dropping the gate drops the last sender, so this ends rather than
        // pending — which is how the channel can be checked to be empty without
        // waiting on a clock for something that will never arrive.
        drop(gate);
        assert!(
            events.recv().await.is_none(),
            "the world was told about a connection that was never served"
        );
    }

    #[tokio::test]
    async fn connections_get_distinct_ids() {
        let (address, mut events) = start(Shutdown::new()).await;
        let _a = TcpStream::connect(address).await.unwrap();
        let _b = TcpStream::connect(address).await.unwrap();

        // Until two ids, not for two events: anything else the gateway says in
        // between — and it does say other things — used to be consumed by this
        // `if let` and dropped, leaving one id and an index panic in a test that
        // is about neither. Failed roughly one run in three.
        let mut ids = Vec::new();
        while ids.len() < 2 {
            if let ServerEvent::Connected { id, .. } = events.recv().await.unwrap() {
                ids.push(id);
            }
        }
        assert_ne!(ids[0], ids[1], "two connections must not share an id");
    }

    #[tokio::test]
    async fn a_stop_ends_the_accept_loop() {
        // `run` used to have exactly one way out: an accept that failed. So the
        // only way to stop a shard was to end the process, and a test that
        // wanted a second world had to leak the first.
        let shutdown = Shutdown::new();
        let (server, _events) = ClientGatewayServer::bind("127.0.0.1:0".parse().unwrap(), shutdown.clone())
            .await
            .unwrap();
        let accepting = tokio::spawn(server.run());

        shutdown.stop();

        tokio::time::timeout(Duration::from_secs(5), accepting)
            .await
            .expect("the accept loop noticed the stop")
            .expect("it did not panic")
            .expect("and a stop is not an accept failure");
    }

    #[tokio::test]
    async fn a_stop_hangs_up_on_a_client_that_is_already_connected() {
        // The half that is easy to leave out. Stopping the listener stops new
        // clients; the ones already in are held by a task of their own, and
        // without the stop reaching *those* the shard saves and goes while a
        // socket is still being read for a world nobody is ticking.
        //
        // The client here does not close: it is the well-behaved-client case
        // that hid the same gap in `dropping_the_outbox_...` above.
        //
        // The outbox being held for the whole of it is also the deadline case of
        // `DRAIN_ON_STOP`: nothing is queued, but the write task cannot end while
        // a sender is alive, so this connection hangs up rudely two seconds later
        // rather than as soon as it hears the stop. That is the design — see the
        // drain in `client_session_serve` — and it is why the timeout below is
        // comfortably longer than the drain.
        let shutdown = Shutdown::new();
        let (address, mut events) = start(shutdown.clone()).await;
        let mut client = TcpStream::connect(address).await.unwrap();
        // Held, exactly as the world server holds one: were it dropped, the
        // hang-up under test would be the outbox's rather than the stop's.
        let ServerEvent::Connected { outbox, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected");
        };
        let _outbox = outbox;

        shutdown.stop();

        let mut trailing = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut trailing))
            .await
            .expect("the client was hung up on")
            .unwrap();
        assert!(trailing.is_empty(), "nothing was sent; the socket was closed");

        let ServerEvent::Disconnected { reason, .. } = events.recv().await.unwrap() else {
            panic!("the gateway never said the connection was gone");
        };
        assert_eq!(reason, None, "this end decided; that is a clean close");
    }
}
