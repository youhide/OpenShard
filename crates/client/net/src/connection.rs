//! Bytes in, packets out. No socket.
//!
//! The mirror of `openshard_gateway::connection`, and it faces the same problem
//! from the other side: TCP delivers whatever it feels like, and a packet may
//! arrive in three reads or three packets in one. What is different here is
//! compression — the game connection's stream is Huffman-coded, and the client
//! is the end that has to undo it.

use openshard_protocol::huffman::{
    self,
    HuffmanError,
};
use openshard_protocol::packet::{
    Frame,
    FrameError,
    MAX_SERVER_PACKET_SIZE,
};
use openshard_protocol::server_packet::{
    ServerDecodeError,
    ServerPacket,
    frame_server_packet,
};
use openshard_protocol::version::ClientVersion;

/// A packet id this crate has no decoder for.
///
/// Distinct from the bare `u8` `openshard_protocol::packet`'s own framing
/// functions take: that layer sizes a packet by its id before anything about
/// it is known, so the id there is the wire's own currency. Here the id has
/// already failed to become a [`ServerPacket`], and is being carried around —
/// logged, compared, displayed — as an opaque fact about what arrived, which
/// is what a newtype is for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct PacketId(pub u8);

impl std::fmt::Display for PacketId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:02X}", self.0)
    }
}

/// A packet arrived.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Event {
    /// A packet this crate decodes.
    Packet(Box<ServerPacket>),
    /// A packet the framer sized but no decoder exists for yet.
    ///
    /// Not an error and not a dropped connection: the stream is intact, because
    /// framing is what keeps it so, and the caller can log the id and read on.
    /// This is the pile the client works through — every decoder written moves
    /// one id from here to [`Event::Packet`].
    Undecoded {
        /// The packet id.
        id:   PacketId,
        /// The whole packet, id byte included, already decompressed.
        body: Vec<u8>,
    },
}

/// The stream cannot be read any further.
///
/// Every variant is fatal for the connection. There is no resynchronising a UO
/// stream: it has no frame markers, so once the cursor is off by a byte, every
/// byte after it is misread.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ConnectionError {
    /// A packet id with no length, or a length that cannot be honoured.
    Frame(FrameError),
    /// The compressed stream held a bit sequence that decodes to nothing.
    Huffman(HuffmanError),
    /// A packet decoded to a body that was not what its id promised.
    Decode(ServerDecodeError),
    /// Bytes are piling up that should have framed and did not.
    ///
    /// Unreachable unless framing has a bug: a variable packet cannot claim
    /// more than [`MAX_SERVER_PACKET_SIZE`], so a well-behaved stream never
    /// buffers more than one packet's worth.
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

impl From<HuffmanError> for ConnectionError {
    fn from(error: HuffmanError) -> Self {
        Self::Huffman(error)
    }
}

impl From<ServerDecodeError> for ConnectionError {
    fn from(error: ServerDecodeError) -> Self {
        Self::Decode(error)
    }
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Frame(error) => error.fmt(f),
            Self::Huffman(error) => error.fmt(f),
            Self::Decode(error) => error.fmt(f),
            Self::Overflow { buffered } => {
                write!(f, "{buffered} bytes buffered and none of them frame")
            }
        }
    }
}

impl std::error::Error for ConnectionError {
}

/// The bound on the inbox, for the same reason the gateway has one: it turns a
/// framing bug into a dropped connection instead of a growing buffer.
///
/// A compressed packet can be *longer* than its input — the table is tuned for
/// typical content, and incompressible bytes cost more than eight bits each —
/// so the cap is twice a packet rather than exactly one.
const MAX_BUFFERED: usize = MAX_SERVER_PACKET_SIZE * 2;

/// A byte stream whose already-consumed prefix does not move the unread tail.
///
/// TCP routinely coalesces many packets into one read. Removing each packet
/// from the front of a `Vec` would copy the entire remaining read every time,
/// making draining that read quadratic. The prefix is reclaimed only when the
/// buffer is empty or when at least half the stored bytes are stale and an append
/// needs its space, so compaction is amortized across consumed bytes.
#[derive(Debug)]
struct FrontBuffer {
    bytes: Vec<u8>,
    start: usize,
}

impl FrontBuffer {
    const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            start: 0,
        }
    }

    fn unread(&self) -> &[u8] {
        &self.bytes[self.start..]
    }

    fn len(&self) -> usize {
        self.bytes.len() - self.start
    }

    fn extend_from_slice(&mut self, bytes: &[u8]) {
        let remaining = self.len();
        let spare = self.bytes.capacity() - self.bytes.len();
        if self.start >= remaining && spare < bytes.len() {
            self.bytes.copy_within(self.start.., 0);
            self.bytes.truncate(remaining);
            self.start = 0;
        }
        self.bytes.extend_from_slice(bytes);
    }

    fn consume(&mut self, length: usize) {
        assert!(length <= self.len());
        self.start += length;
        if self.start == self.bytes.len() {
            self.bytes.clear();
            self.start = 0;
        }
    }

    fn take_prefix(&mut self, length: usize) -> Vec<u8> {
        let prefix = self.unread()[..length].to_vec();
        self.consume(length);
        prefix
    }
}

/// How the stream is encoded.
///
/// # Which one a socket is, is not negotiated
///
/// The login connection is plain and the game connection is compressed from its
/// first byte, and there is no packet that says so — the client simply knows
/// which socket it opened. On the server the same fact comes off the login
/// state machine (`Session::send_packet` asks `LoginSession::is_game_login`),
/// which is why neither end carries a flag the other has to agree with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stream {
    /// The login connection: bytes as written.
    Plain,
    /// The game connection: every packet Huffman-coded on its own.
    Compressed,
}

/// The client's half of one connection.
///
/// ```
/// use openshard_client_net::connection::{Connection, Event, Stream};
/// use openshard_protocol::server_packet::ServerPacket;
/// use openshard_protocol::version::ClientVersion;
/// use openshard_protocol::world::LoginComplete;
///
/// let version = ClientVersion::new(7, 0, 45, 65);
/// let mut connection = Connection::new(Stream::Plain, version);
///
/// // 0x55 "you may start drawing", delivered one byte at a time.
/// connection.receive(&ServerPacket::LoginComplete(LoginComplete).encode(version));
///
/// assert_eq!(
///     connection.poll().unwrap(),
///     Some(Event::Packet(Box::new(ServerPacket::LoginComplete(LoginComplete)))),
/// );
/// assert_eq!(connection.poll().unwrap(), None);
/// ```
#[derive(Debug)]
pub struct Connection {
    stream:  Stream,
    version: ClientVersion,
    /// Bytes received and not yet turned into a packet.
    inbox:   FrontBuffer,
    /// Decompressed bytes waiting to be framed.
    ///
    /// Empty on a [`Stream::Plain`] connection, where the inbox already is the
    /// packet stream. On a compressed one it is the seam between the two
    /// layers: a block that decompressed to two and a half packets leaves the
    /// half here for the next block to complete.
    decoded: FrontBuffer,
}

impl Connection {
    /// A connection reading `stream`, speaking as `version`.
    ///
    /// The version is not optional and never changes: a client knows what it is
    /// before it opens a socket, and it is what it announced in its own seed.
    /// The server's connection carries an `Option` because it has to frame
    /// packets before it has been told what is on the other end.
    #[must_use]
    pub const fn new(stream: Stream, version: ClientVersion) -> Self {
        Self {
            stream,
            version,
            inbox: FrontBuffer::new(),
            decoded: FrontBuffer::new(),
        }
    }

    /// Hand over bytes that arrived. Never fails; [`Connection::poll`] reports.
    pub fn receive(&mut self, bytes: &[u8]) {
        self.inbox.extend_from_slice(bytes);
    }

    /// How many bytes are waiting to be parsed.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.inbox.len()
    }

    /// Take the next event, if one is ready.
    ///
    /// `Ok(None)` means "nothing complete yet, feed me more". Loop until it
    /// returns `None`: one read routinely carries several packets, and handling
    /// only the first stalls the rest until the socket happens to deliver again.
    pub fn poll(&mut self) -> Result<Option<Event>, ConnectionError> {
        let event = match self.stream {
            Stream::Plain => self.poll_plain()?,
            Stream::Compressed => self.poll_compressed()?,
        };

        // Checked after, not before: a full buffer that just yielded a packet is
        // healthy. Only a full buffer that yields nothing is a bug.
        if event.is_none() && self.inbox.len() > MAX_BUFFERED {
            return Err(ConnectionError::Overflow {
                buffered: self.inbox.len(),
            });
        }
        Ok(event)
    }

    fn poll_plain(&mut self) -> Result<Option<Event>, ConnectionError> {
        match frame_server_packet(self.inbox.unread(), self.version)? {
            Frame::Incomplete { .. } => Ok(None),
            Frame::Complete(length) => {
                let packet = self.inbox.take_prefix(length);
                self.interpret(packet).map(Some)
            }
        }
    }

    /// Decompress until there is a whole packet, then frame it.
    ///
    /// # A compressed block is not a packet
    ///
    /// It is tempting to assume one, because the shard compresses each *write*
    /// with its own terminator. But a write is not a packet: the login server
    /// answers a `0x91` with the feature mask and the character list in one
    /// buffer, and that whole buffer is compressed together — 1,115 bytes
    /// carrying two packets under one terminator.
    ///
    /// So the two layers are kept apart, which is what ClassicUO does as well:
    /// decompression fills a plain byte stream, and framing splits *that*. The
    /// alternative — insisting a block be exactly one packet — passes every
    /// unit test written against a compressor and fails on the first real
    /// character list.
    fn poll_compressed(&mut self) -> Result<Option<Event>, ConnectionError> {
        loop {
            match frame_server_packet(self.decoded.unread(), self.version)? {
                Frame::Complete(length) => {
                    let packet = self.decoded.take_prefix(length);
                    return self.interpret(packet).map(Some);
                }
                Frame::Incomplete { .. } => {
                    let Some((block, consumed)) = huffman::decompress_prefix(self.inbox.unread())? else {
                        return Ok(None);
                    };
                    self.inbox.consume(consumed);
                    self.decoded.extend_from_slice(&block);
                }
            }
        }
    }

    /// Decode one whole, uncompressed packet.
    fn interpret(&self, packet: Vec<u8>) -> Result<Event, ConnectionError> {
        let id = PacketId(packet[0]);
        match ServerPacket::decode(&packet, self.version)? {
            Some(decoded) => Ok(Event::Packet(Box::new(decoded))),
            None => Ok(Event::Undecoded { id, body: packet }),
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::feedback::PlaySound;
    use openshard_protocol::login::{
        DenyReason,
        LoginDenied,
    };
    use openshard_protocol::wire::SoundId;
    use openshard_protocol::world::{
        LoginComplete,
        Point,
        Season,
        SeasonChange,
    };

    use super::*;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    fn denied() -> ServerPacket {
        ServerPacket::LoginDenied(LoginDenied {
            reason: DenyReason::BadPassword,
        })
    }

    fn event(packet: ServerPacket) -> Event {
        Event::Packet(Box::new(packet))
    }

    #[test]
    fn a_packet_split_across_reads_arrives_once() {
        // The reason this type is sans-io: a socket cannot be asked to deliver
        // a packet one byte at a time, and this is exactly what one does.
        let bytes = denied().encode(version());
        let mut connection = Connection::new(Stream::Plain, version());

        for byte in &bytes[..bytes.len() - 1] {
            connection.receive(&[*byte]);
            assert_eq!(connection.poll().unwrap(), None, "not whole yet");
        }
        connection.receive(&bytes[bytes.len() - 1..]);
        assert_eq!(connection.poll().unwrap(), Some(event(denied())));
        assert_eq!(connection.poll().unwrap(), None);
    }

    #[test]
    fn a_large_shard_packet_survives_both_streams() {
        // Opening the complete craft catalogue produced a 23,483-byte 0xBF.
        // It is larger than the gateway's hostile-input cap but legal in the
        // server-to-client direction, whose length field can describe it.
        let length = 23_483usize;
        let [high, low] = (length as u16).to_be_bytes();
        let mut packet = vec![0xBF, high, low, 0xFF, 0xFF];
        packet.resize(length, 0);

        for stream in [Stream::Plain, Stream::Compressed] {
            let wire = match stream {
                Stream::Plain => packet.clone(),
                Stream::Compressed => huffman::compress(&packet),
            };
            let mut connection = Connection::new(stream, version());
            connection.receive(&wire);

            let Some(Event::Undecoded { id, body }) = connection.poll().unwrap() else {
                panic!("a large 0xBF must arrive intact");
            };
            assert_eq!(id, PacketId(0xBF));
            assert_eq!(body, packet);
            assert_eq!(connection.poll().unwrap(), None);
        }
    }

    #[test]
    fn several_packets_in_one_read_all_come_out() {
        // The other half of the same problem, and the reason `poll` is a loop
        // rather than a call: TCP coalesces, and a client that handles one
        // packet per read falls behind and never catches up.
        let mut bytes = denied().encode(version());
        bytes.extend(ServerPacket::LoginComplete(LoginComplete).encode(version()));

        let mut connection = Connection::new(Stream::Plain, version());
        connection.receive(&bytes);

        assert_eq!(connection.poll().unwrap(), Some(event(denied())));
        assert_eq!(
            connection.poll().unwrap(),
            Some(event(ServerPacket::LoginComplete(LoginComplete)))
        );
        assert_eq!(connection.poll().unwrap(), None);
    }

    #[test]
    fn a_large_coalesced_read_is_fully_drained_in_both_streams() {
        const PACKET_COUNT: usize = 1_024;

        let packet = denied().encode(version());
        for stream in [Stream::Plain, Stream::Compressed] {
            let mut wire = Vec::new();
            for _ in 0..PACKET_COUNT {
                match stream {
                    Stream::Plain => wire.extend_from_slice(&packet),
                    Stream::Compressed => wire.extend(huffman::compress(&packet)),
                }
            }

            let mut connection = Connection::new(stream, version());
            connection.receive(&wire);

            for _ in 0..PACKET_COUNT {
                assert_eq!(connection.poll().unwrap(), Some(event(denied())));
            }
            assert_eq!(connection.poll().unwrap(), None);
            assert_eq!(connection.buffered(), 0);
        }
    }

    #[test]
    fn repeated_harvest_sounds_are_not_coalesced_by_the_wire_reader() {
        let sound = ServerPacket::PlaySound(PlaySound {
            sound: SoundId(0x013E),
            at:    Point::new(100, 100, 0),
        });
        let mut bytes = sound.encode(version());
        bytes.extend(sound.encode(version()));
        bytes.extend(sound.encode(version()));

        let mut connection = Connection::new(Stream::Plain, version());
        connection.receive(&bytes);
        for _ in 0..3 {
            assert_eq!(connection.poll().unwrap(), Some(event(sound.clone())));
        }
        assert_eq!(connection.poll().unwrap(), None);
    }

    #[test]
    fn a_compressed_stream_comes_apart_packet_by_packet() {
        // What the game connection actually delivers: each packet compressed on
        // its own, then whatever TCP chose to hand over.
        let mut wire = huffman::compress(&denied().encode(version()));
        wire.extend(huffman::compress(
            &ServerPacket::LoginComplete(LoginComplete).encode(version()),
        ));

        let mut connection = Connection::new(Stream::Compressed, version());
        connection.receive(&wire);

        assert_eq!(connection.poll().unwrap(), Some(event(denied())));
        assert_eq!(
            connection.poll().unwrap(),
            Some(event(ServerPacket::LoginComplete(LoginComplete)))
        );
        assert_eq!(connection.poll().unwrap(), None);
    }

    #[test]
    fn a_compressed_packet_split_across_reads_waits() {
        // Half a compressed packet is not an error: the terminator simply has
        // not arrived. Treating it as one would drop the connection every time
        // a packet spanned two segments.
        let wire = huffman::compress(&denied().encode(version()));
        let mut connection = Connection::new(Stream::Compressed, version());

        connection.receive(&wire[..wire.len() - 1]);
        assert_eq!(connection.poll().unwrap(), None);

        connection.receive(&wire[wire.len() - 1..]);
        assert_eq!(connection.poll().unwrap(), Some(event(denied())));
    }

    #[test]
    fn a_packet_with_no_decoder_still_advances_the_stream() {
        // The point of `Undecoded`: an id nobody has written a decoder for must
        // not cost the packets behind it.
        let season = ServerPacket::SeasonChange(SeasonChange {
            season:     Season::Fall,
            play_sound: false,
        });
        let mut bytes = season.encode(version());
        bytes.extend(ServerPacket::LoginComplete(LoginComplete).encode(version()));

        let mut connection = Connection::new(Stream::Plain, version());
        connection.receive(&bytes);

        assert_eq!(
            connection.poll().unwrap(),
            Some(Event::Undecoded {
                id:   PacketId(0xBC),
                body: season.encode(version()),
            })
        );
        assert_eq!(
            connection.poll().unwrap(),
            Some(event(ServerPacket::LoginComplete(LoginComplete)))
        );
    }

    #[test]
    fn an_id_the_shard_never_sends_ends_the_connection() {
        // There is no resynchronising: without a length there is no way to know
        // where the next packet starts, so the only honest move is to stop.
        //
        // `0x1E`, the pre-6.0 animation packet `0x6E` replaced. It has been
        // `0xD6` and then `0x99`, and both had to be moved along once the shard
        // started sending them — the `0xD6` one after the assertion written
        // against it took a shop's connection down. So the id here must be one
        // that *cannot* be implemented later, not merely one that has not been:
        // an obsolete packet stays obsolete, and the test keeps asserting a rule
        // rather than a defect.
        let mut connection = Connection::new(Stream::Plain, version());
        connection.receive(&[0x1E, 0x00, 0x05, 0x01, 0x02]);
        assert!(matches!(
            connection.poll(),
            Err(ConnectionError::Frame(FrameError::UnknownPacket(0x1E)))
        ));
    }

    /// The tooltip a shop sends per stocked item, and the packet behind it.
    ///
    /// Two different things have been asserted here, and the second replaced the
    /// first rather than being added beside it. Originally: a `0xD6` had no
    /// decoder, so this pinned that it was *skipped* — which was the whole
    /// difference between a packet ignored and a session ended, since opening a
    /// vendor's window used to disconnect the player, and from the inside that
    /// looked like a trade gump that would not draw and a paperdoll that would
    /// not open afterwards. Now it decodes, so the assertion is that it arrives
    /// as itself. What survives both is the half that was always the point:
    /// **the packet behind it still arrives**, because framing is what keeps the
    /// stream whole and neither a decoder nor the lack of one changes that.
    #[test]
    fn a_property_list_decodes_and_the_packet_behind_it_still_arrives() {
        let serial = openshard_protocol::serial::Serial::new(0x4000_0001).unwrap();
        let mut list = openshard_protocol::properties::PropertyList::new(serial);
        list.add(openshard_protocol::wire::ClilocId(1_020_000));
        let (tooltip, hash) = list.finish();

        let mut bytes = tooltip;
        bytes.extend(ServerPacket::LoginComplete(LoginComplete).encode(version()));
        let mut connection = Connection::new(Stream::Plain, version());
        connection.receive(&bytes);

        assert_eq!(
            connection.poll().unwrap(),
            Some(event(ServerPacket::PropertyListReply(
                openshard_protocol::properties::PropertyListReply {
                    serial,
                    hash,
                    entries: vec![openshard_protocol::properties::PropertyEntry {
                        cliloc:    openshard_protocol::wire::ClilocId(1_020_000),
                        arguments: String::new(),
                    }],
                }
            )))
        );
        assert_eq!(
            connection.poll().unwrap(),
            Some(event(ServerPacket::LoginComplete(LoginComplete)))
        );
    }

    #[test]
    fn one_compressed_block_can_hold_several_packets() {
        // What the shard actually does, and what this got wrong first time
        // round: a *write* is compressed with one terminator, and a write can
        // be several packets — the login server answers a 0x91 with the feature
        // mask and the character list in one buffer. A client that insisted a
        // block was one packet passed its own tests and died on the first real
        // character list.
        let mut both = denied().encode(version());
        both.extend(ServerPacket::LoginComplete(LoginComplete).encode(version()));
        let wire = huffman::compress(&both);

        let mut connection = Connection::new(Stream::Compressed, version());
        connection.receive(&wire);

        assert_eq!(connection.poll().unwrap(), Some(event(denied())));
        assert_eq!(
            connection.poll().unwrap(),
            Some(event(ServerPacket::LoginComplete(LoginComplete)))
        );
        assert_eq!(connection.poll().unwrap(), None);
    }

    #[test]
    fn a_packet_split_across_two_compressed_blocks_is_reassembled() {
        // The other half of the same fact: if a write can hold two packets, it
        // can also hold one and a half. The tail waits in the decompressed
        // buffer for the block that finishes it.
        let packet = denied().encode(version());
        let first = huffman::compress(&packet[..1]);
        let second = huffman::compress(&packet[1..]);

        let mut connection = Connection::new(Stream::Compressed, version());
        connection.receive(&first);
        assert_eq!(connection.poll().unwrap(), None, "half a packet is not one");

        connection.receive(&second);
        assert_eq!(connection.poll().unwrap(), Some(event(denied())));
    }
}
