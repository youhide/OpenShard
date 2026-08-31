//! The part that touches a stream, and decides nothing.
//!
//! Everything that can be got wrong about the protocol is in
//! [`connection`](crate::connection) and [`session`](crate::session), where it
//! can be tested without a port. What is left here is reading, writing, and the
//! one thing a state machine cannot do for itself: open the second connection
//! the relay names.
//!
//! # Why the stream is a parameter
//!
//! A UO client dials a shard over TCP, and that is [`Tcp`]. It is not the only
//! thing that will ever drive this crate: a shard in the same process has no
//! reason to go out to the loopback and back, and a virtual player pushing
//! states at a world for a fuzzing run has no reason to have a socket at all.
//! Both want *this* login machine and *this* framing rather than a second copy
//! that agrees with them, so the transport is a type parameter and the protocol
//! is not.

use std::io;
use std::net::SocketAddrV4;

use openshard_protocol::login::DenyReason;
use openshard_protocol::version::ClientVersion;
use tokio::io::{
    AsyncRead,
    AsyncReadExt,
    AsyncWrite,
    AsyncWriteExt,
};
use tokio::net::TcpStream;
use tracing::debug;

use crate::connection::{
    Connection,
    ConnectionError,
    Event,
    Stream,
};
use crate::session::{
    Login,
    LoginError,
    Plan,
    Stage,
    Step,
};
use crate::view::WorldView;

/// How big a read is. A packet is at most 18,000 bytes and usually a few dozen;
/// this is a buffer size, not a bound on anything.
const READ_SIZE: usize = 4096;

/// Talking to the shard failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum TransportError {
    /// The socket failed.
    Io(std::io::Error),
    /// The stream could not be read as packets.
    Connection(ConnectionError),
    /// The login conversation went somewhere it cannot continue from.
    Login(LoginError),
    /// The server refused the login and said why.
    Refused(DenyReason),
    /// The far end hung up mid-conversation.
    ///
    /// Its own variant because it is the *normal* way a refusal is felt: the
    /// server sends `0x82` and closes, and a client that only reported "read
    /// zero bytes" would lose the reason it was handed a moment earlier.
    Closed {
        /// How far the login had got.
        stage: Stage,
    },
}

impl From<std::io::Error> for TransportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ConnectionError> for TransportError {
    fn from(error: ConnectionError) -> Self {
        Self::Connection(error)
    }
}

impl From<LoginError> for TransportError {
    fn from(error: LoginError) -> Self {
        Self::Login(error)
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Connection(error) => error.fmt(f),
            Self::Login(error) => error.fmt(f),
            Self::Refused(reason) => write!(f, "the shard refused the login: {reason:?}"),
            Self::Closed { stage } => write!(f, "the shard hung up while {stage:?}"),
        }
    }
}

impl std::error::Error for TransportError {
}

/// How a client opens its connections to a shard.
///
/// Two per login, and they are not the same question: the first goes where the
/// player said, the second goes where the *server* said in its `0x8C` relay.
/// Splitting them is what lets an implementation ignore the relayed address —
/// an in-process shard hands out an address it never listens on — without
/// having to guess which of two calls it was looking at.
///
/// The implementations that matter: [`Tcp`], and whatever `crates/e2e` needs.
pub trait Dial {
    /// What comes back, and what [`Socket`] then reads.
    type Stream: AsyncRead + AsyncWrite + Unpin;

    /// The login connection, to wherever this dialler was pointed.
    fn login(&mut self) -> impl std::future::Future<Output = io::Result<Self::Stream>> + Send;

    /// The game connection, to the address the relay named.
    fn game(
        &mut self,
        address: SocketAddrV4,
    ) -> impl std::future::Future<Output = io::Result<Self::Stream>> + Send;
}

/// A real client on a real network: TCP, to the address it was given.
#[derive(Clone, Copy, Debug)]
pub struct Tcp {
    /// Where the login server is. The game server's address comes off the wire.
    login: SocketAddrV4,
}

impl Tcp {
    /// Dial `address` for the login, and whatever it relays us to for the game.
    #[must_use]
    pub fn at(address: SocketAddrV4) -> Self {
        Self { login: address }
    }
}

impl Dial for Tcp {
    type Stream = TcpStream;

    async fn login(&mut self) -> io::Result<TcpStream> {
        TcpStream::connect(self.login).await
    }

    async fn game(&mut self, address: SocketAddrV4) -> io::Result<TcpStream> {
        TcpStream::connect(address).await
    }
}

/// One connection, with the parser that reads it.
#[derive(Debug)]
pub struct Socket<S> {
    stream:     S,
    connection: Connection,
    /// Events already parsed out of the last read, oldest first.
    ///
    /// A single read routinely carries several packets. Holding them here is
    /// what lets [`Socket::next_event`] be called one packet at a time without
    /// losing the rest.
    pending:    std::collections::VecDeque<Event>,
}

impl Socket<TcpStream> {
    /// Connect over TCP, and read the stream as `kind`.
    pub async fn connect(
        address: SocketAddrV4,
        kind: Stream,
        version: ClientVersion,
    ) -> Result<Self, TransportError> {
        Ok(Self::new(TcpStream::connect(address).await?, kind, version))
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Socket<S> {
    /// Read an already-open stream as `kind`.
    pub fn new(stream: S, kind: Stream, version: ClientVersion) -> Self {
        Self {
            stream,
            connection: Connection::new(kind, version),
            pending: std::collections::VecDeque::new(),
        }
    }

    /// Write bytes as they are. Framing already happened.
    pub async fn send(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.stream.write_all(bytes).await?;
        Ok(())
    }

    /// The next packet, reading the socket as often as it takes.
    ///
    /// `Ok(None)` means the far end closed cleanly with nothing pending.
    pub async fn next_event(&mut self) -> Result<Option<Event>, TransportError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            while let Some(event) = self.connection.poll()? {
                self.pending.push_back(event);
            }
            if !self.pending.is_empty() {
                continue;
            }

            let mut buffer = [0u8; READ_SIZE];
            let read = self.stream.read(&mut buffer).await?;
            if read == 0 {
                return Ok(None);
            }
            self.connection.receive(&buffer[..read]);
        }
    }
}

/// Log in over TCP and walk into the world. See [`enter_world_with`].
pub async fn enter_world(
    address: SocketAddrV4,
    plan: Plan,
    version: ClientVersion,
) -> Result<(Socket<TcpStream>, WorldView), TransportError> {
    enter_world_with(Tcp::at(address), plan, version).await
}

/// Log in and walk into the world, over whatever `dial` opens.
///
/// Drives the whole conversation: the login connection, the relay, the game
/// connection, the character list, and the `0x55` that says the client may draw.
/// What comes back is the game connection — still open, now in the world — and
/// what the server said about the character.
///
/// The login connection is dropped on the way, which is what a real client does:
/// the server closes it behind the relay anyway.
pub async fn enter_world_with<D: Dial>(
    mut dial: D,
    plan: Plan,
    version: ClientVersion,
) -> Result<(Socket<D::Stream>, WorldView), TransportError> {
    let mut login = Login::new(plan, version);

    let mut socket = Socket::new(dial.login().await?, Stream::Plain, version);
    socket.send(&login.open()).await?;

    // The login socket, up to the relay.
    let (endpoint, opening) = loop {
        let Some(event) = socket.next_event().await? else {
            return Err(TransportError::Closed { stage: login.stage() });
        };
        match step(&mut login, event)? {
            Some(Step::Send(bytes)) => socket.send(&bytes).await?,
            Some(Step::Relay { endpoint, opening }) => break (endpoint, opening),
            _ => {}
        }
    };

    // The game connection. Compressed from its first byte, and the auth key
    // inside `opening` is the only thing that makes it this account's.
    let mut game_socket = Socket::new(dial.game(endpoint).await?, Stream::Compressed, version);
    game_socket.send(&opening).await?;

    loop {
        let Some(event) = game_socket.next_event().await? else {
            return Err(TransportError::Closed { stage: login.stage() });
        };
        match step(&mut login, event)? {
            Some(Step::Send(bytes)) => game_socket.send(&bytes).await?,
            Some(Step::Entered(start)) => {
                let mut view = WorldView::entered(start);
                // Wait for the 0x55: a client that starts drawing before it is
                // told to is a client drawing a world the server has not
                // finished sending.
                //
                // And fold in everything that arrives in the meantime, because
                // that window *is* the world being handed over — the player's
                // own `0x20` and `0x78`, a `0x78` for everyone already on
                // screen, the ground items. None of it is sent again, so a loop
                // that only waited would reach `Playing` with an empty view and
                // draw an empty world.
                loop {
                    let Some(handover_event) = game_socket.next_event().await? else {
                        return Err(TransportError::Closed { stage: login.stage() });
                    };
                    if let Event::Packet(packet) = &handover_event {
                        view.apply(packet);
                    }
                    if let Some(Step::Playing) = step(&mut login, handover_event)? {
                        return Ok((game_socket, view));
                    }
                }
            }
            _ => {}
        }
    }
}

/// One event through the login machine, with the refusal turned into an error.
///
/// `None` means the event moved nothing.
fn step(login: &mut Login, event: Event) -> Result<Option<Step>, TransportError> {
    match event {
        Event::Packet(packet) => {
            match login.on_packet(&packet)? {
                Step::Refused(reason) => Err(TransportError::Refused(reason)),
                Step::Idle => Ok(None),
                step => Ok(Some(step)),
            }
        }
        Event::Undecoded { id, .. } => {
            // The features mask, the light level, the music: packets a shard
            // sends around the login that this crate has no decoder for. They
            // are logged and stepped over, because framing already told us
            // where the next one starts.
            debug!(id = format_args!("{id}"), "no decoder yet");
            Ok(None)
        }
    }
}
