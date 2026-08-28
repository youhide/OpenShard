//! `0xD8` and `0xBF 0x1D` — a house whose shape this shard invented.
//!
//! Every other multi on the wire is an id: `0x4000 | id`, and the client draws
//! the hundred and forty-eight statics a villa is made of out of its own
//! `multi.mul`. A **designed** house has no id in that file — its shape was made
//! on this shard five minutes ago — so for exactly one kind of house the shard
//! owes the picture as well as the walls, and owes it as a packet. See
//! `docs/customisation.md`.
//!
//! Two packets, and the small one is the load-bearing one:
//!
//! - [`DesignRevision`] (`0xBF` subcommand `0x1D`) is thirteen bytes saying
//!   "house *S* is at revision *R*". A client that already holds *R* draws what
//!   it cached and asks for nothing. Without it, every client walking into a
//!   neighbourhood re-fetches every design in it on every approach.
//! - [`DesignDetail`] (`0xD8`) is the design itself, deflated.
//!
//! # `0xD8`'s layout, and why it is shaped like that
//!
//! ```text
//! 0xD8                     u8    packet id
//! length                   u16   whole packet, framed
//! 0x03                     u8    compression type; only this one exists
//! response                 u8    whether the client should acknowledge
//! serial                   u32   the house
//! revision                 u32   what DesignRevision announces
//! tile count               u16   before any bucketing
//! buffer length            u16   the plane count byte, plus every plane
//! plane count              u8
//! [plane...]
//! ```
//!
//! Each plane is a four-byte header and a zlib blob:
//!
//! ```text
//! index                    u8    0x20 | n for a grid plane, 9 + n for a stair buffer
//! inflated & 0xFF          u8
//! deflated & 0xFF          u8
//! (inflated >> 4) & 0xF0 | (deflated >> 8) & 0x0F
//! ```
//!
//! — so both lengths are twelve bits, sharing a nibble each in the fourth byte.
//!
//! The nine grid planes are a **sparse encoding by elevation**, and that is the
//! whole trick: a house's tiles cluster at five `dz` values (0, 7, 27, 47, 67 —
//! the storey heights), so each becomes a fixed-stride grid of `u16` graphics
//! with zero meaning "nothing here", and deflate erases the zeroes. Plane 0 is
//! the ground floor; planes 1–4 hold the *floor* tiles of the four storeys above
//! and 5–8 their *walls*, because floors and walls sit on grids of different
//! size — a floor is inset by one tile on each edge and a wall is not.
//!
//! Anything that does not fit — a `dz` off the storey ladder, a tile outside its
//! plane's grid, a plane index past `0x400` — falls into a **stair buffer**,
//! five bytes per tile written out longhand. It is the escape hatch that makes
//! the sparse half safe: no tile is ever dropped for being unusual, it is only
//! ever more expensive.
//!
//! # What this crate cannot know
//!
//! Which plane a tile belongs in depends on whether its graphic is a *floor*,
//! and that is `tiledata`'s height field — a client file, which this crate has
//! never read and must not start. So [`DesignDetail::encode`] takes the
//! predicate rather than the answer, and the caller (which holds a
//! `Terrain`) supplies it.
//!
//! Decoding needs the house's `width` and `height` for the same structural
//! reason: the grid stride is `height`, and no field on the wire carries it. A
//! real client reads it from the foundation's own multi. See
//! [`DesignDetail::decode`].

use crate::codec::{PacketReader, PacketWriter};
use crate::error::{DecodeError, expect_id};
use crate::packet::{DecodePacket, EncodePacket, PacketLength, frame_body};
use crate::serial::RawSerial;
use crate::version::ClientVersion;
use crate::wire::Graphic;

/// Which version of a house's design this is.
///
/// Its own type rather than a `u32` because it is a **cache key on the client**
/// and it travels beside a serial in both packets that carry it — two opaque
/// four-byte numbers about the same house, which is exactly the pair
/// `docs/protocol_newtypes.md` exists to stop being swapped. Nothing compares
/// two of them for order: a client either holds this revision or it does not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Revision(pub u32);

/// One tile of a design: a static, at an offset from the house's origin.
///
/// The offsets are `i8` because the wire's stair buffer writes them as single
/// bytes, so nothing larger could survive a round trip. A house is at most
/// eighteen tiles across, which is what makes that a description rather than a
/// limit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DesignTile {
    /// The static's art id.
    pub graphic: Graphic,
    /// East, from the house's origin.
    pub dx: i8,
    /// South, from the house's origin.
    pub dy: i8,
    /// Up, from the ground the house stands on.
    pub dz: i8,
}

/// `0xBF` subcommand `0x1D` — "this house is at this revision".
///
/// Sent to every client that can see the house, whenever the design commits.
/// What a client does with it is a cache lookup: if it already holds this
/// `(serial, revision)` it draws what it has and asks for nothing, and only a
/// miss costs a [`DesignDetail`].
///
/// This engine sends it and our own client reads it; the classic client has
/// spoken it since 4.0.0a. It is deliberately *not* an
/// [`ExtendedRequest`](crate::extended::ExtendedRequest) variant, which is the
/// client-to-server direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DesignRevision {
    /// The house.
    pub serial: RawSerial,
    /// What its design is at now.
    pub revision: Revision,
}

impl DesignRevision {
    /// The packet id — the extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = 0x1D;
    /// The whole framed packet, id and length included.
    pub const LENGTH_BYTES: u8 = 13;

    /// Encode the whole packet.
    ///
    /// [`EncodePacket`] rather than a bespoke writer, so this rides the same
    /// framing every other packet does and the client can read it back out of
    /// [`ServerPacket`](crate::server_packet::ServerPacket) rather than out of a
    /// second reader of its own.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        crate::packet::encode_packet(&self, ClientVersion::new(4, 0, 0, 0))
    }
}

/// Fixed despite living under `0xBF`, [`CloseGump`](crate::gump::CloseGump)'s
/// reason exactly: the body never varies, so the constant is written by hand
/// because `frame_body` only back-patches a length for
/// [`PacketLength::Variable`].
impl EncodePacket for DesignRevision {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Fixed(13);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(u16::from(Self::LENGTH_BYTES));
        out.u16(Self::SUBCOMMAND);
        out.u32(self.serial.0);
        out.u32(self.revision.0);
    }
}

impl DecodePacket for DesignRevision {
    const ID: u8 = 0xBF;

    /// The reader is past the length, so the body starts at the subcommand —
    /// which is what makes every `0xBF` body uniform. Refuses a different
    /// subcommand rather than reading its body as this one: the bytes would
    /// decode and every field would be wrong.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a design revision",
                value: u32::from(subcommand),
            });
        }
        Ok(Self {
            serial: RawSerial(reader.u32()?),
            revision: Revision(reader.u32()?),
        })
    }
}

/// `0xBF` subcommand `0x1E` — "send me that house's design".
///
/// The middle of the three-packet conversation, and the reason
/// [`DesignRevision`] is worth sending at all: the shard announces a revision,
/// a client that does not hold it asks with this, and only then does a
/// [`DesignDetail`] go out. A client that already has the picture sends nothing
/// and costs nothing.
///
/// The reference registers it as an ordinary extended command —
/// `PacketHandlers.RegisterExtended(0x1E, true, QueryDesignDetails)` in
/// `HouseFoundation.cs` — whose body is one serial and nothing else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DesignDetailsRequest {
    /// The house being asked about.
    pub serial: RawSerial,
}

impl DesignDetailsRequest {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = 0x1E;

    /// Read the body, `reader` already past the id, length and subcommand.
    ///
    /// The serial is not validated here: whether it names a house this player
    /// may see is the seam that acts on the request, not the decoder's — the
    /// same split [`StatLockRequest`](crate::mobile::StatLockRequest) makes.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            serial: RawSerial(reader.u32()?),
        })
    }

    /// Encode the whole packet. Our own client sends this; the shard only ever
    /// decodes it — [`UnicodeTalkRequest`](crate::speech::UnicodeTalkRequest)'s
    /// split.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        frame_body(0xBF, PacketLength::Variable, |out: &mut PacketWriter| {
            out.u16(Self::SUBCOMMAND);
            out.u32(self.serial.0);
        })
    }
}

/// The five `dz` values a storey sits at. A tile at any other elevation is a
/// stair tile — which is what the reference calls them, and where the buffer
/// gets its name.
const STOREY_HEIGHTS: [i8; 5] = [0, 7, 27, 47, 67];

/// How many tiles one stair buffer holds before another is started. The
/// reference's number, and it is what keeps a buffer's inflated length inside
/// the twelve bits the plane header gives it: 750 × 5 = 3750 < 4096.
const MAX_TILES_PER_STAIR_BUFFER: usize = 750;

/// The largest byte offset a grid plane will address. A tile that would land
/// past it goes to a stair buffer instead.
const MAX_PLANE_OFFSET: usize = 0x400;

/// How hard to deflate. The reference asks zlib for its default, which is 6.
const DEFLATE_LEVEL: u8 = 6;

/// `0xD8` — the design itself.
///
/// Borrowing rather than owning its tiles: it is built to be written once, from
/// a design that already lives on the house.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DesignDetail<'a> {
    /// The house.
    pub serial: RawSerial,
    /// Which revision this is the shape of. The same number
    /// [`DesignRevision`] announces, and the client's cache key.
    pub revision: Revision,
    /// Whether the client should acknowledge. Set when the design was sent
    /// because the client asked; clear when the shard volunteered it.
    pub response: bool,
    /// Every tile of the design, in any order.
    pub tiles: &'a [DesignTile],
}

/// A design read back off the wire.
///
/// Owns its tiles, unlike [`DesignDetail`]: they were reconstructed out of nine
/// grids and there was nothing to borrow them from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Design {
    /// The house.
    pub serial: RawSerial,
    /// Which revision this is the shape of.
    pub revision: Revision,
    /// Whether the shard asked to be acknowledged.
    pub response: bool,
    /// Every tile, in the order the planes gave them up — which is not the
    /// order they were sent in. A design is a set.
    pub tiles: Vec<DesignTile>,
}

/// The grid a design's tiles are laid out on, in tiles.
///
/// Not on the wire, and it has to be known by both ends: it is the stride the
/// grid planes are indexed with. The sender derives it from the tiles; a
/// receiver reads it off the foundation's own multi.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DesignBounds {
    /// The westmost `dx` any tile has.
    pub x_min: i8,
    /// The northmost `dy`.
    pub y_min: i8,
    /// How many tiles across.
    pub width: usize,
    /// How many tiles down. The grid stride.
    pub height: usize,
}

impl DesignBounds {
    /// The box a set of tiles occupies, or `None` for no tiles at all.
    #[must_use]
    pub fn of(tiles: &[DesignTile]) -> Option<Self> {
        let x_min = tiles.iter().map(|tile| tile.dx).min()?;
        let y_min = tiles.iter().map(|tile| tile.dy).min()?;
        let x_max = tiles.iter().map(|tile| tile.dx).max()?;
        let y_max = tiles.iter().map(|tile| tile.dy).max()?;
        Some(Self {
            x_min,
            y_min,
            width: usize::from(x_max.abs_diff(x_min)) + 1,
            height: usize::from(y_max.abs_diff(y_min)) + 1,
        })
    }

    /// How many bytes plane `index` covers when it is full.
    ///
    /// Zero when the house is too small for that plane to have any cells at
    /// all, which is not an error: every tile that would have gone there ends
    /// up in a stair buffer, because the emptiness makes the bounds check fail.
    const fn plane_bytes(self, index: usize) -> usize {
        let cells = if index == 0 {
            self.width * self.height
        } else if index < 5 {
            self.width.saturating_sub(1) * self.height.saturating_sub(2)
        } else {
            self.width * self.height.saturating_sub(1)
        };
        cells * 2
    }

    /// The stride plane `index` is addressed with — how many cells one column
    /// holds.
    const fn plane_stride(self, index: usize) -> usize {
        if index == 0 {
            self.height
        } else if index < 5 {
            self.height.saturating_sub(2)
        } else {
            self.height.saturating_sub(1)
        }
    }
}

/// Where one tile goes: a cell in a grid plane, or the stair buffer.
enum Slot {
    Grid { plane: usize, offset: usize },
    Stairs,
}

/// Which plane and cell a tile belongs in, or that it does not fit one.
///
/// The whole of the reference's bucketing, in one place so that the encoder and
/// the tests agree by construction. `floor` is the caller's answer about the
/// graphic — see the module header for why this crate cannot work it out.
fn slot_of(tile: DesignTile, bounds: DesignBounds, floor: bool) -> Slot {
    // The classic client takes a foundation's physical platform as its ground
    // layer and does not reliably apply D8 plane 0 over it. Imported designs
    // use that layer for real slate floors (often more than one static at one
    // cell), while the explicit five-byte stair entries render them exactly.
    // Keep every z=0 component in that longhand form rather than silently
    // losing the floor beneath the foundation.
    if tile.dz == 0 {
        return Slot::Stairs;
    }
    let Some(storey) = STOREY_HEIGHTS.iter().position(|&z| z == tile.dz) else {
        return Slot::Stairs;
    };
    let (plane, x, y) = if floor {
        (
            storey,
            i32::from(tile.dx) - i32::from(bounds.x_min) - 1,
            i32::from(tile.dy) - i32::from(bounds.y_min) - 1,
        )
    } else {
        (
            storey + 4,
            i32::from(tile.dx) - i32::from(bounds.x_min),
            i32::from(tile.dy) - i32::from(bounds.y_min),
        )
    };
    let stride = bounds.plane_stride(plane);
    if x < 0 || y < 0 {
        return Slot::Stairs;
    }
    let Ok(stride) = i32::try_from(stride) else {
        return Slot::Stairs;
    };
    if y >= stride {
        return Slot::Stairs;
    }
    // Guard the multiply before it happens: an absurd bound would otherwise
    // wrap and address a cell that is not the tile's.
    let Some(offset) = x
        .checked_mul(stride)
        .and_then(|column| column.checked_add(y))
        .and_then(|cell| cell.checked_mul(2))
    else {
        return Slot::Stairs;
    };
    let offset = usize::try_from(offset).unwrap_or(MAX_PLANE_OFFSET);
    if offset + 1 >= MAX_PLANE_OFFSET {
        return Slot::Stairs;
    }
    Slot::Grid { plane, offset }
}

/// One plane, ready to be written: its index byte, what it was before deflating,
/// and what it is after.
struct Plane {
    index: u8,
    inflated: usize,
    deflated: Vec<u8>,
}

impl Plane {
    /// The four-byte header, with both twelve-bit lengths folded into it.
    fn header(&self) -> [u8; 4] {
        let inflated = self.inflated;
        let deflated = self.deflated.len();
        [
            self.index,
            u8::try_from(inflated & 0xFF).expect("the inflated length is masked to one byte"),
            u8::try_from(deflated & 0xFF).expect("the deflated length is masked to one byte"),
            u8::try_from(((inflated >> 4) & 0xF0) | ((deflated >> 8) & 0x0F))
                .expect("the folded length nibbles fit one byte"),
        ]
    }
}

impl DesignDetail<'_> {
    /// The packet id.
    pub const ID: u8 = 0xD8;
    /// The only compression type the protocol defines.
    pub const COMPRESSION: u8 = 0x03;

    /// Encode the whole packet.
    ///
    /// `is_floor` answers, for a graphic, whether `tiledata` gives it a height
    /// of zero — which is what decides between the floor planes and the wall
    /// planes. The module header says why it is a parameter.
    ///
    /// Unlike the reference, nothing here is written and then seeked back over:
    /// the planes are built first, so the plane count and the buffer length are
    /// known before a byte goes out. The wire is identical; the reference
    /// patches two fields at offsets 15 and 17 because its writer had already
    /// run.
    #[must_use]
    pub fn encode(&self, is_floor: impl Fn(Graphic) -> bool) -> Vec<u8> {
        let planes =
            DesignBounds::of(self.tiles).map_or_else(Vec::new, |bounds| self.planes(bounds, is_floor));
        self.encode_planes(planes)
    }

    /// Encode against the foundation grid the receiver will use.
    ///
    /// A design packet does not carry its grid's dimensions.  The classic
    /// client takes them from the placed foundation, so a shard must use those
    /// same bounds when it decides which tiles may use a compact grid plane.
    /// Tiles outside the foundation go through the longhand stair buffer and
    /// therefore remain exact rather than being decoded at a shifted x/y.
    #[must_use]
    pub fn encode_with_bounds(&self, bounds: DesignBounds, is_floor: impl Fn(Graphic) -> bool) -> Vec<u8> {
        self.encode_planes(self.planes(bounds, is_floor))
    }

    fn encode_planes(&self, planes: Vec<Plane>) -> Vec<u8> {
        // The plane-count byte, then four header bytes and a blob each.
        let buffer_length: usize = 1 + planes.iter().map(|plane| 4 + plane.deflated.len()).sum::<usize>();
        frame_body(Self::ID, PacketLength::Variable, |out: &mut PacketWriter| {
            out.u8(Self::COMPRESSION);
            out.bool(self.response);
            out.u32(self.serial.0);
            out.u32(self.revision.0);
            out.u16(u16::try_from(self.tiles.len()).unwrap_or(u16::MAX));
            out.u16(u16::try_from(buffer_length).unwrap_or(u16::MAX));
            out.u8(u8::try_from(planes.len()).unwrap_or(u8::MAX));
            for plane in &planes {
                out.bytes(&plane.header());
                out.bytes(&plane.deflated);
            }
        })
    }

    /// Bucket the tiles and deflate each bucket that got anything.
    fn planes(&self, bounds: DesignBounds, is_floor: impl Fn(Graphic) -> bool) -> Vec<Plane> {
        let mut grids: Vec<Vec<u8>> = (0..9).map(|index| vec![0u8; bounds.plane_bytes(index)]).collect();
        // A compact grid cell names one graphic, but a real house design may
        // layer a railing, wall trim or other decorative static at the very
        // same `(x, y, z)`. Keep its first tile in the compact plane and send
        // every overlap through the longhand buffer, whose entries have no
        // one-per-cell limit.
        let mut occupied: Vec<Vec<bool>> = grids.iter().map(|grid| vec![false; grid.len() / 2]).collect();
        let mut used = [false; 9];
        let mut stairs: Vec<u8> = Vec::new();

        for &tile in self.tiles {
            match slot_of(tile, bounds, is_floor(tile.graphic)) {
                Slot::Grid { plane, offset }
                    if offset + 1 < grids[plane].len() && !occupied[plane][offset / 2] =>
                {
                    used[plane] = true;
                    occupied[plane][offset / 2] = true;
                    let [high, low] = tile.graphic.0.to_be_bytes();
                    grids[plane][offset] = high;
                    grids[plane][offset + 1] = low;
                }
                // Either it never fitted, or the plane is too small to hold the
                // cell the arithmetic named. Both are the stair buffer's job.
                _ => {
                    stairs.extend_from_slice(&tile.graphic.0.to_be_bytes());
                    stairs.push(tile.dx.to_le_bytes()[0]);
                    stairs.push(tile.dy.to_le_bytes()[0]);
                    stairs.push(tile.dz.to_le_bytes()[0]);
                }
            }
        }

        let mut planes = Vec::new();
        for (index, grid) in grids.into_iter().enumerate() {
            if !used[index] {
                continue;
            }
            planes.push(deflate_plane(0x20 | u8::try_from(index).unwrap_or(0), &grid));
        }
        for (index, chunk) in stairs.chunks(MAX_TILES_PER_STAIR_BUFFER * 5).enumerate() {
            planes.push(deflate_plane(9 + u8::try_from(index).unwrap_or(0), chunk));
        }
        planes
    }

    /// Read a design back.
    ///
    /// `bounds` is the grid the sender laid the planes out on, and it has to
    /// come from somewhere other than this packet — see the module header. Only
    /// `width` and `height` are read from it for the grid planes; `x_min` and
    /// `y_min` put the tiles back where they started.
    ///
    /// A plane whose blob will not inflate ends the read rather than being
    /// skipped: a design missing one storey is not a design, and half a house
    /// drawn as if it were whole is the failure this whole packet exists to
    /// avoid.
    pub fn decode(bytes: &[u8], bounds: DesignBounds) -> Result<Design, DecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        let compression = reader.u8()?;
        if compression != Self::COMPRESSION {
            return Err(DecodeError::Unsupported {
                packet: Self::ID,
                form: "a compression type other than zlib",
            });
        }
        let response = reader.bool()?;
        let serial = RawSerial(reader.u32()?);
        let revision = Revision(reader.u32()?);
        let _tile_count = reader.u16()?;
        let _buffer_length = reader.u16()?;
        let plane_count = reader.u8()?;

        let mut tiles = Vec::new();
        for _ in 0..plane_count {
            read_plane(&mut reader, bounds, &mut tiles)?;
        }
        Ok(Design {
            serial,
            revision,
            response,
            tiles,
        })
    }
}

/// Deflate one buffer and label it.
fn deflate_plane(index: u8, inflated: &[u8]) -> Plane {
    Plane {
        index,
        inflated: inflated.len(),
        deflated: miniz_oxide::deflate::compress_to_vec_zlib(inflated, DEFLATE_LEVEL),
    }
}

/// Read one plane's header and blob, and append whatever tiles it held.
fn read_plane(
    reader: &mut PacketReader<'_>,
    bounds: DesignBounds,
    tiles: &mut Vec<DesignTile>,
) -> Result<(), DecodeError> {
    let index = reader.u8()?;
    let inflated_low = reader.u8()?;
    let deflated_low = reader.u8()?;
    let shared = reader.u8()?;
    let inflated = usize::from(inflated_low) | (usize::from(shared & 0xF0) << 4);
    let deflated = usize::from(deflated_low) | (usize::from(shared & 0x0F) << 8);
    let blob = reader.bytes(deflated)?;
    let buffer = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(blob, inflated).map_err(|_| {
        DecodeError::UnknownValue {
            field: "design plane",
            value: u32::from(index),
        }
    })?;

    if index < 0x20 {
        // A stair buffer: five bytes a tile, written out longhand.
        for entry in buffer.chunks_exact(5) {
            tiles.push(DesignTile {
                graphic: Graphic(u16::from_be_bytes([entry[0], entry[1]])),
                dx: entry[2] as i8,
                dy: entry[3] as i8,
                dz: entry[4] as i8,
            });
        }
        return Ok(());
    }

    let plane = usize::from(index & 0x1F);
    if plane >= 9 {
        return Err(DecodeError::UnknownValue {
            field: "design plane index",
            value: u32::from(index),
        });
    }
    let stride = bounds.plane_stride(plane);
    if stride == 0 {
        // A plane with no cells cannot have held anything, and dividing by its
        // stride to find out would be worse than saying so.
        return Ok(());
    }
    // Planes 1-4 are the inset floor grids; 5-8 are the walls, on the full one.
    let (dz, inset) = if plane == 0 {
        (0i8, 0i32)
    } else if plane < 5 {
        (STOREY_HEIGHTS[plane], 1)
    } else {
        (STOREY_HEIGHTS[plane - 4], 0)
    };
    for (cell, graphic) in buffer.chunks_exact(2).enumerate() {
        let graphic = Graphic(u16::from_be_bytes([graphic[0], graphic[1]]));
        if graphic.0 == 0 {
            continue;
        }
        let Ok(cell) = i32::try_from(cell) else { continue };
        let Ok(stride) = i32::try_from(stride) else {
            continue;
        };
        let x = cell / stride + inset + i32::from(bounds.x_min);
        let y = cell % stride + inset + i32::from(bounds.y_min);
        let (Ok(dx), Ok(dy)) = (i8::try_from(x), i8::try_from(y)) else {
            continue;
        };
        tiles.push(DesignTile { graphic, dx, dy, dz });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything below a wall's height is a floor, which is what `tiledata`
    /// says for the graphics a floor is drawn with.
    fn floors_below(cut: u16) -> impl Fn(Graphic) -> bool {
        move |graphic| graphic.0 < cut
    }

    fn tile(graphic: u16, dx: i8, dy: i8, dz: i8) -> DesignTile {
        DesignTile {
            graphic: Graphic(graphic),
            dx,
            dy,
            dz,
        }
    }

    /// A design as read back, sorted, so a round trip can be compared without
    /// caring which plane gave a tile up first.
    fn sorted(mut tiles: Vec<DesignTile>) -> Vec<DesignTile> {
        tiles.sort_by_key(|tile| (tile.dz, tile.dx, tile.dy, tile.graphic.0));
        tiles
    }

    /// The request the shard answers with a `0xD8`, round-tripped through the
    /// one `0xBF` envelope decoder rather than through a second reader of its
    /// own — which is the whole point of `ExtendedRequest`.
    #[test]
    fn a_design_request_reads_back_through_the_extended_envelope() {
        use crate::extended::ExtendedRequest;

        let bytes = DesignDetailsRequest {
            serial: RawSerial(0x4000_0123),
        }
        .encode();
        assert_eq!(bytes.len(), 9, "id, length, subcommand and one serial");
        assert_eq!(&bytes[..5], &[0xBF, 0x00, 0x09, 0x00, 0x1E]);
        assert_eq!(
            ExtendedRequest::decode(&bytes).unwrap(),
            ExtendedRequest::DesignDetails(DesignDetailsRequest {
                serial: RawSerial(0x4000_0123),
            }),
        );
    }

    /// A truncated request is refused rather than read as a serial with its top
    /// bytes missing — which would name a different house.
    #[test]
    fn a_truncated_design_request_is_refused_not_panicked() {
        use crate::extended::ExtendedRequest;

        let full = DesignDetailsRequest {
            serial: RawSerial(0x4000_0123),
        }
        .encode();
        for cut in 0..full.len() {
            assert!(
                ExtendedRequest::decode(&full[..cut]).is_err(),
                "a {cut}-byte packet must not decode"
            );
        }
    }

    #[test]
    fn a_revision_is_thirteen_bytes_and_reads_back() {
        let sent = DesignRevision {
            serial: RawSerial(0x4000_0123),
            revision: Revision(0xDEAD_BEEF),
        };
        let bytes = sent.encode();
        assert_eq!(
            bytes.len(),
            usize::from(DesignRevision::LENGTH_BYTES),
            "the length the classic client expects is fixed"
        );
        assert_eq!(&bytes[..5], &[0xBF, 0x00, 0x0D, 0x00, 0x1D]);
        assert_eq!(
            crate::packet::decode_packet::<DesignRevision>(&bytes, ClientVersion::new(4, 0, 0, 0)).unwrap(),
            sent
        );
    }

    /// A different `0xBF` is refused rather than read as this one: its body
    /// would decode and every field would be wrong.
    #[test]
    fn another_extended_subcommand_is_not_a_revision() {
        let mut bytes = DesignRevision {
            serial: RawSerial(1),
            revision: Revision(1),
        }
        .encode();
        bytes[4] = 0x1C;
        assert!(
            crate::packet::decode_packet::<DesignRevision>(&bytes, ClientVersion::new(4, 0, 0, 0)).is_err()
        );
    }

    #[test]
    fn a_truncated_revision_is_refused_not_panicked() {
        let full = DesignRevision {
            serial: RawSerial(7),
            revision: Revision(9),
        }
        .encode();
        for cut in 0..full.len() {
            assert!(
                crate::packet::decode_packet::<DesignRevision>(&full[..cut], ClientVersion::new(4, 0, 0, 0))
                    .is_err(),
                "a {cut}-byte packet must not decode"
            );
        }
    }

    /// The ground floor, which is plane 0 and needs no inset.
    #[test]
    fn a_ground_floor_survives_a_round_trip() {
        let tiles = vec![
            tile(0x0006, 0, 0, 0),
            tile(0x0007, 1, 0, 0),
            tile(0x0008, 0, 2, 0),
            tile(0x0009, 2, 2, 0),
        ];
        let detail = DesignDetail {
            serial: RawSerial(0x4000_0001),
            revision: Revision(4),
            response: true,
            tiles: &tiles,
        };
        let bytes = detail.encode(floors_below(0x1000));
        let bounds = DesignBounds::of(&tiles).unwrap();
        let back = DesignDetail::decode(&bytes, bounds).unwrap();

        assert_eq!(back.serial, RawSerial(0x4000_0001));
        assert_eq!(back.revision, Revision(4));
        assert!(back.response, "the response flag is a field, not a constant");
        assert_eq!(sorted(back.tiles), sorted(tiles));
    }

    /// Floors and walls of the same storey go to different planes, and both
    /// come back at the same `dz`. This is the one the inset gets wrong if the
    /// two grids are confused.
    #[test]
    fn a_storey_splits_into_a_floor_plane_and_a_wall_plane_and_rejoins() {
        let tiles = vec![
            // Inside the inset grid, so it can be a floor.
            tile(0x0010, 2, 2, 7),
            tile(0x0011, 3, 3, 7),
            // Above the cut, so a wall.
            tile(0x2000, 0, 0, 7),
            tile(0x2001, 4, 4, 7),
        ];
        let detail = DesignDetail {
            serial: RawSerial(2),
            revision: Revision(1),
            response: false,
            tiles: &tiles,
        };
        let bytes = detail.encode(floors_below(0x1000));
        let bounds = DesignBounds::of(&tiles).unwrap();
        let back = DesignDetail::decode(&bytes, bounds).unwrap();
        assert_eq!(sorted(back.tiles), sorted(tiles));
    }

    /// A `dz` off the storey ladder has no plane at all, so it takes the
    /// escape hatch — and comes back identical, which is the point of having
    /// one.
    #[test]
    fn a_tile_at_an_unusual_height_goes_through_the_stair_buffer() {
        let tiles = vec![tile(0x0020, 1, 1, 0), tile(0x0021, 2, 3, 13)];
        let detail = DesignDetail {
            serial: RawSerial(3),
            revision: Revision(2),
            response: false,
            tiles: &tiles,
        };
        let bytes = detail.encode(floors_below(0x1000));
        let bounds = DesignBounds::of(&tiles).unwrap();
        let back = DesignDetail::decode(&bytes, bounds).unwrap();
        assert_eq!(sorted(back.tiles), sorted(tiles));
    }

    /// A grid plane is a single graphic per cell, while legacy builds often
    /// layer decorative rails and trim over a wall on that same cell. The
    /// compact representation must not silently overwrite either one.
    #[test]
    fn overlapping_tiles_survive_through_the_stair_buffer() {
        let tiles = vec![tile(0x0030, 1, 1, 0), tile(0x0031, 1, 1, 0)];
        let detail = DesignDetail {
            serial: RawSerial(3),
            revision: Revision(2),
            response: false,
            tiles: &tiles,
        };
        let bytes = detail.encode(floors_below(0x1000));
        let bounds = DesignBounds::of(&tiles).unwrap();
        let back = DesignDetail::decode(&bytes, bounds).unwrap();
        assert_eq!(sorted(back.tiles), sorted(tiles));
    }

    /// Negative offsets are ordinary: a house's origin is its centre, not its
    /// corner, so roughly half of every design is west or north of it.
    #[test]
    fn offsets_west_and_north_of_the_origin_survive() {
        let tiles = vec![
            tile(0x0030, -3, -3, 0),
            tile(0x0031, -1, 2, 0),
            tile(0x0032, 3, -2, 0),
            tile(0x2032, -3, -3, 27),
        ];
        let detail = DesignDetail {
            serial: RawSerial(4),
            revision: Revision(3),
            response: false,
            tiles: &tiles,
        };
        let bytes = detail.encode(floors_below(0x1000));
        let bounds = DesignBounds::of(&tiles).unwrap();
        let back = DesignDetail::decode(&bytes, bounds).unwrap();
        assert_eq!(sorted(back.tiles), sorted(tiles));
    }

    #[test]
    fn a_design_larger_than_its_foundation_keeps_its_exact_offsets() {
        let tiles = vec![
            tile(0x0030, -1, -1, 0),
            // This wall cannot occupy the foundation's grid, so it must travel
            // in the longhand buffer rather than be decoded with a shifted
            // plane stride.
            tile(0x2032, 8, 1, 7),
        ];
        let foundation = DesignBounds {
            x_min: -2,
            y_min: -2,
            width: 5,
            height: 5,
        };
        let bytes = DesignDetail {
            serial: RawSerial(5),
            revision: Revision(1),
            response: false,
            tiles: &tiles,
        }
        .encode_with_bounds(foundation, floors_below(0x1000));
        let back = DesignDetail::decode(&bytes, foundation).unwrap();
        assert_eq!(sorted(back.tiles), sorted(tiles));
    }

    /// The grid belongs to the foundation, not to whatever bounding box a
    /// particular design happened to occupy.  This is the 12×12 custom-house
    /// case: floor tiles and roof tiles occupy the same x/y range, but live in
    /// plane 0 and plane 8 respectively. A one-column stride mismatch used to
    /// wrap their middle columns into unrelated map rows in the classic client.
    #[test]
    fn a_full_foundation_keeps_its_floor_and_roof_rows() {
        let foundation = DesignBounds {
            x_min: -5,
            y_min: -5,
            width: 12,
            height: 12,
        };
        let mut tiles = Vec::new();
        for y in -5..=6 {
            tiles.push(tile(0x049C, -5, y, 0));
            tiles.push(tile(0x049C, 6, y, 0));
            tiles.push(tile(0x0597, -5, y, 47));
            tiles.push(tile(0x0597, 6, y, 47));
        }
        let bytes = DesignDetail {
            serial: RawSerial(6),
            revision: Revision(1),
            response: true,
            tiles: &tiles,
        }
        .encode_with_bounds(foundation, |graphic| graphic.0 == 0x049C);

        let back = DesignDetail::decode(&bytes, foundation).expect("the foundation-sized packet decodes");
        assert_eq!(sorted(back.tiles), sorted(tiles));
    }

    /// The header the classic client parses, byte for byte, against a design
    /// small enough to read by eye.
    #[test]
    fn the_header_is_laid_out_where_the_client_looks_for_it() {
        let tiles = vec![tile(0x0006, 0, 0, 0)];
        let bytes = DesignDetail {
            serial: RawSerial(0x0000_ABCD),
            revision: Revision(0x0000_0007),
            response: true,
            tiles: &tiles,
        }
        .encode(floors_below(0x1000));

        assert_eq!(bytes[0], 0xD8);
        assert_eq!(
            u16::from_be_bytes([bytes[1], bytes[2]]) as usize,
            bytes.len(),
            "the framed length is the whole packet"
        );
        assert_eq!(bytes[3], 0x03, "compression type");
        assert_eq!(bytes[4], 0x01, "response flag");
        assert_eq!(
            u32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]),
            0xABCD
        );
        assert_eq!(u32::from_be_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]), 7);
        assert_eq!(u16::from_be_bytes([bytes[13], bytes[14]]), 1, "tile count");
        let buffer_length = usize::from(u16::from_be_bytes([bytes[15], bytes[16]]));
        assert_eq!(
            buffer_length,
            bytes.len() - 17,
            "the buffer length covers the plane count byte and every plane"
        );
        assert_eq!(bytes[17], 1, "one plane held the one tile");
        assert_eq!(bytes[18], 0x09, "ground tiles are explicit longhand entries");
    }

    /// Both twelve-bit lengths share the fourth header byte, one nibble each.
    /// A plane long enough to need the high bits is what proves the fold.
    #[test]
    fn a_plane_longer_than_a_byte_folds_its_length_into_the_shared_nibble() {
        let plane = Plane {
            index: 0x20,
            inflated: 0x123,
            deflated: vec![0; 0x45],
        };
        let header = plane.header();
        assert_eq!(header[1], 0x23, "the inflated length's low byte");
        assert_eq!(header[2], 0x45, "the deflated length's low byte");
        assert_eq!(
            header[3], 0x10,
            "high nibble is inflated's, low nibble deflated's"
        );

        let inflated = usize::from(header[1]) | (usize::from(header[3] & 0xF0) << 4);
        let deflated = usize::from(header[2]) | (usize::from(header[3] & 0x0F) << 8);
        assert_eq!((inflated, deflated), (0x123, 0x45), "and it unfolds again");
    }

    /// A design with nothing in it is a legal packet, not a panic. It is what a
    /// house being cleared in the editor sends.
    #[test]
    fn an_empty_design_encodes_no_planes() {
        let bytes = DesignDetail {
            serial: RawSerial(5),
            revision: Revision(0),
            response: false,
            tiles: &[],
        }
        .encode(floors_below(0x1000));
        assert_eq!(bytes[17], 0, "no planes");
        let bounds = DesignBounds {
            x_min: 0,
            y_min: 0,
            width: 1,
            height: 1,
        };
        assert!(DesignDetail::decode(&bytes, bounds).unwrap().tiles.is_empty());
    }

    /// A truncated `0xD8` is refused at whatever field ran out, and never
    /// panics — the same guarantee every other decoder in this crate gives.
    #[test]
    fn a_truncated_design_is_refused_not_panicked() {
        let tiles = vec![tile(0x0006, 0, 0, 0), tile(0x0007, 1, 1, 7)];
        let full = DesignDetail {
            serial: RawSerial(6),
            revision: Revision(1),
            response: false,
            tiles: &tiles,
        }
        .encode(floors_below(0x1000));
        let bounds = DesignBounds::of(&tiles).unwrap();
        for cut in 0..full.len() {
            let _ = DesignDetail::decode(&full[..cut], bounds);
        }
    }

    /// A blob that is not zlib fails the read rather than yielding a design
    /// with one storey missing.
    #[test]
    fn a_plane_that_will_not_inflate_fails_the_whole_design() {
        let tiles = vec![tile(0x0006, 0, 0, 0)];
        let mut bytes = DesignDetail {
            serial: RawSerial(7),
            revision: Revision(1),
            response: false,
            tiles: &tiles,
        }
        .encode(floors_below(0x1000));
        // Past the header and the plane's own four bytes: the blob itself.
        let blob = 22;
        bytes[blob] ^= 0xFF;
        let bounds = DesignBounds::of(&tiles).unwrap();
        assert!(DesignDetail::decode(&bytes, bounds).is_err());
    }
}
