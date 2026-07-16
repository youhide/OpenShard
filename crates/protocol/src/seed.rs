//! The login seed: the first thing a client ever sends.
//!
//! # Why this is a state machine and not a packet
//!
//! The seed arrives before packet framing means anything, and it breaks every
//! rule the framing layer relies on:
//!
//! - **Old clients send no id byte at all.** Four raw bytes — historically the
//!   client's own IPv4 address — and that is the whole handshake. There is
//!   nothing to look up in a length table.
//! - **New clients send `0xEF`** plus a seed and four version dwords, 21 bytes.
//! - **The `0xEF` byte can arrive alone.** Sphere's `CNetworkInput.cpp` handles
//!   `uiOrigRemainingLength == 1 && data[0] == XCMD_NewSeed` as its own case and
//!   carries an `m_newseed` flag to the next read. TCP is a stream; a client is
//!   free to flush after one byte and some do.
//!
//! So `0xEF` is deliberately absent from [`crate::client_packet_length`], and a
//! connection runs [`SeedReader`] until it produces a [`Seed`] before it starts
//! framing anything.
//!
//! # The ambiguity
//!
//! An old-style seed is four arbitrary bytes. If the first happens to be `0xEF`,
//! it is indistinguishable from the start of a new-style seed. Sphere resolves
//! this by length: `0xEF` plus at least 21 bytes is new-style, and anything else
//! beginning with four or more bytes is old-style. That is a heuristic, not a
//! rule — but it is the same heuristic every UO server uses, and real clients do
//! not collide with it.

use crate::codec::PacketReader;
use crate::packet::{SEED_LENGTH_NEW, SEED_LENGTH_OLD};
use crate::version::ClientVersion;

/// The `0xEF` byte that opens a new-style seed.
pub const SEED_COMMAND: u8 = 0xEF;

/// What a client sent as its opening handshake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Seed {
    /// The seed value. Doubles as the client's claimed IPv4 address on old
    /// clients, and keys the login encryption on both.
    pub value: u32,
    /// The version the client reports here.
    ///
    /// `None` for an old-style seed, which carries no version — those clients
    /// must be identified some other way, usually the `0xBD` packet later.
    ///
    /// Do not trust this. It is a claim, not a fact: nothing stops a client
    /// reporting 7.0.95 and then speaking 3.0. It picks the dialect the server
    /// *speaks*, which is a compatibility decision, not a security one.
    pub version: Option<ClientVersion>,
}

/// Reads the opening handshake across however many TCP reads it takes.
///
/// Feed it bytes; it tells you how many it consumed and whether it is done.
///
/// ```
/// use openshard_protocol::{SeedReader, ClientVersion};
///
/// let mut reader = SeedReader::new();
///
/// // A client that flushes the 0xEF byte on its own — which happens.
/// assert_eq!(reader.feed(&[0xEF]), (1, None));
///
/// // The rest arrives in the next read: seed, then 7.0.45.65.
/// let rest = [
///     0x0A, 0x00, 0x00, 0x01, // seed
///     0x00, 0x00, 0x00, 0x07, // major
///     0x00, 0x00, 0x00, 0x00, // minor
///     0x00, 0x00, 0x00, 0x2D, // revision
///     0x00, 0x00, 0x00, 0x41, // patch
/// ];
/// let (used, seed) = reader.feed(&rest);
/// assert_eq!(used, 20);
///
/// let seed = seed.unwrap();
/// assert_eq!(seed.value, 0x0A00_0001);
/// assert_eq!(seed.version, Some(ClientVersion::new(7, 0, 45, 65)));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SeedReader {
    /// Whether a lone `0xEF` has already been consumed, so the next bytes are
    /// the body of a new-style seed rather than a legacy seed that happens to
    /// start with 0xEF.
    ///
    /// This flag is the whole reason the type exists — it is state that has to
    /// survive between reads. Sphere calls it `m_newseed`.
    command_seen: bool,
    done: bool,
}

impl SeedReader {
    /// A reader expecting the first byte of a connection.
    pub const fn new() -> Self {
        Self {
            command_seen: false,
            done: false,
        }
    }

    /// Whether a seed has already been produced.
    ///
    /// A second handshake on one connection is not a thing; the caller should
    /// have moved on to framing.
    pub const fn is_done(&self) -> bool {
        self.done
    }

    /// Offer bytes from the front of the connection buffer.
    ///
    /// Returns how many bytes to consume and the [`Seed`] if the handshake
    /// completed. `(0, None)` means "not enough yet, call again with more"; the
    /// caller must not drop anything.
    pub fn feed(&mut self, buffer: &[u8]) -> (usize, Option<Seed>) {
        if self.done {
            return (0, None);
        }

        // A lone 0xEF with nothing behind it: bank the fact and wait. Without
        // this the next read would see a headless body and read the seed as a
        // legacy handshake.
        if !self.command_seen && buffer.len() == 1 && buffer[0] == SEED_COMMAND {
            self.command_seen = true;
            return (1, None);
        }

        if self.command_seen {
            return self.read_modern_body(buffer, 0);
        }

        match buffer.first() {
            None => (0, None),
            // 0xEF plus a full body: new-style. Sphere disambiguates purely on
            // length, since a legacy seed is four arbitrary bytes and could
            // legitimately begin with 0xEF.
            Some(&SEED_COMMAND) if buffer.len() >= SEED_LENGTH_NEW => {
                self.read_modern_body(buffer, 1)
            }
            // Still short of a full modern seed. It could become one on the next
            // read, so we cannot yet decide it is legacy — wait.
            Some(&SEED_COMMAND) => (0, None),
            Some(_) if buffer.len() >= SEED_LENGTH_OLD => {
                let value = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
                self.done = true;
                (
                    SEED_LENGTH_OLD,
                    Some(Seed {
                        value,
                        version: None,
                    }),
                )
            }
            Some(_) => (0, None),
        }
    }

    /// Read seed and four version dwords starting at `offset`.
    fn read_modern_body(&mut self, buffer: &[u8], offset: usize) -> (usize, Option<Seed>) {
        let body_length = SEED_LENGTH_NEW - 1;
        if buffer.len() < offset + body_length {
            return (0, None);
        }

        let mut reader = PacketReader::new(&buffer[offset..]);
        // Every read below is inside the length just checked, so these cannot
        // fail; the codec is fallible regardless because the bytes are hostile.
        let (Ok(value), Ok(major), Ok(minor), Ok(revision), Ok(patch)) = (
            reader.u32(),
            reader.u32(),
            reader.u32(),
            reader.u32(),
            reader.u32(),
        ) else {
            return (0, None);
        };

        self.done = true;
        (
            offset + body_length,
            Some(Seed {
                value,
                // The client sends each field as a dword but every real value
                // fits a byte. Saturating rather than truncating: a claimed
                // major of 256 becoming 0 would silently demote a modern client
                // to the oldest possible dialect, which is worse than clamping.
                version: Some(ClientVersion::new(
                    saturate(major),
                    saturate(minor),
                    saturate(revision),
                    saturate(patch),
                )),
            }),
        )
    }
}

/// Narrow a version dword to the byte the rest of the crate uses.
const fn saturate(value: u32) -> u8 {
    if value > u8::MAX as u32 {
        u8::MAX
    } else {
        value as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed new-style seed for 7.0.45.65 with seed 0x0A000001.
    fn modern_seed() -> Vec<u8> {
        let mut bytes = vec![SEED_COMMAND];
        bytes.extend_from_slice(&0x0A00_0001u32.to_be_bytes());
        for field in [7u32, 0, 45, 65] {
            bytes.extend_from_slice(&field.to_be_bytes());
        }
        assert_eq!(bytes.len(), SEED_LENGTH_NEW);
        bytes
    }

    #[test]
    fn reads_a_modern_seed_in_one_go() {
        let mut reader = SeedReader::new();
        let (used, seed) = reader.feed(&modern_seed());
        assert_eq!(used, SEED_LENGTH_NEW);
        assert_eq!(
            seed,
            Some(Seed {
                value: 0x0A00_0001,
                version: Some(ClientVersion::new(7, 0, 45, 65)),
            })
        );
        assert!(reader.is_done());
    }

    #[test]
    fn reads_a_legacy_seed() {
        let mut reader = SeedReader::new();
        let (used, seed) = reader.feed(&[192, 168, 0, 1]);
        assert_eq!(used, SEED_LENGTH_OLD);
        assert_eq!(
            seed,
            Some(Seed {
                value: 0xC0A8_0001,
                version: None,
            }),
            "a legacy seed carries no version at all"
        );
    }

    #[test]
    fn a_lone_command_byte_is_banked_not_misread() {
        // The case Sphere carries `m_newseed` for. Without the flag, the next
        // read would see a headless body and take the seed dword as a legacy
        // four-byte seed.
        let mut reader = SeedReader::new();
        assert_eq!(reader.feed(&[SEED_COMMAND]), (1, None));
        assert!(!reader.is_done());

        let (used, seed) = reader.feed(&modern_seed()[1..]);
        assert_eq!(used, SEED_LENGTH_NEW - 1);
        assert_eq!(
            seed.unwrap().version,
            Some(ClientVersion::new(7, 0, 45, 65))
        );
    }

    #[test]
    fn a_modern_seed_split_anywhere_still_reads() {
        // TCP may deliver the 21 bytes in any split at all.
        let full = modern_seed();
        for split in 1..full.len() {
            let mut reader = SeedReader::new();
            let mut buffer: Vec<u8> = Vec::new();
            let mut seed = None;

            for chunk in [&full[..split], &full[split..]] {
                buffer.extend_from_slice(chunk);
                let (used, produced) = reader.feed(&buffer);
                buffer.drain(..used);
                if produced.is_some() {
                    seed = produced;
                }
            }

            assert_eq!(
                seed.expect("split at {split} lost the seed").version,
                Some(ClientVersion::new(7, 0, 45, 65)),
                "failed with a split after {split} bytes"
            );
            assert!(buffer.is_empty(), "split at {split} left bytes behind");
        }
    }

    #[test]
    fn a_modern_seed_arriving_one_byte_at_a_time_still_reads() {
        let full = modern_seed();
        let mut reader = SeedReader::new();
        let mut buffer: Vec<u8> = Vec::new();
        let mut seed = None;

        for byte in &full {
            buffer.push(*byte);
            let (used, produced) = reader.feed(&buffer);
            buffer.drain(..used);
            if produced.is_some() {
                seed = produced;
            }
        }

        assert_eq!(seed.unwrap().value, 0x0A00_0001);
        assert!(buffer.is_empty());
    }

    #[test]
    fn a_short_buffer_consumes_nothing() {
        // The caller keeps its bytes; anything else loses data mid-handshake.
        let mut reader = SeedReader::new();
        assert_eq!(reader.feed(&[]), (0, None));
        assert_eq!(reader.feed(&[192, 168]), (0, None));
        assert!(!reader.is_done());
    }

    #[test]
    fn a_legacy_seed_starting_with_the_command_byte_waits_rather_than_guessing() {
        // 0xEF is a legal first byte of a four-byte legacy seed (239.x.x.x).
        // Length is the only thing that separates the two forms, so with fewer
        // than 21 bytes the reader must not commit either way.
        let mut reader = SeedReader::new();
        assert_eq!(reader.feed(&[0xEF, 0x01, 0x02, 0x03]), (0, None));
        assert!(!reader.is_done());
    }

    #[test]
    fn trailing_bytes_are_left_for_the_framing_layer() {
        // The 0x80 login packet usually arrives in the same segment as the seed.
        let mut buffer = modern_seed();
        buffer.extend_from_slice(&[0x80, 0xAA, 0xBB]);

        let mut reader = SeedReader::new();
        let (used, seed) = reader.feed(&buffer);
        assert_eq!(used, SEED_LENGTH_NEW, "only the seed is consumed");
        assert!(seed.is_some());
        assert_eq!(&buffer[used..], &[0x80, 0xAA, 0xBB]);
    }

    #[test]
    fn a_finished_reader_consumes_nothing_more() {
        let mut reader = SeedReader::new();
        reader.feed(&modern_seed());
        assert_eq!(
            reader.feed(&[0x80, 0x00]),
            (0, None),
            "the framing layer owns the stream from here"
        );
    }

    #[test]
    fn an_absurd_version_saturates_rather_than_wrapping() {
        // A truncating cast would turn a claimed major of 256 into 0 and quietly
        // demote the client to the oldest dialect we speak.
        let mut bytes = vec![SEED_COMMAND];
        bytes.extend_from_slice(&1u32.to_be_bytes());
        for field in [256u32, 0xFFFF_FFFF, 0, 0] {
            bytes.extend_from_slice(&field.to_be_bytes());
        }

        let mut reader = SeedReader::new();
        let (_, seed) = reader.feed(&bytes);
        assert_eq!(
            seed.unwrap().version,
            Some(ClientVersion::new(255, 255, 0, 0))
        );
    }

    #[test]
    fn a_zero_seed_is_accepted() {
        // Not meaningful, but not the seed reader's call to reject.
        let mut bytes = vec![SEED_COMMAND];
        bytes.extend_from_slice(&[0u8; 20]);
        let mut reader = SeedReader::new();
        let (used, seed) = reader.feed(&bytes);
        assert_eq!(used, SEED_LENGTH_NEW);
        assert_eq!(seed.unwrap().value, 0);
    }
}
