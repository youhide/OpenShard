//! Mining, Lumberjacking and Fishing — what a tool swung at the ground gives back.
//!
//! ServUO's `Scripts/Services/Harvest/`. Like the bandage and the lockpick, none
//! of these three has a usable button: the action that uses them *is* a
//! double-click on the pickaxe, so they come through the same `ItemUsed` seam and
//! raise their own cursor.
//!
//! The tables — which tiles, which veins, what a bank holds — are
//! [`openshard_state::harvest`]. What the swing *does* is here, and it is the
//! usual four steps: raise a cursor, check the ground, work for a few beats with
//! sound, one continuous gesture, and on the last beat roll [`roll_skill_band`]
//! and pay out.
//!
//! Two things it does *not* decide. Whether the ground is what the client claims
//! it is, which the caller resolves against the map and hands over as a
//! [`HarvestTarget`] — `skills` sits below the client-file readers. And where the yield
//! ends up when the pack will not take it, which is `items`' door.

use openshard_entities::EntityId;
use openshard_map::grid::Tile;
use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::target::{
    TargetCursor,
    TargetKind,
};
use openshard_protocol::wire::{
    ClilocId,
    CursorId,
    Graphic,
    Hue,
    Layer,
    SoundId,
};
use openshard_protocol::world::{
    Facet,
    Point,
};
use openshard_state::components::{
    Client,
    Drawn,
    Equipped,
    Harvesting,
    ItemKind,
    Position,
    Tool,
};
use openshard_state::harvest::{
    Bank,
    HarvestAction,
    HarvestDef,
    HarvestKind,
    HarvestResource,
    TileSource,
    VeinIdx,
    definition_for,
    tool_data,
    tool_data_for_kind,
};
use openshard_state::weapon::{
    LAYER_ONE_HANDED,
    weapon_data,
    weapon_data_for_kind,
    weapon_layer,
};
use openshard_state::{
    Action,
    Skill,
    TargetPurpose,
    WorldState,
    in_range,
};

use crate::check::{
    roll_skill_band,
    skill_value,
};

/// "You have worn out your tool!" — said before a cursor goes up, so a spent
/// pickaxe never asks a question it cannot answer.
const WORN_OUT: ClilocId = ClilocId(1_044_038);
/// "Where do you wish to dig?" — mining's prompt.
const DIG_WHERE: ClilocId = ClilocId(503_033);
/// "What do you want to use this item on?" — ServUO's `BaseAxe` prompt.
const CHOP_WHERE: ClilocId = ClilocId(1_010_018);
/// "What water do you want to fish in?" — fishing's prompt.
const FISH_WHERE: ClilocId = ClilocId(500_974);
/// "Target a mountain or cave." — what a pick says about a patch of grass.
const NOT_MINABLE: ClilocId = ClilocId(501_862);
/// "You can't use an axe on that." — ServUO's `Lumberjacking.OnBadHarvestTarget`.
const NOT_CHOPPABLE: ClilocId = ClilocId(500_489);
/// "You can't fish there." — a line cast at dry land.
const NOT_FISHABLE: ClilocId = ClilocId(500_979);
/// One swing at a time per harvester.
///
/// `500972` looks tempting here, but its actual text is "You are already
/// fishing."  It is only suitable for fishing, and using it for a lumberjack
/// makes a second axe swing claim the player is fishing.  There is no shared
/// harvest cliloc, so this deliberately remains a plain system line.
const ALREADY_HARVESTING: &str = "You are already harvesting.";

/// The facet a Felucca-rate harvest is worth double on.
///
/// ServUO pays `ConsumedPerFeluccaHarvest` on `Map.Felucca` alone, the trade for
/// its being the one facet without protection. Facet `0` is Felucca; the constant
/// is here rather than inline because it is a *rule*, not a coincidence of
/// numbering.
const FELUCCA: Facet = Facet(0);

/// What a resolved harvest target is: the ground, and what is on it.
///
/// The caller resolves this from a `0x6C` reply against the map — a location reply
/// with a graphic of zero is bare land whose tile id the client never sent, and a
/// non-zero graphic is a static that must actually be standing there. That reading
/// needs the client's files, so it happens above this crate and arrives here
/// already settled.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HarvestTarget {
    /// Where.
    pub at:     Point,
    /// The tile graphic, raw.
    pub tile:   Graphic,
    /// Whether it came from the land table or the static table, which decides how
    /// the id is matched.
    pub source: TileSource,
}

/// A tool wore out and should be removed — `items`' door, not this crate's.
///
/// The same shape as `InstrumentSpent`, and for the same reason: a skill decides
/// that a thing is finished, and the crate that owns items makes it gone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ToolWorn {
    /// The tool that broke.
    pub tool: EntityId,
}

/// Somebody got something out of the ground.
///
/// Emitted on every successful swing, so a pack can react — a quest that counts
/// ore, a shard that pays a bounty for fish. The core has already paid out by the
/// time this is read; it is an announcement, not a request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Harvested {
    /// Who swung.
    pub harvester: EntityId,
    /// Which of the three systems.
    pub kind:      HarvestKind,
    /// Semantic kind that came out, if this resource row has migrated.
    pub item_kind: Option<ItemKindId>,
    /// Semantic material grade that came out with the kind.
    pub material:  Option<MaterialId>,
    /// The classic item art that came out, retained for legacy script and
    /// presentation adapters.
    pub graphic:   Graphic,
    /// Its classic presentation hue.
    pub hue:       Hue,
    /// How much.
    pub amount:    u16,
}

/// A double-clicked tool: put up the cursor that asks what to swing it at.
///
/// Returns whether the item was a harvesting tool at all, so the caller knows
/// whether anything happened.
pub fn use_tool(state: &mut WorldState, harvester: EntityId, tool: EntityId) -> bool {
    let Some(graphic) = state
        .registry
        .get::<openshard_state::components::Drawn>(tool)
        .map(|g| g.id)
    else {
        return false;
    };
    let data = match state.registry.get::<ItemKind>(tool) {
        Some(kind) => tool_data_for_kind(kind.0),
        None => tool_data(graphic),
    };
    let Some(data) = data else {
        return false;
    };
    // ServUO's `CheckTool`: a spent tool is refused before anything else, so the
    // player is told rather than left targeting into a no-op.
    if state
        .registry
        .get::<Tool>(tool)
        .is_some_and(|worn| worn.uses_left == 0)
    {
        state.localized_message(harvester, WORN_OUT, "");
        return true;
    }
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(harvester) else {
        return true; // a creature has no cursor to raise
    };
    let Some(serial) = state.registry.serial_of(harvester) else {
        return true;
    };
    state.raise_target(harvester, TargetPurpose::Harvest { tool });
    // A hatchet has one possible harvest definition, unlike a pick which can
    // point at either ore or sand. Send the presentation facts *before* the
    // crosshair: by the time the user can click, the client can start locally.
    if data.skill == Skill::Lumberjacking {
        let lumber = openshard_state::harvest::definition(HarvestKind::Lumber, state.gameplay.is_ml());
        show_backpack_chop_tool(state, harvester, tool);
        state.preview_harvest(
            harvester,
            CursorId(serial.raw()),
            Action::Chop,
            lumber.beat_ticks.saturating_mul(u64::from(lumber.beats)),
            lumber.beats,
        );
    }
    let prompt = match data.skill {
        Skill::Mining => DIG_WHERE,
        Skill::Lumberjacking => CHOP_WHERE,
        Skill::Fishing => FISH_WHERE,
        _ => unreachable!("harvesting tools only use harvesting skills"),
    };
    state.localized_message(harvester, prompt, "");
    // The *location* cursor, not the object one: a mountain face and a patch of
    // water are not entities, and an object cursor would refuse the click.
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(serial.raw()),
            kind:      TargetKind::Location,
        }),
    );
    true
}

/// The cursor came back with a spot on the ground: start swinging, or say why not.
///
/// ServUO's `StartHarvesting`, gate for gate.
pub fn begin_harvest(
    state: &mut WorldState,
    harvester: EntityId,
    tool: EntityId,
    target: HarvestTarget,
) -> bool {
    // The tool may have been dropped, sold or spent while the cursor was up.
    if state.registry.serial_of(tool).is_none() {
        return false;
    }
    let Some(def) = definition_for(target.tile, target.source, state.gameplay.is_ml()) else {
        state.localized_message(harvester, bad_target_line(state, tool), "");
        return false;
    };
    // Which system a swing is comes from the *tile*, never the tool — but a
    // fishing pole still cannot mine, so the two have to agree.
    let matches_tool = match state.registry.get::<ItemKind>(tool) {
        Some(kind) => tool_data_for_kind(kind.0),
        None => {
            state
                .registry
                .get::<Drawn>(tool)
                .and_then(|graphic| tool_data(graphic.id))
        }
    }
    .is_some_and(|data| data.skill == def.skill);
    if !matches_tool {
        state.localized_message(harvester, bad_target_line(state, tool), "");
        return false;
    }
    if !within_reach(state, harvester, target.at, def.max_range) {
        state.localized_message(harvester, def.messages.out_of_range, "");
        return false;
    }
    if !has_stock(state, harvester, def, target.at) {
        state.localized_message(harvester, def.messages.no_resources, "");
        return false;
    }
    // ServUO's `GetLock` returns the *tool*, so a player with two picks may work
    // two veins — "as OSI", its own comment says. One `Harvesting` per mobile is
    // the closest this engine gets: a component is per entity, and a second
    // concurrent swing would need a list. So the refusal is per harvester.
    if state.registry.has::<Harvesting>(harvester) {
        state.system_message(harvester, ALREADY_HARVESTING);
        return false;
    }
    // ServUO reveals a lumberjack (`Lumberjacking.OnHarvestStarted`, `Core.ML`)
    // and nobody else — a miner underground stays hidden, which reads oddly and
    // is what the reference does. Kept as it is rather than tidied.
    if def.kind == HarvestKind::Lumber {
        state.break_cover(harvester);
    }
    state.registry.insert(
        harvester,
        Harvesting {
            tool,
            at: target.at,
            kind: def.kind,
            tile: openshard_state::harvest::tile_key(target.tile, target.source),
            beats_left: def.beats,
            next_beat: state.ticks + def.beat_ticks,
            next_sound: state.ticks + def.sound_ticks,
        },
    );
    // The first swing lands now rather than a beat and a half from now: without
    // this the tool is double-clicked, the cursor answered, and nothing at all
    // happens for two seconds. The noise it makes follows on its own clock.
    swing_gesture(
        state,
        harvester,
        tool,
        def,
        target.at,
        def.beat_ticks.saturating_mul(u64::from(def.beats)),
    );
    true
}

/// Advance every harvest in flight, and finish those whose last beat has come.
///
/// The tick counter drives it, like every other timer here. Returns the tools that
/// broke, for the tick to remove through `items`.
pub fn advance_harvests(state: &mut WorldState) -> Vec<ToolWorn> {
    let now = state.ticks;
    let live: Vec<(EntityId, Harvesting)> = state
        .registry
        .query::<Harvesting>()
        .map(|(entity, work)| (entity, *work))
        .collect();
    let mut worn = Vec::new();
    for (harvester, work) in live {
        let def = openshard_state::harvest::definition(work.kind, state.gameplay.is_ml());
        // The chink of the pick, most of a second after it was raised — its own
        // clock, so the noise falls *inside* a beat rather than on top of one.
        if now >= work.next_sound {
            swing_sound(state, harvester, def);
            state.registry.insert(
                harvester,
                Harvesting {
                    next_sound: openshard_state::WorldTick::MAX,
                    ..work
                },
            );
        }
        if now < work.next_beat {
            continue;
        }
        // Every gate is re-checked on every beat, because all of them can change
        // under a swing that takes seconds: the tool can be dropped, the harvester
        // can walk off, and somebody else can empty the vein.
        if state.registry.serial_of(work.tool).is_none() {
            state.registry.remove::<Harvesting>(harvester);
            state.complete_harvest(harvester);
            continue;
        }
        if !within_reach(state, harvester, work.at, def.max_range) {
            state.registry.remove::<Harvesting>(harvester);
            state.complete_harvest(harvester);
            // A *different* line from the one a too-distant first click gets:
            // walking away mid-swing is giving up, not a mistake.
            state.localized_message(harvester, def.messages.timed_out_of_range, "");
            continue;
        }
        if !has_stock(state, harvester, def, work.at) {
            state.registry.remove::<Harvesting>(harvester);
            state.complete_harvest(harvester);
            state.localized_message(harvester, def.messages.double_harvest, "");
            continue;
        }
        let last = work.beats_left <= 1;
        if !last {
            state.registry.insert(
                harvester,
                Harvesting {
                    beats_left: work.beats_left - 1,
                    next_beat: now + def.beat_ticks,
                    next_sound: now + def.sound_ticks,
                    ..work
                },
            );
            continue;
        }
        state.registry.remove::<Harvesting>(harvester);
        state.complete_harvest(harvester);
        if let Some(broke) = deliver(state, harvester, &work, def) {
            worn.push(broke);
        }
    }
    worn
}

/// The last beat: roll, pay out, and wear the tool down.
fn deliver(
    state: &mut WorldState,
    harvester: EntityId,
    work: &Harvesting,
    def: &'static HarvestDef,
) -> Option<ToolWorn> {
    let vein_index = bank_vein(state, harvester, def, work.at);
    let resource = choose_resource(state, harvester, def, vein_index);

    // ServUO's `CheckHarvestSkill`: the flat requirement *and* the band, in that
    // order — one says the vein is beyond you, the other whether this swing found
    // anything. The band roll is the same call combat's to-hit makes, so a miner
    // trains from the attempt.
    let value = skill_value(state, harvester, def.skill);
    let able = i32::from(value) >= resource.req_skill;
    let struck = able
        && roll_skill_band(
            state,
            harvester,
            def.skill,
            crate::SkillBand::new(resource.min_skill, resource.max_skill),
        );

    if !struck {
        state.localized_message(harvester, def.messages.fail, "");
        return wear_tool(state, harvester, work.tool, def);
    }

    // Felucca pays double, and only up to what the bank can actually give — a
    // half-empty vein hands over what is in it rather than going negative.
    let facet = state.facet_of(harvester);
    let wanted = if facet == FELUCCA {
        def.consumed_felucca
    } else {
        def.consumed
    };
    let amount = wanted.min(bank_stock(state, harvester, def, work.at));
    if amount == 0 {
        state.localized_message(harvester, def.messages.double_harvest, "");
        return wear_tool(state, harvester, work.tool, def);
    }

    let landed = pay_out(state, harvester, def, resource, amount);
    if !landed {
        state.localized_message(harvester, def.messages.pack_full, "");
        return wear_tool(state, harvester, work.tool, def);
    }
    state.localized_message(harvester, resource.success_cliloc, "");

    // Staff do not deplete a vein — ServUO's `AccessLevel.Player` guard, which is
    // what lets a game master test a fix without quietly mining out the shard.
    if !state.is_staff(harvester) {
        consume_bank(state, harvester, def, work.at, amount);
    }
    state.bus.send(Harvested {
        harvester,
        kind: def.kind,
        item_kind: resource.item_kind,
        material: resource.material,
        graphic: resource.graphic,
        hue: resource.hue,
        amount,
    });
    wear_tool(state, harvester, work.tool, def)
}

/// Put the yield in the pack, or at the harvester's feet where the definition
/// allows it. Returns whether it landed anywhere.
fn pay_out(
    state: &mut WorldState,
    harvester: EntityId,
    def: &'static HarvestDef,
    resource: &'static HarvestResource,
    amount: u16,
) -> bool {
    let Some(serial) = state.registry.serial_of(harvester) else {
        return false;
    };
    // Every resource stacks, so this always merges onto the pile already in the
    // pack rather than filling it with singles. Migrated rows enter through the
    // semantic constructor; the graphic/hue branch is the temporary path for
    // sand and fish, whose definition rows have not landed yet.
    let given = match resource.item_kind {
        Some(kind) => {
            openshard_items::give_kind_to_backpack(state, serial, kind, resource.material, amount, true)
        }
        None => {
            openshard_items::give_to_backpack(state, serial, resource.graphic, resource.hue, amount, true)
        }
    };
    if given {
        return true;
    }
    if !def.place_at_feet {
        return false;
    }
    let (Some(&Position(at)), facet) = (
        state.registry.get::<Position>(harvester),
        state.facet_of(harvester),
    ) else {
        return false;
    };
    match resource.item_kind {
        Some(kind) => {
            openshard_items::spawn_item_kind(state, kind, resource.material, amount, true, at, facet)
                .is_some()
        }
        None => {
            openshard_items::spawn_item(state, resource.graphic, resource.hue, amount, true, at, facet)
                .is_some()
        }
    }
}

/// Spend a swing off the tool, and say so if it broke.
fn wear_tool(
    state: &mut WorldState,
    harvester: EntityId,
    tool: EntityId,
    def: &'static HarvestDef,
) -> Option<ToolWorn> {
    let left = state.registry.get::<Tool>(tool)?.uses_left;
    let left = left.saturating_sub(1);
    if left > 0 {
        state.registry.insert(tool, Tool { uses_left: left });
        return None;
    }
    state.registry.remove::<Tool>(tool);
    state.localized_message(harvester, def.messages.tool_broke, "");
    Some(ToolWorn { tool })
}

/// Run something against the bank under a spot, creating it on the first swing.
///
/// The banks live on a facet and the generator on the world, so the two fields are
/// destructured apart rather than reached through `&mut state` twice — a bank roll
/// has to spend the *world's* sequence, or a harvest stops replaying.
fn with_bank<T>(
    state: &mut WorldState,
    harvester: EntityId,
    def: &'static HarvestDef,
    at: Point,
    act: impl FnOnce(&mut Bank, &mut openshard_state::Rng) -> T,
) -> T {
    let facet = state.facet_of(harvester);
    let now = state.ticks;
    let WorldState {
        ref mut facets,
        ref mut rng,
        ..
    } = *state;
    let banks = &mut facets
        .get_mut(&facet)
        .expect("an entity's facet is always loaded")
        .banks;
    act(banks.get(def, at.x, at.y, facet, now, rng), rng)
}

/// Which vein the bank under a spot holds.
fn bank_vein(state: &mut WorldState, harvester: EntityId, def: &'static HarvestDef, at: Point) -> VeinIdx {
    with_bank(state, harvester, def, at, |bank, _| bank.vein)
}

/// What the bank under a spot has left.
fn bank_stock(state: &mut WorldState, harvester: EntityId, def: &'static HarvestDef, at: Point) -> u16 {
    with_bank(state, harvester, def, at, |bank, _| bank.current)
}

/// Take `amount` out of the bank under a spot.
fn consume_bank(
    state: &mut WorldState,
    harvester: EntityId,
    def: &'static HarvestDef,
    at: Point,
    amount: u16,
) {
    let now = state.ticks;
    with_bank(state, harvester, def, at, |bank, rng| {
        bank.consume(def, amount, now, rng);
    });
}

/// Whether the bank under a spot can pay one harvest.
fn has_stock(state: &mut WorldState, harvester: EntityId, def: &'static HarvestDef, at: Point) -> bool {
    bank_stock(state, harvester, def, at) >= 1
}

/// Which resource this swing yields — ServUO's `MutateResource`.
///
/// Two ways to fall back to the vein's poorer cousin: the vein's own
/// disappointment chance, and simply not being good enough to work the primary.
fn choose_resource(
    state: &mut WorldState,
    harvester: EntityId,
    def: &'static HarvestDef,
    vein_index: VeinIdx,
) -> &'static HarvestResource {
    let vein = &def.veins[vein_index.0];
    let primary = &def.resources[vein.primary.0];
    let Some(fallback) = vein.fallback.map(|index| &def.resources[index.0]) else {
        return primary;
    };
    if vein.fallback_chance > state.rng.below(10_000) {
        return fallback;
    }
    let value = i32::from(skill_value(state, harvester, def.skill));
    if value < primary.req_skill || value < primary.min_skill {
        return fallback;
    }
    primary
}

/// Face the spot and begin a continuous harvest animation — the standing rule
/// that a visible action is never a state change alone. The noise follows on
/// [`swing_sound`]'s own clock.
///
/// ServUO's `DoHarvestingEffect` does not animate a *mounted* harvester, and that
/// is kept: a mining swing played on horseback reads as a glitch.
fn swing_gesture(
    state: &mut WorldState,
    harvester: EntityId,
    tool: EntityId,
    def: &'static HarvestDef,
    at: Point,
    duration_ticks: u64,
) {
    state.face_point(harvester, at);
    if state
        .registry
        .has::<openshard_state::components::Riding>(harvester)
    {
        return;
    }
    if def.action == HarvestAction::Chop {
        show_backpack_chop_tool(state, harvester, tool);
    }
    state.animate_timed(
        harvester,
        match def.action {
            HarvestAction::Mine => Action::Mine,
            HarvestAction::Chop => Action::Chop,
            HarvestAction::Fish => Action::Fish,
        },
        duration_ticks,
    );
}

/// Lend a backpack axe to the renderer. It is called when the target cursor
/// opens as well as when the shard accepts its target, so optimistic chopping
/// has the correct hand layer from its first local frame.
fn show_backpack_chop_tool(state: &mut WorldState, harvester: EntityId, tool: EntityId) {
    if state.registry.has::<Equipped>(tool) {
        return;
    }
    let Some(Drawn { id, hue }) = state.registry.get::<Drawn>(tool).copied() else {
        return;
    };
    let weapon = match state.registry.get::<ItemKind>(tool) {
        Some(kind) => weapon_data_for_kind(kind.0),
        None => weapon_data(id),
    };
    let Some(weapon) = weapon else {
        return;
    };
    let layer = weapon_layer(weapon, Layer(state.tiles().static_tile(id.0).layer));
    // No client files leave this byte at zero. The tool is already known to be
    // a `BaseAxe`, whose ordinary one-handed appearance is the useful fallback.
    state.show_harvest_tool(
        harvester,
        id,
        hue,
        if layer == Layer(0) {
            LAYER_ONE_HANDED
        } else {
            layer
        },
    );
}

/// The noise one beat makes, rolled between the definition's on the world's own
/// generator so a harvest replays.
fn swing_sound(state: &mut WorldState, harvester: EntityId, def: &'static HarvestDef) {
    let Some(sound) = pick_sound(&mut state.rng, def.sounds) else {
        return; // fishing is silent until the catch
    };
    state.play_sound(harvester, sound);
}

/// Pick from a definition's sound table, or stay silent for an empty one.
fn pick_sound(rng: &mut openshard_state::Rng, sounds: &[SoundId]) -> Option<SoundId> {
    let length = u16::try_from(sounds.len()).expect("a shipped harvest sound table fits u16");
    let bound = std::num::NonZeroU16::new(length)?;
    let index = usize::from(crate::roll_u16(rng, bound));
    Some(sounds[index])
}

/// Whether the harvester is on the same facet and near enough.
///
/// Re-checked server-side even though the cursor was raised with a range: the
/// range on a `0x6C` is the client's courtesy, never the judge — the rule
/// `ITEM_REACH` holds for a lift.
fn within_reach(state: &WorldState, harvester: EntityId, at: Point, range: u32) -> bool {
    state
        .registry
        .get::<Position>(harvester)
        .is_some_and(|&Position(from)| in_range(from, at, range))
}

/// What a tool says about ground it cannot work — ServUO's per-system
/// `OnBadHarvestTarget`, which is a different sentence for each.
fn bad_target_line(state: &WorldState, tool: EntityId) -> ClilocId {
    let skill = match state.registry.get::<ItemKind>(tool) {
        Some(kind) => tool_data_for_kind(kind.0),
        None => {
            state
                .registry
                .get::<Drawn>(tool)
                .and_then(|graphic| tool_data(graphic.id))
        }
    }
    .map(|data| data.skill);
    match skill {
        Some(Skill::Lumberjacking) => NOT_CHOPPABLE,
        Some(Skill::Fishing) => NOT_FISHABLE,
        _ => NOT_MINABLE,
    }
}

/// Resolve a `0x6C` location reply into a [`HarvestTarget`], against the map.
///
/// **This is the load-bearing half of the whole slice.** ServUO's
/// `PacketHandlers.cs` is the authority: in a location reply a graphic of **zero**
/// means the client clicked bare land, and the land tile id is *not on the wire* —
/// the server looks it up. A non-zero graphic is a static, and ServUO scans the
/// map for a tile of that id at that exact z before believing it, cancelling the
/// target when nothing matches. Both halves are here, because trusting the second
/// would let a client name any tile it liked and mine the middle of Britain.
#[must_use]
pub fn resolve_harvest_target(
    state: &WorldState,
    facet: Facet,
    at: Point,
    graphic: Graphic,
) -> Option<HarvestTarget> {
    // The *map* and not the live ground: what the land under a spot is, and
    // which statics stand on it, are facts about the facet — a crate somebody
    // dropped there does not make it a different tile. A facet reached through
    // `facet_of` always has one loaded; `None` here is a shard with no client
    // files, which has nothing to harvest either.
    let terrain = state.map_terrain(facet)?;
    if graphic.0 == 0 {
        return Some(HarvestTarget {
            at,
            tile: Graphic(terrain.land_tile(Tile::new(at.x, at.y))?.0),
            source: TileSource::Land,
        });
    }
    let mut statics = Vec::new();
    terrain.statics_at(Tile::new(at.x, at.y), &mut statics);
    statics
        .iter()
        .any(|&(id, z)| id == graphic && i32::from(z) == i32::from(at.z))
        .then_some(HarvestTarget {
            at,
            tile: graphic,
            source: TileSource::Static,
        })
}

#[cfg(test)]
mod tests {
    use openshard_state::harvest::definition;

    use super::*;

    #[test]
    fn every_shipped_sound_table_is_a_representable_safe_roll() {
        let mut rng = openshard_state::Rng::new(7);
        for ml in [false, true] {
            for kind in [
                HarvestKind::Ore,
                HarvestKind::Sand,
                HarvestKind::Lumber,
                HarvestKind::Fish,
            ] {
                let sounds = definition(kind, ml).sounds;
                assert!(u16::try_from(sounds.len()).is_ok(), "{kind:?}, ML={ml}");
                for _ in 0..1000 {
                    let picked = pick_sound(&mut rng, sounds);
                    if sounds.is_empty() {
                        assert_eq!(picked, None, "{kind:?}, ML={ml}");
                    } else {
                        assert!(
                            picked.is_some_and(|sound| sounds.contains(&sound)),
                            "{kind:?}, ML={ml}"
                        );
                    }
                }
            }
        }
    }
}
