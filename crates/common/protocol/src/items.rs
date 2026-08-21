//! Item packets: what the client is told about things on the ground, and what it
//! asks to do with them.
//!
//! A mobile and an item are drawn by different packets — `0x78` for a mobile,
//! `0x1A` for an item — but the interest machinery that decides *when* to draw
//! them is the same. This module is the item half of that, plus the two requests
//! a client makes about an item it can reach: `0x07` to pick it up and `0x08` to
//! put it down.

use crate::codec::{PacketReader, PacketWriter};
use crate::direction::Direction;
use crate::error::DecodeError;
use crate::feature::Feature;
use crate::gump::GumpPoint;
use crate::packet::{DecodePacket, EncodePacket, PacketLength};
use crate::serial::{RawSerial, Serial};
use crate::version::ClientVersion;
use crate::wire::{Graphic, Hue, Layer, RawLayer};
use crate::world::Point;

/// Whether `graphic` is one of the classic weapon graphics whose hand use is
/// known to both the shard and the client.  This small shared catalogue keeps
/// paperdoll preview from advertising a weapon combination the shard rejects;
/// custom graphics remain server-authoritative.
#[must_use]
pub const fn is_classic_weapon(graphic: Graphic) -> bool {
    matches!(
        graphic.0,
        0x0E81
            | 0x0E86
            | 0x0E87
            | 0x0E89
            | 0x0EC3
            | 0x0EC4
            | 0x0F43
            | 0x0F45
            | 0x0F47
            | 0x0F49
            | 0x0F4B
            | 0x0F4D
            | 0x0F50
            | 0x0F52
            | 0x0F5C
            | 0x0F5E
            | 0x0F61
            | 0x0F62
            | 0x13B0
            | 0x13B2
            | 0x13B4
            | 0x13B6
            | 0x13B9
            | 0x13F6
            | 0x13F8
            | 0x13FB
            | 0x13FD
            | 0x13FF
            | 0x1401
            | 0x1403
            | 0x1405
            | 0x1407
            | 0x1439
            | 0x143B
            | 0x143D
            | 0x143E
            | 0x1441
            | 0x1443
    )
}

/// How many units an item stack contains.
///
/// This is distinct from other `u16` quantities on the wire: a stack size can
/// be sent in a world-item packet, requested by a drag, or listed by a vendor,
/// but it is never a graphic id, a price, or a body id.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ItemAmount(pub u16);

impl ItemAmount {
    /// One of the thing — what a stack size is when nothing stacked.
    ///
    /// Named because it is written down often and because `0` is what
    /// [`Default`] gives: a single sword is one sword, and an amount of zero
    /// is a stack that is not there. Anything that draws a count reads more
    /// than one as "a pile" — see `openshard-client-render`'s
    /// `items::stack_label`.
    pub const ONE: Self = Self(1);
}

/// The value carried after a `WorldItem` graphic.
///
/// For ordinary items this is a stack size. UO reserves graphic `0x2006` as a
/// corpse marker; for that one graphic the same wire word is the dead mobile's
/// body graphic, which tells the client which death animation to draw.
///
/// # A corpse also faces somewhere
///
/// A body falls in the direction it was facing, and the client draws the last
/// frame of that body's death group *for a direction* — so a corpse without one
/// is a corpse pointing wherever the client last guessed. That is why the facing
/// lives in this variant rather than beside it: `0x1A` carries it in the
/// direction/light byte, which this engine sends for a corpse and for nothing
/// else, exactly as ServUO does (`Corpse.Light = (LightType)Direction`) and as
/// the client reads it back (`item.Layer = (Layer)direction` for `0x2006`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorldItemPayload {
    Stack(ItemAmount),
    Corpse {
        /// The dead mobile's body graphic — which death animation to draw.
        body: Graphic,
        /// Which way it fell. The run bit is never set: the client masks it off
        /// (`(byte)Layer & 0x7F & 7`) and a corpse does not run.
        facing: Direction,
    },
}

impl WorldItemPayload {
    const fn wire_value(self) -> u16 {
        match self {
            Self::Stack(amount) => amount.0,
            Self::Corpse { body, .. } => body.0,
        }
    }

    const fn follows_graphic(self) -> bool {
        match self {
            Self::Stack(amount) => amount.0 > 1,
            Self::Corpse { .. } => true,
        }
    }

    /// The direction byte to send after `y`, if this payload has one to send.
    ///
    /// `None` for a stack — an ordinary item has no facing, and the byte's other
    /// meaning (a light source's id) is not modelled here. `None` for a corpse
    /// facing north too, and that is the wire's own rule rather than a shortcut:
    /// north is `0`, ServUO writes the byte only when it is non-zero, and a
    /// client that reads no byte defaults to north. Sending a zero would be
    /// legal and identical; not sending it is what every shard the client has
    /// ever met does.
    const fn direction_byte(self) -> Option<u8> {
        match self {
            Self::Stack(_) => None,
            Self::Corpse { facing, .. } => match facing.to_bits() {
                0 => None,
                bits => Some(bits),
            },
        }
    }
}

/// The serial a `0x08` drop carries when the item is going onto the ground
/// rather than into a container or onto a mobile.
///
/// This packet's own sentinel, checked as itself rather than through
/// [`RawSerial::validate`]: `validate` says "addresses nothing", and `0` says
/// that too — but a `0` container is a confused client, and `0xFFFFFFFF` is the
/// floor. See `docs/protocol_newtypes.md` N3 amendment 4.
pub const DROP_TO_GROUND: RawSerial = RawSerial(0xFFFF_FFFF);

/// A light source's id, as `light.mul` numbers them.
///
/// What a shard sends to say that *this* item burns with *that* flame, rather
/// than leaving the client to decide from the graphic's `LightSource` tiledata
/// flag. Nothing in this engine sets one — our own client picks a flame by
/// graphic, since `light.mul` is not read yet — but a `0x1A` from a shard that
/// does is otherwise an item this client cannot read at all, which is the whole
/// reason the value is carried rather than skipped.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LightId(pub u8);

/// The client-side flags a `0x1A` may carry about an item.
///
/// Two bits, both ServUO's (`Item.GetPacketFlags`): `0x20` for an item the player
/// may drag, `0x80` for one it must not draw. A shard sends the byte whenever
/// either is set, which for ServUO is nearly every loose item on the ground —
/// so a decoder that refuses the byte refuses most of a real shard's world.
///
/// Absent and zero are the same statement: the byte is only written when it is
/// non-zero, so [`ItemFlags::NONE`] is what "no byte" means and not a stand-in
/// for one that failed to arrive.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct ItemFlags(pub u8);

impl ItemFlags {
    /// Nothing set — an ordinary item, and what an absent flags byte says.
    pub const NONE: Self = Self(0);
    /// `0x20` — the player may pick this up.
    pub const MOVABLE: Self = Self(0x20);
    /// `0x80` — the client must not draw it.
    pub const INVISIBLE: Self = Self(0x80);

    /// Whether every bit of `flag` is set here.
    #[must_use]
    pub const fn has(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }
}

/// The item graphic that makes a `WorldItem` carry a corpse body instead of a
/// stack amount.
pub const CORPSE_GRAPHIC: Graphic = Graphic(0x2006);

/// `0x1A` — draw an item on the ground the client has not seen. Variable length.
///
/// # The shape is a nest of optional fields
///
/// Ported from Sphere's `PacketItemWorld`, and it is the classic UO packet in
/// full awkwardness: which fields are present is encoded in flag bits stolen
/// from other fields, because in 1997 every byte counted.
///
/// - The top bit of the **serial** (`0x8000_0000`) means "a stack amount
///   follows the graphic". A single item does not set it and sends no amount.
/// - The top bit of **x** (`0x8000`) means "a direction or light byte follows",
///   and the byte itself comes *after* y, not after x. We send it only for a
///   corpse, whose facing rides in it — see [`WorldItemPayload`]. `x` itself is
///   15 bits.
/// - The top bit of **y** (`0x8000`) means "a hue word follows"; the next bit
///   (`0x4000`) means "a flags byte follows". `y` itself is 14 bits.
///
/// So a plain grey item is serial, graphic, x, y, z — and a hued stack of gold
/// is serial (with the amount bit), graphic, amount, x, y (with the hue bit), z,
/// hue. Sending a field whose flag bit is clear, or omitting one whose bit is
/// set, desynchronises the client mid-packet and every byte after is read as
/// something else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WorldItem {
    /// The item's wire serial.
    pub serial: Serial,
    /// Its graphic (tiledata id).
    pub graphic: Graphic,
    /// Stack size, or the dead body's graphic for [`CORPSE_GRAPHIC`].
    pub payload: WorldItemPayload,
    /// Where it lies.
    pub position: Point,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue: Hue,
    /// The flame this item burns with, if the sender named one.
    ///
    /// `None` is "the packet said nothing about light", which is what this shard
    /// always sends: an item's light comes from its graphic's tiledata here. A
    /// corpse is always `None` too, and that is not a gap — the byte a light id
    /// would ride in is the corpse's facing, and a corpse's facing belongs with
    /// its body in [`WorldItem::payload`]. The graphic decides which of the two
    /// the byte was, on the way out and on the way in.
    pub light: Option<LightId>,
    /// What the client is told about handling and drawing it.
    ///
    /// [`ItemFlags::NONE`] for everything this shard sends — movability is
    /// decided here rather than announced — but a real shard sets `0x20` on most
    /// of the ground, so this is what stops those items being refused.
    pub flags: ItemFlags,
}

impl EncodePacket for WorldItem {
    const ID: u8 = 0x1A;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        // A stack amount is only sent when there is more than one; the client
        // reads a lone item as a stack of one on its own.
        let payload_follows = self.payload.follows_graphic();
        let hued = self.hue != Hue::NONE;

        // The amount bit rides on top of the serial. Masking it off in the
        // other branch is belt and braces: `Serial` cannot be built above the
        // item pool, so the bit is already clear.
        let serial = if payload_follows {
            self.serial.raw() | 0x8000_0000
        } else {
            self.serial.raw() & 0x7FFF_FFFF
        };
        out.u32(serial);
        out.u16(self.graphic.0);
        if payload_follows {
            out.u16(self.payload.wire_value());
        }

        // One byte, two meanings, and the graphic picks: a corpse's facing, else
        // the light id. A corpse takes it — its payload owns the byte — so a
        // corpse handed a light id sends its facing and not the light, which is
        // the only reading the client has for `0x2006` anyway.
        let directed = self
            .payload
            .direction_byte()
            .or_else(|| self.light.map(|LightId(id)| id));
        let flagged = self.flags != ItemFlags::NONE;

        // x keeps its low 15 bits; its top bit means that byte follows.
        let mut x = self.position.x & 0x7FFF;
        if directed.is_some() {
            x |= 0x8000;
        }
        out.u16(x);
        // y keeps its low 14 bits; the top bit flags a hue word, the next one a
        // flags byte.
        let mut y = self.position.y & 0x3FFF;
        if hued {
            y |= 0x8000;
        }
        if flagged {
            y |= 0x4000;
        }
        out.u16(y);
        // The direction/light byte sits between y and z. Its position is the
        // whole reason the flag bit is on x and not here: put it after x, where
        // it reads naturally, and every field from y on is one byte out.
        if let Some(byte) = directed {
            out.u8(byte);
        }
        out.u8(self.position.z as u8);
        if hued {
            out.u16(self.hue.0);
        }
        if flagged {
            out.u8(self.flags.0);
        }
    }
}

impl DecodePacket for WorldItem {
    const ID: u8 = 0x1A;

    /// The stolen bits this engine never sets on the way out — "a flags byte
    /// follows" on `y`, and "a direction/light byte follows" on `x` for anything
    /// that is not a corpse — are refused rather than silently skipped: nothing
    /// here models what they would introduce, and reading past them as if they
    /// were the next field would produce a wrong value instead of an honest
    /// error.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw_serial = reader.u32()?;
        let stacked = raw_serial & 0x8000_0000 != 0;
        let serial = Serial::new(raw_serial & 0x7FFF_FFFF).ok_or(DecodeError::UnknownValue {
            field: "0x1A world item serial",
            value: raw_serial,
        })?;
        let graphic = Graphic(reader.u16()?);
        let value = if stacked { reader.u16()? } else { 1 };

        let raw_x = reader.u16()?;
        let directed = raw_x & 0x8000 != 0;
        let x = raw_x & 0x7FFF;

        let raw_y = reader.u16()?;
        let hued = raw_y & 0x8000 != 0;
        let flagged = raw_y & 0x4000 != 0;
        let y = raw_y & 0x3FFF;

        // Between y and z, and only when x said so. What it says depends on the
        // graphic: a corpse's facing, or a light source's id. A corpse described
        // without it faces north — the wire's own rule, not a fallback: north is
        // the zero the sender had nothing to write.
        let byte = match directed {
            true => Some(reader.u8()?),
            false => None,
        };
        let corpse = graphic == CORPSE_GRAPHIC;
        let payload = if corpse {
            WorldItemPayload::Corpse {
                body: Graphic(value),
                facing: byte.map_or(Direction::North, Direction::from_bits),
            }
        } else {
            WorldItemPayload::Stack(ItemAmount(value))
        };
        // ...and the same byte is not also a light: a corpse's is spoken for, so
        // reading it into both would make an encode of what was just decoded
        // send the facing twice over.
        let light = match corpse {
            true => None,
            false => byte.map(LightId),
        };

        let z = reader.u8()? as i8;
        let hue = if hued { Hue(reader.u16()?) } else { Hue::NONE };
        let flags = match flagged {
            true => ItemFlags(reader.u8()?),
            false => ItemFlags::NONE,
        };

        Ok(Self {
            serial,
            graphic,
            payload,
            position: Point::new(x, y, z),
            hue,
            light,
            flags,
        })
    }
}

/// `0x07` — the client asks to pick an item up. 7 bytes.
///
/// The item goes onto the client's cursor, dragged, until a `0x08` puts it down.
/// `amount` is how much of a stack to lift; the whole item unless the client is
/// splitting a pile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PickUpItem {
    /// The item's serial.
    pub serial: RawSerial,
    /// How many to lift, for a stack.
    pub amount: ItemAmount,
}

impl DecodePacket for PickUpItem {
    const ID: u8 = 0x07;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            serial: RawSerial(reader.u32()?),
            amount: ItemAmount(reader.u16()?),
        })
    }
}

/// `0x08` — the client asks to put the dragged item down. 14 or 15 bytes.
///
/// # The grid byte
///
/// Where the item goes is [`container`](Self::container): a real item serial
/// drops it *into* that container, a mobile serial equips it, and
/// [`DROP_TO_GROUND`] (`0xFFFFFFFF`) drops it at [`position`](Self::position) on
/// the ground.
///
/// Clients from 6.0.1.7 on (SA, the enhanced client, and every modern 2D client
/// including ClassicUO) slip a one-byte *grid index* in before the container
/// serial, making the packet fifteen bytes. The framer version-gates the length
/// on `Feature::ItemGrid`, so a slice reaching this decoder is either exactly
/// fourteen or exactly fifteen bytes — and the length alone says which, so this
/// reads the grid byte when it is there without needing the version again. The
/// grid slot is discarded: the server places a ground drop by the cursor
/// `x`/`y`/`z`, not the client's paperdoll grid cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DropItem {
    /// The item being dropped.
    pub serial: RawSerial,
    /// Where, when dropping on the ground.
    pub position: Point,
    /// Where the item is going: a container serial, a mobile serial to equip on,
    /// or [`DROP_TO_GROUND`].
    pub container: RawSerial,
}

/// What a `0x08` is actually asking for, with its position field read the way
/// the destination means it.
///
/// # Why this exists
///
/// The packet has one position field and two meanings for it, chosen by the
/// container field: onto the ground it is a world tile, into a container it is a
/// pixel offset inside that container's *gump*, which is a different coordinate
/// space entirely — `(50, 50)` is a spot on a bag's picture, not fifty tiles
/// north. Reading the packet as one struct meant every seam downstream took a
/// world [`Point`] that was sometimes not one, and converted it at whatever
/// depth first noticed. That is the conversion this type deletes: the meaning is
/// fixed here, once, where the container field that decides it is read.
///
/// # Totality
///
/// Every one of the 2³² values the container field can hold lands in exactly one
/// variant, and the order the checks run in is the whole rule. [`DROP_TO_GROUND`]
/// (`0xFFFFFFFF`) is outside both serial pools, so it would otherwise fall in
/// with the values that address nothing; it is tested first, which is what keeps
/// this packet's own sentinel from being confused with a client's `0`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropDestination {
    /// Onto the ground, at a world position. The only variant where the
    /// packet's position field is a [`Point`].
    Ground(Point),
    /// Onto an item — the destination serial is in the item pool.
    ///
    /// Deliberately not called `Container`: the client sends the same shape for
    /// a drop into a bag, onto a stack to merge with, and onto a spellbook, and
    /// which of those it is depends on components the wire knows nothing about.
    /// All this variant claims is that the target is an item.
    ///
    /// `at` is where in that item's gump the client let go. The server may or
    /// may not honour it (this one places by slot), but it is gump-space either
    /// way and typed as such so it can never be added to a world coordinate.
    Item {
        /// What the item is going onto or into.
        item: Serial,
        /// Where in that item's gump the cursor was.
        at: GumpPoint,
    },
    /// Onto a mobile — the destination serial is a mobile's. Equipping it, or
    /// offering it in a trade; which one is the server's business.
    ///
    /// The position field is carried by the packet and means nothing here, so
    /// it is not in this variant: a client dragging onto a person is pointing at
    /// the person, not at a coordinate.
    Mobile(Serial),
    /// The destination addresses nothing at all — a `0`, or a value above the
    /// item pool that is not the ground sentinel.
    ///
    /// Not an error and not a `None`: it is a fourth answer the client can give,
    /// and the seam that acts on it still owes the client a bounce, because the
    /// item is on its cursor either way. Making it a variant rather than an
    /// `Option` is what keeps that obligation in the `match`.
    Nowhere,
}

impl DropItem {
    /// The packet id.
    pub const ID: u8 = 0x08;

    /// Where this drop is going, with the position read as that destination
    /// means it. Total: see [`DropDestination`].
    #[must_use]
    pub const fn destination(&self) -> DropDestination {
        if self.container.0 == DROP_TO_GROUND.0 {
            return DropDestination::Ground(self.position);
        }
        match Serial::new(self.container.0) {
            // A gump's x/y are the packet's x/y read in the other space. The
            // widths differ (the wire is unsigned, gump offsets are signed for
            // layouts that hang off the left edge), so this widens rather than
            // reinterprets.
            Some(serial) if serial.is_item() => DropDestination::Item {
                item: serial,
                at: GumpPoint::new(self.position.x as i32, self.position.y as i32),
            },
            Some(serial) => DropDestination::Mobile(serial),
            None => DropDestination::Nowhere,
        }
    }
}

impl DecodePacket for DropItem {
    const ID: u8 = 0x08;

    /// Decode a whole `0x08` packet, whichever form the framer delivered.
    ///
    /// The framer already chose the fourteen- or fifteen-byte form by
    /// [`Feature::ItemGrid`] before this ran — see [`client_packet_length`](crate::packet::client_packet_length)
    /// — so asking `version` the same question here reads the grid byte when
    /// it is there without re-deriving the choice from the buffer length.
    fn decode_body(reader: &mut PacketReader<'_>, version: ClientVersion) -> Result<Self, DecodeError> {
        let serial = RawSerial(reader.u32()?);
        let x = reader.u16()?;
        let y = reader.u16()?;
        let z = reader.u8()? as i8;
        // The fifteen-byte form carries a grid-slot index here; read past it.
        // The grid slot is discarded: the server places a ground drop by the
        // cursor x/y/z, not the client's paperdoll grid cell.
        if version.supports(Feature::ItemGrid) {
            let _grid = reader.u8()?;
        }
        let container = RawSerial(reader.u32()?);
        Ok(Self {
            serial,
            position: Point::new(x, y, z),
            container,
        })
    }
}

/// Why the server cancelled a drag — the `code` in a `0x27`.
///
/// From Sphere's `PacketDragCancel::Reason`. The client bounces the item back to
/// where it came from whichever it is; the code only changes the message it
/// shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum DragCancelReason {
    /// The item cannot be lifted at all.
    CannotLift = 0x00,
    /// Too far away to reach.
    OutOfRange = 0x01,
    /// Out of line of sight.
    OutOfSight = 0x02,
    /// It is not yours to take.
    TryToSteal = 0x03,
    /// You are already holding something.
    AlreadyHolding = 0x04,
    /// Anything else.
    Other = 0x05,
}

/// `0x27` — cancel a drag and tell the client to bounce the item back. 2 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DragCancel {
    /// Why the drag was cancelled.
    pub reason: DragCancelReason,
}

impl EncodePacket for DragCancel {
    const ID: u8 = 0x27;
    const LENGTH: PacketLength = PacketLength::Fixed(2);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(self.reason as u8);
    }
}

impl DecodePacket for DragCancel {
    const ID: u8 = 0x27;

    /// An unknown reason byte is [`DragCancelReason::Other`] rather than an
    /// error: the *cancel* is the fact — the item is going back where it came
    /// from — and the byte only chooses which line the client prints. Refusing
    /// the packet over it would leave a lift this end still believes in.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let reason = match reader.u8()? {
            0x00 => DragCancelReason::CannotLift,
            0x01 => DragCancelReason::OutOfRange,
            0x02 => DragCancelReason::OutOfSight,
            0x03 => DragCancelReason::TryToSteal,
            0x04 => DragCancelReason::AlreadyHolding,
            _ => DragCancelReason::Other,
        };
        Ok(Self { reason })
    }
}

/// `0x13` — the client asks to equip the dragged item onto a mobile. 10 bytes.
///
/// Dragging an item onto a paperdoll sends this: the item goes onto `mobile` at
/// `layer`, the slot the client worked out from the item's tiledata. The server
/// checks it rather than trusting it, but the layer is the client's to propose.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EquipItemRequest {
    /// The item being worn.
    pub item: RawSerial,
    /// The layer to wear it on.
    pub layer: RawLayer,
    /// The mobile wearing it.
    pub mobile: RawSerial,
}

impl DecodePacket for EquipItemRequest {
    const ID: u8 = 0x13;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            item: RawSerial(reader.u32()?),
            layer: RawLayer(reader.u8()?),
            mobile: RawSerial(reader.u32()?),
        })
    }
}

/// `0x2E` — a mobile is now wearing an item. 15 bytes.
///
/// The single-item counterpart of the equipment list inside a `0x78`: sent when
/// one item is put on or the mobile is already drawn and only its outfit changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EquipUpdate {
    /// The item now worn.
    pub item: Serial,
    /// Its graphic.
    pub graphic: Graphic,
    /// Which layer.
    pub layer: Layer,
    /// The mobile wearing it.
    pub mobile: Serial,
    /// Its hue.
    pub hue: Hue,
}

impl EncodePacket for EquipUpdate {
    const ID: u8 = 0x2E;
    const LENGTH: PacketLength = PacketLength::Fixed(15);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.item.raw());
        out.u16(self.graphic.0);
        out.u8(0); // graphic offset, always zero
        out.u8(self.layer.0);
        out.u32(self.mobile.raw());
        out.u16(self.hue.0);
    }
}

impl DecodePacket for EquipUpdate {
    const ID: u8 = 0x2E;

    /// The graphic-offset byte is read and dropped: this engine always writes
    /// zero, and nothing this end draws is decided by it — the graphic is.
    ///
    /// A client that could not read this saw a mobile's clothes only in the
    /// `0x78` that first drew it, so a hat taken off never came off. A vendor is
    /// where that hurt most: its stock crate arrives *as* a `0x2E` on shop layer
    /// `0x1A`, and without it the buy list that names the crate has nothing to
    /// attach itself to.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw_item = reader.u32()?;
        let item = Serial::new(raw_item).ok_or(DecodeError::UnknownValue {
            field: "0x2E item serial",
            value: raw_item,
        })?;
        let graphic = Graphic(reader.u16()?);
        reader.skip(1)?;
        let layer = Layer(reader.u8()?);
        let raw_mobile = reader.u32()?;
        let mobile = Serial::new(raw_mobile).ok_or(DecodeError::UnknownValue {
            field: "0x2E wearer serial",
            value: raw_mobile,
        })?;
        Ok(Self {
            item,
            graphic,
            layer,
            mobile,
            hue: Hue(reader.u16()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{decode_packet, encode_packet};
    use crate::serial::{ITEM_MAX, ITEM_MIN, MOBILE_MAX, MOBILE_MIN};

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    fn grid_version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    fn pre_grid_version() -> ClientVersion {
        ClientVersion::new(5, 0, 0, 0)
    }

    /// The one item most of these tests draw or move.
    fn item() -> Serial {
        Serial::new(0x4000_0001).unwrap()
    }

    #[test]
    fn a_plain_item_is_the_short_form() {
        // No amount, no hue: serial, graphic, x, y, z and nothing optional.
        let packet = encode_packet(
            &WorldItem {
                serial: item(),
                graphic: Graphic(0x0EED), // a gold coin graphic
                payload: WorldItemPayload::Stack(ItemAmount(1)),
                position: Point::new(1000, 2000, 5),
                hue: Hue::NONE,
                light: None,
                flags: ItemFlags::NONE,
            },
            version(),
        );

        assert_eq!(packet[0], 0x1A);
        assert_eq!(u16::from_be_bytes([packet[1], packet[2]]), packet.len() as u16);
        // serial, unchanged (top bit clear because it is a single item)
        assert_eq!(&packet[3..7], &[0x40, 0x00, 0x00, 0x01]);
        // graphic
        assert_eq!(&packet[7..9], &0x0EEDu16.to_be_bytes());
        // x, y, z — no amount squeezed in between
        assert_eq!(&packet[9..11], &1000u16.to_be_bytes());
        assert_eq!(&packet[11..13], &2000u16.to_be_bytes());
        assert_eq!(packet[13], 5);
        assert_eq!(packet.len(), 14);
    }

    #[test]
    fn a_hued_stack_carries_the_amount_and_hue_with_their_flags() {
        let packet = encode_packet(
            &WorldItem {
                serial: Serial::new(0x4000_00AB).unwrap(),
                graphic: Graphic(0x0EED),
                payload: WorldItemPayload::Stack(ItemAmount(500)),
                position: Point::new(1000, 2000, 5),
                hue: Hue(0x0021),
                light: None,
                flags: ItemFlags::NONE,
            },
            version(),
        );

        // The amount bit is set on top of the serial's own bits.
        assert_eq!(
            u32::from_be_bytes([packet[3], packet[4], packet[5], packet[6]]),
            0xC000_00AB
        );
        assert_eq!(&packet[7..9], &0x0EEDu16.to_be_bytes());
        // amount follows the graphic
        assert_eq!(&packet[9..11], &500u16.to_be_bytes());
        // x plain, y with the hue flag
        assert_eq!(&packet[11..13], &1000u16.to_be_bytes());
        assert_eq!(u16::from_be_bytes([packet[13], packet[14]]), 2000 | 0x8000);
        assert_eq!(packet[15], 5);
        // hue last
        assert_eq!(&packet[16..18], &0x0021u16.to_be_bytes());
    }

    #[test]
    fn a_corpse_carries_its_body_in_the_stack_word() {
        let corpse = WorldItem {
            serial: item(),
            graphic: CORPSE_GRAPHIC,
            payload: WorldItemPayload::Corpse {
                body: Graphic(0x0190),
                facing: Direction::North,
            },
            position: Point::new(1000, 2000, 5),
            hue: Hue::NONE,
            light: None,
            flags: ItemFlags::NONE,
        };

        let packet = encode_packet(&corpse, version());
        assert_ne!(
            u32::from_be_bytes(packet[3..7].try_into().unwrap()) & 0x8000_0000,
            0
        );
        assert_eq!(&packet[9..11], &0x0190u16.to_be_bytes());
        let mut reader = PacketReader::new(&packet[3..]);
        assert_eq!(WorldItem::decode_body(&mut reader, version()).unwrap(), corpse);
    }

    #[test]
    fn a_corpse_carries_the_way_it_fell_in_the_direction_byte() {
        // The body falls the way it was facing, and the client draws the death
        // group *for a direction*: without this byte every corpse on the shard
        // lies the same way, whichever way it died facing.
        let corpse = WorldItem {
            serial: item(),
            graphic: CORPSE_GRAPHIC,
            payload: WorldItemPayload::Corpse {
                body: Graphic(0x0190),
                facing: Direction::West,
            },
            position: Point::new(1000, 2000, 5),
            hue: Hue::NONE,
            light: None,
            flags: ItemFlags::NONE,
        };

        let packet = encode_packet(&corpse, version());
        // x carries the "a direction byte follows" flag, and keeps its own value.
        assert_eq!(u16::from_be_bytes([packet[11], packet[12]]), 1000 | 0x8000);
        assert_eq!(u16::from_be_bytes([packet[13], packet[14]]), 2000);
        // The byte itself sits between y and z — not after x, where the flag is.
        assert_eq!(packet[15], Direction::West.to_bits());
        assert_eq!(packet[16], 5);

        let mut reader = PacketReader::new(&packet[3..]);
        assert_eq!(WorldItem::decode_body(&mut reader, version()).unwrap(), corpse);
    }

    #[test]
    fn a_corpse_facing_north_sends_no_direction_byte() {
        // North is zero, and a zero byte is what ServUO leaves out: the client
        // reads a missing one as north, so the two forms say the same thing and
        // the shorter is the one every client has been met with.
        let packet = encode_packet(
            &WorldItem {
                serial: item(),
                graphic: CORPSE_GRAPHIC,
                payload: WorldItemPayload::Corpse {
                    body: Graphic(0x0190),
                    facing: Direction::North,
                },
                position: Point::new(1000, 2000, 5),
                hue: Hue::NONE,
                light: None,
                flags: ItemFlags::NONE,
            },
            version(),
        );

        assert_eq!(u16::from_be_bytes([packet[11], packet[12]]), 1000);
        assert_eq!(packet[15], 5);
        assert_eq!(packet.len(), 16);
    }

    #[test]
    fn a_lantern_from_a_real_shard_keeps_its_light_and_its_flags() {
        // ServUO's shape for an ordinary movable item that burns: the light id in
        // the byte after y, `0x20` in the flags byte after the hue. Both used to
        // be refused, which lost the item rather than the hint — and `0x20` is on
        // nearly everything lying on a real shard's ground.
        let lantern = WorldItem {
            serial: item(),
            graphic: Graphic(0x0A15),
            payload: WorldItemPayload::Stack(ItemAmount(1)),
            position: Point::new(1000, 2000, 5),
            hue: Hue(0x0021),
            light: Some(LightId(9)),
            flags: ItemFlags::MOVABLE,
        };

        let packet = encode_packet(&lantern, version());
        // x announces the light byte, y announces both the hue and the flags.
        assert_eq!(u16::from_be_bytes([packet[9], packet[10]]), 1000 | 0x8000);
        assert_eq!(
            u16::from_be_bytes([packet[11], packet[12]]),
            2000 | 0x8000 | 0x4000
        );
        assert_eq!(packet[13], 9, "the light id, between y and z");
        assert_eq!(packet[14], 5, "z");
        assert_eq!(&packet[15..17], &0x0021u16.to_be_bytes(), "hue");
        assert_eq!(packet[17], ItemFlags::MOVABLE.0, "and the flags byte last");

        let mut reader = PacketReader::new(&packet[3..]);
        let read = WorldItem::decode_body(&mut reader, version()).unwrap();
        assert_eq!(read, lantern);
        assert!(read.flags.has(ItemFlags::MOVABLE));
    }

    #[test]
    fn a_corpses_byte_is_its_facing_and_never_a_light() {
        // One byte, and the graphic picks which reading it gets. Decoding a
        // corpse into both would make a re-encode send the facing as a light id.
        let packet = encode_packet(
            &WorldItem {
                serial: item(),
                graphic: CORPSE_GRAPHIC,
                payload: WorldItemPayload::Corpse {
                    body: Graphic(0x0190),
                    facing: Direction::West,
                },
                position: Point::new(1000, 2000, 5),
                hue: Hue::NONE,
                light: None,
                flags: ItemFlags::NONE,
            },
            version(),
        );

        let mut reader = PacketReader::new(&packet[3..]);
        let read = WorldItem::decode_body(&mut reader, version()).unwrap();
        assert_eq!(read.light, None, "the byte was the facing");
        assert_eq!(
            read.payload,
            WorldItemPayload::Corpse {
                body: Graphic(0x0190),
                facing: Direction::West,
            }
        );
    }

    #[test]
    fn a_high_z_survives_as_a_signed_byte() {
        // Underground and underwater are negative z; the client reads the byte
        // as signed, so -5 has to go out as 0xFB, not clamp to 0.
        let packet = encode_packet(
            &WorldItem {
                serial: item(),
                graphic: Graphic(0x0001),
                payload: WorldItemPayload::Stack(ItemAmount(1)),
                position: Point::new(0, 0, -5),
                hue: Hue::NONE,
                light: None,
                flags: ItemFlags::NONE,
            },
            version(),
        );
        assert_eq!(packet[13], 0xFB);
    }

    #[test]
    fn a_pickup_is_a_serial_and_an_amount() {
        let bytes = [0x07, 0x40, 0x00, 0x00, 0x2A, 0x00, 0x05];
        let pickup: PickUpItem = decode_packet(&bytes, version()).unwrap();
        assert_eq!(pickup.serial, RawSerial(0x4000_002A));
        assert_eq!(pickup.amount, ItemAmount(5));
    }

    #[test]
    fn a_ground_drop_reads_its_target_as_the_ground() {
        // serial, x=1000, y=2000, z=5, container=0xFFFFFFFF
        let mut bytes = vec![0x08];
        bytes.extend_from_slice(&0x4000_002Au32.to_be_bytes());
        bytes.extend_from_slice(&1000u16.to_be_bytes());
        bytes.extend_from_slice(&2000u16.to_be_bytes());
        bytes.push(5);
        bytes.extend_from_slice(&DROP_TO_GROUND.0.to_be_bytes());
        assert_eq!(bytes.len(), 14);

        let drop: DropItem = decode_packet(&bytes, pre_grid_version()).unwrap();
        assert_eq!(drop.serial, RawSerial(0x4000_002A));
        assert_eq!(drop.position, Point::new(1000, 2000, 5));
        assert_eq!(
            drop.destination(),
            DropDestination::Ground(Point::new(1000, 2000, 5)),
            "the ground is the one destination whose position is a world tile",
        );
    }

    #[test]
    fn the_fifteen_byte_grid_form_reads_past_the_grid_slot() {
        // The modern (6.0.1.7+) drop: a grid-index byte sits between z and the
        // container serial. The bug was reading the container one byte early, so
        // a ground drop's 0xFFFFFFFF decoded as 0x00FFFFFF and bounced. Here the
        // grid byte is skipped and the container reads whole.
        let mut bytes = vec![0x08];
        bytes.extend_from_slice(&0x4000_002Au32.to_be_bytes());
        bytes.extend_from_slice(&1000u16.to_be_bytes());
        bytes.extend_from_slice(&2000u16.to_be_bytes());
        bytes.push(5);
        bytes.push(0x00); // grid slot
        bytes.extend_from_slice(&DROP_TO_GROUND.0.to_be_bytes());
        assert_eq!(bytes.len(), 15);

        let drop: DropItem = decode_packet(&bytes, grid_version()).unwrap();
        assert_eq!(drop.serial, RawSerial(0x4000_002A));
        assert_eq!(drop.position, Point::new(1000, 2000, 5));
        assert_eq!(drop.container, DROP_TO_GROUND);
        assert_eq!(
            drop.destination(),
            DropDestination::Ground(Point::new(1000, 2000, 5)),
            "the grid byte must not shift the container",
        );
    }

    #[test]
    fn a_drop_into_a_container_reads_its_position_in_gump_space() {
        // The same two bytes, and not the same coordinate: an item serial in the
        // container field makes x/y a spot on that container's picture. The
        // conversion happens here and nowhere downstream, which is the whole
        // point of the type — a `Point` this size would be off the far edge of
        // any map, and nothing would have said so.
        let mut bytes = vec![0x08];
        bytes.extend_from_slice(&0x4000_002Au32.to_be_bytes());
        bytes.extend_from_slice(&50u16.to_be_bytes());
        bytes.extend_from_slice(&60u16.to_be_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&0x4000_00FFu32.to_be_bytes()); // a container serial
        let drop: DropItem = decode_packet(&bytes, pre_grid_version()).unwrap();
        assert_eq!(drop.container, RawSerial(0x4000_00FF));
        assert_eq!(
            drop.destination(),
            DropDestination::Item {
                item: Serial::new(0x4000_00FF).unwrap(),
                at: GumpPoint::new(50, 60),
            },
        );
    }

    #[test]
    fn a_drop_onto_a_mobile_keeps_no_position_at_all() {
        // A mobile serial in the container field: the client is pointing at a
        // person, and the x/y it sent are neither a tile nor a gump offset. The
        // variant has nowhere to put them, which is the type saying so.
        let mut bytes = vec![0x08];
        bytes.extend_from_slice(&0x4000_002Au32.to_be_bytes());
        bytes.extend_from_slice(&50u16.to_be_bytes());
        bytes.extend_from_slice(&60u16.to_be_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&0x0000_0007u32.to_be_bytes()); // a mobile serial
        let drop: DropItem = decode_packet(&bytes, pre_grid_version()).unwrap();
        assert_eq!(
            drop.destination(),
            DropDestination::Mobile(Serial::new(7).unwrap()),
        );
    }

    #[test]
    fn every_destination_value_lands_in_exactly_one_variant() {
        // Class B totality, on the field that decides how the *other* field is
        // read. Walking all 2^32 is not worth the minute; these are the four
        // pool boundaries plus the two sentinels, which is where an off-by-one
        // in the ordering of the checks would show.
        let drop = |container: u32| {
            DropItem {
                serial: RawSerial(0x4000_002A),
                position: Point::new(1, 2, 3),
                container: RawSerial(container),
            }
            .destination()
        };

        assert_eq!(
            drop(DROP_TO_GROUND.0),
            DropDestination::Ground(Point::new(1, 2, 3))
        );
        assert_eq!(drop(0), DropDestination::Nowhere, "zero addresses nothing");
        assert_eq!(drop(0x8000_0000), DropDestination::Nowhere, "above the item pool");
        assert!(matches!(drop(MOBILE_MIN), DropDestination::Mobile(_)));
        assert!(matches!(drop(MOBILE_MAX), DropDestination::Mobile(_)));
        assert!(matches!(drop(ITEM_MIN), DropDestination::Item { .. }));
        assert!(matches!(drop(ITEM_MAX), DropDestination::Item { .. }));
    }

    #[test]
    fn a_drag_cancel_is_two_bytes_with_the_reason() {
        assert_eq!(
            encode_packet(
                &DragCancel {
                    reason: DragCancelReason::OutOfRange
                },
                version()
            ),
            vec![0x27, 0x01]
        );
        assert_eq!(
            encode_packet(
                &DragCancel {
                    reason: DragCancelReason::AlreadyHolding
                },
                version()
            ),
            vec![0x27, 0x04]
        );
    }

    #[test]
    fn an_equip_request_is_item_layer_mobile() {
        let mut bytes = vec![0x13];
        bytes.extend_from_slice(&0x4000_0002u32.to_be_bytes());
        bytes.push(2); // layer 2, the left hand
        bytes.extend_from_slice(&0x0000_0001u32.to_be_bytes());
        assert_eq!(bytes.len(), 10);
        let req: EquipItemRequest = decode_packet(&bytes, version()).unwrap();
        assert_eq!(req.item, RawSerial(0x4000_0002));
        assert_eq!(req.layer, RawLayer(2));
        assert_eq!(req.mobile, RawSerial(0x0000_0001));
    }

    #[test]
    fn a_lift_of_nothing_decodes_and_is_refused_at_promotion() {
        // `docs/protocol_newtypes.md` N9. `0` is the wire's word for "no object"
        // and a client is free to send it; the packet is well-formed, so the
        // framer must not drop the connection over it, and the refusal happens
        // where the serial would have addressed something.
        let bytes = [0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
        let pickup: PickUpItem = decode_packet(&bytes, version()).unwrap();
        assert_eq!(pickup.serial, RawSerial(0), "the byte survives decoding");
        assert_eq!(pickup.serial.validate(), None, "and dies at the seam");
    }

    #[test]
    fn a_ground_drops_sentinel_is_not_a_serial() {
        // The other half of the same pair, and the reason `destination` tests the
        // sentinel before asking `Serial::new`: `0xFFFFFFFF` addresses nothing
        // *and* means the floor, which are different answers — check them in the
        // other order and every ground drop becomes `Nowhere` and bounces.
        let mut bytes = vec![0x08];
        bytes.extend_from_slice(&0x4000_002Au32.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&DROP_TO_GROUND.0.to_be_bytes());
        let drop: DropItem = decode_packet(&bytes, pre_grid_version()).unwrap();
        assert!(matches!(drop.destination(), DropDestination::Ground(_)));
        assert_eq!(drop.container.validate(), None, "the floor is not an object");
    }

    #[test]
    fn every_layer_byte_interprets() {
        // Class B is total: a layer is a name, so all 256 come back whole and
        // the wearable range is somebody else's question — see `RawLayer`.
        for byte in 0..=u8::MAX {
            assert_eq!(RawLayer(byte).interpret(), Layer(byte));
        }
    }

    #[test]
    fn an_equip_packet_is_fifteen_bytes() {
        let packet = encode_packet(
            &EquipUpdate {
                item: Serial::new(0x4000_0002).unwrap(),
                graphic: Graphic(0x13B9),
                layer: Layer(1),
                mobile: Serial::new(0x0000_0001).unwrap(),
                hue: Hue(0x0021),
            },
            version(),
        );
        assert_eq!(packet.len(), 15);
        assert_eq!(packet[0], 0x2E);
        assert_eq!(&packet[1..5], &0x4000_0002u32.to_be_bytes());
        assert_eq!(&packet[5..7], &0x13B9u16.to_be_bytes());
        assert_eq!(packet[7], 0);
        assert_eq!(packet[8], 1); // layer
        assert_eq!(&packet[9..13], &0x0000_0001u32.to_be_bytes());
        assert_eq!(&packet[13..15], &0x0021u16.to_be_bytes());
    }
}
