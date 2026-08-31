//! `Cliloc.enu`: the client's own text table, looked up by number.
//!
//! A gump's `{ xmfhtmlgump }`/`{ xmfhtmlgumpcolor }`/`{ xmfhtmltok }` elements
//! (`gump::Element::Localized`) carry no text on the wire at all — only a
//! number — because the string is assumed to already be sitting in every
//! client's own install. That is what this file is: roughly forty thousand
//! numbered English sentences, most of them plain and a few carrying
//! `~1_val~`-style argument slots this reader does not resolve (see
//! [`Cliloc::get`]).
//!
//! # Layout
//!
//! ```text
//!   header   int32 + int16, unread                    6 bytes
//!   record   int32 number, u8 flag, i16 length, text   8 + length bytes
//! ```
//!
//! Records run back to back to the end of the file, in no particular number
//! order — a lookup is a table built once, not a scan per call.
//!
//! Newer clients BWT-compress the complete table and mark it with `0x8E` as the
//! fourth byte. [`Cliloc::load`] expands that form before parsing it, so the
//! caller gets the same number-to-text lookup from either client generation.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// The stable number a client's localized string table assigns to a sentence.
///
/// This is a content identifier, not a byte offset or a record length. Keeping
/// it distinct from other `u32` values prevents a cliloc lookup from accepting
/// an unrelated numeric field by accident.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ClilocNumber(u32);

impl ClilocNumber {
    /// Construct a cliloc number from its client-defined value.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the numeric value used by the file format.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// `Cliloc.enu` could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum ClilocError {
    /// The file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The fourth byte is `0x8E`, but its BWT-compressed payload is malformed.
    Compressed {
        /// Which file.
        path: PathBuf,
    },
}

impl fmt::Display for ClilocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Compressed { path } => {
                write!(f, "{} has an invalid BWT-compressed payload", path.display())
            }
        }
    }
}

impl std::error::Error for ClilocError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Compressed { .. } => None,
        }
    }
}

/// Every numbered string the client's own `Cliloc.enu` holds.
#[derive(Clone, Default)]
pub struct Cliloc {
    entries: HashMap<ClilocNumber, String>,
}

impl fmt::Debug for Cliloc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cliloc")
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl Cliloc {
    /// Read `Cliloc.enu`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ClilocError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| ClilocError::Read {
            path: path.to_owned(),
            source,
        })?;
        let bytes = match bytes.get(3) == Some(&0x8E) {
            true => decompress_bwt(&bytes).ok_or_else(|| ClilocError::Compressed {
                path: path.to_owned(),
            })?,
            false => bytes,
        };
        Ok(Self::parse(&bytes))
    }

    /// Parse bytes already in memory. Never fails: a truncated record simply
    /// ends the table where the file did, the same tolerance
    /// [`crate::gumpart`]'s reader gives a cut-off entry.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Self {
        let mut entries = HashMap::new();
        // 4-byte header + 2-byte header, neither read by anything real.
        let Some(mut rest) = bytes.get(6..) else {
            return Self { entries };
        };
        while rest.len() >= 4 + 1 + 2 {
            let number = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
            let length = u16::from_le_bytes([rest[5], rest[6]]) as usize;
            let Some(text) = rest.get(7..7 + length) else {
                break;
            };
            entries.insert(
                ClilocNumber::new(number),
                String::from_utf8_lossy(text).into_owned(),
            );
            rest = &rest[7 + length..];
        }
        Self { entries }
    }

    /// How many strings this table holds.
    #[must_use]
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// The string a cliloc number names, or `None` past the table's end.
    ///
    /// **No argument substitution.** ServUO's own `~1_val~` slots (an item's
    /// name, a count) are left in the text verbatim — the caller travelled no
    /// arguments to fill them with, because nothing here carries them over
    /// the wire (that is the whole point of sending a number and not a
    /// string). A sentence with no slots — the common case for a plain
    /// dialog's title and buttons — reads exactly as authored.
    #[must_use]
    pub fn get(&self, number: ClilocNumber) -> Option<&str> {
        self.entries.get(&number).map(String::as_str)
    }
}

/// Decode the BWT form used by recent `Cliloc.*` files.
///
/// The first four bytes identify the compressed stream; the rest is a
/// move-to-front stage followed by the client's BWT variant. Its decoded
/// prefix is a 256-entry frequency table, followed by the byte stream itself.
/// Invalid offsets or inconsistent frequencies are rejected instead of making
/// a corrupt client file look like a valid, empty localization table.
fn decompress_bwt(bytes: &[u8]) -> Option<Vec<u8>> {
    // The original reader primes `first_char` from byte four, then consumes the
    // next byte at the end of every iteration. That processes byte four through
    // the penultimate byte; its `file_len - 4` buffer leaves one trailing zero.
    let input = bytes.get(4..bytes.len().checked_sub(1)?)?;
    if input.len() < 1024 {
        return None;
    }

    let mut move_to_front: Vec<u16> = (0..=u16::MAX).collect();
    // ClassicUO allocates `file_len - 4` bytes but fills all except the final
    // one. Preserve that trailing zero: the BWT stage reads it as part of its
    // run data.
    let mut transformed = Vec::with_capacity(input.len() + 1);
    for &current in input {
        let current = usize::from(current);
        let value = move_to_front[current];
        move_to_front.copy_within(0..current, 1);
        move_to_front[0] = value;
        transformed.push(value as u8);
    }
    transformed.push(0);

    let mut partial = [0i32; 256 * 3];
    for (entry, chunk) in partial[..256]
        .iter_mut()
        .zip(transformed[..1024].as_chunks::<4>().0)
    {
        *entry = i32::from_le_bytes(*chunk);
    }
    let length = partial[..256].iter().try_fold(0usize, |sum, &count| {
        sum.checked_add(usize::try_from(count).ok()?)
    })?;
    if length == 0 {
        return Some(Vec::new());
    }

    let mut symbols: [u8; 256] = std::array::from_fn(|index| index as u8);
    let mut frequencies = partial[..256].to_vec();
    let mut ranked = [0u8; 256];
    let mut nonzero = 0usize;
    for rank in &mut ranked {
        // `BwtDecompress.Frequency` picks the first index on ties.  The
        // ordering controls the offsets below, so `Iterator::max_by_key` is
        // not equivalent: it retains the last tied item.
        let mut index = 0;
        let mut count = 0;
        for (candidate, &frequency) in frequencies.iter().enumerate() {
            if frequency > count {
                index = candidate;
                count = frequency;
            }
        }
        if count == 0 {
            break;
        }
        *rank = index as u8;
        frequencies[index] = 0;
        nonzero += 1;
    }

    let mut offset = 0usize;
    for &symbol in ranked.iter().take(nonzero) {
        let symbol = usize::from(symbol);
        let count = usize::try_from(partial[symbol]).ok()?;
        let end = offset.checked_add(count)?;
        if end > transformed.len().saturating_sub(1024) {
            return None;
        }
        symbols[transformed[1024 + offset] as usize] = symbol as u8;
        partial[symbol + 256] = i32::try_from(offset.checked_add(1)?).ok()?;
        offset = end;
        partial[symbol + 512] = i32::try_from(offset).ok()?;
    }

    let mut output = Vec::with_capacity(length);
    let mut value = symbols[0];
    while output.len() < length {
        output.push(value);
        let index = usize::from(value);
        let cursor = usize::try_from(partial[index + 256]).ok()?;
        let end = usize::try_from(partial[index + 512]).ok()?;
        if cursor >= end {
            if nonzero == 0 {
                return None;
            }
            nonzero -= 1;
            symbols.copy_within(1..=nonzero, 0);
            value = symbols[0];
        } else {
            let next = *transformed.get(cursor.checked_add(1024)?)?;
            partial[index + 256] = partial[index + 256].checked_add(1)?;
            if next != 0 {
                let next = usize::from(next);
                symbols.copy_within(1..=next, 0);
                symbols[next] = value;
                value = symbols[0];
            }
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(number: u32, text: &str) -> Vec<u8> {
        let mut out = number.to_le_bytes().to_vec();
        out.push(0); // flag, unread
        out.extend_from_slice(&(text.len() as u16).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        out
    }

    #[test]
    fn reads_records_past_the_header() {
        let mut bytes = vec![0u8; 6]; // the unread header
        bytes.extend(record(1_011_022, "Resurrection"));
        bytes.extend(record(1_011_011, "CONTINUE"));

        let table = Cliloc::parse(&bytes);
        assert_eq!(table.count(), 2);
        assert_eq!(table.get(ClilocNumber::new(1_011_022)), Some("Resurrection"));
        assert_eq!(table.get(ClilocNumber::new(1_011_011)), Some("CONTINUE"));
        assert_eq!(
            table.get(ClilocNumber::new(1)),
            None,
            "a number never written is absent, not a panic"
        );
    }

    #[test]
    fn a_truncated_record_ends_the_table_rather_than_panicking() {
        let mut bytes = vec![0u8; 6];
        bytes.extend(record(1, "whole"));
        bytes.extend_from_slice(&[9, 0, 0, 0, 0, 20, 0]); // a length claiming 20 bytes it does not have
        let table = Cliloc::parse(&bytes);
        assert_eq!(
            table.get(ClilocNumber::new(1)),
            Some("whole"),
            "the good record before the cut still reads"
        );
        assert_eq!(table.count(), 1);
    }

    #[test]
    fn an_empty_file_is_an_empty_table() {
        assert_eq!(Cliloc::parse(&[]).count(), 0);
        assert_eq!(Cliloc::parse(&[0u8; 6]).count(), 0, "header only, no records");
    }

    #[test]
    fn a_malformed_compressed_file_is_rejected() {
        let mut bytes = vec![0u8; 8];
        bytes[3] = 0x8E;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("openshard-cliloc-test-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Cliloc.enu");
        std::fs::write(&path, &bytes).unwrap();
        let error = Cliloc::load(&path).unwrap_err();
        assert!(matches!(error, ClilocError::Compressed { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_compressed_empty_table_loads() {
        // Four header bytes, the 256-entry frequency table, and **one more**:
        // `decompress_bwt` drops the final byte on purpose, mirroring a reader
        // that fills all but the last of a `file_len - 4` buffer. So the
        // smallest file carrying a whole frequency table is 4 + 1024 + 1, and at
        // 4 + 1024 the table is a byte short and the stream is rejected.
        let mut bytes = vec![0u8; 4 + 1024 + 1];
        bytes[3] = 0x8E;
        let table = Cliloc::parse(&decompress_bwt(&bytes).expect("a valid compressed stream"));
        assert_eq!(
            table.count(),
            0,
            "an all-zero frequency table decodes to no records"
        );
    }

    #[test]
    fn a_compressed_stream_too_short_for_its_frequency_table_is_rejected() {
        // One byte less than the test above, which is the case that made that
        // one fail: a truncated table must be `None` rather than decode as an
        // empty localization file, because a client whose Cliloc silently reads
        // as empty shows every string as a number.
        let mut bytes = vec![0u8; 4 + 1024];
        bytes[3] = 0x8E;
        assert!(decompress_bwt(&bytes).is_none());
    }
}
