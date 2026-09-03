use openshard_protocol::casting::SpellId;
use openshard_protocol::identity::RawCharacterName;
use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_protocol::items::DropDestination;
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::wire::{
    Graphic,
    Hue,
    RawCharacterSlot,
};
use openshard_protocol::world::{
    Aggression,
    DamageType,
    PhysicalResistance,
    PoisonLevel,
    RangedRange,
    Sight,
};
use openshard_state::{
    LockKind,
    Skill,
};

use super::*;

/// How a character looks: its body graphic and hue. Chosen on the creation
/// screen, or restored from the save.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Appearance {
    /// The body graphic id. Spelled out rather than imported: the glob above
    /// already carries `openshard_state::components::Drawn`, which is the
    /// *component* an item is drawn by, and the two must not be confused.
    pub body: Graphic,
    /// The skin hue.
    pub hue:  openshard_protocol::wire::Hue,
}

impl Appearance {
    /// What a character with no chosen and no saved look is drawn as: the human
    /// male body in the client's default skin.
    ///
    /// The same fallback `enter` used to inline, named so the two places that
    /// need it — the fallback itself and a character built without a creation
    /// screen — cannot drift apart.
    pub fn default_human() -> Self {
        Self {
            body: BODY_HUMAN_MALE,
            hue:  openshard_protocol::wire::Hue(DEFAULT_HUE),
        }
    }
}

/// A character's stats and skills — chosen at creation for a new character, or
/// restored from the save for a played one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CharacterSheet {
    /// Strength.
    pub strength:        u16,
    /// Dexterity.
    pub dexterity:       u16,
    /// Intelligence.
    pub intelligence:    u16,
    /// Trained skills as `(id, value in tenths, lock, cap in tenths)`. A cap of
    /// zero means "the shard's own", and `enter` fills it from `[gameplay]
    /// skill_cap` — so a newly created character does not have to know the knob.
    pub skills:          Vec<(u8, u16, openshard_protocol::skill::SkillLock, u16)>,
    /// Which way the three stats train, and how long ago each last rose (in
    /// ticks, counted back from the moment the character enters). Both halves are
    /// inputs the stat gain reads, and both used to reset at every login.
    pub stat_locks:      openshard_persistence::StatLockRecord,
    /// Active effects — a poison a relog must not wash off, and the buffs and
    /// debuffs that will join it. Empty for a clean character.
    pub effects:         Vec<openshard_persistence::EffectRecord>,
    /// Whether the character logged out dead — a ghost relogs a ghost. `false`
    /// for a new character and for any living one.
    pub dead:            bool,
    /// How widely known the character is, and which way — ServUO's fame and karma.
    pub fame:            i32,
    /// Which way it is known. Negative is infamy.
    pub karma:           i32,
    /// How many innocents it has killed. A **standing**, and one that used not to
    /// survive a restart: the fifth murder makes a character red for good, and a count
    /// held only in memory washed every murderer blue at the next boot.
    pub murders:         u16,
    /// Quests in progress, with their per-objective progress. Empty for a new
    /// character.
    pub quests:          Vec<openshard_persistence::QuestRecord>,
    /// Quests already finished, with the wait before each may be taken again.
    pub done_quests:     Vec<openshard_persistence::DoneQuestRecord>,
    /// Where it stands in a guild, if it is in one. `None` for the unguilded,
    /// which is most characters.
    pub guild:           Option<GuildSeat>,
    /// A guild that asked it to join while it was away, still waiting on an
    /// answer.
    pub guild_candidate: Option<u32>,
}

/// A character's place in a guild, as it comes off the record.
///
/// Three fields and not a tuple because two of them are easy to swap in the
/// reader's head: the **title** is free text a leader typed, the **rank** is one
/// of five and is what every permission is decided by. See
/// [`GuildMember`](openshard_state::GuildMember), which this is rebuilt into
/// unchanged.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GuildSeat {
    /// Which guild, by its own id.
    pub guild: openshard_state::GuildId,
    /// The title it wears there. Often empty.
    pub title: String,
    /// Where it stands.
    pub rank:  openshard_state::Rank,
}

impl CharacterSheet {
    /// The sheet a character with no chosen and no saved numbers enters on: the
    /// world's flat hundreds, no skills, nothing trained and nothing owed.
    ///
    /// These are the same defaults `enter` used to reach for when the sheet was
    /// absent; naming them here is what let the saved side of `StoredCharacter`
    /// stop being optional. (Not a link: that type is the world's own, and does
    /// not appear in this crate's public API.)
    pub fn starting() -> Self {
        Self {
            strength:        DEFAULT_HITPOINTS,
            dexterity:       DEFAULT_DEXTERITY,
            intelligence:    DEFAULT_MANA,
            skills:          Vec::new(),
            stat_locks:      openshard_persistence::StatLockRecord::default(),
            effects:         Vec::new(),
            dead:            false,
            fame:            0,
            karma:           0,
            murders:         0,
            quests:          Vec::new(),
            done_quests:     Vec::new(),
            guild:           None,
            guild_candidate: None,
        }
    }
}

/// A character entering the world for the first time: made on the creation
/// screen a moment ago, or a name the config seeded that the database has never
/// seen.
///
/// Both fields are honestly optional, and independently so: a config-seeded name
/// has neither a chosen city nor a chosen look, a creation has both, and a
/// creation whose city index the client sent out of range has the look but not
/// the city.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FreshCharacter {
    /// Which facet to spawn on.
    pub facet:      Facet,
    /// Where on it. Absent takes the world's configured start for the facet.
    pub start:      Option<Point>,
    /// The look chosen on the creation screen. Absent for a character that
    /// reached the world without one, which takes the world's default body.
    pub appearance: Option<Appearance>,
    /// The stats and skills chosen there. Absent likewise, and then the world's
    /// flat defaults and no skills apply.
    ///
    /// Boxed, and the reason is the enum this sits inside: a `Command` is one
    /// variant wide, every queued command pays for the widest, and this sheet is
    /// far and away the biggest thing on any of them — fifty-eight skills, the
    /// effects, the quest log. Behind a pointer it costs one word on the queue
    /// and its own allocation once per login, which is the right way round.
    pub sheet:      Option<Box<CharacterSheet>>,
}

/// A character coming back from the save, exactly as it left.
///
/// # Why nothing here is optional
///
/// This replaced four `Option`s on [`Command::Enter`] that were only ever all
/// present or all absent together — the serial, the spot, the look and the
/// sheet. Four correlated `Option`s are four chances to build a state that
/// cannot happen: a saved serial with no saved position would put a character
/// that everything in the database refers to back at the shard's start city.
/// Nothing checked, because there was nothing to check against; the caller
/// simply had to unpack the record correctly every time, in every place that
/// unpacked one. Now the type says it, and [`from_record`](Self::from_record) is
/// the one place that does the unpacking.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct StoredCharacter {
    /// The wire serial it was saved under. It comes back on this one and no
    /// other: it is what every packet ever sent about this character said, and
    /// what its containers' contents point at.
    pub serial:     Serial,
    /// Which facet it stood on.
    pub facet:      Facet,
    /// Where on it, its own z included.
    pub position:   Point,
    /// Which way it was facing, and whether it was running.
    pub facing:     Facing,
    /// How it looked.
    pub appearance: Appearance,
    /// Its stats, skills, effects and quest log.
    pub sheet:      CharacterSheet,
}

impl StoredCharacter {
    /// Read a saved character out of its database row.
    ///
    /// The one place a [`CharacterRecord`](openshard_persistence::CharacterRecord)
    /// becomes something the world will accept, so the row format is unpacked
    /// once instead of at every call site that plays a character.
    ///
    /// Always `Some`: the record's `serial` is a checked [`Serial`], validated
    /// when the row was deserialised, so there is no longer an invalid-serial
    /// case to fail on here. `Option` is kept so `enter.rs`'s `.and_then` reads
    /// the same whether the record came from a lookup that could itself miss.
    pub(crate) fn from_record(record: &openshard_persistence::CharacterRecord) -> Option<Self> {
        Some(Self {
            serial:     record.serial,
            facet:      Facet(record.facet),
            position:   Point::new(record.x, record.y, record.z),
            facing:     Facing::from_bits(record.facing),
            appearance: Appearance {
                body: Graphic(record.body),
                hue:  openshard_protocol::wire::Hue(record.hue),
            },
            sheet:      CharacterSheet {
                strength:        record.strength,
                dexterity:       record.dexterity,
                intelligence:    record.intelligence,
                skills:          record
                    .skills
                    .iter()
                    .map(|skill| {
                        (
                            skill.id,
                            skill.value,
                            openshard_protocol::skill::SkillLock::from_bits(skill.lock),
                            skill.cap,
                        )
                    })
                    .collect(),
                stat_locks:      record.stat_locks,
                effects:         record.effects.clone(),
                dead:            record.dead,
                fame:            record.fame,
                karma:           record.karma,
                murders:         record.murders,
                quests:          record.quests.clone(),
                done_quests:     record.done_quests.clone(),
                // The three ride together: a title or a rank with no guild is a
                // fact about nothing, so the `Option` is over all of them.
                guild:           record.guild.map(|id| {
                    GuildSeat {
                        guild: openshard_state::GuildId(id),
                        title: record.guild_title.clone(),
                        // The floor, for a number the five ranks do not name. A
                        // saved row this engine did not write must not be able to
                        // grant a permission — see `CharacterRecord::guild_rank`.
                        rank:  openshard_state::Rank::from_number(record.guild_rank)
                            .unwrap_or(openshard_state::Rank::Ronin),
                    }
                }),
                guild_candidate: record.guild_candidate,
            },
        })
    }
}

/// Which of the two a client is entering as.
///
/// The distinction the four `Option`s used to encode between them, and the only
/// one the world needs: a stored character binds its saved serial and stands
/// where it stood, a fresh one takes a serial from the pool and the start city.
///
/// # Why [`Saved`](Self::Saved) carries nothing
///
/// It used to carry the row — the shard read its own roster, unpacked it with
/// `StoredCharacter::from_record` and sent the result along. Since S4 of
/// `docs/server/evidence/2026-07-30-the-connection-state-machine.md` the roster
/// is the world's, so the row is already
/// on the far side of this command and sending it would be sending the world
/// something it holds. `Saved` is therefore a *question*, not an answer: play
/// whatever is on file for this account and name. That the answer may be
/// "nothing on file" is not an error — a config-seeded name and a character
/// created this run and never logged out both reach the world that way, and both
/// enter fresh.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Character {
    /// Created on the character screen a moment ago: everything about it is on
    /// this command, because nothing about it is on file yet.
    Fresh(FreshCharacter),
    /// Whatever the world has on file for this account and name, or a bare fresh
    /// character if it has nothing.
    Saved,
}

impl Character {
    /// A character with nothing chosen and nothing saved — the world's default
    /// body, its flat default stats, no skills, and the start city of `facet`.
    ///
    /// What a test enters as when the character it is entering is not the
    /// subject. A real client never sends this: it either created the character
    /// a moment ago, which fills in a [`FreshCharacter`], or it picked one off
    /// the list, which is [`Saved`](Self::Saved) — and a `Saved` the roster has
    /// never heard of enters as exactly this.
    pub fn fresh(facet: Facet) -> Self {
        Self::Fresh(FreshCharacter {
            facet,
            start: None,
            appearance: None,
            sheet: None,
        })
    }
}

/// Everything [`World::enter`](crate::World) needs: who is connecting, on whose
/// account, and as which character.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entering {
    /// Which connection.
    pub connection: ConnectionId,
    /// What the client claims to be. The game socket never says, so this is the
    /// version the login socket carried across on the auth key.
    pub version:    ClientVersion,
    /// The account the character belongs to. Saved with it, so a load knows
    /// whose it is.
    pub account:    AccountName,
    /// The character's name.
    pub name:       CharacterName,
    /// The staff authority the account plays with. Re-derived from the account
    /// each login, never saved with the character.
    pub access:     AccessLevel,
    /// Whether it has been here before, and what it brings.
    pub character:  Character,
}

/// One door in a [`Command::Decorate`] batch. The closed/open graphics and the
/// hinge offset are already resolved by whoever places it (the pack does the
/// door-family arithmetic); the world only stores and toggles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecorDoor {
    /// Its lock, if it starts locked.
    pub lock:     Option<LockKind>,
    /// The shut graphic.
    pub closed:   Graphic,
    /// The open graphic.
    pub open:     Graphic,
    /// East/west hinge swing.
    pub offset_x: i16,
    /// North/south hinge swing.
    pub offset_y: i16,
    /// Where it sits, shut.
    pub position: Point,
}

/// One container in a [`Command::Decorate`] batch — a town chest or crate that
/// opens onto a gump.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecorContainer {
    /// Its lock, if it starts locked.
    pub lock:     Option<LockKind>,
    /// The item graphic.
    pub graphic:  Graphic,
    /// The gump the client opens for it.
    pub gump:     Graphic,
    /// Its hue, or 0.
    pub hue:      Hue,
    /// Where.
    pub position: Point,
}

/// Something for the world to do, from outside the world.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    /// A client finished its game login, and the world takes the connection over.
    ///
    /// The hand-off, and the first the world hears of a connection: the login
    /// conversation before this point is not the world's business — accounts,
    /// passwords and the relay key are not simulation — and everything after it
    /// is. See `docs/server/design_connection_state.md`, D1 and D2.
    ///
    /// Since S5 this is also what *answers* the login: applying it writes the
    /// connection's row and sends the `0xA9` character list (with the `0xB9`
    /// feature mask ahead of it, when the shard advertises one). The login crate
    /// ends here — it has no character list to send, because which characters
    /// exist is the roster's and the roster is the world's.
    ///
    /// The version rides along because the game socket never states it: it was
    /// read from the *login* socket's seed and carried across on the auth key,
    /// and this is the only path by which it reaches the world. The account and
    /// the access level ride along for the same reason — they are what the login
    /// conversation established, and every character-screen packet after this one
    /// names a character without naming whose it is.
    Authenticated {
        /// Which connection.
        connection: ConnectionId,
        /// What its client claims to be.
        version:    ClientVersion,
        /// Whose account it authenticated as.
        account:    AccountName,
        /// The staff authority that account plays with.
        access:     AccessLevel,
    },
    /// A client asked to create a character (`0x00`/`0xF8`) and play it.
    ///
    /// Both halves, because the client asks for both: the packet that names the
    /// character also picks its stats, skills, look and starting city, and a
    /// client that sent it is already waiting to be in the world. The world
    /// validates the name against the account's list, refuses with an `0x82` the
    /// client can render — a full account, an empty or duplicate name — and
    /// otherwise enrols it and enters it, all inside one tick.
    ///
    /// The account is not on the command: it is on the connection's row, put
    /// there by [`Authenticated`](Self::Authenticated). A creation that named its
    /// own account would be a client telling the shard whose character to make.
    CreateCharacter {
        /// Which connection.
        connection: ConnectionId,
        /// What the client filled in on the creation screen.
        create:     openshard_protocol::world::CreateCharacter,
    },
    /// A client picked a character off the list and wants to play it (`0x5D`).
    ///
    /// The name is the *raw* one the client echoed back, and it is validated
    /// against the account's list rather than trusted: a `0x5D` naming a
    /// character the account does not have is refused instead of entering a
    /// character nobody created.
    PlayCharacter {
        /// Which connection.
        connection: ConnectionId,
        /// The name the client echoed off the list it was sent.
        name:       RawCharacterName,
    },
    /// A client picked a character. One field, because [`Entering`] is what the
    /// world's own `enter` takes: the command used to spell out the same seven
    /// values that a private struct next to `enter` then spelled out again to
    /// receive them.
    Enter(Entering),
    /// A client asked to take a step.
    Walk {
        /// Which connection.
        connection: ConnectionId,
        /// The request.
        request:    WalkRequest,
    },
    /// An OpenShard client explicitly asked to turn without stepping.
    Turn {
        /// Which connection.
        connection: ConnectionId,
        /// The typed request.
        request:    openshard_protocol::world::TurnRequest,
    },
    /// A client asked for its own status again — a `0x34` type `0x04`, sent when
    /// the paperdoll opens. The status went out at world entry; this resends it so
    /// a paperdoll opened much later is not stale.
    RequestStatus {
        /// Which connection asked.
        connection: ConnectionId,
    },
    /// A client announced it is logging out — a `0xD1`. It waits for the ack
    /// before returning to the character screen, so a shard that never answers
    /// hangs it on "logging out" until it times out.
    LogoutRequest {
        /// Which connection asked.
        connection: ConnectionId,
    },
    /// A client lost track of the walk and asked where it is — a `0x22` *from the
    /// client*, which is a different packet from the `0x22` this server sends to
    /// acknowledge a step.
    ///
    /// The repair leg of the walk handshake, and the client is waiting on it: it
    /// stops sending steps when it asks, because a `0x22` ack carries no position
    /// and there is nothing local it could work the answer out from. A shard that
    /// drops this leaves that client unable to walk for the rest of the session.
    Resync {
        /// Which connection asked.
        connection: ConnectionId,
    },
    /// A client asked for its skill list — a `0x34` type `0x05`, sent when the
    /// skill window opens. Without this the window opens empty: the login list is
    /// long gone by the time a player clicks the skill button.
    RequestSkills {
        /// Which connection asked.
        connection: ConnectionId,
    },
    /// A client answered a gump — a `0xB1`. The world routes it to whatever opened
    /// the gump; today that is only the `.admin` menu.
    GumpResponse {
        /// Which connection answered.
        connection: ConnectionId,
        /// The decoded response: which gump, which button, and any fields.
        response:   GumpResponse,
    },
    /// A client answered a targeting cursor — a `0x6C`. Routed to whatever raised
    /// the cursor; today that is the `.tele` command.
    TargetResponse {
        /// Which connection answered.
        connection: ConnectionId,
        /// The decoded response: what was clicked, or a cancel.
        response:   openshard_protocol::target::TargetResponse,
    },
    /// Register a spawn region — an area the tick then keeps
    /// populated. See [`crate::spawner`].
    RegisterSpawner {
        /// The region to add.
        spawner: crate::spawner::Spawner,
    },
    /// Register a crop field — a patch of farmland the tick then keeps standing
    /// in cotton. See [`crate::crops`].
    ///
    /// Laid by the same `populate:` verb as a spawn region, and unlike one it is
    /// never saved: a boot re-lays the fields and they sow themselves.
    RegisterCropField {
        /// The field to add.
        field: crate::crops::CropField,
    },
    /// Remove every spawn region and despawn the creatures they were maintaining —
    /// what the admin menu's "Clear spawns" does. Takes the crop fields and their
    /// plants with it: they are content the same verb laid.
    ClearSpawners,
    /// Give a facet its named areas — towns, dungeons, guarded zones. Sent by
    /// whoever answers the `regions:` admin verb: `server::content`, from
    /// `state/data/regions.json`.
    ///
    /// Replaces whatever that facet had, the same replace-all the decoration and
    /// spawner sweeps use, so registering twice cannot leave half an old set
    /// behind — which is also what makes the button safe to press twice. See
    /// [`openshard_state::Region`].
    RegisterRegions {
        /// Which facet the areas belong to.
        facet:   Facet,
        /// The whole set.
        regions: Vec<openshard_state::Region>,
    },
    /// Forget a facet's regions — the admin menu's "Clear regions".
    ClearRegions {
        /// Which facet.
        facet: Facet,
    },
    /// Place a batch of decoration: script-added statics — signs, furniture — on
    /// top of the static art the map already draws, plus the interactive kinds:
    /// doors that open on double-click and containers that open onto a gump. See
    /// [`Decoration`], [`Door`] and [`Container`].
    Decorate {
        /// Which facet.
        facet:      Facet,
        /// The plain statics to place, as `(graphic, hue, position)`.
        statics:    Vec<(Graphic, Hue, Point)>,
        /// The doors to place.
        doors:      Vec<DecorDoor>,
        /// The containers to place.
        containers: Vec<DecorContainer>,
    },
    /// Remove every script-placed decoration.
    ClearDecorations,
    /// Generate functional doors from the map's static frames in a region — the
    /// shop doors a building's art only implies. See [`crate::doorgen`].
    GenerateDoors {
        /// Which facet.
        facet:  Facet,
        /// The region's north-west corner and size, in tiles.
        x:      u16,
        /// North-west corner y.
        y:      u16,
        /// Region width.
        width:  u16,
        /// Region height.
        height: u16,
    },
    /// The server moves a mobile one step — a script or AI decree, not a client
    /// request.
    ///
    /// Server-authoritative, so unlike [`Walk`](Self::Walk) there is no walk
    /// sequence to keep in step and no pace budget to spend: those exist to catch
    /// a *client* lying about how fast it moves, and the server is not lying to
    /// itself. Only the terrain gets a say. Turning is the step, exactly as it is
    /// for a client — a mobile not yet facing `direction` turns to face it and
    /// stays put, and the next `Step` moves it — because the clients watching
    /// animate the turn and the move the same way whoever ordered it.
    Step {
        /// Which mobile.
        serial:    Serial,
        /// Which way: the low three bits of a facing byte (0 N, clockwise).
        direction: u8,
    },
    /// The server puts an item on the ground — a script decree.
    ///
    /// Creates a new item entity on its own serial and draws it for everyone who
    /// can see the tile. The item's *rules* — whether it stacks, when it decays,
    /// what it does when used — are not here; this is only "a thing now lies at
    /// this spot", the item counterpart of a mobile entering.
    SpawnItem {
        /// The tiledata graphic id.
        graphic:   Graphic,
        /// Its hue, or 0 for none.
        hue:       Hue,
        /// How many, for a stackable item; 0 or 1 is a single.
        amount:    u16,
        /// Whether it merges with an identical pile when dropped onto one.
        stackable: bool,
        /// Where it lies.
        position:  Point,
        /// Which facet.
        facet:     Facet,
    },
    /// The server puts a container on the ground — a script decree, like
    /// [`SpawnItem`](Self::SpawnItem) but the thing can hold others.
    SpawnContainer {
        /// The tiledata graphic id (a chest, a backpack).
        graphic:  Graphic,
        /// The gump the client opens when it is double-clicked.
        gump:     Graphic,
        /// Its hue, or 0 for none.
        hue:      Hue,
        /// Where it lies.
        position: Point,
        /// Which facet.
        facet:    Facet,
    },
    /// The server puts a mobile in the world — a script decree. A creature to
    /// fight, a shopkeeper to stand there: an entity with a body and hit points
    /// but no client driving it.
    SpawnMobile {
        /// The body graphic (a creature id, or a human body).
        body:        Graphic,
        /// Its hue.
        hue:         Hue,
        /// Its starting and maximum hit points.
        hits:        u16,
        /// Its standing — the health-bar colour — as a wire byte (1 innocent, 5
        /// enemy, 7 invulnerable). Zero, or anything unknown, is innocent.
        notoriety:   Notoriety,
        /// How hard it hits in melee, before the target's armour.
        damage:      u16,
        /// Its physical resistance, 0–100.
        resistance:  PhysicalResistance,
        /// Ticks between its swings; 0 takes the default.
        swing:       u64,
        /// How far it notices a foe, in tiles; 0 hunts nothing.
        sight:       Sight,
        /// Whether it starts fights (2), answers them (1), or only runs (0).
        aggression:  Aggression,
        /// Ticks between its beats while hunting; 0 takes the shard default.
        beat:        u64,
        /// Its optional ranged attack reach.
        ranged:      Option<RangedRange>,
        /// The ranged attack's damage type.
        ranged_kind: DamageType,
        /// Whether it wanders when idle.
        wander:      bool,
        /// Where it stands.
        position:    Point,
        /// Which facet.
        facet:       Facet,
        /// A name shown on single-click, if any — a townsperson has one. Overrides
        /// `title`.
        name:        Option<String>,
        /// The trade it plies, ServUO-style ("the blacksmith"). `None` for a
        /// creature. The key its dress, its generated name and its speech all hang
        /// off, so it is kept on the mobile and saved with it.
        title:       Option<String>,
        /// What the trade wears on its feet, `ShoeType`'s wire byte. Read only when
        /// the core does the dressing.
        shoe:        u8,
        /// How widely known it is; a killer inherits it.
        fame:        i32,
        /// Which way it is known. Negative is evil.
        karma:       i32,
        /// Where it sleeps, for the optional daily routine.
        night_home:  Option<Point>,
        /// Whether it is a banker (answers "bank").
        banker:      bool,
        /// Whether it is a shopkeeper — double-click opens its shop.
        vendor:      bool,
        /// Whether it is a healer — a ghost that comes near or double-clicks it is
        /// offered a free resurrection.
        healer:      bool,
        /// Worn clothing and gear, as `(graphic, layer, hue)` — so an NPC is not
        /// naked. Drawn in its `0x78`.
        equipment:   Vec<(Graphic, Layer, Hue)>,
        /// Trained combat skills, `(skill id, value in tenths)` — what turns on the
        /// to-hit roll and damage scaling for the creature.
        skills:      Vec<(Skill, u16)>,
        /// What it sells, if it is a shopkeeper. Applied the moment it exists.
        ///
        /// # Why this rides on the spawn
        ///
        /// It used to arrive later, in a `StockVendor` keyed by serial — and the
        /// caller had no serial until the world answered with a `MobileSpawned`,
        /// so a script kept a table keyed by the *tile* an NPC stands on and
        /// looked it up when the event came back. Two round trips and a join key
        /// for something that is simply a fact about this shopkeeper. Content that
        /// knows the shop knows it at placement time; `Command::StockVendor` stays
        /// for a shop whose stock changes later.
        stock:       Vec<openshard_npc::StockLine>,
        /// Where it wants to be escorted to, if it is a traveller waiting for one.
        ///
        /// Empty means "wherever the quest picks" — ServUO's
        /// `PickRandomDestination` — which is not the same as `None`, meaning it
        /// is not escortable at all. Rides on the spawn for `stock`'s reason.
        escort_to:   Option<String>,
        /// The quests it offers, by key. Rides on the spawn for `stock`'s reason;
        /// [`BindQuestGiver`](Command::BindQuestGiver) stays for binding one to an
        /// NPC that already exists.
        ///
        /// An escortable traveller gets `escort` added to whatever is here,
        /// because an escort *is* a quest — the offer, the log entry and the
        /// reward all come from one.
        quests:      Vec<openshard_state::QuestKey>,
    },
    /// Deal damage to a mobile — a script or another mobile's blow.
    Damage {
        /// Whom.
        serial:      Serial,
        /// How much, before armour.
        amount:      u16,
        /// What kind, as a wire byte (0 physical, 1 fire, …). The target's
        /// resistance to that kind takes its cut.
        damage_type: u8,
        /// Who dealt it, or `None` for unattributed damage — the caster a script
        /// blames a spell's damage on, so killing a blue with it is a murder the
        /// same as a sword.
        by:          Option<Serial>,
    },
    /// Cast a spell: pay mana, roll the casting skill, and say what happened with
    /// a [`SpellCast`](openshard_magic::SpellCast). The spell's *effect* is a
    /// script's — this is only the mana-and-skill gate every spell passes.
    CastSpell {
        /// The caster.
        serial:    Serial,
        /// Which spell, by id.
        spell:     SpellId,
        /// The target, or `None` for a spell that needs none.
        target:    Option<Serial>,
        /// The mana it costs.
        mana:      u16,
        /// The lower edge of the skill band it is cast against, in tenths: below
        /// it the cast cannot succeed.
        min_skill: i32,
        /// The upper edge, in tenths: at or above it the cast cannot fail.
        max_skill: i32,
        /// The skill it rolls (Magery, and its id is the caller's to name).
        skill:     u8,
        /// The container to draw reagents from, or `None` for a spell that needs
        /// none. The caster's pack, in the usual case.
        pack:      Option<Serial>,
        /// The reagents the spell consumes, as `(graphic, count)`. All must be in
        /// the pack or the spell fizzles, spending nothing.
        reagents:  Vec<(Graphic, u16)>,
    },
    /// Heal a mobile — a spell's or a script's mending. Raises hit points toward
    /// the maximum and never past it.
    Heal {
        /// Whom.
        serial: Serial,
        /// By how much.
        amount: u16,
    },
    /// Set a mobile's stats — a script building a character or a monster.
    /// Strength and intelligence re-cap hit points and mana as they change.
    SetStats {
        /// Whose.
        serial:       Serial,
        /// Strength.
        strength:     u16,
        /// Dexterity.
        dexterity:    u16,
        /// Intelligence.
        intelligence: u16,
    },
    /// Set a mobile's skill value — a script configuring a character. `value` is
    /// in tenths, capped at that skill's own ceiling.
    SetSkill {
        /// Whose.
        serial: Serial,
        /// Which skill, by id.
        skill:  u8,
        /// The value in tenths.
        value:  u16,
    },
    /// Override a weapon item's speed and damage — a shard's magic sword.
    SetWeapon {
        /// The weapon item.
        serial: Serial,
        /// Swing-speed base (higher swings faster).
        speed:  u16,
        /// Minimum damage before resistance.
        min:    u16,
        /// Maximum damage before resistance.
        max:    u16,
    },
    /// Put poison on an item — a dose in a bottle, or a coating on a blade.
    ///
    /// The pack's door to the poison economy, and the only one: all four poison
    /// potions share a graphic (`0x0F0A`), so which poison a bottle holds cannot be
    /// keyed off a core table the way a weapon's damage is. A shard's alchemist
    /// stocks bottles and calls this on them; the Poisoning skill does the rest.
    /// `charges` of zero removes any poison instead.
    SetPoison {
        /// The item.
        serial:  Serial,
        /// The poison level, 0 (lesser) .. 4 (lethal).
        level:   PoisonLevel,
        /// Doses. One for a bottle; zero clears the poison.
        charges: u16,
    },
    /// Use a skill against a difficulty band: roll it, gain from it, and say what
    /// happened with a [`SkillUsed`](openshard_skills::SkillUsed) event.
    ///
    /// The band is ServUO's `CheckSkill(skill, minSkill, maxSkill)` — under the
    /// lower edge the attempt is beyond the mobile and fails without a draw, at
    /// the upper it is no challenge and succeeds without one, and how far between
    /// them the skill sits decides both the odds and how much is learned. Both
    /// edges are in tenths, like the skill, and may be negative.
    UseSkill {
        /// Whose.
        serial:    Serial,
        /// Which skill, by id.
        skill:     u8,
        /// The lower edge of the band, in tenths.
        min_skill: i32,
        /// The upper edge, in tenths.
        max_skill: i32,
    },
    /// A client pressed a skill's button on the window (`0x12` type `0x24`).
    ///
    /// Whether anything happens is `skills`' to decide — a passive skill answers
    /// "that skill cannot be used directly", a skill with behaviour emits a
    /// [`SkillRequested`](openshard_skills::SkillRequested) for whoever owns its
    /// effect.
    UseSkillButton {
        /// Which connection.
        connection: ConnectionId,
        /// Which skill, by id, exactly as sent — unchecked until
        /// `openshard_skills::use_skill_button` looks it up; the queue is a
        /// delivery, not a checkpoint.
        skill:      openshard_protocol::wire::RawSkillId,
    },
    /// Open the tool-free craft catalogue. The request has no body: recipes,
    /// pack, tool and workbench are all authoritative world state.
    OpenCraftCatalogue {
        /// Which player asked.
        connection: ConnectionId,
    },
    /// Search or open one result in the current house's read-only inventory.
    HouseInventory {
        connection: ConnectionId,
        request:    openshard_protocol::house_inventory::HouseInventoryRequest,
    },
    /// A client moved one of the status bar's stat arrows (`0xBF` `0x1A`).
    SetStatLock {
        /// Which connection.
        connection: ConnectionId,
        /// Which stat's arrow moved — one the status bar actually has, checked
        /// where the packet was read.
        stat:       openshard_protocol::mobile::Stat,
        /// The new arrow.
        lock:       openshard_state::StatLock,
    },
    /// A client moved a skill's up/down/lock arrow (`0x3A`).
    SetSkillLock {
        /// Which connection.
        connection: ConnectionId,
        /// Which skill, by id, exactly as sent — unchecked until
        /// `World::set_skill_lock` looks it up; the queue is a delivery, not a
        /// checkpoint.
        skill:      openshard_protocol::wire::RawSkillId,
        /// The new lock state.
        lock:       openshard_protocol::skill::SkillLock,
    },
    /// A client toggled war mode (`0x72`).
    WarMode {
        /// Which connection.
        connection: ConnectionId,
        /// True for war, false for peace.
        war:        bool,
    },
    /// A client asked to attack a mobile (`0x05`).
    Attack {
        /// Which connection.
        connection: ConnectionId,
        /// The target's serial, or none to clear the aim.
        target:     Option<openshard_protocol::serial::Serial>,
    },
    /// A client said something (`0x03`).
    ///
    /// Everything the client chose arrives unchecked — the promotion is
    /// `World::say`'s, which is the seam that acts on it.
    Say {
        /// Which connection.
        connection: ConnectionId,
        /// How it is said, as the client sent it.
        mode:       RawTalkMode,
        /// The colour the client chose.
        hue:        RawHue,
        /// The font the client chose.
        font:       RawFont,
        /// The words.
        text:       String,
    },
    /// A mobile speaks by decree — a script's NPC, or a keyword answer.
    Speak {
        /// Who.
        serial: Serial,
        /// The colour.
        hue:    Hue,
        /// The words.
        text:   String,
    },
    /// A client double-clicked an object (`0x06`) — for now, to open a container.
    DoubleClick {
        /// Which connection.
        connection: ConnectionId,
        /// What was asked for: a use, or a paperdoll. The serial inside it is
        /// still the client's — the queue is a delivery, not a checkpoint, so
        /// [`RawSerial::validate`](openshard_protocol::serial::RawSerial::validate)
        /// runs where the command is acted on.
        request:    UseRequest,
    },
    /// A client single-clicked something and wants its name (`0x09`).
    SingleClick {
        /// Which connection asked.
        connection: ConnectionId,
        /// The clicked object, by serial — checked where the packet was read, so
        /// this addresses something or the command was never queued.
        serial:     Serial,
    },
    /// A client asked for the AoS tooltip of one or more objects (`0xD6`).
    QueryProperties {
        /// Which connection asked.
        connection: ConnectionId,
        /// The objects whose tooltips are wanted, by serial.
        serials:    Vec<RawSerial>,
    },
    /// A client asked to open an object's context menu (`0xBF` `0x13`).
    ContextMenuRequest {
        /// Which connection asked.
        connection: ConnectionId,
        /// The object, as the client named it.
        serial:     RawSerial,
    },
    /// A client asked for a designed house's picture (`0xBF` `0x1E`).
    ///
    /// The middle of the three-packet design conversation: the shard announced a
    /// revision with the draw, the client did not hold it, and this is the ask.
    DesignDetails {
        /// Which connection asked.
        connection: ConnectionId,
        /// The house, as the client named it.
        serial:     RawSerial,
    },
    /// A client of ours asked for some of the ground (`0xBF` `0xE002`).
    ///
    /// Only a client of ours ever sends this — no reference client has a word
    /// for it — so receiving one *is* the capability negotiation. Every chunk
    /// named is answered exactly once, with its bytes or with a refusal; see
    /// [`openshard_protocol::chunks`].
    RequestChunks {
        /// Which connection asked.
        connection: ConnectionId,
        /// Which facet's ground, as the client named it. Not checked here: a
        /// facet this shard does not hold is a refusal the tick sends, and the
        /// queue is a delivery rather than a checkpoint.
        facet:      Facet,
        /// Which chunks of it, already capped at
        /// [`MAX_CHUNKS`](openshard_protocol::chunks::MAX_CHUNKS) by the
        /// decoder — a request over the cap never became a command.
        chunks:     Vec<openshard_protocol::chunks::ChunkAt>,
    },
    /// A client of ours asked what has moved since it last held this facet
    /// (`0xBF` `0xE007`).
    ///
    /// The question a client with a cache asks instead of fetching the world
    /// again, and it is answered exactly once — with the chunks that moved, or
    /// with "take the facet again". See [`openshard_protocol::chunks::Changes`].
    RequestChanges {
        /// Which connection asked.
        connection: ConnectionId,
        /// Which facet's ground, as the client named it. Not checked here, for
        /// [`RequestChunks`](Self::RequestChunks)'s reason.
        facet:      Facet,
        /// The revision the client says it already holds. An input and not an
        /// invariant: a revision this shard never published is one of the things
        /// the answer accounts for.
        revision:   openshard_protocol::chunks::WorldRevision,
    },
    /// A client of ours asked to commit a map-editor draft (`0xBF` `0xE009`).
    ///
    /// This carries no author or authority.  Both are read from `connection`'s
    /// authenticated row when the tick applies it, so neither can be forged by
    /// a packet.
    CommitMapEdit {
        /// Which authenticated connection submitted the draft.
        connection: ConnectionId,
        /// Facet, exact parent and bounded canonical operations.
        request:    openshard_protocol::mapedit::MapEditRequest,
    },
    /// A client picked a context-menu entry (`0xBF` `0x15`).
    ContextMenuSelect {
        /// Which connection asked.
        connection: ConnectionId,
        /// The object the menu was opened on, as the client named it.
        serial:     RawSerial,
        /// The chosen entry, by the tag the menu gave it. Checked against the
        /// entries the object offers in the tick, not here: the queue is a
        /// delivery, not a checkpoint.
        index:      openshard_protocol::context::RawContextMenuIndex,
    },
    /// A client asked its party to do something (`0xBF` `0x06`).
    ///
    /// The whole request travels rather than being split into seven commands:
    /// which of the seven it is already lives in
    /// [`PartyRequest`](openshard_protocol::party::PartyRequest), and a second
    /// enum with the same shape would be two places to add the eighth to.
    Party {
        /// Which connection asked.
        connection: ConnectionId,
        /// What it asked for.
        request:    openshard_protocol::party::PartyRequest,
    },
    /// A client asked to wear the item on its cursor (`0x13`).
    EquipItem {
        /// Which connection.
        connection: ConnectionId,
        /// The item to wear, as the client names it.
        item:       RawSerial,
        /// The layer the client proposes. Which slots may be worn into is
        /// `openshard_items`' rule, and it is applied there.
        layer:      RawLayer,
        /// The mobile to wear it — usually the player's own.
        mobile:     RawSerial,
    },
    /// A client asked to pick an item up onto its cursor (`0x07`).
    PickUpItem {
        /// Which connection.
        connection: ConnectionId,
        /// The item's serial, as the client names it. A lift the server refuses
        /// is answered with a `0x27`, so the check is at the seam and not here.
        serial:     RawSerial,
        /// How many of a stack to lift. Honoured for a ground pile — part is
        /// taken, the remainder left as a new dupe — and ignored for a contained
        /// or worn item, which lifts whole (the split there is still roadmap).
        amount:     u16,
    },
    /// A client asked to put the item on its cursor down (`0x08`).
    DropItem {
        /// Which connection.
        connection:  ConnectionId,
        /// The item's serial, as the client names it.
        serial:      RawSerial,
        /// Where it is going, already read as the destination means it — a world
        /// tile for the ground, a gump offset for a container, nothing at all
        /// for a mobile. The packet has one position field for all three; the
        /// choice is made once, in
        /// [`DropItem::destination`](openshard_protocol::items::DropItem::destination),
        /// so nothing below here can add a gump pixel to a map coordinate.
        /// [`Nowhere`](openshard_protocol::items::DropDestination::Nowhere)
        /// still owes the client a bounce: the item is on its cursor either way.
        destination: DropDestination,
    },
    /// A client acted on its secure trade window (`0x6F`).
    ///
    /// Cancelling and ticking the checkbox are the two the engine acts on; the
    /// virtual gold/platinum action is decoded and ignored, because gold is an
    /// item here and there is no account balance to move.
    TradeAction {
        /// Which connection.
        connection: ConnectionId,
        /// The escrow container the window is drawn on, as the client names it.
        /// Checked against the trades this side remembers opening — the queue is
        /// a delivery and not a checkpoint.
        container:  RawSerial,
        /// Whether the checkbox is now ticked. Cancelling is
        /// [`TradeCancel`](Self::TradeCancel).
        accepted:   bool,
    },
    /// A client closed its secure trade window (`0x6F` action 1).
    TradeCancel {
        /// Which connection.
        connection: ConnectionId,
        /// The escrow container the window is drawn on, as the client names it.
        container:  RawSerial,
    },
    /// A connection went away.
    Disconnect {
        /// Which connection.
        connection: ConnectionId,
    },
    /// Delete a character from the character-select screen (`0x83`).
    ///
    /// Everything the world has under that name goes: its place on the account's
    /// list, the record a re-login would read, the store row, and the saved
    /// inventory — so the deletion reaches the store on the next save. The serial
    /// stays reserved, since a packet still in flight may name it.
    ///
    /// # By slot, and why that is safe now
    ///
    /// The slot indexes the list the client was last sent, so it is only as good
    /// as the two ends agreeing on what that list was. They agree by construction
    /// since S5: `0xA9` is built from the roster and so is this lookup, out of one
    /// value in one process. It used to index the login crate's copy while the
    /// client had been shown — potentially — another order entirely.
    ///
    /// The account is not on the command; it is on the connection's row. So is
    /// the answer the client gets: a refusal (`0x85`) when the slot names no
    /// character or somebody is playing it, and the redrawn list (`0x86`)
    /// otherwise.
    DeleteCharacter {
        /// Which connection asked.
        connection: ConnectionId,
        /// Which slot of the list it was last sent.
        slot:       RawCharacterSlot,
    },
    /// Show a gump to a mobile's client — a content-built dialog (a quest offer). The
    /// reply comes back as a [`GumpAnswered`](crate::events::GumpAnswered) event.
    ShowGump {
        /// Who sees it.
        serial:  Serial,
        /// The gump id the reply is keyed on. A [`GumpId`] and not a raw one:
        /// the pack chose it, and the script bridge is where its JSON number
        /// became a type — the same seam `Command::Speak` crosses.
        gump_id: openshard_protocol::gump::GumpId,
        /// Where the window opens on the screen.
        at:      openshard_protocol::gump::GumpPoint,
        /// The gump layout string.
        layout:  String,
        /// The text lines the layout indexes into.
        lines:   Vec<String>,
    },
    /// Replace every trade's speech.
    /// Queued at load time.
    RegisterNpcSpeech {
        /// Each trade's table, keyed by the title its NPCs wear.
        trades: Vec<(String, openshard_state::SpeechTable)>,
    },
    /// Replace every quest this shard knows. Queued at load time.
    RegisterQuests {
        /// The quests.
        quests: Vec<openshard_state::quest::QuestDef>,
    },
    /// Mark an NPC as offering a set of quests, and save that with it. From a
    /// script.
    BindQuestGiver {
        /// Which NPC.
        serial: Serial,
        /// Which quests, by key. Empty un-binds it.
        keys:   Vec<openshard_state::QuestKey>,
    },
    /// Mark an NPC as escortable, and save that with it.
    MakeEscortable {
        /// Which NPC.
        serial:      Serial,
        /// The region it wants to reach; empty lets the quest decide.
        destination: String,
    },
    /// A client pressed the paperdoll's Quest button (`0xD7`/`0x32`) — open its
    /// quest log.
    QuestLogRequest {
        /// Whose.
        connection: ConnectionId,
    },
    /// A client pressed the paperdoll's Guild button (`0xD7`/`0x28`) — open the
    /// guild window.
    GuildWindowRequest {
        /// Whose.
        connection: ConnectionId,
    },
    /// A client closed its house-design window (`0xD7`/`0x0C`) — end whatever
    /// design session it had open.
    ///
    /// There is no opposite command: a session *begins* from the house's own
    /// window, server-side, exactly as the reference's `BeginCustomize` does.
    EndDesignSession {
        /// Whose.
        connection: ConnectionId,
    },
    /// A client changed the design it has open (`0xD7`/`0x05`, `0x06`, `0x12`)
    /// — lay a piece, take one away, or move to another storey.
    ///
    /// One command for the three because the wire is one packet with three
    /// verbs, and because every one of them is answered the same way: the
    /// working copy changes and nothing else does. Which house it is about is
    /// not on the wire and is not here — the shard knows what this connection
    /// has open, and a client asserting it would be asking the wrong end.
    DesignEdit {
        /// Whose.
        connection: ConnectionId,
        /// Which verb, with what it names.
        edit:       openshard_protocol::encoded::DesignEdit,
    },
    /// A client committed the design it has open (`0xD7`/`0x04`) — make the
    /// working copy the house's shape and close the editor over it.
    ///
    /// Carries no design, for [`DesignEdit`](Self::DesignEdit)'s reason turned
    /// up one notch: the shape that commits is the one the shard has been
    /// keeping, and a client that named its own would be committing something
    /// nothing checked.
    CommitDesign {
        /// Whose.
        connection: ConnectionId,
    },
    /// A client reverted the design it has open (`0xD7`/`0x1A`) — throw the
    /// working copy away and start again from the house as it stands.
    RevertDesign {
        /// Whose.
        connection: ConnectionId,
    },
    /// Close an open gump on a player's client — the dialog a page chain is
    /// replacing.
    CloseGump {
        /// Whose client.
        serial:  Serial,
        /// Which dialog, by the id it was opened under.
        gump_id: openshard_protocol::gump::GumpId,
    },
    /// Send a player a private system line.
    Message {
        /// Who reads it.
        serial: Serial,
        /// The words.
        text:   String,
    },
    /// Play a sound for one player.
    PlaySound {
        /// Who hears it.
        serial: Serial,
        /// The sound id. The script bridge is where the raw JSON number becomes
        /// this type — the same seam `Command::Speak` crosses for its `Hue`.
        sound:  openshard_protocol::wire::SoundId,
    },
    /// Put an item into a player's backpack — a quest reward.
    GiveItem {
        /// Whose backpack.
        serial:    Serial,
        /// The item graphic.
        graphic:   Graphic,
        /// Its hue, or 0.
        hue:       Hue,
        /// How many.
        amount:    u16,
        /// Whether it merges onto a like pile.
        stackable: bool,
    },
    /// Put a semantic item identity into a player's backpack. New scripts use
    /// this instead of selecting a classic graphic/hue projection.
    GiveItemKind {
        /// Whose backpack.
        serial:    Serial,
        /// The item type to award.
        item_kind: ItemKindId,
        /// Its material, when the item kind declares a material family.
        material:  Option<MaterialId>,
        /// How many.
        amount:    u16,
        /// Whether compatible piles merge.
        stackable: bool,
    },
    /// Take up to `amount` of a graphic from a player's backpack — a quest
    /// collect turn-in. All-or-nothing; reports back with an
    /// [`ItemsTaken`](crate::ItemsTaken) event.
    TakeItem {
        /// Whose backpack.
        serial:  Serial,
        /// The item graphic to take.
        graphic: Graphic,
        /// How many to take.
        amount:  u16,
    },
    /// Take an exact semantic item identity from a player's backpack — the
    /// migration-safe counterpart of [`Self::TakeItem`]. The request accepts a
    /// legacy pile only when it has the registry's audited presentation for the
    /// same kind/material pair.
    TakeItemKind {
        /// Whose backpack.
        serial:    Serial,
        /// The semantic item kind to take.
        item_kind: ItemKindId,
        /// The required material, if this kind is materialized.
        material:  Option<MaterialId>,
        /// How many to take.
        amount:    u16,
    },
    /// A client asked to cast a spell (from its spellbook or a macro). The world
    /// only says it happened, via [`SpellRequested`]; a script does the casting.
    RequestCast {
        /// Which connection asked.
        connection: ConnectionId,
        /// Which spell, zero-based.
        spell:      SpellId,
    },
    /// Fill a vendor's stock crate with priced goods.
    StockVendor {
        /// The vendor mobile.
        serial: Serial,
        /// The goods, priced and labelled.
        stock:  Vec<npc::StockLine>,
    },
    /// Put an item into a container — a pack filling a corpse with loot off a
    /// [`CorpseCreated`](crate::events::CorpseCreated) event.
    AddLoot {
        /// The container — a corpse, a chest.
        container: Serial,
        /// The item graphic.
        graphic:   Graphic,
        /// Its hue, or 0.
        hue:       Hue,
        /// How many; a stackable merges, a single is one item.
        amount:    u16,
        /// Whether it stacks (gold, reagents, arrows) or is a discrete piece
        /// (a weapon, a suit of armour).
        stackable: bool,
    },
    /// Put a semantic item identity into a container. New death/quest scripts
    /// use this instead of selecting a classic graphic/hue projection.
    AddLootKind {
        /// The container — a corpse, a chest.
        container: Serial,
        /// The semantic item kind to place.
        item_kind: ItemKindId,
        /// Its material, when the item kind declares one.
        material:  Option<MaterialId>,
        /// How many.
        amount:    u16,
        /// Whether compatible piles merge.
        stackable: bool,
    },
    /// Remove an item by serial, wherever it lives — a used item vanishing (a
    /// drunk potion, a read-once scroll).
    ConsumeItem {
        /// The item.
        serial: Serial,
        /// How many to take: 0 (or the whole stack) removes the item; a smaller
        /// amount decrements a stackable pile.
        amount: u16,
    },
    /// A client bought from a vendor's shop (`0x3B`).
    Buy {
        /// Which connection.
        connection: ConnectionId,
        /// The vendor mobile, as the client named it — checked in
        /// `openshard_npc::vendor::buy`, which is the seam. The queue is a
        /// delivery and not a checkpoint; see
        /// `docs/protocol/evidence/2026-08-31-the-newtype-sweep.md`'s N-commands
        /// amendments.
        vendor:     openshard_protocol::serial::RawSerial,
        /// What it took, by stock serial and amount.
        purchases:  Vec<openshard_protocol::vendor::Purchase>,
    },
    /// A client sold to a vendor (`0x9F`).
    Sell {
        /// Which connection.
        connection: ConnectionId,
        /// The vendor mobile, as the client named it — checked in
        /// `openshard_npc::vendor::sell`.
        vendor:     openshard_protocol::serial::RawSerial,
        /// What it let go, by item serial and amount.
        sales:      Vec<openshard_protocol::vendor::Sale>,
    },
}

impl Command {
    /// Stable, value-free name for tick diagnostics.
    ///
    /// Values are deliberately excluded: commands carry chat, character names
    /// and large content batches that do not belong in a watchdog line. Keeping
    /// this exhaustive also means a new kind of tick work cannot silently show
    /// up as "unknown" when it is the thing making a tick slow.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Authenticated { .. } => "Authenticated",
            Self::CreateCharacter { .. } => "CreateCharacter",
            Self::PlayCharacter { .. } => "PlayCharacter",
            Self::Enter(_) => "Enter",
            Self::Walk { .. } => "Walk",
            Self::Turn { .. } => "Turn",
            Self::RequestStatus { .. } => "RequestStatus",
            Self::LogoutRequest { .. } => "LogoutRequest",
            Self::Resync { .. } => "Resync",
            Self::RequestSkills { .. } => "RequestSkills",
            Self::GumpResponse { .. } => "GumpResponse",
            Self::TargetResponse { .. } => "TargetResponse",
            Self::RegisterSpawner { .. } => "RegisterSpawner",
            Self::RegisterCropField { .. } => "RegisterCropField",
            Self::ClearSpawners => "ClearSpawners",
            Self::RegisterRegions { .. } => "RegisterRegions",
            Self::ClearRegions { .. } => "ClearRegions",
            Self::Decorate { .. } => "Decorate",
            Self::GenerateDoors { .. } => "GenerateDoors",
            Self::ClearDecorations => "ClearDecorations",
            Self::Step { .. } => "Step",
            Self::SpawnItem { .. } => "SpawnItem",
            Self::SpawnContainer { .. } => "SpawnContainer",
            Self::SpawnMobile { .. } => "SpawnMobile",
            Self::Damage { .. } => "Damage",
            Self::CastSpell { .. } => "CastSpell",
            Self::Heal { .. } => "Heal",
            Self::SetStats { .. } => "SetStats",
            Self::SetSkill { .. } => "SetSkill",
            Self::SetWeapon { .. } => "SetWeapon",
            Self::SetPoison { .. } => "SetPoison",
            Self::UseSkill { .. } => "UseSkill",
            Self::UseSkillButton { .. } => "UseSkillButton",
            Self::OpenCraftCatalogue { .. } => "OpenCraftCatalogue",
            Self::HouseInventory { .. } => "HouseInventory",
            Self::SetStatLock { .. } => "SetStatLock",
            Self::SetSkillLock { .. } => "SetSkillLock",
            Self::WarMode { .. } => "WarMode",
            Self::Attack { .. } => "Attack",
            Self::Say { .. } => "Say",
            Self::Speak { .. } => "Speak",
            Self::DoubleClick { .. } => "DoubleClick",
            Self::SingleClick { .. } => "SingleClick",
            Self::QueryProperties { .. } => "QueryProperties",
            Self::ContextMenuRequest { .. } => "ContextMenuRequest",
            Self::DesignDetails { .. } => "DesignDetails",
            Self::RequestChunks { .. } => "RequestChunks",
            Self::RequestChanges { .. } => "RequestChanges",
            Self::CommitMapEdit { .. } => "CommitMapEdit",
            Self::ContextMenuSelect { .. } => "ContextMenuSelect",
            Self::Party { .. } => "Party",
            Self::EquipItem { .. } => "EquipItem",
            Self::PickUpItem { .. } => "PickUpItem",
            Self::DropItem { .. } => "DropItem",
            Self::TradeAction { .. } => "TradeAction",
            Self::TradeCancel { .. } => "TradeCancel",
            Self::Disconnect { .. } => "Disconnect",
            Self::DeleteCharacter { .. } => "DeleteCharacter",
            Self::ShowGump { .. } => "ShowGump",
            Self::RegisterNpcSpeech { .. } => "RegisterNpcSpeech",
            Self::RegisterQuests { .. } => "RegisterQuests",
            Self::BindQuestGiver { .. } => "BindQuestGiver",
            Self::MakeEscortable { .. } => "MakeEscortable",
            Self::QuestLogRequest { .. } => "QuestLogRequest",
            Self::GuildWindowRequest { .. } => "GuildWindowRequest",
            Self::EndDesignSession { .. } => "EndDesignSession",
            Self::DesignEdit { .. } => "DesignEdit",
            Self::CommitDesign { .. } => "CommitDesign",
            Self::RevertDesign { .. } => "RevertDesign",
            Self::CloseGump { .. } => "CloseGump",
            Self::Message { .. } => "Message",
            Self::PlaySound { .. } => "PlaySound",
            Self::GiveItem { .. } => "GiveItem",
            Self::GiveItemKind { .. } => "GiveItemKind",
            Self::TakeItem { .. } => "TakeItem",
            Self::TakeItemKind { .. } => "TakeItemKind",
            Self::RequestCast { .. } => "RequestCast",
            Self::StockVendor { .. } => "StockVendor",
            Self::AddLoot { .. } => "AddLoot",
            Self::AddLootKind { .. } => "AddLootKind",
            Self::ConsumeItem { .. } => "ConsumeItem",
            Self::Buy { .. } => "Buy",
            Self::Sell { .. } => "Sell",
        }
    }
}
