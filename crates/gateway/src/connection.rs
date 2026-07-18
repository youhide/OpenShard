//! One client connection, as a state machine that has never heard of a socket.

use openshard_protocol::{
    frame_client_packet, ClientVersion, Frame, FrameError, Seed, SeedReader, MAX_PACKET_SIZE,
};

/// Something a connection produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    /// The opening handshake completed. Carries the client's claimed version,
    /// if it sent one.
    Seeded(Seed),
    /// A whole framed packet, id byte first.
    ///
    /// Owned rather than borrowed: handing out a slice would borrow the
    /// connection for as long as the caller held it, which makes the obvious
    /// `while let Some(event) = connection.poll()?` loop impossible. One
    /// allocation per packet is worth an API nobody has to fight.
    Packet(Vec<u8>),
}

/// A connection cannot continue.
///
/// Every variant is fatal. There is no resynchronising a UO stream: it has no
/// frame markers, so once the read cursor is in the wrong place every
/// subsequent byte is misread. Drop the connection.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ConnectionError {
    /// The client sent a packet that cannot be framed.
    Frame(FrameError),
    /// More than a packet's worth of bytes are pending and none of them frame.
    ///
    /// This should be unreachable — see [`MAX_BUFFERED`]. If it fires, framing
    /// has a bug, and dropping the connection is better than growing a buffer
    /// on a client's say-so.
    Overflow {
        /// How many bytes are pending.
        buffered: usize,
    },
}

impl From<FrameError> for ConnectionError {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Frame(error) => error.fmt(f),
            Self::Overflow { buffered } => {
                write!(f, "{buffered} bytes buffered without a complete packet")
            }
        }
    }
}

impl std::error::Error for ConnectionError {}

/// How many unparsed bytes a connection may hold before it is presumed broken.
///
/// # This is a backstop, not the defence
///
/// It is tempting to read this as protection against a client that opens a
/// socket and dribbles one byte a second. It is not, and believing otherwise
/// would be worse than not having it — the real bound is upstream, in
/// `frame_client_packet`, which rejects any declared length above
/// [`MAX_PACKET_SIZE`] before a byte is reserved. A fixed packet tops out at
/// 268 bytes; a variable one cannot claim more than 18000. So the inbox is
/// already bounded by the protocol, and a dribbling client can pin at most one
/// packet's worth of memory no matter how long it waits.
///
/// What this check catches is a *framing bug*: bytes piling up that should have
/// framed and did not. That is unreachable today, and the test below pins the
/// bound that makes it so. Keeping the check costs one comparison and turns a
/// silent memory leak into a dropped connection.
const MAX_BUFFERED: usize = MAX_PACKET_SIZE;

/// Whether the connection is still reading the opening handshake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// Running [`SeedReader`]. Nothing is framed yet.
    Handshake,
    /// The seed is in; every byte from here is a framed packet.
    Framing,
}

/// One client connection.
///
/// # Sans-io
///
/// This type performs no reads, no writes, and no waiting. Bytes go in with
/// [`Connection::receive`]; events come out of [`Connection::poll`]. It does not
/// know whether it is talking to a TCP socket, a test vector, or a replay log.
///
/// That is not purity for its own sake. The interesting parts of a gateway —
/// a seed split across three TCP segments, two packets in one read, a client
/// claiming a 60KB length — are all about *byte boundaries*, and boundaries are
/// exactly what a real socket refuses to reproduce on demand. As a pure state
/// machine, every one of those is a deterministic unit test with no ports, no
/// timing, and no flakes. The Tokio layer above stays thin enough to eyeball.
///
/// ```
/// use openshard_gateway::{Connection, Event};
///
/// let mut connection = Connection::new();
///
/// // A legacy four-byte seed, then a 0x73 ping, arriving together.
/// connection.receive(&[192, 168, 0, 1, 0x73, 0x00]);
///
/// assert!(matches!(connection.poll().unwrap(), Some(Event::Seeded(_))));
/// assert_eq!(connection.poll().unwrap(), Some(Event::Packet(vec![0x73, 0x00])));
/// assert_eq!(connection.poll().unwrap(), None);
/// ```
#[derive(Debug)]
pub struct Connection {
    state: State,
    seed_reader: SeedReader,
    /// Bytes received but not yet consumed by the handshake or the framer.
    inbox: Vec<u8>,
    /// The client's version, once the server has resolved it and told us.
    ///
    /// `None` until then, which frames the drop packet (`0x08`) in its older,
    /// shorter form — see [`frame_client_packet`]. A game connection carries no
    /// version of its own (only the auth key does), so the server learns it from
    /// the paired login connection and pushes it down with [`set_version`]. A
    /// `0x08` cannot arrive before that, because a client must be in the world to
    /// drag an item.
    ///
    /// [`set_version`]: Connection::set_version
    version: Option<ClientVersion>,
}

impl Default for Connection {
    fn default() -> Self {
        Self::new()
    }
}

impl Connection {
    /// A connection expecting the first byte of a handshake.
    pub const fn new() -> Self {
        Self {
            state: State::Handshake,
            seed_reader: SeedReader::new(),
            inbox: Vec::new(),
            version: None,
        }
    }

    /// Tell the framer which client this is, so it can size the packets whose
    /// length changed across eras. Idempotent; the server may call it more than
    /// once with the same version.
    pub fn set_version(&mut self, version: ClientVersion) {
        self.version = Some(version);
    }

    /// Hand over bytes that arrived. Never fails; [`Connection::poll`] reports.
    pub fn receive(&mut self, bytes: &[u8]) {
        self.inbox.extend_from_slice(bytes);
    }

    /// How many bytes are waiting to be parsed.
    pub fn buffered(&self) -> usize {
        self.inbox.len()
    }

    /// Whether the handshake is done.
    pub fn is_handshaking(&self) -> bool {
        matches!(self.state, State::Handshake)
    }

    /// Take the next event, if one is ready.
    ///
    /// `Ok(None)` means "nothing complete yet, feed me more". Loop until it
    /// returns `None`; one read can carry several packets, and dispatching only
    /// the first would stall the rest until the next read arrives.
    pub fn poll(&mut self) -> Result<Option<Event>, ConnectionError> {
        let event = match self.state {
            State::Handshake => self.poll_handshake(),
            State::Framing => self.poll_packet()?,
        };

        // Checked after, not before: a full buffer that just yielded a packet is
        // healthy. Only a full buffer that yields nothing is an attack.
        if event.is_none() && self.inbox.len() > MAX_BUFFERED {
            return Err(ConnectionError::Overflow {
                buffered: self.inbox.len(),
            });
        }
        Ok(event)
    }

    fn poll_handshake(&mut self) -> Option<Event> {
        let (used, seed) = self.seed_reader.feed(&self.inbox);
        self.inbox.drain(..used);
        let seed = seed?;
        self.state = State::Framing;
        Some(Event::Seeded(seed))
    }

    fn poll_packet(&mut self) -> Result<Option<Event>, ConnectionError> {
        match frame_client_packet(&self.inbox, self.version)? {
            Frame::Incomplete { .. } => Ok(None),
            Frame::Complete(length) => {
                // `drain` memmoves the tail down. A ring buffer would not, but a
                // packet is a few hundred bytes and this keeps the type a plain
                // `Vec` that anything can hand bytes to.
                let packet: Vec<u8> = self.inbox.drain(..length).collect();
                Ok(Some(Event::Packet(packet)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_protocol::{ClientVersion, SEED_COMMAND};

    /// A well-formed new-style seed for 7.0.45.65.
    fn modern_seed() -> Vec<u8> {
        let mut bytes = vec![SEED_COMMAND];
        bytes.extend_from_slice(&0x0A00_0001u32.to_be_bytes());
        for field in [7u32, 0, 45, 65] {
            bytes.extend_from_slice(&field.to_be_bytes());
        }
        bytes
    }

    /// Drain every event a connection is holding.
    fn drain(connection: &mut Connection) -> Result<Vec<Event>, ConnectionError> {
        let mut events = Vec::new();
        while let Some(event) = connection.poll()? {
            events.push(event);
        }
        Ok(events)
    }

    #[test]
    fn handshake_then_framing() {
        let mut connection = Connection::new();
        assert!(connection.is_handshaking());

        connection.receive(&modern_seed());
        let events = drain(&mut connection).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::Seeded(seed)
            if seed.version == Some(ClientVersion::new(7, 0, 45, 65))));
        assert!(!connection.is_handshaking());
    }

    #[test]
    fn a_legacy_seed_needs_no_command_byte() {
        let mut connection = Connection::new();
        connection.receive(&[192, 168, 0, 1]);
        let events = drain(&mut connection).unwrap();
        assert!(matches!(&events[0], Event::Seeded(seed)
            if seed.value == 0xC0A8_0001 && seed.version.is_none()));
    }

    #[test]
    fn two_packets_in_one_read_both_come_out() {
        // The bug this guards: polling once per read leaves the second packet
        // stuck until more bytes happen to arrive.
        let mut connection = Connection::new();
        let mut bytes = modern_seed();
        bytes.extend_from_slice(&[0x73, 0x00]); // ping
        bytes.extend_from_slice(&[0x73, 0x01]); // ping
        connection.receive(&bytes);

        let events = drain(&mut connection).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[1], Event::Packet(vec![0x73, 0x00]));
        assert_eq!(events[2], Event::Packet(vec![0x73, 0x01]));
        assert_eq!(connection.buffered(), 0);
    }

    #[test]
    fn a_packet_split_across_reads_is_reassembled() {
        let mut connection = Connection::new();
        connection.receive(&modern_seed());
        drain(&mut connection).unwrap();

        // 0xAD talk, variable length, five bytes total.
        connection.receive(&[0xAD, 0x00]);
        assert_eq!(
            drain(&mut connection).unwrap(),
            vec![],
            "no length field yet"
        );

        connection.receive(&[0x05, 0xAA]);
        assert_eq!(
            drain(&mut connection).unwrap(),
            vec![],
            "length known, body short"
        );

        connection.receive(&[0xBB]);
        assert_eq!(
            drain(&mut connection).unwrap(),
            vec![Event::Packet(vec![0xAD, 0x00, 0x05, 0xAA, 0xBB])]
        );
    }

    #[test]
    fn everything_arriving_one_byte_at_a_time_still_works() {
        // The worst split TCP can produce, and the one no real client does —
        // which is exactly why it has to be a test and not a hope.
        let mut stream = modern_seed();
        stream.extend_from_slice(&[0xAD, 0x00, 0x05, 0xAA, 0xBB]);
        stream.extend_from_slice(&[0x73, 0x00]);

        let mut connection = Connection::new();
        let mut events = Vec::new();
        for byte in &stream {
            connection.receive(&[*byte]);
            events.extend(drain(&mut connection).unwrap());
        }

        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], Event::Seeded(_)));
        assert_eq!(events[1], Event::Packet(vec![0xAD, 0x00, 0x05, 0xAA, 0xBB]));
        assert_eq!(events[2], Event::Packet(vec![0x73, 0x00]));
        assert_eq!(connection.buffered(), 0);
    }

    #[test]
    fn the_drop_packet_is_framed_by_the_set_version() {
        // The item-drop disconnect: a modern client's 0x08 is fifteen bytes, and
        // a framer that thinks it is fourteen leaves a stray byte that reframes as
        // "unknown packet 0xFF". With the version set, the whole fifteen frame.
        let drop15: Vec<u8> = {
            let mut p = vec![0x08];
            p.extend_from_slice(&0x4000_002Au32.to_be_bytes()); // serial
            p.extend_from_slice(&1000u16.to_be_bytes()); // x
            p.extend_from_slice(&2000u16.to_be_bytes()); // y
            p.push(5); // z
            p.push(0); // grid slot (the fifteenth-byte difference)
            p.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes()); // drop to ground
            p
        };
        assert_eq!(drop15.len(), 15);

        // Version unknown: 0x08 frames as fourteen, so a fifteen-byte drop leaves
        // one byte over — and that byte, 0xFF (the low byte of the ground
        // target), reframes as an unknown packet and drops the connection. This
        // is the exact bug, reproduced.
        let mut legacy = Connection::new();
        legacy.receive(&modern_seed());
        drain(&mut legacy).unwrap();
        legacy.receive(&drop15);
        assert_eq!(
            legacy.poll().unwrap(),
            Some(Event::Packet(drop15[..14].to_vec())),
            "the framer takes only fourteen"
        );
        assert_eq!(
            legacy.poll(),
            Err(ConnectionError::Frame(FrameError::UnknownPacket(0xFF))),
            "the stranded byte is the disconnect the user saw"
        );

        // Version set to a grid-capable client: the whole fifteen frame cleanly.
        let mut modern = Connection::new();
        modern.receive(&modern_seed());
        drain(&mut modern).unwrap();
        modern.set_version(ClientVersion::new(7, 0, 45, 65));
        modern.receive(&drop15);
        assert_eq!(
            drain(&mut modern).unwrap(),
            vec![Event::Packet(drop15.clone())]
        );
        assert_eq!(modern.buffered(), 0, "nothing left over");
    }

    #[test]
    fn an_unknown_packet_is_fatal() {
        let mut connection = Connection::new();
        connection.receive(&modern_seed());
        drain(&mut connection).unwrap();

        connection.receive(&[0x01, 0x02, 0x03]);
        assert_eq!(
            connection.poll(),
            Err(ConnectionError::Frame(FrameError::UnknownPacket(0x01))),
            "an unknown length means the stream cannot be advanced past it"
        );
    }

    #[test]
    fn a_dribbling_client_cannot_grow_the_buffer_forever() {
        // The attack: open a socket, announce the largest packet the protocol
        // allows, then send its body one byte at a time and never finish. What
        // stops it is not a cap here — it is that `frame_client_packet` will not
        // accept a declared length above MAX_PACKET_SIZE, so the client cannot
        // ask the server to hold more than one packet's worth on its behalf.
        let mut connection = Connection::new();
        connection.receive(&modern_seed());
        drain(&mut connection).unwrap();

        connection.receive(&[0xAD, 0x46, 0x50]); // 0x4650 == 18000 == the cap
        for _ in 0..MAX_PACKET_SIZE * 2 {
            connection.receive(&[0x00]);
            drain(&mut connection).unwrap();
            assert!(
                connection.buffered() <= MAX_PACKET_SIZE,
                "a client pinned {} bytes",
                connection.buffered()
            );
        }
    }

    #[test]
    fn a_length_beyond_the_cap_is_refused_outright() {
        // The check that makes the test above hold: the claim is rejected
        // before anything is reserved for it, not after the bytes arrive.
        let mut connection = Connection::new();
        connection.receive(&modern_seed());
        drain(&mut connection).unwrap();

        connection.receive(&[0xAD, 0xFF, 0xFF]); // claims 65535
        assert!(matches!(
            connection.poll(),
            Err(ConnectionError::Frame(FrameError::BadLength { .. }))
        ));
    }

    #[test]
    fn a_full_buffer_that_yields_a_packet_is_not_an_overflow() {
        // The cap must not fire on a legitimate maximum-size packet, which is
        // why it is checked after polling rather than before.
        let mut connection = Connection::new();
        connection.receive(&modern_seed());
        drain(&mut connection).unwrap();

        let mut packet = vec![0xAD, 0x46, 0x50]; // 0x4650 == 18000 == MAX_PACKET_SIZE
        packet.resize(MAX_PACKET_SIZE, 0);
        connection.receive(&packet);

        let events = drain(&mut connection).unwrap();
        assert_eq!(events, vec![Event::Packet(packet)]);
        assert_eq!(connection.buffered(), 0);
    }

    #[test]
    fn a_silent_client_is_not_an_error() {
        let mut connection = Connection::new();
        assert_eq!(connection.poll().unwrap(), None);
        connection.receive(&[]);
        assert_eq!(connection.poll().unwrap(), None);
        assert!(connection.is_handshaking());
    }

    #[test]
    fn the_handshake_does_not_frame_its_own_bytes() {
        // 0xEF is not in the packet table precisely so that this cannot happen:
        // the seed must be consumed by the handshake, not misread as a packet.
        let mut connection = Connection::new();
        connection.receive(&modern_seed());
        let events = drain(&mut connection).unwrap();
        assert_eq!(events.len(), 1, "21 seed bytes are one event, not two");
        assert_eq!(connection.buffered(), 0);
    }
}
