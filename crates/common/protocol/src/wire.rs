//! The small named things packet fields are made of.
//!
//! A UO packet is mostly numbers, and the numbers are not interchangeable: a
//! graphic is not a hue, a sound is not a cliloc, and the compiler is the only
//! thing that will ever notice the difference. So every id-shaped field gets a
//! newtype, and `.0` is unwrapped inside a codec and nowhere else.
//!
//! # These arrive as they are needed
//!
//! A newtype for a packet nobody has read closely yet is a guess, and a guess
//! that hardens before it is right is worse than a bare `u16`. So this module
//! grows one type at a time, in the stage that first has a field for it — see
//! `docs/protocol/design_packet_enums.md`. [`Serial`](crate::serial::Serial) is the
//! exception and lives in its own module: it carries a validity rule and a
//! pool split, not just a name.

use std::fmt;

use serde::{
    Deserialize,
    Serialize,
};

/// An art id: what the client draws. Tiles, items, effect sprites and gump art
/// all index the same `art.mul`, so they share one type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Graphic(pub u16);

/// A colour index into `hues.mul`. `0` means "as the art was drawn".
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Hue(pub u16);

impl Hue {
    /// No tint: the art's own colours.
    pub const NONE: Self = Self(0);

    /// The client's muted grey, and the only place the number is written.
    ///
    /// Three unrelated things are drawn in it — server feedback, townsfolk
    /// chatter, an object's name label — and both references write `0x3B2` out
    /// separately for each (ServUO's `AsciiMessage` fallback, its
    /// `Item.OnSingleClick`, its `Notoriety.Hues[3]`; Sphere's `HUE_TEXT_DEF`).
    /// They coincide because the client's palette has one grey that reads as
    /// "not a person talking", not because they are the same rule. So the
    /// *value* lives here once and each *meaning* gets its own name below: a
    /// shard that recolours its system messages must not silently recolour
    /// every shopkeeper too.
    ///
    /// Private on purpose. Nothing outside this impl should name the grey — it
    /// should name what it is drawing.
    const MUTED_GREY: Self = Self(0x03B2);

    /// The hue the server talks in: a private system line, a staff command's
    /// reply, a shard-wide announcement. Sent under the system serial with no
    /// graphic, so it reads as feedback rather than as somebody speaking.
    pub const SYSTEM: Self = Self::MUTED_GREY;

    /// The hue ClassicUO uses for its "Your skill in … has increased" notice.
    ///
    /// This is deliberately distinct from [`SYSTEM`](Self::SYSTEM): the skill
    /// notice is feedback about a character's progress, not ordinary shard
    /// speech. ClassicUO writes `0x0058` in its skill-update handler.
    pub const SKILL_CHANGED: Self = Self(0x0058);

    /// The hue an NPC's own voice is spoken in — a banker's answer, a vendor's
    /// line, a guard's warning, an escortable's thanks. Over the NPC's head and
    /// heard by everyone nearby, which is what separates it from
    /// [`SYSTEM`](Self::SYSTEM): one is the shard talking, this is a mobile.
    pub const NPC_SPEECH: Self = Self::MUTED_GREY;

    /// The hue a spell's power words are said in, paired with
    /// [`TalkMode::Spell`](crate::speech::TalkMode::Spell).
    ///
    /// ServUO says a mantra in the caster's own `Mobile.SpeechHue` — the hue the
    /// player last chose in the client, remembered per mobile. This engine does
    /// not remember it: a player's chosen hue is passed straight through
    /// `chat::say` and never stored, so there is nothing to read back. Until
    /// there is, the words come in the same grey every other line the engine
    /// itself speaks does, and this name is where that decision is written down.
    pub const SPELL_WORDS: Self = Self::MUTED_GREY;

    /// The hue an object's single-click name label comes back in, paired with
    /// [`TalkMode::Label`](crate::speech::TalkMode::Label).
    ///
    /// For *items* only. A mobile's name label is coloured by its standing
    /// instead — see [`Notoriety::name_hue`](crate::mobile::Notoriety::name_hue),
    /// whose neutral entry lands on the same grey by way of a different table.
    pub const LABEL: Self = Self::MUTED_GREY;

    /// The hue a stack's own count is written in, where the client draws one
    /// over a pile — see `openshard-client-render`'s `items::stack_label`.
    ///
    /// ClassicUO uses `0x0481` for an item's own label. A count has the same
    /// job — identifying a particular item rather than carrying speech — and
    /// needs that bright ink over the often-dark, often-busy art in a bag.
    /// The muted system grey is lost against both.
    pub const STACK_COUNT: Self = Self(0x0481);
}

/// An index into the client's sound files.
///
/// `Deserialize`/`Serialize` so a sound id can be read straight out of a
/// content table (gameplay data, not protocol data) instead of arriving as a
/// bare `u16` that a script or config loader has to wrap by hand at every one
/// of its call sites.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SoundId(pub u16);

/// A multi's own id — a house, a ship, a boat's hold.
///
/// **Not a [`Graphic`]**, and the distinction is the whole reason this exists.
/// A placed multi arrives as a world item whose graphic is `0x4000 | id`, so the
/// two id spaces overlap and a value that means "cottage" in one means an
/// unrelated static in the other. `0x99` writes the bare id; `0x1A` writes the
/// masked graphic; something holding a `u16` cannot tell you which it had.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct MultiId(pub u16);

impl MultiId {
    /// The bit that turns an id into the graphic a placed multi draws as.
    pub const FLAG: u16 = 0x4000;

    /// The graphic a placed copy of this multi carries.
    #[must_use]
    pub const fn graphic(self) -> Graphic {
        Graphic(Self::FLAG | self.0)
    }

    /// The multi a graphic names, whichever spelling it was in.
    ///
    /// A mask rather than a subtraction: a caller may hold `0x4064` or `0x0064`
    /// and both mean the same house.
    #[must_use]
    pub const fn from_graphic(graphic: Graphic) -> Self {
        Self(graphic.0 & !Self::FLAG)
    }
}

/// The id a targeting cursor request carries and its response echoes back.
///
/// Opaque to the client: the server picks it, the client repeats it, and that is
/// how a click is matched to the request that asked for it. Nothing about it is
/// a serial, even where a server happens to use one as the value.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct CursorId(pub u32);

/// The key a `0x8C` relay hands out and a `0x91` game login must echo back.
///
/// Opaque the same way [`CursorId`] is: the login server picks it from OS
/// entropy (see `openshard_login::auth::AuthKeys::issue`), and the only thing
/// that makes it valid is that it was issued and not yet redeemed — nothing
/// about the number itself means anything.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct AuthKey(pub u32);

/// A number in the client's own `cliloc.enu`: a line of text the client already
/// has a translation for, so a message costs four bytes instead of a string.
///
/// Always the server's choice — the client only ever looks one up and draws it —
/// so there is no `Raw` counterpart. It lives here rather than beside the packet
/// that first needed it because five carry one: `0xC1` and `0xCC` speech,
/// `0x14`'s context-menu entries, `0xD6`'s property lists, and the start-city
/// descriptions in a `0xA9`. Same reason [`Layer`] is here.
///
/// `Deserialize`/`Serialize` for the same reason as [`SoundId`]: a message id
/// is gameplay content (which line the pack wants shown), so it should arrive
/// out of a content table already typed rather than as a bare `u32` that every
/// one of its ~190 call sites wraps by hand.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ClilocId(pub u32);

/// Where a worn item sits on a mobile: the hand, the head, the mount slot.
///
/// The numbers are the client's — it decides which sprite a layer draws over
/// which — so the type is a byte with a name and nothing more, exactly as
/// [`StatusFlags`](crate::mobile::StatusFlags) is. Modelling the twenty-odd
/// layers as an enum would be a guess about the ones this engine has never sent.
///
/// It lives here rather than beside either packet that carries it because both
/// do: a mobile's `0x78` outfit list and an item's `0x2E`/`0x13` equip pair.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Layer(pub u8);

impl Layer {
    /// The weapon hand.
    ///
    /// This and the twenty-two that follow are the reference client's `Layer`
    /// enum, verbatim and in its own order. They are named — rather than
    /// written as `Layer(0x14)` where they are needed — because a *table* needs
    /// them: the paperdoll's draw order is twenty-five layer names per row,
    /// three rows of them, and a table of hex bytes cannot be checked against
    /// the reference it was ported from. Naming them here rather than in the
    /// renderer that reads them is [`HAIR`](Self::HAIR)'s reason: one list, and
    /// both ends of the wire read it.
    ///
    /// Still constants and not an enum, for the reason above the type: the
    /// numbers past these are a shard's business — a bank box, a trade window —
    /// and modelling the whole byte would be a claim about slots this engine
    /// has never sent.
    pub const ONE_HANDED: Self = Self(0x01);
    /// The off hand: a shield, or the second half of a two-handed weapon.
    pub const TWO_HANDED: Self = Self(0x02);
    /// Shoes.
    pub const SHOES: Self = Self(0x03);
    /// Pants.
    pub const PANTS: Self = Self(0x04);
    /// A shirt.
    pub const SHIRT: Self = Self(0x05);
    /// A helmet or a hat.
    pub const HELMET: Self = Self(0x06);
    /// Gloves.
    pub const GLOVES: Self = Self(0x07);
    /// A ring.
    pub const RING: Self = Self(0x08);
    /// A talisman.
    pub const TALISMAN: Self = Self(0x09);
    /// A necklace or a gorget.
    pub const NECKLACE: Self = Self(0x0A);
    /// `Layer.Hair`. Hair is an ordinary worn item on this wire, and the two
    /// ends disagree about what to do with it: the shard dresses a corpse and a
    /// ghost in it, and the client refuses to draw it on either
    /// (`IsDead && (layer == Layer.Hair || layer == Layer.Beard)`).
    pub const HAIR: Self = Self(0x0B);
    /// A belt or a sash.
    pub const WAIST: Self = Self(0x0C);
    /// The chest piece worn under the tunic.
    pub const TORSO: Self = Self(0x0D);
    /// A bracelet.
    pub const BRACELET: Self = Self(0x0E);
    /// A mask, and whatever else sits on the face.
    pub const FACE: Self = Self(0x0F);
    /// `Layer.FacialHair` — a beard. [`HAIR`](Self::HAIR)'s twin everywhere it
    /// is asked about, which is why the pair is named here rather than at the
    /// two call sites that had one each.
    pub const BEARD: Self = Self(0x10);
    /// The outer chest piece — `Layer.Tunic`, the surcoat over the armour.
    pub const TUNIC: Self = Self(0x11);
    /// Earrings.
    pub const EARRINGS: Self = Self(0x12);
    /// Arms.
    pub const ARMS: Self = Self(0x13);
    /// A cloak — and, on the wire, a quiver.
    pub const CLOAK: Self = Self(0x14);
    /// The backpack. Drawn last on a paperdoll and outside its ordering,
    /// which is what `openshard_client_render::paperdoll` keeps it apart for.
    pub const BACKPACK: Self = Self(0x15);
    /// A robe or a dress.
    pub const ROBE: Self = Self(0x16);
    /// A skirt or a kilt.
    pub const SKIRT: Self = Self(0x17);
    /// Leg armour.
    pub const LEGS: Self = Self(0x18);
    /// What is being ridden. Never drawn on a paperdoll — the mount is a body
    /// of its own on the ground, not a picture on a doll.
    pub const MOUNT: Self = Self(0x19);
}

/// A layer exactly as a client packet proposed it.
///
/// Only `0x13` carries one inbound — the client works the slot out from the
/// item's tiledata and offers it — so by N4's counting rule in
/// `docs/protocol/design_wire_types.md` this would live in `items.rs`. It is here
/// instead, beside its validated twin, the way [`RawHue`] sits beside [`Hue`]
/// and `RawSerial` beside `Serial`: a pair split across two modules is a pair
/// the next reader has to be told about.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawLayer(pub u8);

impl RawLayer {
    /// The [`Layer`] this names.
    ///
    /// Total, and structurally identical to what it wraps, for
    /// `RawStepSequence`'s reason: every byte names a slot, because a layer is
    /// a name and not a range — see [`Layer`]. What the pair records is
    /// *provenance*, which is the only thing that differs between the layer a
    /// client proposed and the layer a server sends back. Whether the slot may
    /// be *worn into* is a different question, and a gameplay one:
    /// `openshard_items::equip_item` answers it.
    #[inline]
    #[must_use]
    pub const fn interpret(self) -> Layer {
        Layer(self.0)
    }
}

/// A colour choice exactly as a client packet carried it: not yet checked
/// against the set of hues this shard actually allows. See
/// `docs/protocol/design_wire_types.md` — the allowed set is content, so the check that
/// turns this into a real [`Hue`] lives above `protocol`, and does not exist
/// yet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RawHue(pub u16);

/// An art id exactly as a client packet carried it — a hairstyle, a beard —
/// not yet checked against the set this shard actually allows. Same status as
/// [`RawHue`]: the check does not exist yet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RawGraphic(pub u16);

/// Which character slot a client asked to fill, play, or delete, exactly as
/// sent.
///
/// Three packets carry one and only the third reads it. `create_character`
/// fills the first free slot and `character_play` looks the character up by
/// name, so for `0x00`/`0xF8` and `0x5D` this stays what the pilot called it:
/// a class D value, named and ignored. `0x83` delete is different — the slot
/// *is* the whole request — so the type grew [`validate`](Self::validate) in
/// N6, and the promotion is there for the other two the day slot choice is
/// honoured. See `docs/protocol/design_wire_types.md`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawCharacterSlot(pub u32);

impl RawCharacterSlot {
    /// The slot this names, when the account has `held` characters.
    ///
    /// The same "is this one I offered" check `RawContextMenuIndex::validate`
    /// makes, against a different list: the client is answering the character
    /// list it was last sent, so that list's length is the whole domain. A slot
    /// past the end is a stale screen or a crafted packet, and is refused
    /// rather than clamped — clamping would delete *some* character instead of
    /// none.
    ///
    /// `held` is a count, so an empty account refuses every slot, zero
    /// included.
    pub const fn validate(self, held: usize) -> Result<CharacterSlot, InvalidCharacterSlot> {
        if (self.0 as usize) < held {
            Ok(CharacterSlot(self.0))
        } else {
            Err(InvalidCharacterSlot { slot: self.0, held })
        }
    }
}

/// A character slot the account actually has: an index into the list the client
/// was last sent, counted from zero.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct CharacterSlot(pub u32);

/// A packet named a character slot the account does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InvalidCharacterSlot {
    /// The slot the client sent.
    pub slot: u32,
    /// How many characters the account actually holds.
    pub held: usize,
}

impl fmt::Display for InvalidCharacterSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "character slot {} was named on an account holding {}",
            self.slot, self.held
        )
    }
}

impl std::error::Error for InvalidCharacterSlot {
}

/// A client's self-reported IPv4 address, exactly as sent. Never trusted,
/// never read — the server already knows the real address from the socket.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawClientIp(pub u32);

/// A skill id exactly as a client packet carried it, not yet checked against
/// `openshard_state::Skill`'s known ids.
///
/// No promotion method here, and none is coming: the domain type,
/// `openshard_state::Skill`, lives in a server crate above `protocol` (its
/// meaning is gameplay content, not wire shape), so the check that turns this
/// into one is `Skill::from_id`, called at whichever seam has the domain in
/// hand — the same licence `RawSerial::validate` documents for `Serial::new`.
/// Named here rather than in `world.rs`, where the pilot first needed it,
/// because `skill.rs` is its second user — N4's "two or more modules"
/// counting rule. See `docs/protocol/design_wire_types.md`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawSkillId(pub u8);
