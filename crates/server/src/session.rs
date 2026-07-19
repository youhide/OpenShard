use super::*;

/// Per-connection state this loop owns.
pub(crate) struct Session {
    pub(crate) login: LoginSession,
    /// Whether a character has been asked for. The world owns the entity; this
    /// is only enough to know a `0x02` is worth queueing.
    pub(crate) in_world: bool,
    /// Whether this is a game-server connection, whose every server-to-client
    /// packet is Huffman-compressed.
    ///
    /// The UO login connection is uncompressed; the game connection compresses
    /// everything from the character list on. This mirrors Sphere's
    /// `CONNECT_GAME`, which it sets during the game socket's crypt handshake —
    /// before the character list is sent — so the list and all world traffic go
    /// out compressed. Here the seam is the `0x91` game login: see the flag being
    /// set in `handle`.
    pub(crate) game: bool,
    pub(crate) outbox: mpsc::UnboundedSender<Vec<u8>>,
    /// Tells the gateway framer this connection's client version. A game
    /// connection sends no version of its own, so the framer defaults to the older
    /// dialect until this carries the real one across — needed for the packets
    /// whose length changed across eras (the drop packet). Sent at character
    /// select, well before any in-world packet that depends on it.
    pub(crate) control: mpsc::UnboundedSender<ClientVersion>,
}

impl Session {
    /// Act on a login response. Returns `false` if the connection should go.
    ///
    /// Dropping the outbox is what closes the socket: the gateway's write task
    /// ends when its channel does. There is no separate "close" to forget.
    pub(crate) fn apply(&self, response: Response, id: ConnectionId) -> bool {
        match response {
            Response::Idle => true,
            Response::Send(bytes) => self.send_packet(bytes),
            Response::SendThenClose(bytes) => {
                let _ = self.send_packet(bytes);
                false
            }
            Response::Close => {
                warn!(%id, "closing on a protocol error");
                false
            }
        }
    }

    /// Send one server-to-client packet, compressing it on a game connection.
    ///
    /// The login connection sends plain bytes; the game connection Huffman-
    /// compresses every packet, each one independently — terminator and all —
    /// exactly as Sphere's `CNetworkOutput` does for `CONNECT_GAME`. Skip this
    /// and ClassicUO, which decompresses the game stream unconditionally, decodes
    /// the raw bytes through its Huffman tree, produces plausible garbage for a
    /// while, and then desyncs on a fabricated packet id far downstream —
    /// surfacing as `need more data ID: 0E ...` hundreds of bytes in, looking
    /// nothing like a compression problem.
    pub(crate) fn send_packet(&self, bytes: Vec<u8>) -> bool {
        let bytes = if self.game {
            huffman::compress(&bytes)
        } else {
            bytes
        };
        self.outbox.send(bytes).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(game: bool) -> (Session, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (outbox, wire) = mpsc::unbounded_channel();
        let (control, _control_rx) = mpsc::unbounded_channel();
        (
            Session {
                login: LoginSession::new(),
                in_world: false,
                game,
                outbox,
                control,
            },
            wire,
        )
    }

    #[test]
    fn a_game_connection_compresses_and_a_login_one_does_not() {
        // The whole bug. ClassicUO Huffman-decodes every packet on the game
        // connection; send one raw and it decodes garbage and desyncs later on a
        // fabricated id ("need more data ID: 0E ..."). A character-list-shaped
        // packet, since 0xA9 is the first thing the game connection ever sends.
        let packet = vec![0xA9u8, 0x00, 0x08, 0x05, b'L', b'o', b'r', b'd'];

        let (game, mut wire) = session(true);
        assert!(game.send_packet(packet.clone()));
        let on_wire = wire.try_recv().expect("a packet was sent");
        assert_ne!(on_wire, packet, "a game packet must not leave raw");
        assert_eq!(
            huffman::decompress(&on_wire).expect("valid stream"),
            packet,
            "and the client must get its bytes back"
        );

        let (login, mut wire) = session(false);
        assert!(login.send_packet(packet.clone()));
        assert_eq!(
            wire.try_recv().expect("a packet was sent"),
            packet,
            "the login connection is never compressed"
        );
    }
}
