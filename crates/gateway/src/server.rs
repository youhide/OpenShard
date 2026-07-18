//! The Tokio adapter.
//!
//! Everything interesting lives in [`Connection`]. This module only moves bytes
//! between a socket and that state machine, and it is kept small enough to read
//! in one sitting on purpose — code that cannot be unit tested should not be
//! where the thinking happens.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::connection::{Connection, ConnectionError, Event};

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
        outbox: mpsc::UnboundedSender<Vec<u8>>,
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

/// Accepts connections and drives a [`Connection`] for each.
///
/// Events go onto a channel rather than through a callback: the world server
/// consumes them on its own tick, and a callback would run world code inside a
/// network task on an arbitrary thread. The channel is the boundary between
/// "async everywhere" and "the deterministic simulation".
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    events: mpsc::UnboundedSender<ServerEvent>,
    next_id: Arc<AtomicU64>,
}

impl Server {
    /// Bind to `address`.
    ///
    /// Returns the server and the channel its events arrive on.
    pub async fn bind(
        address: SocketAddr,
    ) -> io::Result<(Self, mpsc::UnboundedReceiver<ServerEvent>)> {
        let listener = TcpListener::bind(address).await?;
        let (events, receiver) = mpsc::unbounded_channel();
        Ok((
            Self {
                listener,
                events,
                next_id: Arc::new(AtomicU64::new(1)),
            },
            receiver,
        ))
    }

    /// The address actually bound, which matters when port 0 was requested.
    pub fn local_address(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accept forever, spawning a task per connection.
    ///
    /// Only returns if accepting itself fails, which means the listener is gone.
    pub async fn run(self) -> io::Result<()> {
        info!(address = ?self.local_address()?, "gateway listening");
        loop {
            let (stream, address) = self.listener.accept().await?;
            let id = ConnectionId(self.next_id.fetch_add(1, Ordering::Relaxed));
            let events = self.events.clone();
            tokio::spawn(async move {
                // A panic in here takes this connection down and nothing else.
                // That is why the release profile does not set panic = "abort".
                if let Err(error) = serve(id, address, stream, events).await {
                    debug!(%id, %error, "connection ended");
                }
            });
        }
    }
}

/// Drive one connection until it closes.
async fn serve(
    id: ConnectionId,
    address: SocketAddr,
    stream: TcpStream,
    events: mpsc::UnboundedSender<ServerEvent>,
) -> io::Result<()> {
    // Nagle batches small writes, and nearly everything a UO server sends is a
    // small write that the client is waiting on. Latency beats packet count.
    stream.set_nodelay(true)?;

    let (mut reader, mut writer) = stream.into_split();
    let (outbox, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();

    if events
        .send(ServerEvent::Connected {
            id,
            address,
            outbox,
        })
        .is_err()
    {
        return Ok(()); // The world server is gone; nothing to serve.
    }

    // Writes get their own task so that a slow client cannot block reading.
    let writes = tokio::spawn(async move {
        while let Some(bytes) = outgoing.recv().await {
            if writer.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });

    let reason = read_loop(id, &mut reader, &events).await;

    writes.abort();
    let _ = events.send(ServerEvent::Disconnected { id, reason });
    Ok(())
}

/// Read until the socket closes or the client breaks the protocol.
async fn read_loop(
    id: ConnectionId,
    reader: &mut tokio::net::tcp::OwnedReadHalf,
    events: &mpsc::UnboundedSender<ServerEvent>,
) -> Option<String> {
    let mut connection = Connection::new();
    let mut buffer = [0u8; 4096];

    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) => return None, // clean close
            Ok(count) => count,
            Err(error) => return Some(error.to_string()),
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
    use super::*;
    use openshard_protocol::SEED_COMMAND;

    fn modern_seed() -> Vec<u8> {
        let mut bytes = vec![SEED_COMMAND];
        bytes.extend_from_slice(&0x0A00_0001u32.to_be_bytes());
        for field in [7u32, 0, 45, 65] {
            bytes.extend_from_slice(&field.to_be_bytes());
        }
        bytes
    }

    /// Bind to an ephemeral port and start accepting.
    async fn start() -> (SocketAddr, mpsc::UnboundedReceiver<ServerEvent>) {
        let (server, events) = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let address = server.local_address().unwrap();
        tokio::spawn(server.run());
        (address, events)
    }

    #[tokio::test]
    async fn a_client_can_connect_and_be_heard() {
        let (address, mut events) = start().await;
        let mut client = TcpStream::connect(address).await.unwrap();

        let ServerEvent::Connected { id, .. } = events.recv().await.unwrap() else {
            panic!("expected Connected first");
        };

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
        assert_eq!(event, Event::Packet(vec![0x73, 0x00]));
    }

    #[tokio::test]
    async fn the_server_can_write_back() {
        let (address, mut events) = start().await;
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
        let (address, mut events) = start().await;
        let client = TcpStream::connect(address).await.unwrap();
        let ServerEvent::Connected { .. } = events.recv().await.unwrap() else {
            panic!("expected Connected");
        };
        drop(client);

        let ServerEvent::Disconnected { reason, .. } = events.recv().await.unwrap() else {
            panic!("expected Disconnected");
        };
        assert_eq!(reason, None, "hanging up is not an error");
    }

    #[tokio::test]
    async fn a_protocol_violation_drops_the_connection() {
        let (address, mut events) = start().await;
        let mut client = TcpStream::connect(address).await.unwrap();
        let ServerEvent::Connected { .. } = events.recv().await.unwrap() else {
            panic!("expected Connected");
        };

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

    #[tokio::test]
    async fn connections_get_distinct_ids() {
        let (address, mut events) = start().await;
        let _a = TcpStream::connect(address).await.unwrap();
        let _b = TcpStream::connect(address).await.unwrap();

        let mut ids = Vec::new();
        for _ in 0..2 {
            if let ServerEvent::Connected { id, .. } = events.recv().await.unwrap() {
                ids.push(id);
            }
        }
        assert_ne!(ids[0], ids[1]);
    }
}
