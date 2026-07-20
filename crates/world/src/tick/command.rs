use super::*;

/// How a freshly created character looks: its body graphic and hue.
///
/// [`Command::Enter`] carries this when the client just made the character and
/// chose it. Playing an existing one carries `None`, and the world falls back to
/// its default body until persistence can supply the stored appearance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Appearance {
    /// The body graphic id.
    pub body: u16,
    /// The skin hue.
    pub hue: u16,
}

/// A character's stats and skills, carried on [`Command::Enter`] — chosen at
/// creation for a new character, or restored from the save for a played one.
/// `None` (a bare test enter, or a character from before these were stored) takes
/// the world's flat defaults and no skills.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CharacterSheet {
    /// Strength.
    pub strength: u16,
    /// Dexterity.
    pub dexterity: u16,
    /// Intelligence.
    pub intelligence: u16,
    /// Trained skills as `(id, value in tenths, lock)`.
    pub skills: Vec<(u8, u16, openshard_protocol::SkillLock)>,
    /// Active effects — a poison a relog must not wash off, and the buffs and
    /// debuffs that will join it. Empty for a clean character.
    pub effects: Vec<openshard_persistence::EffectRecord>,
}

/// One door in a [`Command::Decorate`] batch. The closed/open graphics and the
/// hinge offset are already resolved by whoever places it (the pack does the
/// door-family arithmetic); the world only stores and toggles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecorDoor {
    /// The shut graphic.
    pub closed: u16,
    /// The open graphic.
    pub open: u16,
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
    /// The item graphic.
    pub graphic: u16,
    /// The gump the client opens for it.
    pub gump: u16,
    /// Its hue, or 0.
    pub hue: u16,
    /// Where.
    pub position: Point,
}

/// Something for the world to do, from outside the world.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    /// A client picked a character.
    Enter {
        /// Which connection.
        connection: ConnectionId,
        /// What the client claims to be.
        version: ClientVersion,
        /// The account the character belongs to. Saved with the character so a
        /// load knows whose it is.
        account: String,
        /// The character's name.
        name: String,
        /// The saved wire serial, when a stored character is being played. `None`
        /// creates a fresh one — a character being made for the first time. A
        /// played character must come back with the serial it was saved under,
        /// because that serial is what every packet ever sent about it referred to.
        serial: Option<u32>,
        /// Where to spawn, when a stored character is loaded at its saved spot.
        /// `None` uses the world's configured start — a newly created character,
        /// or a played one from before positions were stored.
        position: Option<Point>,
        /// Which facet to spawn on: a stored character's saved one, or the
        /// world's default for a new character. An unloaded facet falls back to
        /// the default.
        facet: u8,
        /// How the character looks: chosen at creation, or restored from the save.
        /// `None` falls back to the default body.
        appearance: Option<Appearance>,
        /// The character's stats and skills, from creation or the save. `None`
        /// takes the world's defaults.
        sheet: Option<CharacterSheet>,
        /// The staff authority the account plays with — what privileged commands
        /// its characters may run. Re-derived from the account each login, never
        /// saved with the character.
        access: AccessLevel,
    },
    /// A client asked to take a step.
    Walk {
        /// Which connection.
        connection: ConnectionId,
        /// The request.
        request: WalkRequest,
    },
    /// A client asked for its own status again — a `0x34` type `0x04`, sent when
    /// the paperdoll opens. The status went out at world entry; this resends it so
    /// a paperdoll opened much later is not stale.
    RequestStatus {
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
        response: openshard_protocol::GumpResponse,
    },
    /// A client answered a targeting cursor — a `0x6C`. Routed to whatever raised
    /// the cursor; today that is the `.tele` command.
    TargetResponse {
        /// Which connection answered.
        connection: ConnectionId,
        /// The decoded response: what was clicked, or a cancel.
        response: openshard_protocol::TargetResponse,
    },
    /// The script pack registers a spawn region — an area the tick then keeps
    /// populated. See [`crate::spawner`].
    RegisterSpawner {
        /// The region to add.
        spawner: crate::spawner::Spawner,
    },
    /// Remove every spawn region and despawn the creatures they were maintaining —
    /// what the admin menu's "Clear spawns" does.
    ClearSpawners,
    /// Place a batch of decoration: script-added statics — signs, furniture — on
    /// top of the static art the map already draws, plus the interactive kinds:
    /// doors that open on double-click and containers that open onto a gump. See
    /// [`Decoration`], [`Door`] and [`Container`].
    Decorate {
        /// Which facet.
        facet: u8,
        /// The plain statics to place, as `(graphic, hue, position)`.
        statics: Vec<(u16, u16, Point)>,
        /// The doors to place.
        doors: Vec<DecorDoor>,
        /// The containers to place.
        containers: Vec<DecorContainer>,
    },
    /// Remove every script-placed decoration.
    ClearDecorations,
    /// Generate functional doors from the map's static frames in a region — the
    /// shop doors a building's art only implies. See [`crate::doorgen`].
    GenerateDoors {
        /// Which facet.
        facet: u8,
        /// The region's north-west corner and size, in tiles.
        x: u16,
        /// North-west corner y.
        y: u16,
        /// Region width.
        width: u16,
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
        /// Which mobile, by wire serial.
        serial: u32,
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
        graphic: u16,
        /// Its hue, or 0 for none.
        hue: u16,
        /// How many, for a stackable item; 0 or 1 is a single.
        amount: u16,
        /// Whether it merges with an identical pile when dropped onto one.
        stackable: bool,
        /// Where it lies.
        position: Point,
        /// Which facet.
        facet: u8,
    },
    /// The server puts a container on the ground — a script decree, like
    /// [`SpawnItem`](Self::SpawnItem) but the thing can hold others.
    SpawnContainer {
        /// The tiledata graphic id (a chest, a backpack).
        graphic: u16,
        /// The gump the client opens when it is double-clicked.
        gump: u16,
        /// Its hue, or 0 for none.
        hue: u16,
        /// Where it lies.
        position: Point,
        /// Which facet.
        facet: u8,
    },
    /// The server puts a mobile in the world — a script decree. A creature to
    /// fight, a shopkeeper to stand there: an entity with a body and hit points
    /// but no client driving it.
    SpawnMobile {
        /// The body graphic (a creature id, or a human body).
        body: u16,
        /// Its hue.
        hue: u16,
        /// Its starting and maximum hit points.
        hits: u16,
        /// Its standing — the health-bar colour — as a wire byte (1 innocent, 5
        /// enemy, 7 invulnerable). Zero, or anything unknown, is innocent.
        notoriety: u8,
        /// How hard it hits in melee, before the target's armour.
        damage: u16,
        /// Its physical resistance, 0–100.
        resistance: u8,
        /// Ticks between its swings; 0 takes the default.
        swing: u64,
        /// How far it notices a foe, in tiles; 0 hunts nothing.
        sight: u8,
        /// Whether it starts fights (2), answers them (1), or only runs (0).
        aggression: u8,
        /// Ticks between its beats while hunting; 0 takes the shard default.
        beat: u64,
        /// How far its ranged attack reaches, in tiles; 0 fights hand to hand.
        ranged: u8,
        /// The ranged attack's damage type (see `DamageType::from_u8`).
        ranged_kind: u8,
        /// Whether it wanders when idle.
        wander: bool,
        /// Where it stands.
        position: Point,
        /// Which facet.
        facet: u8,
        /// A name shown on single-click, if any — a townsperson has one.
        name: Option<String>,
        /// Whether it is a banker (answers "bank").
        banker: bool,
        /// Whether it is a shopkeeper — double-click opens its shop.
        vendor: bool,
        /// Worn clothing and gear, as `(graphic, layer, hue)` — so an NPC is not
        /// naked. Drawn in its `0x78`.
        equipment: Vec<(u16, u8, u16)>,
    },
    /// Deal damage to a mobile — a script or another mobile's blow.
    Damage {
        /// Whom, by wire serial.
        serial: u32,
        /// How much, before armour.
        amount: u16,
        /// What kind, as a wire byte (0 physical, 1 fire, …). The target's
        /// resistance to that kind takes its cut.
        damage_type: u8,
        /// Who dealt it, by wire serial, or zero for unattributed damage — the
        /// caster a script blames a spell's damage on, so killing a blue with it
        /// is a murder the same as a sword.
        by: u32,
    },
    /// Cast a spell: pay mana, roll the casting skill, and say what happened with
    /// a [`SpellCast`](openshard_magic::SpellCast). The spell's *effect* is a
    /// script's — this is only the mana-and-skill gate every spell passes.
    CastSpell {
        /// The caster's serial.
        serial: u32,
        /// Which spell, by id.
        spell: u16,
        /// The target's serial, or zero for a spell that needs none.
        target: u32,
        /// The mana it costs.
        mana: u16,
        /// The casting difficulty, 0–100.
        difficulty: u16,
        /// The skill it rolls (Magery, and its id is the caller's to name).
        skill: u8,
        /// The container to draw reagents from, or zero for a spell that needs
        /// none. The caster's pack, in the usual case.
        pack: u32,
        /// The reagents the spell consumes, as `(graphic, count)`. All must be in
        /// the pack or the spell fizzles, spending nothing.
        reagents: Vec<(u16, u16)>,
    },
    /// Heal a mobile — a spell's or a script's mending. Raises hit points toward
    /// the maximum and never past it.
    Heal {
        /// Whom.
        serial: u32,
        /// By how much.
        amount: u16,
    },
    /// Set a mobile's stats — a script building a character or a monster.
    /// Strength and intelligence re-cap hit points and mana as they change.
    SetStats {
        /// Whose, by wire serial.
        serial: u32,
        /// Strength.
        strength: u16,
        /// Dexterity.
        dexterity: u16,
        /// Intelligence.
        intelligence: u16,
    },
    /// Set a mobile's skill value — a script configuring a character. `value` is
    /// in tenths, capped at [`SKILL_CAP`](openshard_skills::SKILL_CAP).
    SetSkill {
        /// Whose, by wire serial.
        serial: u32,
        /// Which skill, by id.
        skill: u8,
        /// The value in tenths.
        value: u16,
    },
    /// Use a skill against a difficulty (0–100): roll it, gain from it, and say
    /// what happened with a [`SkillUsed`](openshard_skills::SkillUsed) event.
    UseSkill {
        /// Whose, by wire serial.
        serial: u32,
        /// Which skill, by id.
        skill: u8,
        /// The difficulty, 0–100.
        difficulty: u16,
    },
    /// A client moved a skill's up/down/lock arrow (`0x3A`).
    SetSkillLock {
        /// Which connection.
        connection: ConnectionId,
        /// Which skill, by id.
        skill: u8,
        /// The new lock state.
        lock: openshard_protocol::SkillLock,
    },
    /// A client toggled war mode (`0x72`).
    WarMode {
        /// Which connection.
        connection: ConnectionId,
        /// True for war, false for peace.
        war: bool,
    },
    /// A client asked to attack a mobile (`0x05`).
    Attack {
        /// Which connection.
        connection: ConnectionId,
        /// The target's serial.
        target: u32,
    },
    /// A client said something (`0x03`).
    Say {
        /// Which connection.
        connection: ConnectionId,
        /// How it is said (mode byte).
        mode: u8,
        /// The colour.
        hue: u16,
        /// The font.
        font: u16,
        /// The words.
        text: String,
    },
    /// A mobile speaks by decree — a script's NPC, or a keyword answer.
    Speak {
        /// Who, by wire serial.
        serial: u32,
        /// The colour.
        hue: u16,
        /// The words.
        text: String,
    },
    /// A client double-clicked an object (`0x06`) — for now, to open a container.
    DoubleClick {
        /// Which connection.
        connection: ConnectionId,
        /// The object's serial.
        serial: u32,
    },
    /// A client single-clicked something and wants its name (`0x09`).
    SingleClick {
        /// Which connection asked.
        connection: ConnectionId,
        /// The clicked object, by serial.
        serial: u32,
    },
    /// A client asked for the AoS tooltip of one or more objects (`0xD6`).
    QueryProperties {
        /// Which connection asked.
        connection: ConnectionId,
        /// The objects whose tooltips are wanted, by serial.
        serials: Vec<u32>,
    },
    /// A client asked to open an object's context menu (`0xBF` `0x13`).
    ContextMenuRequest {
        /// Which connection asked.
        connection: ConnectionId,
        /// The object, by serial.
        serial: u32,
    },
    /// A client picked a context-menu entry (`0xBF` `0x15`).
    ContextMenuSelect {
        /// Which connection asked.
        connection: ConnectionId,
        /// The object the menu was opened on.
        serial: u32,
        /// The chosen entry, by its tag (its position in the list).
        index: u16,
    },
    /// A client asked to wear the item on its cursor (`0x13`).
    EquipItem {
        /// Which connection.
        connection: ConnectionId,
        /// The item to wear.
        item: u32,
        /// The layer to wear it on.
        layer: u8,
        /// The mobile to wear it — usually the player's own.
        mobile: u32,
    },
    /// A client asked to pick an item up onto its cursor (`0x07`).
    PickUpItem {
        /// Which connection.
        connection: ConnectionId,
        /// The item's serial.
        serial: u32,
        /// How many of a stack to lift. Honoured for a ground pile — part is
        /// taken, the remainder left as a new dupe — and ignored for a contained
        /// or worn item, which lifts whole (the split there is still roadmap).
        amount: u16,
    },
    /// A client asked to put the item on its cursor down (`0x08`).
    DropItem {
        /// Which connection.
        connection: ConnectionId,
        /// The item's serial, as the client names it.
        serial: u32,
        /// Where, for a ground drop.
        position: Point,
        /// Where it is going: a container serial, a mobile to equip on, or
        /// [`DROP_TO_GROUND`](openshard_protocol::DROP_TO_GROUND). Only ground
        /// drops are handled yet; anything else is bounced.
        container: u32,
    },
    /// A connection went away.
    Disconnect {
        /// Which connection.
        connection: ConnectionId,
    },
    /// Hand a mobile's brain to the script: the built-in `ai` stops driving it and
    /// its `onTick` takes over. A script controls a creature it spawned.
    Control {
        /// The mobile, by wire serial.
        serial: u32,
    },
    /// A client asked to cast a spell (from its spellbook or a macro). The world
    /// only says it happened, via [`SpellRequested`]; a script does the casting.
    RequestCast {
        /// Which connection asked.
        connection: ConnectionId,
        /// Which spell, zero-based.
        spell: u16,
    },
    /// Fill a vendor's stock crate with priced goods. From a script.
    StockVendor {
        /// The vendor mobile's wire serial.
        serial: u32,
        /// The goods, priced and labelled.
        stock: Vec<npc::StockLine>,
    },
    /// A client bought from a vendor's shop (`0x3B`).
    Buy {
        /// Which connection.
        connection: ConnectionId,
        /// The vendor mobile's wire serial.
        vendor: u32,
        /// What it took, by stock serial and amount.
        purchases: Vec<openshard_protocol::Purchase>,
    },
    /// A client sold to a vendor (`0x9F`).
    Sell {
        /// Which connection.
        connection: ConnectionId,
        /// The vendor mobile's wire serial.
        vendor: u32,
        /// What it let go, by item serial and amount.
        sales: Vec<openshard_protocol::Sale>,
    },
}
