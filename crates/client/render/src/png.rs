//! PNG, written by hand, because every picture this crate dumps is read by a
//! person and a person's image viewer.
//!
//! Thirteen places in this crate used to spell `P6\n{width} {height}\n255\n`
//! into a byte vector — the same idiom restated once per tool, which is the
//! shape `docs/lighting_rebuild.md`'s backlog complains about elsewhere. This is
//! that idiom, once.
//!
//! # Why not the `png` crate
//!
//! It is already in the lock file, and it would compress. But it would have to
//! be a *dependency of the library* for [`crate::plan::Picture`] to reach it —
//! a codec shipped inside the renderer so that debug dumps are smaller — or
//! else the encoder would live in `examples/` and the library's own picture type
//! could not write one. Neither is worth it for files nobody keeps.
//!
//! So: stored deflate blocks, no compression at all. The output is a valid PNG
//! by the spec's own least interesting path, about the size of the PPM it
//! replaces, and it opens in anything. If a dump ever gets big enough that this
//! matters, the answer is the `png` crate and a dev-dependency, not a Huffman
//! coder here.

/// The eight bytes every PNG starts with.
const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// The largest a single stored deflate block may be, from the format: `LEN` is
/// two bytes.
const MAX_STORED: usize = 65_535;

/// Encode 8-bit RGB pixels, row-major and tightly packed, as a PNG.
///
/// # Panics
///
/// If `rgb` is not exactly `width * height * 3` bytes. A picture whose buffer
/// does not match its stated size is a caller bug, and encoding it anyway would
/// write a file that opens and is wrong.
pub fn encode(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgb.len(),
        (width as usize) * (height as usize) * 3,
        "a {width}×{height} RGB picture is {} bytes, not {}",
        (width as usize) * (height as usize) * 3,
        rgb.len(),
    );

    // Each scanline is preceded by its filter byte. Filter 0 — none: the rows
    // are dumps of a rendered frame and a predictor buys nothing without a
    // compressor behind it.
    let mut raw = Vec::with_capacity(rgb.len() + height as usize);
    for row in rgb.chunks_exact((width as usize) * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    let mut out = Vec::from(SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // 8 bits a channel, colour type 2 (truecolour), the only compression and
    // filter methods the format defines, not interlaced.
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The same for `RGBA8` pixels, which is what every frame in this crate is read
/// back as. The alpha is dropped rather than written: a dump is looked at, and
/// a viewer showing a debug frame over its own checkerboard is a viewer showing
/// something other than the frame.
///
/// # Panics
///
/// If `rgba` is not exactly `width * height * 4` bytes.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        (width as usize) * (height as usize) * 4,
        "a {width}×{height} RGBA picture is {} bytes, not {}",
        (width as usize) * (height as usize) * 4,
        rgba.len(),
    );
    let mut rgb = Vec::with_capacity((width as usize) * (height as usize) * 3);
    for pixel in rgba.as_chunks::<4>().0 {
        rgb.extend_from_slice(&pixel[..3]);
    }
    encode(width, height, &rgb)
}

/// Encode and write, since every caller here does both and a dump that failed
/// to open should say which file it was.
pub fn write(path: &std::path::Path, width: u32, height: u32, rgb: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, encode(width, height, rgb))
}

/// The grey rule between two strips of [`write_strips`], and how many pixels
/// wide it is.
const RULE: [u8; 3] = [64, 64, 64];
const RULE_WIDTH: u32 = 2;

/// Several equally sized RGB pictures written side by side as one file,
/// separated by a thin grey rule.
///
/// **The comparison and the picture want to be one file.** Two files at the
/// same scale still need a person to align them, and what is being read across
/// a comparison like this is a difference of a few pixels in the position of an
/// edge — which is exactly what alignment error looks like.
///
/// # Panics
///
/// If `strips` is empty, or if any of them is not `width * height * 3` bytes.
pub fn write_strips(
    path: &std::path::Path,
    width: u32,
    height: u32,
    strips: &[&[u8]],
) -> std::io::Result<()> {
    assert!(!strips.is_empty(), "a comparison of nothing is not a picture");
    let row_bytes = (width as usize) * 3;
    for (index, strip) in strips.iter().enumerate() {
        assert_eq!(
            strip.len(),
            row_bytes * height as usize,
            "strip {index} is not {width}×{height}",
        );
    }

    let count = strips.len() as u32;
    let total = width * count + RULE_WIDTH * (count - 1);
    let mut rgb = Vec::with_capacity((total as usize) * (height as usize) * 3);
    for row in 0..height as usize {
        for (index, strip) in strips.iter().enumerate() {
            rgb.extend_from_slice(&strip[row * row_bytes..(row + 1) * row_bytes]);
            if index + 1 < strips.len() {
                for _ in 0..RULE_WIDTH {
                    rgb.extend_from_slice(&RULE);
                }
            }
        }
    }
    write(path, total, height, &rgb)
}

/// One PNG chunk: length, type, data, and the CRC over type and data.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc::new();
    crc.eat(kind);
    crc.eat(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// `data` as a zlib stream of stored — uncompressed — deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    // Deflate, 32 KiB window, no preset dictionary, fastest compression level.
    // The two header bytes must be a multiple of 31 read big-endian, which this
    // pair is; it is the conventional one.
    let mut out = vec![0x78, 0x01];

    // A stored block carries `LEN` and its ones' complement, so it cannot be
    // longer than `MAX_STORED`. An empty input still needs one final block, or
    // the stream ends without ever setting `BFINAL`.
    let mut blocks = data.chunks(MAX_STORED).peekable();
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none()));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Adler-32, zlib's own checksum of the *uncompressed* data.
fn adler32(data: &[u8]) -> u32 {
    // The largest prime below 65536, which is what makes the running sums fit.
    const BASE: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + u32::from(*byte)) % BASE;
        b = (b + a) % BASE;
    }
    (b << 16) | a
}

/// CRC-32, PNG's own — the ordinary reflected polynomial, table built once per
/// instance because a dump writes four chunks and a static table would need a
/// lock or a `OnceLock` for no measurable gain.
struct Crc {
    value: u32,
}

impl Crc {
    fn new() -> Self {
        Self { value: 0xFFFF_FFFF }
    }

    fn eat(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.value ^= u32::from(*byte);
            for _ in 0..8 {
                // The reflected form of 0x04C11DB7, which is what PNG specifies.
                let carry = self.value & 1;
                self.value >>= 1;
                if carry != 0 {
                    self.value ^= 0xEDB8_8320;
                }
            }
        }
    }

    fn finish(self) -> u32 {
        self.value ^ 0xFFFF_FFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chunk layout is what a decoder walks, so walk it: signature, then
    /// length-prefixed chunks in order, and the last one is `IEND`.
    #[test]
    fn a_picture_encodes_as_ihdr_idat_iend() {
        let encoded = encode(2, 2, &[0u8; 12]);
        assert_eq!(&encoded[..8], &SIGNATURE);

        let mut at = 8;
        let mut kinds = Vec::new();
        while at < encoded.len() {
            let len = u32::from_be_bytes(encoded[at..at + 4].try_into().unwrap()) as usize;
            let kind = String::from_utf8(encoded[at + 4..at + 8].to_vec()).unwrap();
            // The CRC covers the type and the data, and a decoder checks it —
            // so this test checks it too, or a wrong one would never be noticed
            // until somebody opened a file.
            let mut crc = Crc::new();
            crc.eat(&encoded[at + 4..at + 8 + len]);
            let stated = u32::from_be_bytes(encoded[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(crc.finish(), stated, "the CRC of {kind} is wrong");
            kinds.push(kind);
            at += 12 + len;
        }
        assert_eq!(kinds, ["IHDR", "IDAT", "IEND"]);
        assert_eq!(at, encoded.len(), "a chunk runs past the end of the file");
    }

    /// The `IHDR` says what was asked for. A picture that decodes as the wrong
    /// size is the one defect here that produces a file that still opens.
    #[test]
    fn the_header_states_the_size_and_the_format() {
        let encoded = encode(7, 3, &[0u8; 63]);
        let ihdr = &encoded[16..29];
        assert_eq!(u32::from_be_bytes(ihdr[0..4].try_into().unwrap()), 7);
        assert_eq!(u32::from_be_bytes(ihdr[4..8].try_into().unwrap()), 3);
        // Eight bits, truecolour, the only defined compression and filter
        // methods, no interlace.
        assert_eq!(&ihdr[8..], &[8, 2, 0, 0, 0]);
    }

    /// Adler-32 against the value the zlib specification's own worked example
    /// gives, so the checksum is pinned to something outside this file.
    #[test]
    fn the_checksum_is_zlibs_own() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        // The empty stream's checksum is 1, not 0 — `a` starts at one, and a
        // zero here is the classic off-by-an-initial-value.
        assert_eq!(adler32(b""), 1);
    }

    /// CRC-32 against the standard's own check value: the digest of the nine
    /// ASCII digits is `0xCBF43926`.
    #[test]
    fn the_crc_is_the_standard_one() {
        let mut crc = Crc::new();
        crc.eat(b"123456789");
        assert_eq!(crc.finish(), 0xCBF4_3926);
    }

    /// A picture wider than one stored block still round-trips its bytes: the
    /// block loop is where a stored-deflate encoder gets its lengths wrong, and
    /// a 65 535-byte boundary is exactly where nothing smaller would show it.
    #[test]
    fn a_picture_larger_than_one_deflate_block_carries_every_row() {
        let (width, height) = (300u32, 300u32);
        let rgb: Vec<u8> = (0..width * height * 3).map(|byte| byte as u8).collect();
        let encoded = encode(width, height, &rgb);

        // Walk to the `IDAT`, undo the stored blocks, and check the scanlines
        // came back — filter byte and all.
        let mut at = 8;
        let idat = loop {
            let len = u32::from_be_bytes(encoded[at..at + 4].try_into().unwrap()) as usize;
            if &encoded[at + 4..at + 8] == b"IDAT" {
                break &encoded[at + 8..at + 8 + len];
            }
            at += 12 + len;
        };
        let mut raw = Vec::new();
        let mut cursor = 2;
        loop {
            let final_block = idat[cursor] == 1;
            let len = u16::from_le_bytes(idat[cursor + 1..cursor + 3].try_into().unwrap()) as usize;
            let nlen = u16::from_le_bytes(idat[cursor + 3..cursor + 5].try_into().unwrap());
            assert_eq!(
                nlen,
                !(len as u16),
                "a stored block's NLEN is not its LEN complemented"
            );
            raw.extend_from_slice(&idat[cursor + 5..cursor + 5 + len]);
            cursor += 5 + len;
            if final_block {
                break;
            }
        }
        assert_eq!(cursor + 4, idat.len(), "the checksum is not where the blocks end");

        assert_eq!(raw.len(), rgb.len() + height as usize);
        for (row, line) in raw.chunks_exact((width as usize) * 3 + 1).enumerate() {
            assert_eq!(line[0], 0, "row {row} does not say it is unfiltered");
            let start = row * (width as usize) * 3;
            assert_eq!(&line[1..], &rgb[start..start + (width as usize) * 3]);
        }
    }
}
