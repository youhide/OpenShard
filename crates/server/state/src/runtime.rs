//! The world's runtime state: the data a tick reads and writes.
//!
//! [`WorldState`] gathers everything a gameplay system touches — the registry,
//! the event bus, the spatial index, the seeded generator, who is on each
//! client's screen — into one value that lives *below* the systems that act on
//! it. That is what lets a system be a function in its own crate
//! (`combat::swings(&mut WorldState)`) rather than a method on a single
//! ever-growing world object.
//!
//! What is deliberately *not* here: the tick itself, the persistence journal,
//! and the client's map files. Those sit above, in `openshard-world`, which owns
//! a `WorldState` and drives it. This crate knows the shape of world state and
//! nothing about when it changes or how it is saved.

use std::collections::{BTreeMap, HashMap, HashSet};

use openshard_commands::StaffCommand;
use openshard_config::CombatEra;
use openshard_entities::{EntityId, Registry};
use openshard_events::EventBus;
use openshard_gateway::ConnectionId;
use openshard_map::grid::Tile;
use openshard_map::overlay::{Cover, Doors};
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::ground::Ground;
use openshard_movement::{Footing, MapTerrain, NavigationGraph};
use openshard_protocol::casting::SpellId;
use openshard_protocol::combat::HealthBar;
use openshard_protocol::feedback::{Animation, NewAnimation, PlaySound};
use openshard_protocol::items::WorldItem;
use openshard_protocol::localized;
use openshard_protocol::mobile::{Equipment, MobileIncoming, MobileMove, Notoriety, Remove, StatusFlags};
use openshard_protocol::properties::{PropertyList, TooltipRevision};
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::{Font, LocalizedMessage, SpokenMessage, TalkMode};
use openshard_protocol::wire::{ClilocId, Hue, SoundId};
use openshard_protocol::world::{
    Facet, MapChange, MapSize, PlayerUpdate, Point, Season, encode_server_change,
};
use openshard_protocol::{access::AccessLevel, feature::Feature, version::ClientVersion};
use openshard_tiles::TileData;

use crate::boat::Plank;
use crate::components::{
    Access, Amount, Body, Client, Combat, Contained, CorpseBody, CraftedBy, Drawn, Equipped, Ghost, Heading,
    HearsGhosts, Hidden, Hitpoints, InRegion, Meditating, Movement, Name, Position, Quality, Staff,
    Stealthing, TradeWindow, body_opens_doors,
};
use crate::connection::Connection;
use crate::dialogue::Dialogue;
use crate::harvest::Banks;
use crate::obstruct::Obstructions;
use crate::quest::QuestDefs;
use crate::region::{Region, Regions};
use crate::rng::Rng;
use crate::sectors::{Occupant, Sectors, VIEW_RANGE};
use crate::skill::Skill;

/// A character's height above the ground when the facet has no map to ask.
const Z_WITHOUT_A_MAP: i8 = 0;

/// "You stop meditating." — the line a broken trance says, ServUO's 500134.
const STOP_MEDITATING: ClilocId = ClilocId(500_134);

/// The hue and font a private system line is drawn in — the client's usual muted
/// grey, so it reads as the server talking rather than as a mobile speaking.
const SYSTEM_HUE: Hue = Hue::SYSTEM;
const SYSTEM_FONT: Font = Font::DEFAULT;

/// Ticks in one second — the reciprocal of the world's 50ms tick interval. The
/// world defines the interval; this is the whole-number rate config uses to turn
/// operator-facing seconds into the tick counts timers run on. If one moves, the
/// other must.
pub const TICKS_PER_SECOND: u64 = 20;

/// The gameplay rules an operator tuned, in the form the systems read them: the
/// [`GameplayConfig`](../../openshard_config) knobs, with the second-valued ones
/// already converted to ticks. A plain value the [`WorldState`] carries so any
/// system can reach the number it needs — combat the swing era, chat the speech
/// ranges, items the decay timer — without a config crate below them.
#[derive(Clone, Copy, Debug)]
pub struct Gameplay {
    /// Which swing-speed formula combat uses (Sphere's `CombatEra`, 0–4).
    pub combat_era: CombatEra,
    /// The swing formula's numerator (Sphere's `SpeedScaleFactor`).
    pub speed_scale_factor: u64,
    /// Chance, in per-mille, for a landed weapon or ranged blow to be critical.
    /// A shard extension: zero keeps strictly classic damage rolls.
    pub critical_chance: u16,
    /// Damage a critical blow deals as a percentage of its normally scaled hit.
    pub critical_damage_percent: u16,
    /// The ceiling any one skill trains to, in tenths — the cap a character's
    /// skills are given when nothing raises one of them.
    pub skill_cap: u16,
    /// The ceiling on *all* skills added together, in tenths — ServUO's
    /// `PlayerCaps.TotalSkillCap`, the classic 700.0. What makes a character a
    /// build rather than a list: past it, one skill only rises if another gives
    /// ground.
    pub total_skill_cap: u32,
    /// The ceiling on the three stats added together — the classic 225.
    pub stat_cap: u16,
    /// The ceiling on any one stat — the classic 125.
    pub stat_cap_individual: u16,
    /// How long after a stat rises before it may rise again, in ticks. ServUO
    /// ships the long delay *off*, leaving half a second.
    pub stat_gain_ticks: u64,
    /// The chance, in per-mille, that a skill gain also tries for a stat — only
    /// the ML mechanic (`combat_era` 4) reads it; the older one rolls each stat's
    /// own weight from the skill table instead.
    pub stat_gain_chance: u32,
    /// How long an item lies on the ground before it rots, in ticks.
    pub decay_ticks: u64,
    /// How long a house stands without being refreshed before it collapses, in
    /// ticks. ServUO's five days, and D6's operator setting.
    pub house_decay_ticks: u64,
    /// How long a criminal flag lasts, in ticks.
    pub criminal_ticks: u64,
    /// How far normal speech carries, in tiles.
    pub distance_talk: u32,
    /// How far a whisper carries, in tiles.
    pub distance_whisper: u32,
    /// How far a yell carries, in tiles.
    pub distance_yell: u32,
    /// Ticks between a hunting creature's steps. 8 (0.4s) is the references'
    /// base-monster pace — slower than a running player on purpose; 5 (0.25s)
    /// matches a runner, for shards that want monsters to catch people. Idle
    /// creatures amble at twice this.
    pub creature_step_ticks: u64,
    /// How a spell is cast — Sphere's cast-while-walking, or the UO/ServUO
    /// stop-to-cast with the target after.
    pub cast_style: CastStyle,
    /// Whether taking damage while casting disturbs the spell (UO's fizzle). Only
    /// meaningful in [`CastStyle::Stop`], where there is a cast to disturb.
    pub spell_disturb: bool,
    /// How AoS object tooltips are served — Sphere's `TOOLTIPMODE`, plus an off
    /// gate. Read by the interest substrate to decide what to send when a thing is
    /// drawn, and by the world when the client asks for a full list.
    pub tooltip_mode: TooltipMode,
    /// Whether the server answers a context-menu request with a popup.
    pub context_menus: bool,
    /// Whether spells require and consume reagents at all (classic UO on; a
    /// no-reagent shard off).
    pub reagents: bool,
    /// Whether a failed cast still spends mana — Sphere's `ManaLossFail`. Spent at
    /// resolution once success is known; a successful cast always spends.
    pub mana_loss_on_fail: bool,
    /// Whether a failed cast still consumes reagents — Sphere's `ReagentLossFail`.
    pub reagent_loss_on_fail: bool,
    /// Whether the status bar's gold adds the bank box. Off is ServUO's truth (a
    /// virtual box, whose gold never reaches the character's total); on sums both.
    /// Never affects weight — banked goods are not carried either way.
    pub bank_gold_in_status: bool,
    /// Whether an NPC purchase falls back to the bank when the pack is short —
    /// ServUO's `BaseVendor`, which tries the pack, then the bank.
    pub vendor_bank_payment: bool,
    /// Whether Recall and Gate Travel may cross to another facet.
    ///
    /// Off is the classic rule: pre-AoS, ServUO refuses both outright ("You can
    /// not recall to another facet"), and a rune marked in Ilshenar is a rune
    /// you have to walk to. On is the behaviour from AoS onward.
    ///
    /// A setting of its own rather than a reading of `expansion`, which cannot
    /// express pre-AoS — its floor *is* AoS — or of `combat_era`, which would be
    /// a combat knob quietly deciding a travel rule. The machinery underneath
    /// works either way; this is only whether the spells are allowed to use it.
    pub cross_facet_travel: bool,
    /// Level-of-detail: when on, a creature with no player within
    /// [`lod_radius`](Self::lod_radius) dozes at a stretched beat instead of
    /// paying for the full AI decision each beat. Off simulates every creature at
    /// full rate. Read by `World::think`.
    pub lod: bool,
    /// How close (tiles, Chebyshev) a player must be for a creature to think at
    /// full rate under [`lod`](Self::lod). Above the view range and the largest
    /// sight, so a visible creature is never dozed.
    pub lod_radius: u32,
    /// How many times its normal beat a dozing creature's next think is pushed
    /// out under [`lod`](Self::lod). At least 1.
    pub lod_idle_factor: u64,
    /// Ticks in one UO minute — how fast the world clock runs. ServUO's five real
    /// seconds to the minute puts a whole UO day in two real hours.
    pub uo_minute_ticks: u64,
    /// The season the client draws. Static for now; sent on world entry.
    pub season: Season,
    /// Whether guards answer at all in the regions marked guarded — ServUO's
    /// per-region `Disabled`, as one shard-wide switch.
    pub guards: bool,
    /// Whether townsfolk keep a daily routine: at its post inside working hours,
    /// at its `NightHome` outside them.
    ///
    /// Off by default, and deliberately marked as ours: neither reference ties an
    /// NPC to the clock. ServUO's nearest equivalent is a hand-placed `WayPoint`
    /// chain, which a builder walks an NPC along with no notion of the hour. With
    /// no `NightHome` in the pack's data the setting does nothing.
    pub npc_schedule: bool,
    /// The hour townsfolk arrive at their posts, with
    /// [`npc_schedule`](Self::npc_schedule) on.
    pub npc_work_hour: u8,
    /// The hour townsfolk leave for home, with
    /// [`npc_schedule`](Self::npc_schedule) on. Must be after
    /// [`npc_work_hour`](Self::npc_work_hour) — `config` rejects a working day that
    /// wraps midnight, so nothing downstream has to reason about one.
    pub npc_home_hour: u8,
    /// Which expansion the shard runs, as an index into `config::EXPANSIONS`:
    /// `0` AoS, `1` SE, `2` ML.
    ///
    /// An ordinal rather than the config's string, because this crate is below
    /// `config` and a rule wants a comparison, not a name. It is the same setting
    /// the `0xB9` mask is built from, so the paperdoll a client draws and the
    /// content the shard runs cannot disagree — see
    /// [`is_ml`](Self::is_ml), which the seven-wood lumber table reads.
    pub expansion: u8,
}

/// How AoS object tooltips (the "cliloc" hover names) are served — Sphere's
/// `TOOLTIPMODE`, with an added off state.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum TooltipMode {
    /// No tooltips, and AoS is not advertised — a modern client falls back to the
    /// classic single-click name label.
    Off,
    /// Send only a revision (`0xDC`) when a thing is drawn and wait for the client
    /// to request the full list (`0xD6`). Sphere's `TOOLTIPMODE_SENDVERSION`, the
    /// bandwidth-cheap standard.
    #[default]
    SendVersion,
    /// Send the whole tooltip (`0xD6`) up front. Sphere's `TOOLTIPMODE_SENDFULL`.
    SendFull,
}

impl TooltipMode {
    /// Parse the operator's `tooltips` string. `"off"` disables them, `"full"`
    /// sends the whole list up front; anything else is the send-version default.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" => Self::Off,
            "full" | "sendfull" => Self::SendFull,
            _ => Self::SendVersion,
        }
    }
}

/// How a spell is cast — the choice both reference emulators make differently.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum CastStyle {
    /// The UO/ServUO original: the caster stops, says the words over a cast
    /// delay, and only then does the target cursor appear (after which it may
    /// move again). Damage during the delay can disturb it.
    #[default]
    Stop,
    /// Sphere's feel: the spell resolves as it is cast, with no rooting delay —
    /// the caster keeps walking, and a target cursor (if any) comes up at once.
    Walk,
}

impl CastStyle {
    /// Parse the operator's `cast_style` string. `"sphere"`/`"walk"` is the
    /// walking cast; anything else is the stop-to-cast default.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "sphere" | "walk" | "walking" => Self::Walk,
            _ => Self::Stop,
        }
    }
}

impl Gameplay {
    /// [`expansion`](Self::expansion) for Age of Shadows.
    pub const AOS: u8 = 0;
    /// For Samurai Empire.
    pub const SE: u8 = 1;
    /// For Mondain's Legacy — the default, and where the quest system comes from.
    pub const ML: u8 = 2;

    /// Whether the shard runs Mondain's Legacy or later.
    ///
    /// ServUO's `Core.ML`, which several content tables key off: the seven woods a
    /// lumberjack can find are ML's, and before it there is one kind of log.
    #[must_use]
    pub const fn is_ml(&self) -> bool {
        self.expansion >= Self::ML
    }

    /// Seconds, as a count of ticks.
    ///
    /// The operator writes seconds; every system counts ticks, because a tick
    /// count replays and a wall clock does not. One conversion, here, so no
    /// caller has to remember the tick rate.
    #[must_use]
    pub const fn ticks(seconds: u64) -> u64 {
        seconds * TICKS_PER_SECOND
    }

    /// Milliseconds, as a count of ticks — at least one, so a sub-tick interval
    /// still advances.
    #[must_use]
    pub const fn ticks_from_ms(milliseconds: u64) -> u64 {
        let ticks = milliseconds / (1000 / TICKS_PER_SECOND);
        if ticks == 0 { 1 } else { ticks }
    }
}

impl Default for Gameplay {
    /// The pre-AoS feel the systems were built with — the values that were
    /// compile-time constants before an operator could tune them.
    ///
    /// Written as a literal, and the one place the defaults live. This used to be
    /// a twenty-seven-argument `new`, which is how a config knob ends up
    /// positionally next to the wrong one; a caller now names each field it means
    /// to change and takes the rest from here.
    fn default() -> Self {
        Self {
            combat_era: CombatEra::from(1),
            speed_scale_factor: 15000,
            critical_chance: 50,
            critical_damage_percent: 150,
            skill_cap: 1000,
            total_skill_cap: 7000,
            stat_cap: 225,
            stat_cap_individual: 125,
            // ServUO ships the fifteen-minute delay switched off, which leaves
            // the half second its config falls back to.
            stat_gain_ticks: Self::ticks_from_ms(500),
            stat_gain_chance: 50, // 5%, ServUO's PlayerChanceToGainStats
            decay_ticks: Self::ticks(20 * 60),
            house_decay_ticks: Self::ticks(5 * 24 * 60 * 60),
            criminal_ticks: Self::ticks(2 * 60),
            distance_talk: 18,
            distance_whisper: 3,
            distance_yell: 31,
            creature_step_ticks: Self::ticks_from_ms(400),
            cast_style: CastStyle::Stop,
            spell_disturb: true,
            tooltip_mode: TooltipMode::SendVersion,
            context_menus: true,
            reagents: true,
            mana_loss_on_fail: true,
            reagent_loss_on_fail: true,
            // The bank is not a second pocket, so its gold is not on the bar.
            bank_gold_in_status: false,
            // But a vendor does fall back to it, as ServUO's does.
            vendor_bank_payment: true,
            // The classic rule: a rune marked on another facet is a walk.
            cross_facet_travel: false,
            lod: false, // opt-in
            lod_radius: 32,
            lod_idle_factor: 8,
            // ServUO's rate: a whole UO day in two real hours.
            uo_minute_ticks: Self::ticks(5),
            season: Season::Spring,
            guards: true,
            // Ours, not the references'; opt-in, and inert without pack data.
            npc_schedule: false,
            npc_work_hour: 7,
            npc_home_hour: 21,
            expansion: Gameplay::ML,
        }
    }
}

/// Bytes for a connection, produced by a tick.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Outbound {
    /// Who to send to.
    pub connection: ConnectionId,
    /// What to send.
    pub packet: Vec<u8>,
}

/// One facet: its ground, and who is near what on it.
///
/// The world keeps one of these per loaded facet. Two mobiles on different
/// facets never share a sector grid, so they never see each other and never
/// block each other — the isolation is a property of the data structure, not a
/// check anyone has to remember to write.
///
/// The ground is the map itself — a [`MapSnapshot`], which is a published
/// revision of a facet and the thing an edit republishes. It used to be a boxed
/// [`Terrain`] trait object, on the argument that this crate sits below the
/// client-file parsers; that stopped being true when `openshard-uofiles` became
/// a dependency, and the box bought nothing but a second name for one concrete
/// type. A facet with no map carries `None` and every step is allowed.
///
/// **Nothing here reads the tile table.** What a graphic *is* belongs to the
/// shard, not to a facet ([`WorldState::tiles`]), so the pair is put back
/// together for the length of one question by
/// [`WorldState::map_terrain`](WorldState::map_terrain) — see
/// `docs/map/terrain_seam.md`'s node D.
pub struct FacetState {
    /// The ground, what the live world has laid over it, and where a body may
    /// stand on the pair.
    ///
    /// **One value, and private.** The three used to be a public
    /// `Option<MapSnapshot>`, a private overlay beside it and a private span
    /// bake beside that, which meant nothing stopped a reader taking one of
    /// them and forgetting the others — and the set could be given a map and an
    /// overlay of different facets, or a bake over a map this facet no longer
    /// held, without anything noticing. See [`Ground`], and
    /// [`WorldState::footing`](WorldState::footing), which is the one
    /// composition over it.
    ground: Ground,
    /// Static long-distance connectivity, built with the terrain at facet load.
    /// It deliberately has no live doors or placed items in it; a caller still
    /// refines every hop through [`WorldState::live_terrain`] or its
    /// doors-open sibling.
    pub coarse: Option<NavigationGraph>,
    /// How wide this facet's map is, in tiles.
    ///
    /// Kept here rather than asked of the terrain because the client has to be
    /// *told* it — twice, at login (`0x1B`) and at every facet change (`0x76`) —
    /// and the facets are not all the same shape: Felucca is 7168×4096 and
    /// Tokuno is 1448×1448. A shard that sends Britannia's size for every facet
    /// hands a character in Ilshenar a map three times too large, and the client
    /// draws the edge of the world wherever it likes.
    ///
    /// **Private, and there is nothing to write it with.** It is set by
    /// [`new`](Self::new) and never again: the sector grid and the region index
    /// are sized from this pair, so a facet whose width is assigned after
    /// construction has two indexes that disagree with it and no way to notice.
    /// Read through [`width()`](Self::width).
    width: u32,
    /// How tall this facet's map is, in tiles. See [`width`](Self::width).
    height: u32,
    /// Who is near what, on this facet.
    ///
    /// **Private, and written only through
    /// [`WorldState::place_mobile`](WorldState::place_mobile),
    /// [`WorldState::place_item`](WorldState::place_item) and
    /// [`WorldState::unplace`](WorldState::unplace).** It was public and written
    /// from forty-five places in six crates, which is the same "a public field
    /// is a way to forget" its two neighbours here are private for — and the
    /// forgettable half was the removal: an entity taken out of the world but
    /// left in the grid is answered to every lookup that passes over its tile,
    /// forever.
    ///
    /// The second thing the seam buys is that [`Occupant`] is named once per
    /// kind of thing rather than once per call site. The kind is still the
    /// caller's to declare — see [`Occupant`] for why it is never derived — but
    /// it is declared by *which* of the two calls is made, so there is no third
    /// spelling to get wrong.
    sectors: Sectors,
    /// What the live world has put in the way: closed doors, placed decoration.
    ///
    /// **Private, and mutated only through this facet.** Every write here has to
    /// be followed by a rewrite of the same tile in [`overlay`](Self::overlay),
    /// and a public field is a way to forget — which is the failure mode
    /// `docs/map/terrain_seam.md` is entirely about. See
    /// [`FacetState::block`].
    obstructions: Obstructions,
    /// The ships moored on this facet, and the decks they put over the water.
    ///
    /// Beside the obstruction index rather than in it, because the two move on
    /// different clocks: a door flips where it hangs, a ship sails. What they
    /// used to be beside each other *for* — being asked separately by every step
    /// — is over; they project into one overlay now. Private for the same reason
    /// as [`obstructions`](Self::obstructions).
    boats: crate::boat::Boats,
    /// The named areas of this facet — towns, dungeons, guarded zones.
    ///
    /// **Public on purpose**, alone among the indexes here, and the distinction
    /// is worth naming because the neighbours were all made private for the
    /// opposite reason. [`Regions`] carries its own seam: its two mutators
    /// ([`Regions::set`], [`Regions::clear`]) both rebuild the bucket grid that
    /// accelerates [`Regions::at`], and the grid is private to that type, so a
    /// caller holding `&mut` to this field still cannot leave it disagreeing
    /// with the regions beside it. That is what [`sectors`](Self::sectors) did
    /// not have — there the *field* was the API — and there is no follow-up
    /// write here of the kind that makes [`obstructions`](Self::obstructions)
    /// forgettable. Hiding it behind an accessor pair would rename the leak
    /// rather than close one.
    pub regions: Regions,
    /// What each block of this facet's ground still has left to give: the
    /// mining, lumberjacking and fishing stock, per [`crate::harvest`] bank.
    ///
    /// Beside the sector grid and the obstruction index because it is the same
    /// kind of thing — a fact about *this ground*, keyed by coordinates, that no
    /// entity owns. Not persisted; see [`Banks`].
    pub banks: Banks,
}

impl FacetState {
    /// A facet `width` by `height`, with its ground and nothing on it yet.
    ///
    /// The sector grid and the region index are sized from the same pair rather
    /// than passed, because every caller built them that way and a facet whose
    /// grid disagrees with its own width is not a configuration anybody wants
    /// to be able to spell. The three live indexes start empty and are private
    /// — see [`obstructions`](Self::obstructions), and the live layer of
    /// [`world`](Self::world), which is a projection of the other two.
    ///
    /// The tile table is the one argument that is not a fact about this facet:
    /// it is the install's, and it is here because the span bake inside
    /// [`Ground`] is built from the pair.
    #[must_use]
    pub fn new(
        map: Option<MapSnapshot>,
        coarse: Option<NavigationGraph>,
        width: u32,
        height: u32,
        tiles: &TileData,
    ) -> Self {
        Self {
            ground: Ground::new(map, tiles),
            coarse,
            width,
            height,
            sectors: Sectors::new(width, height),
            obstructions: Obstructions::default(),
            boats: crate::boat::Boats::default(),
            regions: Regions::new(width, height),
            banks: Banks::default(),
        }
    }

    /// The facet's static long-distance guide, when it has a map in movement's
    /// coordinate space. Kept as an accessor so no caller can mistake it for
    /// the live terrain that actually approves a step.
    #[must_use]
    pub const fn coarse_router(&self) -> Option<&NavigationGraph> {
        self.coarse.as_ref()
    }

    /// How wide this facet is, in tiles. See [`width`](Self::width) for why a
    /// facet carries its own extent at all.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// How tall this facet is, in tiles.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// What the live world has put in the way, with the entity that put it
    /// there.
    ///
    /// The *identity* half, and the only reason to come here rather than to the
    /// overlay: the overlay says a door is in the way, and this says which door
    /// — which is what a townsperson about to open one needs.
    #[must_use]
    pub const fn obstructions(&self) -> &Obstructions {
        &self.obstructions
    }

    /// The ships moored here.
    #[must_use]
    pub const fn boats(&self) -> &crate::boat::Boats {
        &self.boats
    }

    /// Who is near what, on this facet — to read.
    ///
    /// Every lookup lives here: [`Sectors::mobiles_near`],
    /// [`Sectors::items_near`], [`Sectors::everything_near`],
    /// [`Sectors::mobiles_in_block`]. Read-only, because a row written here has
    /// to agree with the [`Position`] the registry holds, and the pair moves
    /// through [`WorldState::place_mobile`](WorldState::place_mobile) and its
    /// two siblings.
    #[must_use]
    pub const fn sectors(&self) -> &Sectors {
        &self.sectors
    }

    /// This facet's ground, the live world over it and the bake that says where
    /// a body may stand on the two — as one value.
    ///
    /// Read-only: the live layer is a projection of
    /// [`obstructions`](Self::obstructions) and [`boats`](Self::boats), so the
    /// only way to change it is to change one of those and let
    /// [`refresh`](Self::refresh) follow; the ground itself moves through
    /// [`set_map`](Self::set_map), which is the seam the bake follows.
    #[must_use]
    pub const fn ground(&self) -> &Ground {
        &self.ground
    }

    /// Give this facet its ground, or take it away.
    ///
    /// A facet is inserted and *then* loaded — `tick`'s facet loader builds the
    /// state before it has read a map, and a test builds one and then hands it
    /// the scene it is about. It used to be a public field, which is what made
    /// the pair forgettable.
    ///
    /// **The span bake follows the ground, and it does so inside [`Ground`].**
    /// It is a projection of what is being replaced, so the two move together or
    /// the shard decides steps against a map it no longer holds — which is why
    /// this facet cannot separate them even if it tried. The tile table is the
    /// install's and is passed in for the same reason [`new`](Self::new) takes
    /// one.
    pub fn set_map(&mut self, map: Option<MapSnapshot>, tiles: &TileData) {
        self.ground.set_base(map, tiles);
    }

    /// Bring the span bake back in step with a tile table that arrived after
    /// the ground did.
    ///
    /// The world builder takes its tables and its facets in either order, and a
    /// bake is a statement about both: see `World::with_tiles`, the only
    /// caller. A facet with no map has nothing to bake and stays that way.
    pub fn rebake(&mut self, tiles: &TileData) {
        self.ground.rebake(tiles);
    }

    /// Put `entity`'s `cover` on `(x, y)`.
    ///
    /// See [`Obstructions::block`] for what the identity is and why one entity
    /// may hold several covers on one tile. The overlay follows.
    pub fn block(&mut self, x: u16, y: u16, entity: EntityId, cover: Cover) {
        self.obstructions.block(x, y, entity, cover);
        self.refresh(x, y);
    }

    /// Remove `entity`'s block on `(x, y)`, if it holds one.
    pub fn unblock(&mut self, x: u16, y: u16, entity: EntityId) {
        self.obstructions.unblock(x, y, entity);
        self.refresh(x, y);
    }

    /// Put a boat's tiles down, replacing whatever that boat had before.
    pub fn moor(&mut self, boat: EntityId, planks: impl IntoIterator<Item = ((u16, u16), Plank)>) {
        // The old footprint first: a boat that moved leaves tiles behind that no
        // longer carry it, and only `cast_off` knows which those were.
        self.cast_off(boat);
        self.boats.moor(boat, planks);
        for &(x, y) in self.boats.covered_by(boat).to_vec().as_slice() {
            self.refresh(x, y);
        }
    }

    /// Take a boat's tiles back out.
    pub fn cast_off(&mut self, boat: EntityId) {
        let was = self.boats.covered_by(boat).to_vec();
        self.boats.cast_off(boat);
        for (x, y) in was {
            self.refresh(x, y);
        }
    }

    /// Rewrite one tile's covers from both indexes. **The invariant.**
    ///
    /// Both sources at once, rather than each index maintaining its own slice of
    /// the overlay: a crate lashed to a deck is one tile with entries from both,
    /// and a per-source rule for merging them is a rule that can be wrong. This
    /// has nothing to merge.
    fn refresh(&mut self, x: u16, y: u16) {
        let covers = crate::obstruct::covers_at(&self.obstructions, &self.boats, x, y);
        self.ground.live_mut().set(Tile::new(x, y), covers);
    }
}

/// An item on a cursor: the entity, and where it was lifted from.
///
/// The origin is the whole reason to remember more than the entity. A drag that
/// is refused — dropped out of reach, into nothing — has to put the item back
/// exactly where it was, and by then it is off the ground (and out of any
/// container) with no place of its own to return to.
#[derive(Clone, Copy, Debug)]
pub struct HeldItem {
    /// The lifted item.
    pub entity: EntityId,
    /// Where it was, so a cancelled drag can undo cleanly.
    pub origin: Origin,
}

impl std::fmt::Debug for FacetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FacetState")
            .field("has_map", &self.ground.snapshot().is_some())
            .field("sectors", &self.sectors.len())
            .finish()
    }
}

/// Where a held item came from, so a cancelled drag can put it back.
#[derive(Clone, Copy, Debug)]
pub enum Origin {
    /// It was on the ground.
    Ground {
        /// Where it lay.
        position: Point,
        /// On which facet.
        facet: Facet,
    },
    /// It was inside a container.
    Container(Contained),
    /// It was worn by a mobile.
    Worn(Equipped),
}

/// One party to a secure trade: who they are and what they have agreed to.
#[derive(Clone, Debug)]
pub struct TradeSide {
    /// The trading player.
    pub player: EntityId,
    /// Their connection, which is what a trade packet is addressed to.
    pub connection: ConnectionId,
    /// Their escrow container.
    pub container: EntityId,
    /// Its serial — the id the client names the window by, and the only handle a
    /// `0x6F` from the client carries.
    pub container_serial: Serial,
    /// Whether their checkbox is ticked.
    pub accepted: bool,
}

/// Two players exchanging goods, and the two escrow containers between them.
///
/// Nothing moves until both sides have ticked; every other ending puts each
/// side's offering back in its own pack. Live only — a trade is transient, like
/// a cast in flight or a spell field, and is never saved.
#[derive(Clone, Debug)]
pub struct Trade {
    /// The player who started it, by dropping something on the other.
    pub from: TradeSide,
    /// The player it was offered to.
    pub to: TradeSide,
    /// What was in the two escrows when a checkbox was last ticked.
    ///
    /// ServUO clears both boxes from the container's own `OnItemAdded`/
    /// `OnItemRemoved` — a call beside every mutation, the pattern this engine
    /// avoids. The contents are diffed against this instead, and only while
    /// somebody has actually ticked: an unticked box has nothing to clear, which
    /// is what keeps the check off the common path.
    pub witnessed: Vec<Serial>,
}

impl Trade {
    /// The side `player` is on, and the other one.
    #[must_use]
    pub fn sides_for(&self, player: EntityId) -> Option<(&TradeSide, &TradeSide)> {
        if self.from.player == player {
            Some((&self.from, &self.to))
        } else if self.to.player == player {
            Some((&self.to, &self.from))
        } else {
            None
        }
    }

    /// Whether `player` is one of the two parties.
    #[must_use]
    pub fn involves(&self, player: EntityId) -> bool {
        self.from.player == player || self.to.player == player
    }
}

/// The world's runtime state — the data every gameplay system operates on.
///
/// A plain value with public fields: it is a data carrier, not an encapsulation
/// boundary. The boundary that matters is the event bus (systems emit, they do
/// not call), not field privacy. Nothing here is a static; a test builds as many
/// as it likes.
/// The worn-items index behind [`WorldState::equipment_of`] — a cache of which
/// items each mobile has on, and the `Equipped` column version it was built from.
///
/// Not a mirror: nothing maintains it. It is rebuilt whole the first time it is
/// read after the column changes, which the column reports for itself.
#[derive(Debug, Default)]
pub struct WornIndex {
    /// The `Registry::column_version::<Equipped>` this was built from.
    version: u64,
    /// Mobile serial -> the item entities it is wearing.
    by_mobile: HashMap<Serial, Vec<EntityId>>,
}

pub struct WorldState {
    /// Everything in the world.
    pub registry: Registry,
    /// What happened, for anyone to read: the client, persistence, scripts.
    pub bus: EventBus,
    /// The loaded facets, each with its own ground and interest grid, keyed by
    /// facet number. There is always at least the default one.
    pub facets: BTreeMap<Facet, FacetState>,
    /// The facet a new character spawns on, and the one anything asking for a
    /// facet it does not have falls back to.
    pub default_facet: Facet,
    /// What the client's `tiledata.mul` says about a graphic: whether it blocks,
    /// how tall it is, what it weighs, which hand it is held in, what it is
    /// called.
    ///
    /// **One table for the shard, not one per facet.** It used to be reached
    /// through [`FacetState::terrain`], which meant an item's weight was found by
    /// first asking which facet the item was standing on — a lookup with nothing
    /// to do with the answer, and one that returned nothing at all for an item in
    /// a pack on a mapless facet.
    ///
    /// **There is always a table.** A shard with no client files gets
    /// [`TileData::empty`](openshard_tiles::TileData::empty), which is
    /// not a stand-in for the file but the file saying nothing: every graphic
    /// defined, unremarkable and weightless. That is the same answer every caller
    /// used to reach for itself when the field was `None`, written once here
    /// instead of a dozen times at the lookups — and it makes "no client files"
    /// one state rather than one per reader, which is exactly the defect that
    /// two different ways of blanking a shard turned out to be.
    ///
    /// **Owned outright.** It sat behind an `Arc` for exactly one reason: every
    /// facet's terrain was boxed, so it had to own its own copy of the table and
    /// the `Arc` was what stopped that from being a copy per facet. Nothing is
    /// boxed now — a `MapTerrain` borrows this and the facet's map together at
    /// the question — so there is one holder and nothing to share it with.
    ///
    /// **Private, read through [`tiles`](Self::tiles) and replaced through
    /// [`set_tiles`](Self::set_tiles).** Every facet's span bake is a statement
    /// about this table as much as about its ground — what a graphic is decides
    /// how tall a wall is and whether a tile is water — so a write that does not
    /// rebake leaves the shard deciding steps by the heights of a world it no
    /// longer has. It was a public field, and a direct `state.tiles = table` was
    /// the one remaining way to hold a bake that describes neither world in
    /// hand: [`Ground`] closed the other, where the ground moved under the bake.
    tiles: openshard_tiles::TileData,
    /// Every multi the client knows: what a house or a ship is made of.
    ///
    /// Beside [`tiles`](Self::tiles) and for the same reason — a multi's
    /// components are a fact about the install, not about a facet — and total for
    /// the same reason too: an empty table knows about no houses, which is what a
    /// shard whose install has no `multi.mul` in fact knows.
    ///
    /// Owned outright, like [`tiles`](Self::tiles) beside it: one holder each,
    /// and nothing on the shard to share either with.
    pub multis: openshard_uofiles::multi::Multis,
    /// Which entity a connection is driving.
    pub players: HashMap<ConnectionId, EntityId>,
    /// Every connection the world is holding, playing a character or not.
    ///
    /// Wider than [`players`](Self::players) on purpose: a connection exists from
    /// the moment the login conversation hands it over, which is before it has
    /// picked a character and after it has left one behind. See
    /// [`Connection`](crate::connection::Connection) for why that has to be
    /// expressible.
    ///
    /// It carries what the client is in the middle of as well as who it is — what
    /// is on its cursor, what it was last told about the light, the music and its
    /// own numbers. Those were maps of their own, cleared by name on the way out;
    /// the row is what makes forgetting one impossible. Let go of through
    /// [`forget_connection`](Self::forget_connection), never `connections.remove`.
    pub connections: HashMap<ConnectionId, Connection>,
    /// What each player's client currently has on screen.
    ///
    /// The server has to remember, because the client never says. There is no
    /// "what can you see" packet — only "draw this" and "forget that" — so the
    /// only way to send a mobile exactly once is to know what was sent before.
    pub seen: HashMap<EntityId, HashSet<EntityId>>,
    /// Where new characters appear. The height comes from the map.
    pub start: (u16, u16),
    /// The generator behind every roll — a swing landing, a skill gaining. Part
    /// of the state so replay is exact; advanced only inside the tick.
    pub rng: Rng,
    /// How many ticks have run.
    pub ticks: u64,
    /// Who is wearing what, rebuilt from the `Equipped` column when it changes.
    /// A cache with no contents of its own — read it through
    /// [`equipment_of`](WorldState::equipment_of), never directly.
    pub worn: WornIndex,
    /// The world's hour, 0–23, refreshed once per tick from the tick counter.
    ///
    /// Derived, not stored — `world/tick/ambient.rs` computes it and drops it
    /// here at the top of every tick, the same way `ticks` is the one clock every
    /// system reads. It is state rather than a parameter because more than one
    /// system now asks what time it is (a townsperson's routine, a shop's opening
    /// hours, its greeting), and threading an `hour` argument through each of them
    /// is a signature to keep in step for a value that has exactly one source.
    pub hour: u64,
    /// Packets the last tick produced.
    pub outbox: Vec<Outbound>,
    /// Which connections have each container open, so a change to its contents —
    /// an item consumed as a reagent, one decaying inside — can be pushed to the
    /// clients looking at it. A connection's opens are cleared on logout.
    pub open_containers: HashMap<Serial, HashSet<ConnectionId>>,
    /// Every secure trade in progress.
    ///
    /// A `Vec` and not a map because there is almost never one: it is scanned
    /// whole once a tick to find a trade whose parties have walked apart, which
    /// is cheaper than the region diff it copies, and a player is in at most one.
    pub trades: Vec<Trade>,
    /// Every quest this shard knows. Replaced
    /// wholesale on a pack reload, and never persisted — the pack is the truth
    /// about what a quest *is*, every boot; only a player's progress is saved.
    pub quests: QuestDefs,
    /// What every trade says. Replaced wholesale
    /// on a reload and never persisted, for the same reason as
    /// [`quests`](Self::quests): the pack is the truth about content.
    pub dialogue: Dialogue,
    /// Every guild on the shard, and how they regard each other.
    ///
    /// Here rather than only in `openshard-guilds` because the `0x78` has a
    /// notoriety byte and that byte depends on who is looking — see
    /// [`notoriety_toward`](Self::notoriety_toward).
    pub guilds: crate::guild::Guilds,
    /// Every named alliance. See [`Alliance`](crate::guild::Alliance).
    pub alliances: crate::guild::Alliances,
    /// Every party. Runtime-only — see [`crate::party`].
    pub parties: crate::party::Parties,
    // The targeting cursor and the four gump contexts used to be maps here, keyed
    // by the *player's entity*. They are fields on the connection's row now —
    // `connection::Connection` — reached through `row_of`/`row_of_mut`. They are
    // about a client's screen and not about a mobile: every one of them was
    // already unreachable without a `Client`, and keying them by the entity meant
    // nothing swept them when the client went. Four of the five leaked outright.
    /// The tunable rules — swing era, speech ranges, timers — the systems read.
    pub gameplay: Gameplay,
    /// Set by a staff `.save` to ask the tick for an immediate snapshot. The world
    /// clears it once taken — a request, not the save itself, because taking the
    /// snapshot is the `World`'s to do, not a system's.
    pub save_requested: bool,
}

/// Which page of the quest dialog a player is looking at.
///
/// ServUO's `MondainQuestGump.Section`, and the same one window for all of it: a
/// quest log, an offer, an objectives page and a rewards page are the same frame
/// with a different middle, so they share an id and a reply handler.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuestSection {
    /// The quest log: every quest in progress, one row each.
    Main,
    /// A quest's prose — the offer's first page, and the log's detail page.
    Description,
    /// What it asks for, with progress.
    Objectives,
    /// What it pays.
    Rewards,
    /// What the giver says when the offer is turned down.
    Refuse,
    /// What the giver says at turn-in.
    Complete,
    /// What the giver says when it is not finished yet.
    InProgress,
    /// What is said when a timer ran out.
    Failed,
}

/// What a player's open quest dialog is showing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestGumpContext {
    /// Which quest, by the pack's key. Empty on the log page, which is about no
    /// single quest.
    pub quest: String,
    /// Which page.
    pub section: QuestSection,
    /// Whether this is an *offer* (Accept/Refuse) rather than the log's view of a
    /// quest already taken (Resign/Close). The same pages, different buttons — and
    /// the difference decides whether a button id means "accept" or "resign", so it
    /// is remembered here rather than trusted from the reply.
    pub offer: bool,
    /// Whether the quest is finished, which is what lets the rewards page pay out.
    pub completed: bool,
    /// The giver the dialog was opened at, so a turn-in knows who to thank.
    pub giver: Option<Serial>,
}

/// Which page of the guild window a player is looking at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuildPage {
    /// The front: found a guild, or what yours is and what you may do with it.
    Main,
    /// Who is in it, one row each.
    Roster,
    /// Every other guild, and where this one stands with it.
    Diplomacy,
}

/// What a player's open guild window is showing, and what its rows meant.
///
/// The `listed` half is the point. A reply names a *row*, never an id — the
/// client is free to send any number it likes — so which guild or which member
/// row three was is the server's memory of what it drew, and a reply to a window
/// this side never opened resolves to nothing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GuildGumpContext {
    /// Which page.
    pub page: GuildPage,
    /// The guilds the diplomacy page drew, in row order.
    pub guilds: Vec<crate::guild::GuildId>,
    /// The members the roster drew, by serial, in row order. Serials rather than
    /// entities because the window outlives a tick and an entity id does not
    /// survive a despawn — a row naming a logged-out member resolves to nobody
    /// rather than to whoever took the slot.
    pub members: Vec<Serial>,
}

/// What a house sign's window is showing, and what its rows meant.
///
/// [`GuildGumpContext`]'s shape and its reason: a reply names a row, and which
/// person row three was is what this side remembers drawing.
///
/// The house is held by **entity** and not by serial, unlike the members: the
/// window is closed the moment it is answered, so it cannot outlive the house
/// the way a roster row outlives a member's logout — and an entity that has been
/// despawned resolves to nothing, which is the right answer for a window opened
/// on a house that has since come down.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HouseGumpContext {
    /// Which house's sign was clicked.
    pub house: EntityId,
    /// Everyone the window drew, in row order, with which list they were on.
    pub rows: Vec<(HouseList, Serial)>,
}

/// One of a house's three lists of people.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HouseList {
    /// Co-owners, who may do everything but hand the house over.
    CoOwners,
    /// Friends, who may come in and open the doors.
    Friends,
    /// The banned, who may do neither.
    Bans,
}

/// Which part of the craft window a player is looking at.
///
/// ServUO keeps this as a `CraftPage` plus a separate `CraftGumpItem` gump; the
/// two are one window here with one id, because they are the same reply channel
/// and a second id is a second thing to route.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CraftGumpPage {
    /// The recipe list of the selected category.
    Items,
    /// The material list, in place of the recipe list.
    Resources,
    /// One recipe's detail page, by its index in the system's table.
    Details(u16),
}

/// What a player's open craft window is showing, and what it will make.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftGumpContext {
    /// Which trade, by its index in the core table.
    pub system: u8,
    /// The tool the window was opened from. Re-checked on every attempt: a tool
    /// dropped with the window still up makes nothing.
    pub tool: EntityId,
    /// The selected category.
    pub group: u16,
    /// The selected material, indexed into the system's axis.
    pub sub_res: u8,
    /// Which page.
    pub page: CraftGumpPage,
    /// The cliloc in the window's notice box — what the last attempt had to say,
    /// or `None` when the box is empty.
    ///
    /// An `Option` and not a zero sentinel: cliloc `0` is a number the client
    /// would look up, so "no notice" and "notice number zero" were the same
    /// value here, and only the `!= 0` at the draw site kept them apart.
    pub notice: Option<ClilocId>,
}

/// What a house-list cursor is about to do with the mobile it is answered with.
///
/// One enum and not five purposes, because the five differ only in which call
/// they make — and a `TargetPurpose` variant per call would be five places to
/// remember to add the sixth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HouseChange {
    /// Trust them to come in.
    Friend,
    /// Trust them with the house.
    CoOwner,
    /// Take them off both trusted lists.
    Drop,
    /// Turn them away, and put them out if they are inside.
    Ban,
    /// Let them back to the door, as a stranger.
    Unban,
}

/// What a house-storage cursor is about to do with the item it is answered with.
///
/// [`HouseChange`]'s shape and its reason: the three differ only in the call
/// they make.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HouseStorage {
    /// Pin it in place.
    LockDown,
    /// Pin it and make it a secure container, opening for this standing and
    /// above.
    Secure(crate::Standing),
    /// Let it go loose again.
    Release,
}

/// What a raised targeting cursor is waiting to do with the click.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetPurpose {
    /// A house's list waiting for the name to put on it — the cursor `.hfriend`
    /// and its four siblings raise, and the one a house sign will raise when
    /// there is one.
    HouseList {
        /// Which list, and which direction.
        change: HouseChange,
    },
    /// A house's storage waiting for the item — the cursor the sign's
    /// lock-down, secure and release buttons raise.
    HouseStorage {
        /// Which of the three.
        change: HouseStorage,
        /// Which house, by entity.
        ///
        /// Carried, unlike [`HouseList`](Self::HouseList)'s, which resolves to
        /// "the house the actor is standing in" when the click lands. A list
        /// change is about a person and the actor is inside their own house
        /// while they make it; a lockdown is about an *item*, and a player who
        /// pressed the button by the sign is standing outside the walls the item
        /// is behind. A despawned entity resolves to nothing, which is the right
        /// answer for a window opened on a house that has since come down.
        house: EntityId,
    },
    /// A house waiting for its plot — the cursor a deed raises.
    ///
    /// Carries the *deed* rather than the multi id, and the difference is a rule:
    /// the id can be read back off the deed when the click lands, and a deed
    /// dropped, sold or destroyed while the cursor was up must not still place a
    /// house. Same shape as [`SkillSecond`](Self::SkillSecond)'s carried potion,
    /// and for the same reason.
    PlaceHouse {
        /// The deed being spent.
        deed: EntityId,
    },
    /// Teleport the targeter to the clicked spot — the cursor `.tele`.
    Teleport,
    /// A targeted spell waiting for its aim — the cursor a spell puts up once
    /// the cast resolves. `success` is the skill roll already made, carried here
    /// so a fumbled cast that still raises a cursor simply lands no effect.
    Spell {
        /// Which spell, by id.
        spell: SpellId,
        /// Whether the cast's skill roll passed.
        success: bool,
    },
    /// A skill waiting for the thing it was pointed at — "whom shall I examine?".
    /// Which skill asked is all that needs remembering; the rest is the skill's own.
    Skill {
        /// Which skill.
        skill: Skill,
    },
    /// A skill's *second* cursor: it has one answer and wants another.
    ///
    /// Poisoning is the reason — ServUO asks for the potion, then for the blade —
    /// and it is a separate variant rather than an `Option` on
    /// [`Skill`](Self::Skill) so that the common case stays a skill and one click.
    /// The first answer is carried as an entity and re-checked when the second
    /// lands: a potion drunk or dropped while the cursor was up poisons nothing.
    SkillSecond {
        /// Which skill.
        skill: Skill,
        /// What its first cursor came back with.
        first: EntityId,
    },
    /// A staff `.trap` waiting for the container to put a trap on.
    SetTrap {
        /// What the trap will do.
        kind: crate::components::TrapKind,
        /// How hard it hits, and how hard it is to take off.
        power: u16,
    },
    /// A harvesting tool waiting for the ground it will be swung at.
    ///
    /// The one purpose that raises a *location* cursor and needs the whole answer:
    /// a mountain face is not an entity, so the reply's point and tile graphic are
    /// the target, and the serial is nothing. See `skills`' harvest handler.
    Harvest {
        /// The tool, by entity. Re-checked when the click lands: a pickaxe dropped
        /// while the cursor was up mines nothing.
        tool: EntityId,
    },
    /// A guild leader's cursor, waiting for whoever is to be asked to join.
    ///
    /// Carries nothing: the guild is the one the clicker leads, and it is read
    /// again when the click lands rather than remembered here — a leader who
    /// disbanded, or was deposed, while the cursor was up invites nobody.
    GuildInvite,
    /// A party leader's cursor, waiting for whoever is to be asked along.
    ///
    /// [`GuildInvite`](Self::GuildInvite)'s twin and carries nothing for the
    /// same reason — with one difference worth naming: a party leader who has no
    /// party yet is the *ordinary* case, since asking is what creates one.
    PartyInvite,
    /// A key waiting to be turned on something — ServUO's `Key.OnDoubleClick`, which
    /// raises a cursor rather than guessing which of several nearby doors was meant.
    TurnKey {
        /// The key, by entity. Checked to still exist when the click lands: a key
        /// dropped or consumed while the cursor was up opens nothing.
        key: EntityId,
    },
}

impl WorldState {
    /// A world holding `facets`, over the install's `tiles` and `multis`, with
    /// new characters appearing at `start` and every roll drawn from `seed`.
    ///
    /// Named rather than written as a literal because
    /// [`tiles`](Self::tiles) is private, and there were five copies of that
    /// literal — one per crate with a fixture — each naming twenty-four fields
    /// so that a field added here had to be added in five places or nowhere.
    /// Everything not named in the arguments starts empty: nobody connected,
    /// nothing on a screen, no trade in progress, and no content tables, which
    /// is the state a shard is in before it has read anything.
    ///
    /// The facets are taken already built, because a [`FacetState`] bakes its
    /// ground against the same table. Either build them with `tiles` and hand
    /// both over, or build them with [`TileData::empty`] and let
    /// [`set_tiles`](Self::set_tiles) rebake them when the real one arrives —
    /// which is the order `World::with_tiles` takes.
    ///
    /// [`TileData::empty`]: openshard_tiles::TileData::empty
    #[must_use]
    pub fn new(
        facets: BTreeMap<Facet, FacetState>,
        default_facet: Facet,
        tiles: openshard_tiles::TileData,
        multis: openshard_uofiles::multi::Multis,
        start: (u16, u16),
        seed: u64,
    ) -> Self {
        Self {
            registry: Registry::new(),
            bus: EventBus::new(),
            facets,
            default_facet,
            tiles,
            multis,
            players: HashMap::new(),
            connections: HashMap::new(),
            seen: HashMap::new(),
            start,
            rng: Rng::new(seed),
            ticks: 0,
            worn: WornIndex {
                version: 0,
                by_mobile: HashMap::new(),
            },
            hour: 0,
            outbox: Vec::new(),
            open_containers: HashMap::new(),
            trades: Vec::new(),
            quests: QuestDefs::default(),
            dialogue: Dialogue::default(),
            guilds: crate::guild::Guilds::default(),
            alliances: crate::guild::Alliances::default(),
            parties: crate::party::Parties::default(),
            gameplay: Gameplay::default(),
            save_requested: false,
        }
    }

    /// What the client's `tiledata.mul` says about a graphic. See
    /// [`tiles`](Self::tiles) for why it is one table for the shard.
    #[must_use]
    pub const fn tiles(&self) -> &openshard_tiles::TileData {
        &self.tiles
    }

    /// Give the shard the table the install actually has, and bring every
    /// facet's span bake back in step with it.
    ///
    /// The rebake is the whole reason this is a method rather than a field. A
    /// bake says where a body may stand on this ground, and it reads the table
    /// to say it: a facet still holding a bake over the table being replaced
    /// answers steps for a world of height-zero, flag-less statics.
    /// [`FacetState::set_map`] is the same seam from the other side, where the
    /// ground moves under the bake.
    pub fn set_tiles(&mut self, tiles: openshard_tiles::TileData) {
        self.tiles = tiles;
        for facet in self.facets.values_mut() {
            facet.rebake(&self.tiles);
        }
    }

    /// Which facet an entity is on: its [`Facet`] component, or the world default
    /// so callers can index [`facets`](Self::facets) with the result.
    #[must_use]
    pub fn facet_of(&self, entity: EntityId) -> Facet {
        self.registry
            .get::<Facet>(entity)
            .copied()
            .unwrap_or(self.default_facet)
    }

    /// The state of a facet the world is known to have.
    #[must_use]
    pub fn facet_state(&self, facet: Facet) -> &FacetState {
        &self.facets[&facet]
    }

    /// The same, mutably. Panics only on a facet no entity should carry —
    /// `facet_of` and `enter` keep every live entity on a loaded facet.
    pub fn facet_state_mut(&mut self, facet: Facet) -> &mut FacetState {
        self.facets
            .get_mut(&facet)
            .expect("an entity's facet is always loaded")
    }

    /// The bodies within `reach` of `centre` that `mover` has to get past, as
    /// [`Bodies`](openshard_movement::Bodies) wants them: feet, sorted by tile.
    ///
    /// **The sector grid, and nothing kept beside it.** The grid is already the
    /// authority from tile to entity and is already kept honest by the step
    /// itself, so the crowd a step or a route is decided against is *derived*
    /// where the question is asked rather than maintained as a second copy of
    /// `Position` — see `docs/roadmap.md`'s *a mobile is not an obstacle*, which
    /// weighed the two and took this one. Nothing here can drift, because
    /// nothing here survives the answer.
    ///
    /// `reach` is the caller's, because only the caller knows what it is
    /// asking: a step wants `1` — the eight neighbours and no more — and a route
    /// wants the ground it might cross. A body outside the reach is invisible to
    /// the plan, which costs a re-plan when the route reaches it, and never a
    /// wrong step: the step itself is decided with its own crowd.
    ///
    /// The grid's mobile list, which is not the same as "everything that
    /// blocks": a body that is dead, or a hidden game master, is in the list and
    /// in nobody's way, which is why every candidate is then asked
    /// [`body_blocks`](Self::body_blocks). What the list *does* spare this is
    /// the furniture — a decorated house's lockdowns share a sector with the
    /// street outside it, and this runs on every step by anyone.
    #[must_use]
    pub fn crowd_near(&self, facet: Facet, mover: EntityId, centre: Point, reach: u32) -> Vec<Point> {
        if self.walks_through_bodies(mover) {
            return Vec::new();
        }
        let mut crowd: Vec<Point> = self
            .facet_state(facet)
            .sectors()
            .mobiles_near(centre, reach)
            // `mover` is absent from its own crowd, which is the whole of "a
            // mobile may always step off the tile it is standing on".
            .filter(|&(entity, _)| entity != mover && self.body_blocks(entity))
            .map(|(_, position)| position)
            .collect();
        crowd.sort_unstable_by_key(|body| (body.x, body.y));
        crowd
    }

    /// Whether `mover` walks through other bodies rather than round them.
    ///
    /// ServUO's `CanMoveOver`, the mover's half
    /// (`Scripts/Services/Pathing/Movement.cs:359`): **the dead are stopped by
    /// nobody.** A ghost has to be able to walk home, and the living cannot see
    /// it to get out of its way.
    ///
    /// Staff is ours rather than ServUO's, where it is a saved per-mobile
    /// `IgnoreMobiles` toggle that happens to be on for game masters. It is the
    /// same permission as walking through walls: [`Staff`] is the flag a `.gm`
    /// takes off, so a game master who has put it down is held to the rule like
    /// anyone else.
    #[must_use]
    pub fn walks_through_bodies(&self, mover: EntityId) -> bool {
        self.registry.has::<Staff>(mover) || !self.is_alive(mover)
    }

    /// How `mover` reads the shut doors when it is the one taking the step.
    ///
    /// ServUO's `MovementImpl.Check`, where `ignoreDoors` is set for
    /// `!m.Alive` (`Scripts/Services/Pathing/Movement.cs:173`) and `IsOk` then
    /// steps past anything carrying `TileFlag.Door`: **a leaf does not stop the
    /// dead.** It is the other half of being dead beside
    /// [`walks_through_bodies`](Self::walks_through_bodies), and it is the same
    /// argument — a ghost has to be able to walk home, and it has no hands to
    /// work a latch with on the way. The same `is_alive` answers both, so there
    /// is one definition of "dead" here and not two that can drift apart.
    ///
    /// **A house's door is not an exception**, which is worth saying because it
    /// reads like one. What `BaseHouseDoor` guards with `CheckAccess` is `Use`
    /// — the hand on the latch, which is `items::doors::may_pass` at this end —
    /// and movement never asks whose door it is once `ignoreDoors` is set. A
    /// ghost drifting through a stranger's front door arrives somewhere it can
    /// lift nothing, open nothing and be heard by nobody living.
    ///
    /// A planner reads [`Doors::AllOpen`] too, for a different reason, and the
    /// two must not be confused: **this is the only seam where a step that
    /// reaches the wire may take that reading**, because here the doors really
    /// are no obstacle rather than being ones somebody intends to open.
    #[must_use]
    pub fn walking_doors(&self, mover: EntityId) -> Doors {
        match self.is_alive(mover) {
            true => Doors::AsTheyStand,
            false => Doors::AllOpen,
        }
    }

    /// Whether `entity` is a body other movers have to walk around.
    ///
    /// ServUO's `CanMoveOver`, the other half. A [`Body`] is what separates a
    /// mobile from the items and decoration sharing the sector grid with it — a
    /// corpse is [`Drawn`], not a `Body`, so it is walked over like the chest it
    /// is.
    ///
    /// **A hidden game master is not in anybody's way**, which is ServUO's
    /// `t.Hidden && t.IsStaff()` exactly. A hidden *player* still blocks: being
    /// walked into is how you find one, and in ServUO the shove reveals them.
    #[must_use]
    pub fn body_blocks(&self, entity: EntityId) -> bool {
        self.registry.has::<Body>(entity)
            && self.is_alive(entity)
            && !(self.registry.has::<Hidden>(entity) && self.registry.has::<Staff>(entity))
    }

    /// Whether a mobile is alive for the purpose of getting in somebody's way: a
    /// ghost is not, and neither is anything worn down to no hit points.
    ///
    /// Both halves are needed. A dead *player* becomes a [`Ghost`] and keeps its
    /// hit points at zero; a creature at zero is reaped a tick later and is a
    /// body standing in a doorway until it is.
    fn is_alive(&self, entity: EntityId) -> bool {
        !self.registry.has::<Ghost>(entity)
            && self
                .registry
                .get::<Hitpoints>(entity)
                .is_none_or(|hitpoints| hitpoints.current > 0)
    }

    /// The region a point on `facet` falls in, if any.
    #[must_use]
    pub fn region_at(&self, facet: Facet, point: Point) -> Option<&Region> {
        self.facets.get(&facet)?.regions.at(point)
    }

    /// The region an entity is standing in, if any. The lookup every rule that
    /// asks "is this allowed here" goes through.
    #[must_use]
    pub fn region_of(&self, entity: EntityId) -> Option<&Region> {
        let position = self.registry.get::<Position>(entity)?;
        self.region_at(self.facet_of(entity), position.0)
    }

    /// Where a character appears on `facet`: the configured x and y, at that
    /// facet's height.
    ///
    /// The `z` is read from the map rather than configured. A second source of
    /// truth that disagrees by three units leaves a character unable to take a
    /// single step — every one is more than a two-unit climb — with nothing in
    /// the log to explain it.
    #[must_use]
    pub fn start_position(&self, facet: Facet) -> Point {
        let (x, y) = self.start;
        let z = self
            .map_terrain(facet)
            .and_then(|terrain| terrain.ground_z(Tile::new(x, y)))
            .unwrap_or(Z_WITHOUT_A_MAP);
        Point::new(x, y, z)
    }

    /// What the map alone says about `facet`, and `None` where it has no map.
    ///
    /// **Two borrows out of one `&self`.** The facet owns the ground and the
    /// shard owns the table that says what is on it; a `MapTerrain` is the pair
    /// read together, built here and living exactly as long as the question
    /// being asked. Nothing stores one — that is the whole of node D.
    ///
    /// This is the *bare* map: no doors, no placed crates, no decks. Anything
    /// deciding a step wants [`live_terrain`](Self::live_terrain) instead.
    #[must_use]
    pub fn map_terrain(&self, facet: Facet) -> Option<MapTerrain<'_>> {
        self.facets.get(&facet)?.ground().terrain(&self.tiles)
    }

    /// The ground every movement decision is actually decided against: the map,
    /// the live world over it, and how the doors are read. Works with no map
    /// too — an open world with doors in it still has doors.
    ///
    /// `Doors::AsTheyStand` is what a step takes; `Doors::AllOpen` is what a
    /// door-opener *plans* over, because the mobile walking that route opens
    /// them on arrival.
    ///
    /// # Panics
    ///
    /// On a facet that is not loaded, like [`facet_state`](Self::facet_state)
    /// and for the same reason: every live entity is on a loaded facet.
    #[must_use]
    pub fn footing(&self, facet: Facet, doors: Doors) -> Footing<'_> {
        let state = self.facet_state(facet);
        Footing::of(state.ground(), &self.tiles, doors)
    }

    /// The same ground with nothing the shard has put on it — the bare map
    /// [`FacetState::coarse`] was baked over, and the reading a long route's
    /// *corridor* is proposed and joined by.
    ///
    /// Never what approves a step: a route from the coarse graph is refined hop
    /// by hop through [`footing`](Self::footing), which is where a shut door and
    /// a dropped crate get their say. See
    /// [`Footing::guide`](openshard_movement::Footing::guide) for why the two
    /// readings are separate, and `openshard_ai::step_toward` for the one caller
    /// on this end.
    ///
    /// # Panics
    ///
    /// On a facet that is not loaded, like [`footing`](Self::footing).
    #[must_use]
    pub fn guide(&self, facet: Facet) -> Footing<'_> {
        Footing::guide(self.facet_state(facet).ground(), &self.tiles)
    }

    /// Is any connected player within `range` tiles (Chebyshev) of `centre` on
    /// `facet`? Cheap: players are few, so this walks the player table rather than
    /// the sector grid, and stops at the first hit. The primitive level-of-detail
    /// gates a creature's AI on — a creature no player is near need not think.
    #[must_use]
    pub fn any_player_near(&self, centre: Point, range: u32, facet: Facet) -> bool {
        self.players.values().any(|&entity| {
            self.facet_of(entity) == facet
                && self
                    .registry
                    .get::<Position>(entity)
                    .is_some_and(|pos| crate::sectors::in_range(pos.0, centre, range))
        })
    }

    /// Put `entity` on `facet`'s sector grid at `at`, **as a body**.
    ///
    /// The grid is a second copy of [`Position`] and this is the line that keeps
    /// it honest, so it belongs beside the registry write and not somewhere
    /// later: a mobile whose row says the old tile is seen from where it used to
    /// stand and missed from where it is. Calling it again *moves* the row —
    /// there is never a second one, on this facet or on any other.
    ///
    /// **The caller declares the kind by coming here rather than to
    /// [`place_item`](Self::place_item)**, and the two edges are why that cannot
    /// be worked out instead: a corpse carries a body *graphic* and is
    /// furniture, and a mount is an item on a layer while it is ridden and a
    /// body again the moment it is dismounted. See [`Occupant`].
    ///
    /// # Panics
    ///
    /// If `facet` is not loaded, like
    /// [`facet_state_mut`](Self::facet_state_mut).
    pub fn place_mobile(&mut self, facet: Facet, entity: EntityId, at: Point) {
        self.facet_state_mut(facet)
            .sectors
            .insert(entity, at, Occupant::Mobile);
    }

    /// Put `entity` on `facet`'s sector grid at `at`, **as a thing on the
    /// ground**: an item, a corpse, a door, a house, a ship, a moongate.
    ///
    /// [`place_mobile`](Self::place_mobile) in every other respect — including
    /// that handing the same entity to the other one moves it between the two
    /// lists rather than filing it twice, which is the one thing a dismounted
    /// horse legitimately does.
    ///
    /// The list this files into is read by exactly one gameplay question — what
    /// is on the ground here, a forge to craft at — and by the screen sweep.
    /// Filing a body here is therefore not merely wasteful: it is invisible to
    /// sight, chat, guards and the crowd a step is decided against.
    ///
    /// # Panics
    ///
    /// If `facet` is not loaded, like
    /// [`facet_state_mut`](Self::facet_state_mut).
    pub fn place_item(&mut self, facet: Facet, entity: EntityId, at: Point) {
        self.facet_state_mut(facet)
            .sectors
            .insert(entity, at, Occupant::Item);
    }

    /// Take `entity` off `facet`'s sector grid, whichever of the two lists it is
    /// in.
    ///
    /// **The half a public field made forgettable.** Every way out of the world
    /// comes through here — a despawn, a decay, an item picked up, a mount put
    /// back on its rider's layer, a boat sunk, a traveller leaving a facet — and
    /// a row left behind is worse than stale: it is handed to every lookup that
    /// passes over that tile for as long as the shard runs, and
    /// [`Sectors::position_of`] keeps swearing the thing is there.
    ///
    /// Harmless for an entity the grid never held.
    ///
    /// # Panics
    ///
    /// If `facet` is not loaded, like
    /// [`facet_state_mut`](Self::facet_state_mut).
    pub fn unplace(&mut self, facet: Facet, entity: EntityId) {
        self.facet_state_mut(facet).sectors.remove(entity);
    }

    /// Take a mobile out of the world: forget it from every screen, drop it from
    /// the sector grid, despawn it.
    ///
    /// The counterpart of the spawn path, and the one place that order is
    /// written down — forgetting *after* the despawn would leave the serial
    /// unresolvable and the mobile drawn on every screen that had it, which is
    /// exactly the "ghost that never leaves" bug.
    pub fn despawn_mobile(&mut self, entity: EntityId) {
        let Some(serial) = self.registry.serial_of(entity) else {
            return;
        };
        let facet = self.facet_of(entity);
        for watcher in self.watchers_of(entity) {
            self.forget(watcher, entity, serial);
        }
        self.seen.remove(&entity);
        self.unplace(facet, entity);
        self.registry.despawn(entity);
    }

    /// Everyone who currently has `entity` on their screen — the mobiles whose
    /// `seen` set holds it. The audience for a redraw: a health bar, a change of
    /// colour.
    #[must_use]
    pub fn watchers_of(&self, entity: EntityId) -> Vec<EntityId> {
        self.seen
            .iter()
            .filter(|(watcher, seen)| **watcher != entity && seen.contains(&entity))
            .map(|(watcher, _)| *watcher)
            .collect()
    }

    /// Redraw `entity`'s health bar: the real numbers to itself, a 0–100 scale to
    /// everyone watching. The `0xA1` a blow or a heal sends.
    pub fn broadcast_health(&mut self, entity: EntityId) {
        let Some(&Hitpoints { current, max }) = self.registry.get::<Hitpoints>(entity) else {
            return;
        };
        let Some(serial) = self.registry.serial_of(entity) else {
            return;
        };
        if let Some((connection, version)) = self.client_of(entity) {
            let exact = ServerPacket::Health(HealthBar::exact(serial, max, current));
            self.outbox.push(Outbound {
                connection,
                packet: exact.encode(version),
            });
        }
        let scaled = ServerPacket::Health(HealthBar::scaled(serial, max, current));
        for watcher in self.watchers_of(entity) {
            if let Some((connection, version)) = self.client_of(watcher) {
                self.outbox.push(Outbound {
                    connection,
                    packet: scaled.encode(version),
                });
            }
        }
    }

    /// Send one prebuilt, version-independent packet to every player within
    /// view range of `source` — its own client included.
    ///
    /// The audience for a sound or a graphical effect is who is *near*, not the
    /// `seen` set a health redraw uses: a door never enters anyone's `seen` (it is
    /// decoration, redrawn by `reveal`, not tracked as an interest), yet its creak
    /// must still be heard — so this asks the spatial index for neighbours the way
    /// `reveal` does, and keeps the ones with a client. There is no self-vs-others
    /// split: a sound and an effect are the same bytes for everyone, so a caller
    /// builds the packet once and this fans it out. The feedback seam every
    /// gameplay system reaches for — a swing, a spell, a door — so the world is
    /// *felt*, not merely correct.
    pub fn broadcast_from(&mut self, source: EntityId, packet: Vec<u8>) {
        let facet = self.facet_of(source);
        let sectors = self.facet_state(facet).sectors();
        let Some(centre) = sectors.position_of(source) else {
            return;
        };
        // Collected before the mutation so the sectors borrow is dropped. The
        // mobile list, because only a mobile has a client to hear it.
        let audience: Vec<EntityId> = sectors
            .mobiles_near(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .collect();
        for entity in audience {
            if let Some(&Client { connection, .. }) = self.registry.get::<Client>(entity) {
                self.outbox.push(Outbound {
                    connection,
                    packet: packet.clone(),
                });
            }
        }
    }

    /// Play `sound` at `source`'s position, heard by everyone who can see it.
    ///
    /// A no-op for a source with no `Position` (a contained item) — its holder's
    /// tile is where such a sound belongs, and that is the caller's to place. The
    /// `0x54` is placed in 3D so the client attenuates it by distance.
    pub fn play_sound(&mut self, source: EntityId, sound: SoundId) {
        let Some(&Position(at)) = self.registry.get::<Position>(source) else {
            return;
        };
        let packet = ServerPacket::PlaySound(PlaySound { sound, at });
        self.broadcast_packet(source, &packet);
    }

    /// Send `mobile` a private system line — seen by that client and no one else.
    ///
    /// The server talking, not a mobile: it goes out under the system serial in
    /// the client's usual grey, so it reads as feedback rather than as somebody
    /// speaking. A mobile with no client (an NPC, a scripted actor) simply hears
    /// nothing.
    pub fn system_message(&mut self, mobile: EntityId, text: &str) {
        let Some(&Client { connection, .. }) = self.registry.get::<Client>(mobile) else {
            return;
        };
        let packet = ServerPacket::SpokenMessage(SpokenMessage {
            serial: None, // the system talking, not a mobile
            graphic: None,
            mode: TalkMode::Regular,
            hue: SYSTEM_HUE,
            font: SYSTEM_FONT,
            name: "System".to_owned(),
            text: text.to_owned(),
        });
        self.send_packet(connection, &packet);
    }

    /// Tell a player that one of their skills rose, in ClassicUO's exact
    /// wording and hue. `previous` and `current` are in tenths, as on `0x3A`.
    pub fn skill_changed_message(&mut self, mobile: EntityId, skill: Skill, previous: u16, current: u16) {
        debug_assert!(
            current > previous,
            "a skill-change notice only reports an increase"
        );
        let Some(&Client { connection, .. }) = self.registry.get::<Client>(mobile) else {
            return;
        };
        let value = |tenths: u16| format!("{}.{:01}", tenths / 10, tenths % 10);
        let text = format!(
            "Your skill in {} has increased by {}.  It is now {}.",
            skill.info().name,
            value(current - previous),
            value(current),
        );
        let packet = ServerPacket::SpokenMessage(SpokenMessage {
            serial: None,
            graphic: None,
            mode: TalkMode::Regular,
            hue: Hue::SKILL_CHANGED,
            font: SYSTEM_FONT,
            name: "System".to_owned(),
            text,
        });
        self.send_packet(connection, &packet);
    }

    /// Send `mobile` a private **localized** line — a cliloc the client looks up
    /// in its own translation file and draws.
    ///
    /// The form nearly every stock message takes: a number travels, the player
    /// reads it in their own language, and the shard ships no English. `arguments`
    /// fills the cliloc's `~1_val~` slots, tab-separated, and is usually empty.
    /// A mobile with no client hears nothing, like [`system_message`](Self::system_message).
    pub fn localized_message(&mut self, mobile: EntityId, cliloc: ClilocId, arguments: &str) {
        debug_assert!(
            localized::contains(cliloc),
            "server emitted cliloc {} outside the shared catalogue",
            cliloc.0
        );
        let Some(&Client { connection, .. }) = self.registry.get::<Client>(mobile) else {
            return;
        };
        let packet = ServerPacket::LocalizedMessage(LocalizedMessage {
            serial: None, // the system talking, not a mobile
            graphic: None,
            mode: TalkMode::Regular,
            hue: SYSTEM_HUE,
            font: SYSTEM_FONT,
            cliloc,
            name: "System".to_owned(),
            arguments: arguments.to_owned(),
        });
        self.send_packet(connection, &packet);
    }

    /// Draw a localized line over `source`'s head, for `watcher` alone.
    ///
    /// ServUO's `PrivateOverheadMessage`: the same `0xC1`, but addressed with the
    /// looked-at thing's serial and graphic rather than the system's, so the text
    /// floats over *it* — and sent to one connection, so a crowded street does not
    /// read everybody's Anatomy check. The whole lore family answers this way.
    pub fn private_overhead_cliloc(
        &mut self,
        watcher: EntityId,
        source: EntityId,
        cliloc: ClilocId,
        arguments: &str,
    ) {
        debug_assert!(
            localized::contains(cliloc),
            "server emitted cliloc {} outside the shared catalogue",
            cliloc.0
        );
        let Some(&Client { connection, .. }) = self.registry.get::<Client>(watcher) else {
            return;
        };
        // A source with neither a serial nor a graphic falls back to the system's
        // sentinels, which is what the line degrades to: the watcher still reads
        // it, drawn as the server talking rather than over a thing that has no
        // wire identity to draw it over.
        let serial = self.registry.serial_of(source);
        let graphic = self
            .registry
            .get::<Body>(source)
            .map(|body| body.id)
            .or_else(|| self.registry.get::<Drawn>(source).map(|g| g.id));
        let packet = ServerPacket::LocalizedMessage(LocalizedMessage {
            serial,
            graphic,
            mode: TalkMode::Regular,
            hue: SYSTEM_HUE,
            font: SYSTEM_FONT,
            cliloc,
            name: String::new(),
            arguments: arguments.to_owned(),
        });
        self.send_packet(connection, &packet);
    }

    /// Draw `text` over `source` for `watcher` alone — ServUO's
    /// `PrivateOverheadMessage` with a plain string rather than a cliloc.
    ///
    /// The cliloc form ([`private_overhead_cliloc`](Self::private_overhead_cliloc))
    /// is what nearly everything should use, and this is for the one case it cannot
    /// serve: a line whose *content* is a name the client has no number for — Item
    /// Identification saying what an item turned out to be. Ships no English of its
    /// own; the text is a name already in the world.
    pub fn private_overhead_text(&mut self, watcher: EntityId, source: EntityId, text: &str) {
        let Some(&Client { connection, .. }) = self.registry.get::<Client>(watcher) else {
            return;
        };
        // Degrades to a system line for a source with no wire identity, exactly as
        // [`private_overhead_cliloc`](Self::private_overhead_cliloc) does.
        let serial = self.registry.serial_of(source);
        let graphic = self
            .registry
            .get::<Body>(source)
            .map(|body| body.id)
            .or_else(|| self.registry.get::<Drawn>(source).map(|g| g.id));
        let packet = ServerPacket::SpokenMessage(SpokenMessage {
            serial,
            graphic,
            mode: TalkMode::Regular,
            hue: SYSTEM_HUE,
            font: SYSTEM_FONT,
            name: String::new(),
            text: text.to_owned(),
        });
        self.send_packet(connection, &packet);
    }

    /// Play `sound` for `mobile` alone — a sound about the player, not about the
    /// world.
    ///
    /// The quest sounds are the reason this exists beside [`play_sound`]: ServUO's
    /// accept, resign, complete and objective-update chimes are feedback on a
    /// dialog only one person is looking at, and broadcasting them would have a
    /// whole street hear a stranger take a quest. The packet is still placed at the
    /// mobile's own tile, so the client does not attenuate it away.
    ///
    /// A no-op for a mobile with no client (an NPC) or no position.
    pub fn play_sound_to(&mut self, mobile: EntityId, sound: SoundId) {
        let Some(&Client { connection, .. }) = self.registry.get::<Client>(mobile) else {
            return;
        };
        let Some(&Position(at)) = self.registry.get::<Position>(mobile) else {
            return;
        };
        let packet = ServerPacket::PlaySound(PlaySound { sound, at });
        self.send_packet(connection, &packet);
    }

    /// Turn `mobile` to look at `other`, and tell everyone watching.
    ///
    /// Two people talking face each other; ServUO does it with `GetDirectionTo`
    /// before a greeting or a beg. A no-op if either has no position, or if the
    /// mobile is already facing that way — the broadcast is not free.
    pub fn face_toward(&mut self, mobile: EntityId, other: EntityId) {
        let Some(&Position(to)) = self.registry.get::<Position>(other) else {
            return;
        };
        self.face_point(mobile, to);
    }

    /// The same, at a spot on the ground rather than at somebody.
    ///
    /// A harvester turns to the rock face it is about to swing at, and there is
    /// nothing there to be an entity — ServUO's `DoHarvestingEffect` opens with
    /// `from.Direction = from.GetDirectionTo(loc)`.
    pub fn face_point(&mut self, mobile: EntityId, to: Point) {
        let Some(&Position(from)) = self.registry.get::<Position>(mobile) else {
            return;
        };
        let Some(direction) = openshard_movement::direction_toward(from, to) else {
            return; // standing on the same tile: no way to face
        };
        let facing = openshard_protocol::direction::Facing::walking(direction);
        if self.registry.get::<Heading>(mobile).map(|h| h.0) == Some(facing) {
            return;
        }
        self.registry.insert(mobile, Heading(facing));
        if let Some(Movement(mut walker)) = self.registry.get::<Movement>(mobile).copied() {
            walker.facing = facing;
            self.registry.insert(mobile, Movement(walker));
        }
        self.broadcast_move(mobile);
        // `0x77` is deliberately ignored by its owner: the local client moves
        // that body from its walk acknowledgements and prediction.  A combat
        // turn has no acknowledgement, though, so its owner also needs the
        // authoritative `0x20` update or will keep rendering the old facing.
        self.send_player_update(mobile, from);
    }

    /// Tell everyone watching that `mobile` just died, and which corpse it leaves.
    ///
    /// `0xAF`, and the only thing on the wire that pairs the two. A death is
    /// otherwise two unrelated facts — a mobile stops being drawn, an item
    /// appears — so a client that wants to run the fall into the body lying there
    /// has to guess the pairing, and a tile with two identical creatures dying on
    /// it is enough to make the guess wrong.
    ///
    /// Not sent to the dying mobile's own client. ServUO skips it the same way
    /// (`Mobile.Kill` excludes `m_NetState` from the range loop): that client is
    /// told it died by `0x2C`, and what it is watching afterwards is its own
    /// ghost, not a corpse it has to pair anything with.
    ///
    /// `corpse` is `None` for a death that leaves no body — a creature with no
    /// `Body`, which is the same case that leaves the corpse item bodiless.
    pub fn announce_death(&mut self, mobile: EntityId, corpse: Option<Serial>) {
        let Some(serial) = self.registry.serial_of(mobile) else {
            return;
        };
        // What the body was doing when it fell. A client with a second death
        // group draws a running death differently; ours has one, and sending the
        // truth costs nothing and cannot be recovered later.
        let running = self
            .registry
            .get::<Heading>(mobile)
            .is_some_and(|Heading(facing)| facing.running);
        let packet = ServerPacket::DeathAnimation(openshard_protocol::world::DeathAnimation {
            killed: serial,
            corpse,
            running,
        });
        let own = self
            .registry
            .get::<Client>(mobile)
            .map(|client| client.connection);
        for (connection, version) in self.audience_of(mobile) {
            if Some(connection) == own {
                continue;
            }
            self.outbox.push(Outbound {
                connection,
                packet: packet.encode(version),
            });
        }
    }

    /// Animate `mobile` performing `action` — a swing, a death throe, a cast
    /// gesture — for everyone who can see it.
    ///
    /// The wire is per-client, not per-packet: a modern client (7.0.0.0+) gets the
    /// `0xE2` new-animation packet, where the server names a body-agnostic
    /// [`AnimationType`](Action) and the client picks the frames for that body —
    /// which is why a swing needs no body table there. An older client gets the
    /// `0x6E` classic packet, whose action id *is* body-specific, so it is chosen
    /// off a coarse humanoid-vs-creature split (the same `body_opens_doors` line
    /// the door AI uses). The split is deliberately rough: exact per-weapon,
    /// per-body actions want the animation tables the references key off body id,
    /// and the modern path — the one the test clients take — does not need them.
    pub fn animate(&mut self, mobile: EntityId, action: Action) {
        let Some(serial) = self.registry.serial_of(mobile) else {
            return;
        };
        let humanoid = self
            .registry
            .get::<Body>(mobile)
            .is_some_and(|body| body_opens_doors(body.id));
        // Built once each; the per-recipient choice is only which to send.
        let new_packet = ServerPacket::NewAnimation(NewAnimation {
            serial,
            animation_type: action.animation_type(),
            action: action.sub_action(),
            delay: 0,
        });
        let (old_action, frames) = action.classic_action(humanoid);
        let old_packet = ServerPacket::Animation(Animation {
            serial,
            action: old_action,
            frame_count: openshard_protocol::feedback::AnimationFrameCount(frames),
            repeat_count: 1,
            forward: true,
            repeat: false,
            delay: 0,
        });

        for (connection, version) in self.audience_of(mobile) {
            let packet = if version.supports(Feature::NewMobileAnimation) {
                &new_packet
            } else {
                &old_packet
            };
            self.outbox.push(Outbound {
                connection,
                packet: packet.encode(version),
            });
        }
    }
}

/// A mobile action worth animating — the semantic the caller names, which
/// [`WorldState::animate`] turns into the wire animation each client understands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// A melee or ranged swing.
    Attack,
    /// A death throe.
    Die,
    /// A spellcasting gesture.
    Cast,
    /// Swinging a pick at a rock face.
    Mine,
    /// Swinging an axe at a tree.
    Chop,
    /// Casting a line.
    Fish,
    /// A bow — what a beggar does before asking, and the one action here that is a
    /// courtesy rather than a blow.
    Bow,
}

impl Action {
    /// The `0xE2` [`AnimationType`](Action) — ServUO's enum: Attack 0, Die 3,
    /// Spell 11, Bow 9. The client maps it to the right frames for whatever body it
    /// is, so no body table is needed on this path.
    const fn animation_type(self) -> u16 {
        match self {
            Self::Attack => 0, // Attack
            Self::Die => 3,    // Die
            Self::Cast => 11,  // Spell
            Self::Bow => 9,    // Bow
            // ServUO's `DoHarvestingEffect` animates a harvest as an *attack* and
            // says which one in the sub-action — see [`sub_action`](Self::sub_action).
            Self::Mine | Self::Chop | Self::Fish => 0, // Attack
        }
    }

    /// The `0xE2` sub-action, which narrows the category above.
    ///
    /// Zero — "whatever this body does for that category" — for everything the
    /// client can pick itself. Harvesting is the exception: ServUO passes
    /// `AnimationType.Attack` with the number of the swing it wants
    /// (`DoHarvestingEffect`, the `Core.SA` branch), because mining, chopping and
    /// casting a line are three different motions and none of them is "attack".
    const fn sub_action(self) -> u16 {
        match self {
            Self::Mine => 3,
            Self::Fish => 6,
            Self::Chop => 7,
            _ => 0,
        }
    }

    /// The `0x6E` classic action id and frame count, which *are* body-specific.
    /// The humanoid ids are ServUO's people-animation values (Wrestle 31, human
    /// die 21, human directed-cast 16); the creature ids its monster-group values
    /// (attack 4, die 2, cast 12). A coarse split until weapon and body tables
    /// land — good enough for the old 2D client, which is the minority path.
    const fn classic_action(self, humanoid: bool) -> (u16, u16) {
        match (self, humanoid) {
            (Self::Attack, true) => (31, 7), // WeaponAnimation.Wrestle
            (Self::Attack, false) => (4, 4), // monster attack1
            (Self::Die, true) => (21, 6),    // human die
            (Self::Die, false) => (2, 4),    // monster die
            (Self::Cast, true) => (16, 7),   // human directed-cast
            (Self::Cast, false) => (12, 7),  // monster cast
            // Only a person bows; a creature that is asked for money simply looks
            // at you, so the classic path animates nothing body-specific for it.
            (Self::Bow, true) => (32, 5), // human bow
            (Self::Bow, false) => (4, 4), // nothing better on a monster
            // The pre-SA harvest actions, ServUO's `EffectActions`. Only a person
            // swings a pick or casts a line; the creature arm is unreachable in
            // practice, since a tool is double-clicked by a client.
            (Self::Mine, true) => (11, 5), // human bend down / mine
            (Self::Chop, true) => (13, 6), // human two-handed swing
            (Self::Fish, true) => (12, 5), // human cast a line
            (Self::Mine | Self::Chop | Self::Fish, false) => (4, 4),
        }
    }
}

/// Interest management: the machinery that keeps each client's screen in sync
/// with the world — who to draw, who to forget, who to redraw on a move. Shared
/// by every system that changes what a mobile looks like or where it stands.
impl WorldState {
    /// Move a mobile to `to` at once — a teleport, not a walk. Sets its position
    /// everywhere the world tracks it, tells its own client to jump there, and
    /// refreshes what it and everyone around it can see.
    ///
    /// The own-client `0x20` is the part a plain position write forgets: without
    /// it the client keeps drawing its character at the old tile while the new
    /// neighbours appear around where it used to stand — the "teleport did not
    /// refresh" bug. A walk does not need this because the client predicts its own
    /// step; a decree does, because the client was not expecting to move.
    pub fn teleport(&mut self, entity: EntityId, to: Point) {
        self.move_to(entity, self.facet_of(entity), to);
    }

    /// Move a mobile to `to` on `facet`, which may not be the one it is standing
    /// on. The one door for every relocation: [`teleport`](Self::teleport) is
    /// this with the facet it already has.
    ///
    /// A facet change is not a longer teleport. Five things remember where a
    /// mobile is, and none of them is checked by a compiler: the traveller's own
    /// screen, every watcher's screen, the old facet's sector grid, the region it
    /// was last seen in, and the music its client is playing. Leave any one of
    /// them behind and nothing errors — the client simply keeps drawing mobiles
    /// from a world it is no longer in, at coordinates that now mean somewhere
    /// else, and every `nearby` query on the facet it left keeps handing back
    /// someone who is not there. So the order below is ServUO's `Mobile.Map`
    /// setter, and each step says what it is for.
    pub fn move_to(&mut self, entity: EntityId, facet: Facet, to: Point) {
        let from = self.facet_of(entity);
        // Never strand someone on a facet the shard did not load; a mobile there
        // would have no ground, no neighbours and no way back.
        if from != facet && !self.facets.contains_key(&facet) {
            return;
        }

        if from != facet {
            // Take the traveller off every screen on the facet it is leaving,
            // while `watchers_of` can still be trusted — after the move it is on
            // another grid and this finds nobody.
            if let Some(serial) = self.registry.serial_of(entity) {
                for watcher in self.watchers_of(entity) {
                    self.forget(watcher, entity, serial);
                }
            }
            // And clear the traveller's own screen: everything on it belongs to
            // the other facet. ServUO's `ClearScreen`, and the one step whose
            // absence is invisible in a test and permanent in a client.
            let remembered: Vec<EntityId> = self
                .seen
                .get(&entity)
                .map(|seen| seen.iter().copied().collect())
                .unwrap_or_default();
            for other in remembered {
                if let Some(serial) = self.registry.serial_of(other) {
                    self.forget(entity, other, serial);
                }
            }
            // Out of the old grid. `teleport` never had to do this, which is why
            // the removal is easy to leave out and costly to leave out.
            self.unplace(from, entity);
            self.registry.insert(entity, facet);
            // The remembered region indexes the *old* facet's list, so keeping it
            // would compare a Felucca id against an Ilshenar one.
            self.registry.remove::<InRegion>(entity);
        }

        self.registry.insert(entity, Position(to));
        // Keep the walker's own copy in step, or the next walk starts from the old
        // tile. The sequence goes back to fresh with it: the client zeroes its own
        // on a jump, and a server that does not asks it for a resync it cannot give
        // (Sphere says as much beside its own reset).
        if let Some(Movement(mut walker)) = self.registry.get::<Movement>(entity).copied() {
            walker.position = to;
            walker.sequence.reset();
            self.registry.insert(entity, Movement(walker));
        }
        // A mobile: every caller of this is a traveller — a gate, a recall, a
        // `.go`, a body relocated by the ship it is standing on. An item is put
        // down by `items::place_on_ground`, which files it as one.
        self.place_mobile(facet, entity, to);

        if from != facet {
            if let Some(&Client { connection, .. }) = self.registry.get::<Client>(entity) {
                if let Some(version) = self.version_of(connection) {
                    let size = {
                        let state = self.facet_state(facet);
                        MapSize::for_client(facet, state.width(), state.height(), version)
                    };
                    // Which map to draw, then where on it and how big it is. No
                    // `0x1B`: that is the "entering the world" packet, and neither
                    // reference re-sends it mid-session.
                    self.send_packet(connection, &ServerPacket::MapChange(MapChange { map: facet }));
                    self.send(connection, encode_server_change(to, size));
                }
            }
        }

        self.send_player_update(entity, to);
        self.refresh_around(entity);
    }

    /// Tell `entity`'s own client where it is standing: the `0x20`.
    ///
    /// The packet the client cannot deduce. A `0x22` ack carries no position, so
    /// this is the only thing that ever *states* one for the player's own body —
    /// which is why both a decreed move and a resync end with it.
    ///
    /// `at` rather than the position on the row, because [`Self::move_to`] calls
    /// this in the middle of a relocation and the two must not be able to
    /// disagree about which tile is being announced.
    fn send_player_update(&mut self, entity: EntityId, at: Point) {
        let Some(&Client { connection, .. }) = self.registry.get::<Client>(entity) else {
            return;
        };
        // The serial joins the body and the facing in the `if let`: a `0x20`
        // addressed to nothing is not worth sending, and the old `map_or(0, …)`
        // sent one — zero is not a serial, it is the wire's word for "no object".
        let serial = self.registry.serial_of(entity);
        let body = self.registry.get::<Body>(entity).copied();
        let facing = self.registry.get::<Heading>(entity).map(|h| h.0);
        // The same byte the `0x77`/`0x78` about this body would carry — see
        // [`stance_of`](Self::stance_of), which is where the argument for the
        // `0x20` carrying it at all lives.
        let flags = self.stance_of(entity);
        if let (Some(serial), Some(body), Some(facing)) = (serial, body, facing) {
            self.send_packet(
                connection,
                &ServerPacket::PlayerUpdate(PlayerUpdate {
                    serial,
                    body: body.id,
                    hue: body.hue,
                    flags,
                    position: at,
                    facing,
                }),
            );
        }
    }

    /// Answer a client's `0x22` resync: where it really is, what is really around
    /// it, and the walk sequence back to zero on both ends.
    ///
    /// # Why a client asks
    ///
    /// It has lost track of the walk handshake — an ack it cannot place — and a
    /// `0x22` ack carries no position, so there is nothing local it can work the
    /// answer out from. Our own client stops walking when that happens and waits
    /// for this; so does ClassicUO (`WalkerManager.ConfirmWalk`'s bad-step path
    /// sets `WalkingFailed` and sends the request, and its `0x20` handler is what
    /// clears the flag). Which means a shard that ignores the request leaves such
    /// a client frozen for good — this is not optional politeness, it is the other
    /// half of a handshake.
    ///
    /// # What it sends, and why the screen is cleared first
    ///
    /// ServUO's `Resynchronize`: `MobileUpdate`, `MobileIncoming`,
    /// `SendEverything`, `state.Sequence = 0`, `ClearFastwalkStack`. The list
    /// here is the same one in our own terms — the sequence, then everything on
    /// this client's screen forgotten so that [`Self::refresh_around`] sends it
    /// again, then the `0x20`.
    ///
    /// Forgetting the screen is the part that looks like waste and is not: a
    /// client that has lost the walk may have lost more than the walk, and
    /// `seen` exists precisely so that nothing is re-sent to a client that
    /// already has it — so without this, "resend everything" sends nothing at
    /// all. It costs one full redraw of a screen's worth of mobiles, once, on an
    /// event that should be rare enough to be worth logging.
    pub fn resync(&mut self, entity: EntityId) {
        if let Some(Movement(mut walker)) = self.registry.get::<Movement>(entity).copied() {
            // Both ends fresh: the client zeroes its own counter when it asks,
            // and a server still expecting the old byte would refuse the first
            // step after the repair — which is the freeze this exists to end,
            // arrived at by a different road.
            walker.sequence.reset();
            self.registry.insert(entity, Movement(walker));
        }
        let remembered: Vec<EntityId> = self
            .seen
            .get(&entity)
            .map(|seen| seen.iter().copied().collect())
            .unwrap_or_default();
        for other in remembered {
            if let Some(serial) = self.registry.serial_of(other) {
                self.forget(entity, other, serial);
            }
        }
        let at = self.registry.get::<Position>(entity).map(|position| position.0);
        if let Some(at) = at {
            self.send_player_update(entity, at);
        }
        self.refresh_around(entity);
    }

    /// Bring `entity`'s neighbourhood up to date, both ways.
    ///
    /// Whoever it can see, and whoever can see it. Both, because visibility is
    /// symmetric here and doing one direction leaves the other end with a mobile
    /// that walked away and never left the screen.
    pub fn refresh_around(&mut self, entity: EntityId) {
        // Only this entity's facet: two mobiles on different facets share no
        // sector grid, so a lookup here never turns up anyone on another one.
        let facet = self.facet_of(entity);
        let sectors = self.facet_state(facet).sectors();
        let Some(centre) = sectors.position_of(entity) else {
            return;
        };

        // A mobile with no client has no screen, and `show` says so on its first
        // line — so for an NPC every one of the two directions below but one is
        // work done to be thrown away. Only "who can see *me*" means anything, and
        // the answer to that is a walk of the players, of whom there are a
        // handful, rather than a sweep of the sector block, which in a decorated
        // town hands back several hundred statics to sift for a few neighbours.
        //
        // This is the difference between an NPC step costing O(everything nearby)
        // and O(players), and almost every step taken in a populated shard is an
        // NPC's.
        if !self.registry.has::<Client>(entity) {
            self.refresh_watchers(entity, centre, facet);
            self.broadcast_move(entity);
            return;
        }

        // Collect first. The lookup borrows the index and the sends borrow `self`
        // mutably, and more importantly a snapshot here is what keeps the set of
        // neighbours from shifting while it is walked. A set and not a `Vec`:
        // it is membership-tested once per remembered entity and once per watcher
        // below, which on a `Vec` is a linear scan inside two loops.
        //
        // Both lists, and the only lookup in this file that wants both: a screen
        // holds the people *and* the furniture, so a player who walks up to a
        // house has to be sent the house.
        let neighbours: HashSet<EntityId> = sectors
            .everything_near(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .filter(|id| *id != entity)
            .collect();

        for other in &neighbours {
            self.show(entity, *other);
            self.show(*other, entity);
        }

        // Anything this one used to see and no longer can. `nearby` says who is
        // close; only the remembered set says who *was*.
        let gone: Vec<EntityId> = self
            .seen
            .get(&entity)
            .map(|seen| {
                seen.iter()
                    .filter(|id| !neighbours.contains(id))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        for other in gone {
            if let Some(serial) = self.registry.serial_of(other) {
                self.forget(entity, other, serial);
            }
        }

        // And anyone who used to see this one and no longer can.
        for watcher in self.watchers_of(entity) {
            if !neighbours.contains(&watcher) {
                if let Some(serial) = self.registry.serial_of(entity) {
                    self.forget(watcher, entity, serial);
                }
            }
        }

        self.broadcast_move(entity);
    }

    /// The half of [`refresh_around`](Self::refresh_around) that matters for a
    /// mobile with no screen of its own: draw it for the players who can now see
    /// it, and take it off the screens of those who cannot.
    ///
    /// Reached by walking the players, not the sector index — see the caller.
    fn refresh_watchers(&mut self, entity: EntityId, centre: Point, facet: Facet) {
        let players: Vec<EntityId> = self.players.values().copied().collect();
        for player in players {
            if player == entity {
                continue;
            }
            let near = self.facet_of(player) == facet
                && self
                    .registry
                    .get::<Position>(player)
                    .is_some_and(|at| crate::sectors::in_range(at.0, centre, VIEW_RANGE));
            if near {
                self.show(player, entity);
            } else if let Some(serial) = self.registry.serial_of(entity) {
                self.forget(player, entity, serial);
            }
        }
    }

    /// Tell everyone already watching `entity` that it moved.
    ///
    /// Only those who already have it: someone seeing it for the first time gets
    /// a `0x78` from [`show`](Self::show), and a `0x77` for a mobile the client
    /// has never heard of is ignored.
    pub fn broadcast_move(&mut self, entity: EntityId) {
        // Built inside the loop, not once above it: the notoriety byte is the
        // *watcher's* answer, and a guildmate is green to one of these clients
        // and blue to the next. The rest of the packet is the same for everyone,
        // and rebuilding it is a handful of component reads — see
        // `notoriety_toward` for where that cost is and is not paid.
        for watcher in self.watchers_of(entity) {
            let Some((connection, version)) = self.client_of(watcher) else {
                continue;
            };
            let Some(packet) = self.mobile_move(watcher, entity) else {
                return;
            };
            self.outbox.push(Outbound {
                connection,
                packet: ServerPacket::MobileMove(packet).encode(version),
            });
        }
    }

    /// Draw `other` for `watcher`, if it is not already on screen.
    pub fn show(&mut self, watcher: EntityId, other: EntityId) {
        // Only players have screens. An NPC "seeing" someone is an AI question,
        // and it does not belong in the packet path.
        let Some((connection, version)) = self.client_of(watcher) else {
            return;
        };
        if self.seen.get(&watcher).is_some_and(|seen| seen.contains(&other)) {
            return;
        }
        // The living cannot see the dead: a ghost is drawn only to another ghost
        // or to staff. Skip it here, before it ever enters `seen`, so a living
        // watcher never has it on screen to move or forget.
        if !self.can_see_mobile(watcher, other) {
            return;
        }
        let Some(packet) = self.draw_packet(watcher, other, version) else {
            return;
        };
        self.seen.entry(watcher).or_default().insert(other);
        self.outbox.push(Outbound { connection, packet });
        // The health bar rides along with the draw. There is no "what is its
        // health" packet the client can count on us answering — it opens the bar
        // from what it was last told — so a mobile whose bar is never sent shows an
        // empty frame until the first blow moves it. Send the scaled bar on sight
        // and it reads full from the moment you see it, like every other client.
        if let Some(&Hitpoints { current, max }) = self.registry.get::<Hitpoints>(other) {
            if let Some(serial) = self.registry.serial_of(other) {
                let bar = ServerPacket::Health(HealthBar::scaled(serial, max, current));
                self.outbox.push(Outbound {
                    connection,
                    packet: bar.encode(version),
                });
            }
        }
        // AoS tooltip: the drawn thing's property revision rides along, so the
        // client knows its cached tooltip is stale and can ask for a fresh one.
        if let Some(tooltip) = self.tooltip_packet(other, version) {
            self.outbox.push(Outbound {
                connection,
                packet: tooltip,
            });
        }
        // And a designed house's *picture* revision, which is the same shape one
        // question along: the client knows its cached walls are stale and can ask
        // for a fresh `0xD8`. The reference does exactly this and in exactly this
        // place — `HouseFoundation.SendInfoTo` overrides the item's own "show
        // yourself" and appends the general-info packet after it.
        if let Some(design) = self.design_revision_packet(other, version) {
            self.outbox.push(Outbound {
                connection,
                packet: design,
            });
        }
    }

    /// The `0xBF 0x1D` that says which revision a designed house's picture is
    /// at, to send *alongside* its draw — or `None` for anything that is not a
    /// designed house, and for a client too old to speak the design packets.
    ///
    /// [`tooltip_packet`](Self::tooltip_packet)'s twin, and the parallel is
    /// exact: both are a cheap "what you have cached may be stale" that rides
    /// with the draw so the client can ask for the expensive thing only on a
    /// miss. Without it, every client walking into a neighbourhood would
    /// re-fetch every design in it on every approach.
    ///
    /// A classic multi answers `None` and costs nothing: its picture is in the
    /// client's own files and has no revision.
    fn design_revision_packet(&self, entity: EntityId, version: ClientVersion) -> Option<Vec<u8>> {
        if !version.supports(Feature::CustomMulti) {
            return None;
        }
        let design = self.registry.get::<crate::components::HouseDesign>(entity)?;
        let serial = self.registry.serial_of(entity)?;
        Some(
            openshard_protocol::design::DesignRevision {
                serial: openshard_protocol::serial::RawSerial(serial.raw()),
                revision: openshard_protocol::design::Revision(design.revision),
            }
            .encode(),
        )
    }

    /// The `0xD8` a client asked for, or `None` when the entity is not a
    /// designed house or this shard has no terrain to answer with.
    ///
    /// Built here rather than in `openshard-housing` because everything it needs
    /// is here: the components are a component, the bounds are arithmetic over
    /// them, and the *floor* predicate is the facet's terrain. Housing owns the
    /// rules about who may change a design; this is the drawing substrate, which
    /// is where [`show`](Self::show) and [`tooltip_packet`](Self::tooltip_packet)
    /// already live.
    ///
    /// `response` is the reference's own flag: set when the design goes out
    /// because a client asked, clear when the shard volunteered it.
    #[must_use]
    pub fn design_detail_packet(&self, house: EntityId, response: bool) -> Option<Vec<u8>> {
        use openshard_protocol::design::{DesignDetail, DesignTile};

        let design = self.registry.get::<crate::components::HouseDesign>(house)?;
        let serial = self.registry.serial_of(house)?;
        // The table, not the facet the house stands on: how tall a graphic is has
        // nothing to do with where it was placed. A shard with no client files
        // holds an empty one, and every component in it is a floor — the picture
        // an install that cannot tell a wall from a floor deserves.
        let tiledata = &self.tiles;

        // Only what the client draws. An undrawn component is not part of the
        // picture, and the signature tile every multi opens with is one.
        let tiles: Vec<DesignTile> = design
            .components
            .iter()
            .filter(|component| component.drawn())
            .filter_map(|component| {
                Some(DesignTile {
                    graphic: openshard_protocol::wire::Graphic(component.graphic),
                    dx: i8::try_from(component.dx).ok()?,
                    dy: i8::try_from(component.dy).ok()?,
                    dz: i8::try_from(component.dz).ok()?,
                })
            })
            .collect();
        if tiles.is_empty() {
            return None;
        }
        Some(
            DesignDetail {
                serial: openshard_protocol::serial::RawSerial(serial.raw()),
                revision: openshard_protocol::design::Revision(design.revision),
                response,
                tiles: &tiles,
            }
            // A *floor* is a static with no height, which is `tiledata`'s answer
            // and the one thing `openshard-protocol` refused to guess — C1
            // recorded the seam and this is the caller that holds the table.
            .encode(|graphic| tiledata.static_tile(graphic.0).height == 0),
        )
    }

    /// Send a designed house's picture to one player, because they asked.
    pub fn send_design_detail(&mut self, watcher: EntityId, house: EntityId) {
        let Some((connection, version)) = self.client_of(watcher) else {
            return;
        };
        if !version.supports(Feature::CustomMulti) {
            return;
        }
        let Some(packet) = self.design_detail_packet(house, true) else {
            return;
        };
        self.outbox.push(Outbound { connection, packet });
    }

    /// Tell everyone who can see a designed house that its picture changed.
    ///
    /// [`show`](Self::show)'s packet, sent again on its own when the design
    /// commits — a client that is already looking at the house will never be
    /// shown it a second time, so the draw's copy cannot reach it.
    ///
    /// Encoded per recipient rather than fanned out as one buffer, because the
    /// gate is per client version: `Feature::CustomMulti` is 4.0.0a and a shard
    /// may well have older clients on it.
    pub fn broadcast_design_revision(&mut self, house: EntityId) {
        let Some(design) = self.registry.get::<crate::components::HouseDesign>(house) else {
            return;
        };
        let Some(serial) = self.registry.serial_of(house) else {
            return;
        };
        let packet = openshard_protocol::design::DesignRevision {
            serial: openshard_protocol::serial::RawSerial(serial.raw()),
            revision: openshard_protocol::design::Revision(design.revision),
        }
        .encode();
        for (connection, version) in self.audience_of(house) {
            if !version.supports(Feature::CustomMulti) {
                continue;
            }
            self.outbox.push(Outbound {
                connection,
                packet: packet.clone(),
            });
        }
    }

    /// The tooltip packet to send *alongside* a draw, or `None` when tooltips are
    /// off, the client is too old for them, or the object has no properties.
    ///
    /// In send-version mode a client new enough for revision hashes ([`0xDC`],
    /// [`Feature::TooltipHash`]) gets just the revision and asks for the list on
    /// hover; an older AoS client, or send-full mode, gets the whole list up front
    /// — it cannot request one it was never told a revision for. Sphere's
    /// `TOOLTIPMODE`.
    fn tooltip_packet(&self, entity: EntityId, version: ClientVersion) -> Option<Vec<u8>> {
        if self.gameplay.tooltip_mode == TooltipMode::Off || !version.supports(Feature::Tooltips) {
            return None;
        }
        let (full, hash) = self.object_properties(entity)?;
        let send_version =
            self.gameplay.tooltip_mode == TooltipMode::SendVersion && version.supports(Feature::TooltipHash);
        if send_version {
            let serial = self.registry.serial_of(entity)?;
            Some(ServerPacket::TooltipRevision(TooltipRevision { serial, hash }).encode(version))
        } else {
            Some(full)
        }
    }

    /// The `0xD6` property list for an object and its revision hash, or `None` for
    /// something with no name to show. Name-only for now: a mobile is cliloc
    /// `1050045` (`~1_PREFIX~~2_NAME~~3_SUFFIX~`) with its [`Name`]; an item is
    /// cliloc `1020000 + graphic` — the client's own tiledata-name range, so no
    /// string is sent — pluralised through cliloc `1050039` when it is a stack.
    /// The item-vs-mobile split is [`draw_packet`](Self::draw_packet)'s, read for
    /// a tooltip rather than a draw. Ported from ServUO's `AddNameProperties` /
    /// `Item.AddNameProperty`.
    #[must_use]
    pub fn object_properties(&self, entity: EntityId) -> Option<(Vec<u8>, u32)> {
        let serial = self.registry.serial_of(entity)?;
        let mut list = PropertyList::new(serial);
        if let Some(Name(name)) = self.registry.get::<Name>(entity) {
            // The earned name — a fame title once the mobile is famous enough for an
            // onlooker to have heard of it. The cliloc is `~1_PREFIX~~2_NAME~~3_SUFFIX~`
            // and ServUO fills the three separately; the title table interleaves a
            // prefix and a suffix around the name in one string, so it goes in the name
            // slot whole and the other two stay empty.
            let name = crate::title::titled_name(self, entity, name);
            // The guild's abbreviation is the suffix slot the cliloc already has —
            // ServUO's `PlayerMobile.AddNameProperties` writes "[OSS]" there.
            let suffix = self
                .guild_of(entity)
                .map_or_else(String::new, |guild| format!("[{}]", guild.abbreviation));
            list.add_args(ClilocId(1_050_045), &format!(" \t{name}\t {suffix}"));
            // And a line of its own for the title and the guild's full name, which
            // is where ServUO puts them. Cliloc `1042971` is `~1_NOTHING~`, the
            // client's own way of showing a line the server wrote.
            if let Some((title, guild)) = self.guild_title_of(entity) {
                list.add_args(ClilocId(1_042_971), &format!("{title}, {guild}"));
            }
        } else if let Some(&Drawn { id, .. }) = self.registry.get::<Drawn>(entity) {
            let cliloc = ClilocId(1_020_000 + u32::from(id.0));
            match self.registry.get::<Amount>(entity) {
                Some(Amount(amount)) if *amount > 1 => {
                    list.add_args(ClilocId(1_050_039), &format!("{amount}\t#{}", cliloc.0));
                }
                _ => list.add(cliloc),
            }
        } else {
            return None;
        }
        // What a player made says so, and says whose work it is — ServUO's
        // `AddCraftedProperties`. Both lines are appended rather than folded into
        // the name, so an exceptional dagger is still "a dagger" to everything
        // that reads the name.
        if self
            .registry
            .get::<Quality>(entity)
            .is_some_and(|quality| quality.exceptional)
        {
            list.add(ClilocId(1_060_636)); // Exceptional
        }
        if let Some(CraftedBy(maker)) = self.registry.get::<CraftedBy>(entity) {
            list.add_args(ClilocId(1_050_043), maker); // crafted by ~1_NAME~
        }
        Some(list.finish())
    }

    /// Send `entity`'s full `0xD6` property list to one connection — the answer to
    /// a client's tooltip request. Nothing is sent for an object with no name.
    pub fn send_property_list(&mut self, connection: ConnectionId, entity: EntityId) {
        if let Some((packet, _)) = self.object_properties(entity) {
            self.outbox.push(Outbound { connection, packet });
        }
    }

    /// The packet that draws `entity` on a client, or `None` for something not
    /// drawable. A mobile is a `0x78`, an item a `0x1A` — the interest system does
    /// not care which, only that there is one packet per thing on screen.
    #[must_use]
    pub fn draw_packet(
        &mut self,
        viewer: EntityId,
        entity: EntityId,
        version: ClientVersion,
    ) -> Option<Vec<u8>> {
        if self.registry.has::<Body>(entity) {
            let incoming = self.mobile_incoming(viewer, entity)?;
            Some(ServerPacket::MobileIncoming(incoming).encode(version))
        } else if self.registry.has::<Drawn>(entity) {
            Some(ServerPacket::WorldItem(self.world_item(entity)?).encode(version))
        } else {
            None
        }
    }

    /// Build a `0x1A` for an entity, if it is a drawable item.
    #[must_use]
    pub fn world_item(&self, entity: EntityId) -> Option<WorldItem> {
        let serial = self.registry.serial_of(entity)?;
        let Drawn { id, hue } = *self.registry.get::<Drawn>(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        // No `Amount` means a single. The encoder treats 1 and absent the same.
        let payload = if id == crate::components::CORPSE_GRAPHIC {
            let CorpseBody { body, facing } = *self.registry.get::<CorpseBody>(entity)?;
            openshard_protocol::items::WorldItemPayload::Corpse { body, facing }
        } else {
            let amount = self.registry.get::<Amount>(entity).map_or(1, |a| a.0);
            openshard_protocol::items::WorldItemPayload::Stack(openshard_protocol::items::ItemAmount(amount))
        };
        Some(WorldItem {
            serial,
            graphic: id,
            payload,
            position,
            hue,
            // An item's light comes from its graphic here, and what a player may
            // pick up is decided at the moment they try — see `items::drag`. Both
            // bytes exist for a shard that says otherwise; this one does not.
            light: None,
            flags: openshard_protocol::items::ItemFlags::NONE,
        })
    }

    /// Take `other` off `watcher`'s screen.
    pub fn forget(&mut self, watcher: EntityId, other: EntityId, serial: Serial) {
        if let Some(seen) = self.seen.get_mut(&watcher) {
            if !seen.remove(&other) {
                return;
            }
        } else {
            return;
        }
        if let Some(&Client { connection, .. }) = self.registry.get::<Client>(watcher) {
            self.send_packet(connection, &ServerPacket::Remove(Remove { serial }));
        }
    }

    /// What `watcher`'s *account* is held at — [`AccessLevel::Player`] for one
    /// with no [`Access`] of its own, which is every ordinary character and
    /// every creature the world spawned.
    ///
    /// The level itself rather than the gate, for the one caller that reports it
    /// instead of testing it: `AuthorityNotice`, which tells a client what it may
    /// offer to complete. Every *gate* asks [`staff_authority`](Self::staff_authority),
    /// which is this compared against a level and is what a permission check
    /// should read.
    #[must_use]
    pub fn access_level(&self, watcher: EntityId) -> AccessLevel {
        self.registry
            .get::<Access>(watcher)
            .map_or(AccessLevel::Player, |access| access.0)
    }

    /// Whether `watcher`'s *account* may command — a GameMaster or above.
    ///
    /// The authority half of Sphere's split: `PLEVEL` says who may run a staff
    /// command, and it never moves within a session. Every `.`-command gate reads
    /// this, which is what lets a game master who has turned their staff mode
    /// *off* turn it back on again.
    ///
    /// The threshold is [`StaffCommand::AUTHORITY`] and not a level written
    /// here, because the client's completer filters by the same constant
    /// (`StaffCommand::matching`): a shard that raised its bar and a client that
    /// did not would offer words this refuses, which is the drift the shared
    /// vocabulary crate exists to make impossible.
    #[must_use]
    pub fn staff_authority(&self, watcher: EntityId) -> bool {
        self.access_level(watcher).allows(StaffCommand::AUTHORITY)
    }

    /// Whether `watcher` is *acting* as staff right now — the exemptions half.
    ///
    /// Sphere's `PRIV_GM`, which its `.GM` toggles and every in-game rule reads
    /// (`IsPriv(PRIV_GM)`), never the level. Here it is the [`Staff`] marker,
    /// given at login to an account with [`staff_authority`](Self::staff_authority)
    /// and taken off by `.gm`. Staff see the dead and do not tire; a game master
    /// with the mode off walks the world under exactly the rules a player does,
    /// which is the only way to test them from a staff account.
    #[must_use]
    pub fn is_staff(&self, watcher: EntityId) -> bool {
        self.registry.has::<Staff>(watcher)
    }

    /// Whether `who` may open `container` — the house-secure gate.
    ///
    /// Here rather than in `openshard-housing` for the reason
    /// [`Standing`](crate::Standing) is: the double-click that opens a container
    /// is `openshard-items`', which has no business depending on the housing
    /// crate. It is `Guild::at_war_with`'s split — the rules stay in the system
    /// crate, and the question a wire path asks lives beside the data.
    ///
    /// A container with no [`LockedDown`](crate::LockedDown) is not a secure and
    /// opens for anybody, which is every chest in Britannia.
    #[must_use]
    pub fn may_open_secure(&self, who: EntityId, container: EntityId) -> bool {
        let Some(&crate::LockedDown {
            house,
            secure: Some(access),
        }) = self.registry.get::<crate::LockedDown>(container)
        else {
            return true;
        };
        let (Some(entry), Some(serial)) = (
            self.registry
                .entity_of(house)
                .and_then(|entity| self.registry.get::<crate::House>(entity)),
            self.registry.serial_of(who),
        ) else {
            // A secure whose house is gone. Shut rather than open: what happens
            // to the contents when a house comes down is the moving crate's
            // question, and until there is one the safe reading of "no house" is
            // "not yours".
            return false;
        };
        entry.standing_of(serial, self.is_staff(who)) >= access
    }

    /// Whether one more item will fit inside `container`, if it is a house's
    /// secure.
    ///
    /// `true` for every container that is not one, which is every container in
    /// Britannia. Asked by the drop path, and here rather than in
    /// `openshard-housing` for [`may_open_secure`](Self::may_open_secure)'s
    /// reason: the drop is `openshard-items`', and the ceiling it needs is a
    /// number on the [`House`](crate::House) component rather than a footprint
    /// it would have to recompute.
    ///
    /// The count is one level deep, which is the rule the ceiling means: a bag
    /// inside a secure chest is one item against the house's allowance, and what
    /// is inside the *bag* is `capacity`'s ceiling rather than this one.
    #[must_use]
    pub fn secure_has_room(&self, container: EntityId, more: usize) -> bool {
        let Some(&crate::LockedDown {
            house,
            secure: Some(_),
        }) = self.registry.get::<crate::LockedDown>(container)
        else {
            return true;
        };
        let Some(entry) = self
            .registry
            .entity_of(house)
            .and_then(|entity| self.registry.get::<crate::House>(entity))
        else {
            return false;
        };
        let secures: Vec<Serial> = self
            .registry
            .query::<crate::LockedDown>()
            .filter(|(item, pinned)| {
                pinned.house == house && pinned.secure.is_some() && self.registry.serial_of(*item).is_some()
            })
            .filter_map(|(item, _)| self.registry.serial_of(item))
            .collect();
        let stored = self
            .registry
            .query::<Contained>()
            .filter(|(_, held)| secures.contains(&held.container))
            .count();
        stored + more <= entry.storage() as usize
    }

    /// Whether a mobile may teleport to a point — the `no_teleport` region flag.
    ///
    /// Both ends are checked, not just the destination: a region that bars
    /// teleporting bars it *out* as well as *in*, or a jail is a jail only until
    /// someone inside casts. ServUO's `SpellHelper.CheckTravel` makes the same
    /// two checks for the same reason.
    ///
    /// Staff pass. Every in-game exemption goes through [`is_staff`](Self::is_staff),
    /// so `.gm off` puts a game master under this rule with everyone else.
    #[must_use]
    pub fn may_teleport(&self, mobile: EntityId, to: Point) -> bool {
        if self.is_staff(mobile) {
            return true;
        }
        let facet = self.facet_of(mobile);
        let barred = |point: Option<Point>| {
            point.is_some_and(|point| {
                self.region_at(facet, point)
                    .is_some_and(|region| region.flags.no_teleport)
            })
        };
        let from = self.registry.get::<Position>(mobile).map(|p| p.0);
        !barred(from) && !barred(Some(to))
    }

    /// Whether `watcher` may see mobile `other`. The living cannot see the dead: a
    /// ghost is drawn only to itself, to another ghost, or to staff. Every other
    /// mobile in range is visible to everyone; an item is never a ghost, so this
    /// bites only mobiles.
    ///
    /// **Two of ServUO's clauses are missing, and they are the same clause twice:
    /// a ghost the living are *supposed* to see.** `CanSee(Mobile m)`
    /// (`Server/Mobile.cs:9229`) ends
    /// `((m.Alive || (Core.SE && Skills.SpiritSpeak.Value >= 100.0)) || !Alive ||
    /// IsStaff() || m.Warmode)`. The last term is the manifest — **a ghost that
    /// draws its stance is visible to the living**, which is how a player who
    /// died in the woods is found and resurrected, and it is a gameplay rule
    /// rather than a detail. The Spirit Speak term is the other way in. Neither
    /// is implemented here; both are filed in `docs/roadmap.md` under the ghost
    /// entry, together with what they cost — a war toggle becomes a `reveal`/
    /// `hide` for every living watcher in range, which is a draw-path change and
    /// not a predicate change.
    ///
    /// Until then the client's own body-blocking rule is written not to depend on
    /// their absence: `clutter::crowd` reads a stranger's death off the body id
    /// *and* `IGNORE_MOBILES` together, so a manifested ghost is already answered
    /// for at that end.
    ///
    /// It gates *hearing* as well as drawing (`chat::speak`), because ServUO's
    /// speech runs through the same `CanSee`: a ghost nobody can see should not be
    /// a disembodied voice either.
    #[must_use]
    pub fn can_see_mobile(&self, watcher: EntityId, other: EntityId) -> bool {
        if watcher == other {
            return true; // you always see yourself, hidden or dead
        }
        // Hidden is the stricter of the two: nobody sees you but staff.
        if self.registry.has::<Hidden>(other) && !self.is_staff(watcher) {
            return false;
        }
        if !self.registry.has::<Ghost>(other) {
            return true;
        }
        self.registry.has::<Ghost>(watcher) || self.is_staff(watcher)
    }

    /// A mobile did something that gives away where it is — ServUO's
    /// `Mobile.RevealingAction`.
    ///
    /// Attacking, speaking, casting, lifting, dying: the list is ServUO's, and it
    /// also disrupts (`DisruptiveAction` is the last line of `RevealingAction`,
    /// with the comment "anything that unhides you will also disrupt meditation"),
    /// so the two are one call here as they are there.
    ///
    /// Substrate, not a rule, for the same reason [`disrupt`](Self::disrupt) is:
    /// every crate that does something revealing has to be able to say so, and none
    /// of them can depend on the crate that owns Hiding.
    pub fn break_cover(&mut self, mobile: EntityId) {
        self.registry.remove::<Stealthing>(mobile);
        if self.registry.remove::<Hidden>(mobile).is_some() {
            // Back onto every screen in range. `reveal` is the one draw path, so
            // this is the only line that has to know a mobile just became visible.
            self.refresh_around(mobile);
        }
        self.disrupt(mobile);
    }

    /// Take a mobile off every screen but its own — the mirror of
    /// [`break_cover`](Self::break_cover), and the only place a mobile becomes
    /// hidden.
    ///
    /// The marker alone would be enough for anything drawn *after* it, since
    /// `can_see_mobile` gates every draw; what this adds is telling the clients that
    /// already have it on screen to forget it, which is the same `0x1D` a mobile
    /// walking out of range gets.
    pub fn conceal(&mut self, mobile: EntityId) {
        self.registry.insert(mobile, Hidden);
        let Some(serial) = self.registry.serial_of(mobile) else {
            return;
        };
        for watcher in self.watchers_of(mobile) {
            if watcher != mobile && !self.is_staff(watcher) {
                self.forget(watcher, mobile, serial);
            }
        }
    }

    /// A hidden mobile took a step. Spends a stealth step, or gives it away.
    ///
    /// ServUO's `Mobile.OnMove`: running or riding breaks cover outright, and so
    /// does a step past the budget Stealth bought. Called from both movement paths —
    /// there is no shared step, which is why it is called twice and lives here once.
    pub fn step_while_hidden(&mut self, mobile: EntityId, running: bool, mounted: bool) {
        if !self.registry.has::<Hidden>(mobile) || self.is_staff(mobile) {
            return;
        }
        let budget = self
            .registry
            .get::<Stealthing>(mobile)
            .map_or(0, |s| s.steps_left);
        if running || mounted || budget == 0 {
            self.break_cover(mobile);
            return;
        }
        self.registry.insert(
            mobile,
            Stealthing {
                steps_left: budget - 1,
            },
        );
    }

    /// A mobile did something that breaks concentration — ServUO's
    /// `Mobile.DisruptiveAction`.
    ///
    /// Today that means one thing: a meditative trance ends and the mobile is told
    /// so. It is substrate rather than a rule for the same reason `can_see_mobile`
    /// is — every crate that *does* something disruptive has to be able to say so
    /// (a step, a blow taken, a word spoken, an item lifted), and none of them can
    /// depend on the crate that owns Meditation. ServUO calls it from exactly those
    /// places, and this is called from their counterparts here.
    pub fn disrupt(&mut self, mobile: EntityId) {
        if self.registry.remove::<Meditating>(mobile).is_some() {
            self.localized_message(mobile, STOP_MEDITATING, "");
        }
    }

    /// Whether `listener` may *hear* mobile `other` speak.
    ///
    /// Everything anyone can see, they can hear — and one thing more: a living
    /// mobile under Spirit Speak catches what the dead are saying, which is the
    /// whole point of the classic skill. The two questions are deliberately two
    /// predicates: a ghost stays *invisible* to that listener, so `can_see_mobile`
    /// must not be relaxed to cover it, or contacting the netherworld would make
    /// the dead walk visibly among the living.
    #[must_use]
    pub fn can_hear_mobile(&self, listener: EntityId, other: EntityId) -> bool {
        if self.can_see_mobile(listener, other) {
            return true;
        }
        self.registry.has::<Ghost>(other) && self.registry.has::<HearsGhosts>(listener)
    }

    /// A mobile's standing — the colour of its health bar. Absent reads as
    /// [`Notoriety::Innocent`], a blue bar, the safe default.
    #[must_use]
    pub fn notoriety_of(&self, entity: EntityId) -> Notoriety {
        self.registry
            .get::<Notoriety>(entity)
            .copied()
            .unwrap_or(Notoriety::Innocent)
    }

    /// The guild a mobile belongs to, if it belongs to one that still exists.
    ///
    /// The two halves are separate on purpose — the [`GuildMember`] component
    /// names an id, and the id is looked up here — so this is also where a
    /// membership naming a disbanded guild reads as no membership rather than as
    /// a panic. `disband` does not walk the roster stripping components, and a
    /// player logged out at the time could not be reached if it did.
    #[must_use]
    pub fn guild_of(&self, entity: EntityId) -> Option<&crate::guild::Guild> {
        let member = self.registry.get::<crate::components::GuildMember>(entity)?;
        self.guilds.get(member.guild)
    }

    /// The party a mobile is in, if it is in one that still exists.
    ///
    /// [`guild_of`](Self::guild_of)'s twin, and the same two halves for a
    /// weaker reason: a disbanded party *does* strip its components, because
    /// every member of one is online by construction. The lookup is written this
    /// way anyway so that a component surviving its party — a disconnect racing
    /// a disband — reads as no party rather than as a panic.
    #[must_use]
    pub fn party_of(&self, entity: EntityId) -> Option<&crate::party::Party> {
        let member = self.registry.get::<crate::components::PartyMember>(entity)?;
        self.parties.get(member.party)
    }

    /// Whether the party may take from this mobile's corpse.
    ///
    /// `false` for anybody in no party, which is also the default inside one:
    /// a player has to say so. Read by the corpse rather than by the party, so
    /// it answers for a mobile rather than needing one looked up first.
    #[must_use]
    pub fn party_may_loot(&self, entity: EntityId) -> bool {
        self.registry
            .get::<crate::components::PartyMember>(entity)
            .is_some_and(|member| member.can_loot && self.parties.get(member.party).is_some())
    }

    /// A member's guild title and their guild's full name, for the line the
    /// tooltip draws under the name. `None` for a mobile in no guild, and for one
    /// the guild has given no title — ServUO draws the line only when there is a
    /// title to put in it.
    #[must_use]
    pub fn guild_title_of(&self, entity: EntityId) -> Option<(String, String)> {
        let title = self
            .registry
            .get::<crate::components::GuildMember>(entity)?
            .title
            .clone();
        if title.is_empty() {
            return None;
        }
        Some((title, self.guild_of(entity)?.name.clone()))
    }

    /// The bracketed line drawn *above* a mobile's name on a single click:
    /// `[Warlord, OSS]`, or `[OSS]` for a member with no title.
    ///
    /// A separate overhead label rather than part of the name, which is what
    /// ServUO's `Mobile.OnSingleClick` sends — the name below it keeps its
    /// notoriety hue, and the guild line is the speech hue, so the two read as
    /// two different things.
    #[must_use]
    pub fn guild_label(&self, entity: EntityId) -> Option<String> {
        let guild = self.guild_of(entity)?;
        let title = self
            .registry
            .get::<crate::components::GuildMember>(entity)
            .map_or("", |member| member.title.trim());
        if title.is_empty() {
            Some(format!("[{}]", guild.abbreviation))
        } else {
            Some(format!("[{title}, {}]", guild.abbreviation))
        }
    }

    /// What colour `target` draws in on `viewer`'s screen.
    ///
    /// The wire answer, and the only one that is *relative*.
    /// [`notoriety_of`](Self::notoriety_of) is the mobile's own standing — what
    /// combat, the guards and a shopkeeper ask about, and the same for everyone.
    /// This is what a particular client is told, which is not the same question:
    /// a guildmate is green to you and blue to a stranger.
    ///
    /// # ServUO's order, and why guild loses
    ///
    /// `Scripts/Misc/Notoriety.cs` asks about standing first: a murderer is red
    /// and a criminal is grey **before** any guild question. Only then does the
    /// same guild or an ally read green, and a guild at war read orange. So a red
    /// cannot hide inside a guild tabard, which is the whole reason for the
    /// order.
    ///
    /// # The cost, and where it is not paid
    ///
    /// This runs once per watcher per drawn mobile, on the movement path. A
    /// mobile with no [`GuildMember`] — every creature, every townsperson, every
    /// unguilded player — costs one failed component lookup and returns the
    /// absolute answer, so the common case does not touch the guild table at all.
    #[must_use]
    pub fn notoriety_toward(&self, viewer: EntityId, target: EntityId) -> Notoriety {
        let standing = self.notoriety_of(target);
        // Standing wins. Anything but a plain blue is already the answer, and
        // asking about guilds would be asking a question that cannot change it.
        if standing != Notoriety::Innocent {
            return standing;
        }
        // Through `guild_of` rather than the component, so a membership naming a
        // disbanded guild is no membership: two players left holding the same
        // dead id would otherwise read green to each other forever.
        let Some(theirs) = self.guild_of(target).map(|guild| guild.id) else {
            return standing;
        };
        let Some(mine) = self.guild_of(viewer) else {
            return standing;
        };
        if mine.id == theirs {
            return Notoriety::Friend;
        }
        // War before alliance, and the order costs nothing to state: two guilds
        // cannot be both, because joining an alliance is refused while a war
        // with one of its members stands (`openshard_guilds::join_alliance`).
        // Asked in this order anyway, so that a pair which somehow became both
        // reads as *enemies* — the safe direction, since drawing an enemy green
        // is the mistake a player cannot recover from.
        if mine.at_war_with(theirs) {
            return Notoriety::Enemy;
        }
        match self.allied(mine.id, theirs) {
            true => Notoriety::Friend,
            false => standing,
        }
    }

    /// Whether two guilds are in the same alliance.
    ///
    /// `false` for a guild with itself, which never reaches here — the caller
    /// answers that as the same guild — and for a membership naming an alliance
    /// that is gone, which is the [`guild_of`](Self::guild_of) rule one level up.
    #[must_use]
    pub fn allied(&self, one: crate::guild::GuildId, other: crate::guild::GuildId) -> bool {
        let Some(alliance) = self.guilds.get(one).and_then(|guild| guild.alliance) else {
            return false;
        };
        self.alliances
            .get(alliance)
            .is_some_and(|alliance| alliance.contains(one) && alliance.contains(other))
    }

    /// The flag byte a `0x77`/`0x78` carries about a mobile.
    ///
    /// Two bits of the eight are set by this engine. The stance, because a
    /// client draws a body at war standing in a different animation group — the
    /// difference between a shopkeeper and a shopkeeper with a sword out. And
    /// [`IGNORE_MOBILES`](StatusFlags::IGNORE_MOBILES), because the client keeps
    /// its own copy of "a body is in the way" and would otherwise refuse to
    /// predict a step this shard allows: [`walks_through_bodies`] is the rule at
    /// this end, and this bit is the same rule sent to the other. The remaining
    /// six are named in [`StatusFlags`](openshard_protocol::mobile::StatusFlags)'
    /// own table and nothing here sets them.
    ///
    /// **The bit is `walks_through_bodies` and not a second reading of it** —
    /// staff *and* the dead, so a ghost gets it too. A ghost is drawn only to
    /// other ghosts and to staff, so the case is narrow, but it is real: two
    /// ghosts can see each other and a client that has not been told would
    /// refuse to walk one through the other. A separate condition here would be
    /// the same rule written twice, and the second copy is the one that ends up
    /// disagreeing.
    ///
    /// Read off [`Combat`] rather than remembered beside it, and asked at the
    /// moment the packet is built: `war_mode` writes `Combat::warmode` and
    /// every screen is rebuilt from these two functions, so there is one
    /// answer to "is this body at war" and no copy of it to fall behind.
    ///
    /// **Every packet that carries the byte reads it here**, the `0x20`
    /// included. That one is the player's own body, and it is the one the
    /// exemption is actually *for*: a `0x77`/`0x78` tells a client about
    /// somebody else, and a client only ever predicts its own step. It used to
    /// send [`StatusFlags::NONE`] in all three of its call sites, so a game
    /// master learned that every other staff member walks through bodies and
    /// never that they do — and the `0x78` about their own body, which does say
    /// so, is overwritten by the next `0x20` a step or a relocation sends.
    ///
    /// [`walks_through_bodies`]: Self::walks_through_bodies
    #[must_use]
    pub fn stance_of(&self, entity: EntityId) -> StatusFlags {
        let war = StatusFlags::of_stance(self.registry.get::<Combat>(entity).is_some_and(|c| c.warmode));
        if self.walks_through_bodies(entity) {
            war.with(StatusFlags::IGNORE_MOBILES)
        } else {
            war
        }
    }

    /// Build a `0x78` for an entity, if it is a drawable mobile.
    #[must_use]
    pub fn mobile_incoming(&mut self, viewer: EntityId, entity: EntityId) -> Option<MobileIncoming> {
        let serial = self.registry.serial_of(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        let Heading(facing) = *self.registry.get::<Heading>(entity)?;
        let body = *self.registry.get::<Body>(entity)?;
        let flags = self.stance_of(entity);
        Some(MobileIncoming {
            serial,
            body: body.id,
            position,
            facing,
            hue: body.hue,
            flags,
            notoriety: self.notoriety_toward(viewer, entity),
            equipment: self.equipment_of(serial),
        })
    }

    /// What a mobile is wearing, as the `0x78` equipment list.
    ///
    /// # Why this keeps an index
    ///
    /// This is called on every *first sight* of a mobile — each `0x78` carries
    /// what its subject is wearing — and the honest version of it filters the
    /// whole `Equipped` column by owner. That is fine until a shard is populated:
    /// 726 dressed townsfolk is a column of ~3,800 rows, scanned in full to find
    /// the five a single NPC has on, once per NPC as a player walks past. One
    /// walk across a market square is millions of comparisons.
    ///
    /// The index is a *cache*, not a mirror. It is keyed on
    /// [`Registry::column_version`], which the column bumps for itself whenever
    /// an entity gains or loses the component, so it rebuilds when it is stale
    /// and nothing anywhere has to remember to invalidate it. That distinction is
    /// the whole design: a hand-maintained "what is worn by whom" map is a
    /// `touch` beside every equip, and the first system that equips something
    /// without knowing the map exists breaks it silently.
    ///
    /// It holds *entities*, not the finished list, so a re-dyed or re-graphicked
    /// item still reads its current `Drawn` here — only membership is cached,
    /// and only membership is what the version tracks.
    #[must_use]
    pub fn equipment_of(&mut self, mobile: Serial) -> Vec<Equipment> {
        let version = self.registry.column_version::<Equipped>();
        if self.worn.version != version {
            self.worn.by_mobile.clear();
            for (item, worn) in self.registry.query::<Equipped>() {
                self.worn.by_mobile.entry(worn.mobile).or_default().push(item);
            }
            self.worn.version = version;
        }
        let Some(items) = self.worn.by_mobile.get(&mobile) else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|&item| {
                // A trade escrow is worn so that reach and dropping-in work with
                // no new machinery, but it is not clothing: drawing it hangs a
                // mystery box off both traders on every onlooker's screen.
                if self.registry.has::<TradeWindow>(item) {
                    return None;
                }
                let serial = self.registry.serial_of(item)?;
                let worn = self.registry.get::<Equipped>(item)?;
                let Drawn { id, hue } = *self.registry.get::<Drawn>(item)?;
                Some(Equipment {
                    serial,
                    graphic: id,
                    layer: worn.layer,
                    hue,
                })
            })
            .collect()
    }

    /// Build a `0x77` for an entity.
    #[must_use]
    pub fn mobile_move(&self, viewer: EntityId, entity: EntityId) -> Option<MobileMove> {
        let serial = self.registry.serial_of(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        let Heading(facing) = *self.registry.get::<Heading>(entity)?;
        let body = *self.registry.get::<Body>(entity)?;
        Some(MobileMove {
            serial,
            body: body.id,
            position,
            facing,
            hue: body.hue,
            // The same question the `0x78` asks — a body that changed stance
            // between two steps says so on the next one, which is how a
            // watcher learns about a stance nobody was there to see start.
            flags: self.stance_of(entity),
            notoriety: self.notoriety_toward(viewer, entity),
        })
    }

    /// Queue a raw packet for a connection.
    pub fn send(&mut self, connection: ConnectionId, packet: Vec<u8>) {
        self.outbox.push(Outbound { connection, packet });
    }

    /// The client version negotiated on `connection`.
    ///
    /// `None` for a connection the world has never been handed — one still in the
    /// login conversation, or one already gone. That is absence and not ignorance:
    /// a connection the world holds always knows what its client is, because the
    /// version arrives with the hand-off (`Command::Authenticated`) and never
    /// changes afterwards.
    ///
    /// Read off the session row rather than off the player's
    /// [`Client`](crate::components::Client) component, which is what made a
    /// connection with no character unaddressable — see
    /// [`session`](crate::session).
    #[must_use]
    pub fn version_of(&self, connection: ConnectionId) -> Option<ClientVersion> {
        self.connections.get(&connection).map(|client| client.version)
    }

    /// What the world remembers about `connection`, or `None` for one it is not
    /// holding — still in the login conversation, or already gone.
    #[must_use]
    pub fn connection(&self, connection: ConnectionId) -> Option<&Connection> {
        self.connections.get(&connection)
    }

    /// The same row, to write what this client is in the middle of.
    ///
    /// A write to a connection the world does not hold is a no-op rather than a
    /// panic: a tick can be applying work queued for a socket that has since
    /// closed, which is ordinary rather than exceptional.
    pub fn connection_mut(&mut self, connection: ConnectionId) -> Option<&mut Connection> {
        self.connections.get_mut(&connection)
    }

    /// Let go of a connection, and of everything the world was holding for it.
    ///
    /// The one exit. Returns the row so the caller can deal with what was still in
    /// flight — an item on the cursor has to be put back somewhere, and only the
    /// item code knows where. Everything else on the row simply ceases to exist,
    /// which is the point: teardown is a `remove`, not a list of maps to clear
    /// that a new field can be left off.
    ///
    /// It does not touch the *character*: letting go of the entity, its serial and
    /// its saved record is `World::disconnect`'s, because it involves the journal
    /// and the roster and this crate has neither.
    pub fn forget_connection(&mut self, connection: ConnectionId) -> Option<Connection> {
        // The one per-connection fact that is not on the row: which containers this
        // client has open is indexed by *container*, because every read of it asks
        // "who is watching this one" as an item inside changes. Inverting it onto
        // the row would turn each of those into a scan of every connection, so it
        // stays an index — and this is the sweep the row cannot do for it.
        self.open_containers.retain(|_, watchers| {
            watchers.remove(&connection);
            !watchers.is_empty()
        });
        self.connections.remove(&connection)
    }

    /// The row of the client playing `entity`, if it is a connected player.
    ///
    /// The seam for the state that is *about a screen* but is reached holding a
    /// mobile — a targeting cursor, an open gump. `None` for a creature, for a
    /// character between sessions, and for an entity that is not a mobile at all,
    /// which is one absence and not three: none of them has a screen.
    ///
    /// One lookup and not a walk of [`players`](Self::players): the entity says
    /// which connection through its [`Client`] component.
    #[must_use]
    pub fn row_of(&self, entity: EntityId) -> Option<&Connection> {
        let &Client { connection } = self.registry.get::<Client>(entity)?;
        self.connections.get(&connection)
    }

    /// The same row, to write to. See [`row_of`](Self::row_of).
    pub fn row_of_mut(&mut self, entity: EntityId) -> Option<&mut Connection> {
        let &Client { connection } = self.registry.get::<Client>(entity)?;
        self.connections.get_mut(&connection)
    }

    /// The connection `entity` is played over, if it is a connected player.
    ///
    /// For the caller that needs the *name* of the connection rather than what is
    /// on its row — one that sends a packet, or hands the id to something that
    /// keys by it. A caller that goes on to read the row wants
    /// [`row_of`](Self::row_of) instead, and one that also needs the client
    /// version wants [`client_of`](Self::client_of): both are this lookup with the
    /// second half already done.
    ///
    /// One lookup, and the reason to reach for it is that the obvious alternative
    /// is not: [`players`](Self::players) is keyed by connection, so answering
    /// this from it means walking every player on the shard to find the one entry
    /// that matches.
    #[must_use]
    pub fn connection_of(&self, entity: EntityId) -> Option<ConnectionId> {
        let &Client { connection } = self.registry.get::<Client>(entity)?;
        Some(connection)
    }

    /// Put a targeting cursor up for `entity`, remembering what the click is for.
    ///
    /// Nothing happens for a mobile with no client, which is the invariant every
    /// caller used to spell out for itself: a creature has no cursor to raise.
    pub fn raise_target(&mut self, entity: EntityId, purpose: TargetPurpose) {
        if let Some(row) = self.row_of_mut(entity) {
            row.pending_target = Some(purpose);
        }
    }

    /// Take down `entity`'s targeting cursor and say what it was for. `None` when
    /// none was up — a `0x6C` for a cursor this side never raised.
    pub fn take_target(&mut self, entity: EntityId) -> Option<TargetPurpose> {
        self.row_of_mut(entity).and_then(|row| row.pending_target.take())
    }

    /// Whether `entity` already has a cursor up and is therefore busy.
    #[must_use]
    pub fn has_target(&self, entity: EntityId) -> bool {
        self.row_of(entity)
            .is_some_and(|row| row.pending_target.is_some())
    }

    /// What `connection` is dragging on its cursor, if anything.
    #[must_use]
    pub fn held_of(&self, connection: ConnectionId) -> Option<HeldItem> {
        self.connections.get(&connection).and_then(|row| row.held)
    }

    /// Put an item on `connection`'s empty cursor.
    ///
    /// Refuses both a missing connection and an occupied cursor. Replacing the
    /// old value would orphan the first item in limbo, so callers must reserve
    /// the cursor successfully before detaching a new item from its origin.
    pub fn hold(&mut self, connection: ConnectionId, held: HeldItem) -> Result<(), HeldItem> {
        let Some(row) = self.connections.get_mut(&connection) else {
            return Err(held);
        };
        if row.held.is_some() {
            return Err(held);
        }
        row.held = Some(held);
        Ok(())
    }

    /// Take whatever is on `connection`'s cursor off it.
    pub fn take_held(&mut self, connection: ConnectionId) -> Option<HeldItem> {
        self.connections
            .get_mut(&connection)
            .and_then(|row| row.held.take())
    }

    /// Who to answer for `entity`, and in which dialect: its connection and that
    /// connection's client version, or `None` if it has no client at all.
    ///
    /// The two halves used to sit together on the [`Client`] component and are
    /// now one lookup apiece — the entity says which connection, the connection
    /// says which client. This is the pair every packet path needs, so it is one
    /// call and not two: a caller that reached for `Client { connection }` and
    /// then guessed a version would be back to the bug that split them.
    #[must_use]
    pub fn client_of(&self, entity: EntityId) -> Option<(ConnectionId, ClientVersion)> {
        let &Client { connection } = self.registry.get::<Client>(entity)?;
        Some((connection, self.version_of(connection)?))
    }

    /// Queue `packet` for a connection, framed for the version that connection
    /// negotiated.
    ///
    /// The seam every server-to-client packet should go through: the caller names
    /// *what* to say and this decides *how* to say it to this particular client.
    /// A connection the world does not hold is skipped — see
    /// [`version_of`](Self::version_of).
    /// Encoding for a guessed version instead is how a client silently drops a
    /// packet it cannot parse, which is the failure mode that is hardest to see.
    /// Send one packet to whoever is playing `entity`, if anybody is.
    ///
    /// [`send_packet`](Self::send_packet) addressed by *mobile* rather than by
    /// connection, which is what almost every caller actually has: an NPC, or a
    /// player who logged out mid-tick, simply gets nothing. The lookup it wraps
    /// is written out by hand in a dozen places in this file; new callers should
    /// reach for this one.
    pub fn send_to(&mut self, entity: EntityId, packet: &ServerPacket) {
        if let Some(&Client { connection, .. }) = self.registry.get::<Client>(entity) {
            self.send_packet(connection, packet);
        }
    }

    pub fn send_packet(&mut self, connection: ConnectionId, packet: &ServerPacket) {
        let Some(version) = self.version_of(connection) else {
            return;
        };
        let bytes = packet.encode(version);
        self.outbox.push(Outbound {
            connection,
            packet: bytes,
        });
    }

    /// Send `packet` to every player within view range of `source` — its own
    /// client included — each encoded for their own client version.
    ///
    /// The audience for a sound or an effect is who is *near*, not the `seen` set
    /// a health redraw uses: a door never enters anyone's `seen` (it is decoration,
    /// redrawn by `reveal`, not tracked as an interest), yet its creak must still
    /// be heard — so this asks the spatial index for neighbours the way `reveal`
    /// does, and keeps the ones with a client. The feedback seam every gameplay
    /// system reaches for — a swing, a spell, a door — so the world is *felt*, not
    /// merely correct.
    ///
    /// Unlike [`broadcast_from`](Self::broadcast_from) this encodes per recipient
    /// rather than fanning out one buffer, so a packet that grows a
    /// version-conditional tail needs no new call shape: the caller never learns
    /// that the bytes differ.
    pub fn broadcast_packet(&mut self, source: EntityId, packet: &ServerPacket) {
        for (connection, version) in self.audience_of(source) {
            let bytes = packet.encode(version);
            self.outbox.push(Outbound {
                connection,
                packet: bytes,
            });
        }
    }

    /// The clients within view range of `source`, with the version each speaks.
    ///
    /// Collected up front so the sectors borrow is dropped before anything is
    /// queued.
    fn audience_of(&self, source: EntityId) -> Vec<(ConnectionId, ClientVersion)> {
        let facet = self.facet_of(source);
        let sectors = self.facet_state(facet).sectors();
        let Some(centre) = sectors.position_of(source) else {
            return Vec::new();
        };
        sectors
            .mobiles_near(centre, VIEW_RANGE)
            .filter_map(|(entity, _)| self.client_of(entity))
            .collect()
    }

    /// Draw a newly placed or changed `entity` to everyone in range who does not
    /// already have it — a fresh item, a spawned creature, an equipped mobile.
    pub fn reveal(&mut self, entity: EntityId) {
        let facet = self.facet_of(entity);
        let sectors = self.facet_state(facet).sectors();
        let Some(centre) = sectors.position_of(entity) else {
            return;
        };
        // The watchers, so the mobile list: `show` wants a screen to draw on and
        // an item has none.
        let watchers: Vec<EntityId> = sectors
            .mobiles_near(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .filter(|id| *id != entity)
            .collect();
        for watcher in watchers {
            self.show(watcher, entity);
        }
    }
}

impl std::fmt::Debug for WorldState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorldState")
            .field("ticks", &self.ticks)
            .field("entities", &self.registry.len())
            .field("players", &self.players.len())
            .field("facets", &self.facets.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use openshard_movement::scene::Scene;
    use openshard_protocol::direction::Direction;
    use openshard_tiles::TileData;

    /// One block of flat ground with a wall on (4, 4), and the table that says
    /// the wall is twenty tall and solid.
    ///
    /// Built per facet rather than cloned, because a facet holds its own map and
    /// a snapshot names the facet it is for.
    fn walled(facet: Facet) -> (MapSnapshot, TileData) {
        let mut scene = Scene::flat_holding(7, 7, 0);
        scene.wall(4, 4, 0, 20);
        scene.into_shard(facet)
    }

    /// A shard over one flat block of ground with nobody on it.
    fn a_shard() -> WorldState {
        let (map, tiles) = Scene::flat_holding(16, 16, 0).into_shard(Facet(0));
        let mut facets = BTreeMap::new();
        facets.insert(Facet(0), FacetState::new(Some(map), None, 16, 16, &tiles));
        WorldState::new(
            facets,
            Facet(0),
            tiles,
            openshard_uofiles::multi::Multis::default(),
            (0, 0),
            1,
        )
    }

    /// Put a mobile at `at`: the [`Body`] that makes it one, the [`Position`]
    /// the world holds, and the sector row the crowd is read out of.
    ///
    /// The sector write is the part that matters here — it is what `crowd_near`
    /// actually asks, and writing it beside `Position` is the bargain the tick
    /// makes on every step.
    fn a_body_at(state: &mut WorldState, at: Point) -> EntityId {
        let entity = state.registry.spawn();
        state.registry.insert(
            entity,
            Body {
                id: openshard_protocol::wire::Graphic(0x0190),
                hue: Hue(0),
            },
        );
        state.registry.insert(entity, Position(at));
        state.place_mobile(Facet(0), entity, at);
        entity
    }

    /// **Who is in the way, and who only looks it.**
    ///
    /// The three rules that come with a mobile obstacle live in `body_blocks`
    /// and `walks_through_bodies`, and every one of them is a bug a player
    /// reports when it is missing. The ghost is the one that was: a dead player
    /// keeps its [`Body`] — a shroud is still a body graphic — so before this it
    /// stood in a doorway the living could neither see nor pass.
    #[test]
    fn a_crowd_holds_the_living_and_nobody_else() {
        let mut state = a_shard();
        let walker = a_body_at(&mut state, Point::new(8, 8, 0));
        let bystander = a_body_at(&mut state, Point::new(9, 8, 0));

        assert_eq!(
            state.crowd_near(Facet(0), walker, Point::new(8, 8, 0), 1),
            vec![Point::new(9, 8, 0)],
            "a living body beside the walker is in its way, and the walker is not in its own"
        );

        // A chest is in the same sector grid and is not a body. It is what the
        // obstruction index is for, and asking it here would be the second
        // reading of one fact that this whole seam exists to avoid. Two things
        // keep it out of the crowd now and this asserts the pair: it is filed as
        // an [`Occupant::Item`], so the mobile list never offers it, and
        // `body_blocks` would refuse it if it were.
        let chest = state.registry.spawn();
        state.registry.insert(chest, Position(Point::new(7, 8, 0)));
        state.place_item(Facet(0), chest, Point::new(7, 8, 0));
        assert!(
            !state.body_blocks(chest),
            "an item is not a body wherever it is filed"
        );
        assert_eq!(
            state.crowd_near(Facet(0), walker, Point::new(8, 8, 0), 1),
            vec![Point::new(9, 8, 0)],
            "an item sharing the sector grid is not a body"
        );

        // The dead do not block. Both halves: the ghost marker a dead player
        // carries, and the zero hit points a creature has for the tick before it
        // is reaped.
        state.registry.insert(
            bystander,
            Ghost {
                body: Body {
                    id: openshard_protocol::wire::Graphic(0x0190),
                    hue: Hue(0),
                },
            },
        );
        assert!(
            state
                .crowd_near(Facet(0), walker, Point::new(8, 8, 0), 1)
                .is_empty(),
            "a ghost stands in nobody's way"
        );
        state.registry.remove::<Ghost>(bystander);
        state
            .registry
            .insert(bystander, Hitpoints { current: 0, max: 50 });
        assert!(
            state
                .crowd_near(Facet(0), walker, Point::new(8, 8, 0), 1)
                .is_empty(),
            "and neither does a body worn down to nothing, before the reap takes it"
        );
        state
            .registry
            .insert(bystander, Hitpoints { current: 1, max: 50 });
        assert_eq!(
            state.crowd_near(Facet(0), walker, Point::new(8, 8, 0), 1).len(),
            1,
            "one hit point is alive"
        );

        // Staff walk through bodies as they walk through walls — and it is
        // `Staff`, the flag a `.gm` puts down, so a game master playing by the
        // rules is held to them.
        state.registry.insert(walker, Staff);
        assert!(
            state
                .crowd_near(Facet(0), walker, Point::new(8, 8, 0), 1)
                .is_empty(),
            "a game master is stopped by nobody"
        );
        state.registry.remove::<Staff>(walker);

        // A hidden game master is in nobody's way either — ServUO's
        // `t.Hidden && t.IsStaff()`. A hidden *player* still blocks: being
        // walked into is how you find one.
        state.registry.insert(bystander, Hidden);
        assert_eq!(
            state.crowd_near(Facet(0), walker, Point::new(8, 8, 0), 1).len(),
            1,
            "a hidden player is still standing there"
        );
        state.registry.insert(bystander, Staff);
        assert!(
            state
                .crowd_near(Facet(0), walker, Point::new(8, 8, 0), 1)
                .is_empty(),
            "a hidden game master is not"
        );
    }

    /// The crowd comes back sorted by tile, which is
    /// [`Bodies::standing`](openshard_movement::Bodies::standing)'s whole
    /// contract — and its lookup silently misses occupants if it is broken,
    /// which a debug assertion catches only in a debug build.
    #[test]
    fn a_crowd_comes_back_sorted_by_tile() {
        let mut state = a_shard();
        let walker = a_body_at(&mut state, Point::new(8, 8, 0));
        for at in [
            Point::new(9, 9, 0),
            Point::new(7, 7, 0),
            Point::new(9, 7, 0),
            Point::new(7, 9, 0),
        ] {
            a_body_at(&mut state, at);
        }
        let crowd = state.crowd_near(Facet(0), walker, Point::new(8, 8, 0), 1);
        assert_eq!(
            crowd,
            vec![
                Point::new(7, 7, 0),
                Point::new(7, 9, 0),
                Point::new(9, 7, 0),
                Point::new(9, 9, 0),
            ],
        );
        assert!(
            openshard_movement::Bodies::standing(&crowd).blocks(Point::new(9, 9, 0)),
            "the last tile in the run is found, which is what an unsorted crowd loses"
        );
    }

    /// A tile table that arrives after the facets do rebakes every one of them.
    ///
    /// [`WorldState::tiles`] was a public field, and a write to it left each
    /// loaded facet holding a span bake over the table that had just been
    /// replaced — a shard deciding steps by the heights of a world it no longer
    /// has. The wall is the visible half of that: to the empty table it is a
    /// graphic of no height and no flags, so a body walks over where it stands.
    ///
    /// **Two facets, because the loop is the point.** A rebake of the default
    /// facet alone passes every assertion a one-facet world can make.
    #[test]
    fn a_late_tile_table_rebakes_every_facet() {
        let (first, tiles) = walled(Facet(0));
        let (second, _) = walled(Facet(1));
        let mut facets = BTreeMap::new();
        facets.insert(
            Facet(0),
            FacetState::new(Some(first), None, 8, 8, &TileData::empty()),
        );
        facets.insert(
            Facet(1),
            FacetState::new(Some(second), None, 8, 8, &TileData::empty()),
        );
        let mut state = WorldState::new(
            facets,
            Facet(0),
            TileData::empty(),
            openshard_uofiles::multi::Multis::default(),
            (0, 0),
            1,
        );

        // The step onto the wall, which is the assertion in both directions.
        let onto_the_wall = |state: &WorldState, facet: Facet| {
            openshard_movement::step_allowed(
                &state.footing(facet, Doors::AsTheyStand),
                Point::new(4, 3, 0),
                Direction::South,
            )
        };

        for facet in [Facet(0), Facet(1)] {
            assert!(
                onto_the_wall(&state, facet).is_some(),
                "the empty table has no wall in it, so facet {} has flat ground here",
                facet.0
            );
        }

        state.set_tiles(tiles);

        for facet in [Facet(0), Facet(1)] {
            assert!(
                onto_the_wall(&state, facet).is_none(),
                "facet {} is still baked over the table it no longer holds",
                facet.0
            );
        }
    }
}
