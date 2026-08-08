//! Making a creature or townsperson: the one function that turns a spec into a
//! living mobile, and the event that announces it.

use openshard_entities::EntityId;
use openshard_movement::{Tile, Walker};
use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::serial::{Serial, SerialKind};
use openshard_protocol::wire::{Graphic, Hue, Layer};
use openshard_protocol::world::{Facet, Point};
use openshard_state::WorldState;
use openshard_state::components::{
    Aggression, Banker, Body, Brain, Fame, Heading, Hitpoints, Karma, MeleeDamage, Movement, Name, NightHome,
    Npc, Position, RangedAttack, Resistance, Skills, SwingSpeed, Title, body_opens_doors, creature_name,
};
use tracing::{debug, warn};

use openshard_items as items;

use crate::dress::{ShoeType, dress_townsperson};
use crate::live::BEAT_TICKS;
use crate::names::townsperson_name;

/// How far an idle townsperson may drift from its post before it heads back — a
/// couple of tiles of shuffling near the counter, not a stroll out the door.
/// ServUO's `RangeHome` for a `BaseVendor`.
const TOWNSFOLK_WANDER: u8 = 2;

/// A creature or NPC appeared in the world.
///
/// The mobile counterpart of `PlayerEntered`, for the mobiles no client drives
/// — a spawned creature. A script reads it to learn a fresh mobile's serial, the
/// name it needs to take control of it (`op_control`) or act on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileSpawned {
    /// The entity.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Where it appeared.
    pub position: Point,
}

/// Everything [`spawn`] needs — a plain bundle, so the one function that makes a
/// creature takes one argument instead of eleven.
#[derive(Debug)]
pub struct SpawnSpec {
    pub body: Graphic,
    pub hue: Hue,
    pub hits: u16,
    pub notoriety: u8,
    pub damage: u16,
    pub resistance: u8,
    pub swing: u64,
    pub sight: u8,
    /// Whether it starts fights (2), answers them (1), or only runs (0).
    pub aggression: u8,
    /// Ticks between its beats while hunting; 0 takes the shard default.
    pub beat: u64,
    /// How far its ranged attack reaches, in tiles; 0 fights hand to hand.
    pub ranged: u8,
    /// The ranged attack's damage type wire value.
    pub ranged_kind: u8,
    pub wander: bool,
    pub position: Point,
    pub facet: u8,
    /// A name the client shows on single-click, if any. Overrides `title`.
    pub name: Option<String>,
    /// The trade this NPC plies, ServUO-style ("the blacksmith"). `None` for a
    /// creature. It is the key three things hang off — the generated name, the
    /// generated outfit, and the speech table — so it is saved with the mobile.
    pub title: Option<String>,
    /// What the trade wears on its feet. Read only when there is a `title`, since
    /// that is when the core does the dressing.
    pub shoe: ShoeType,
    /// How widely known it is — what a killer inherits. A creature's own fame.
    pub fame: i32,
    /// Which way it is known. **Negative is evil**, so killing it *earns* karma: the
    /// killer is awarded the negation. A positive-karma creature is innocent and killing
    /// it costs.
    pub karma: i32,
    /// Where it sleeps, for the optional daily routine. `None` keeps it at its post
    /// around the clock, which is what both references do and what every pack does
    /// today — but it has to be *settable*, or `gameplay.npc_schedule` is a flag
    /// nothing can ever satisfy.
    pub night_home: Option<Point>,
    /// Whether this mobile is a banker — it answers "bank".
    pub banker: bool,
    /// Whether this mobile is a shopkeeper — it answers double-click with a
    /// buy gump and "sell" with an offer.
    pub vendor: bool,
    /// Worn clothing and gear, `(graphic, layer, hue)` — so it is not naked.
    ///
    /// **Additive, not a replacement.** A mobile with a `title` is always dressed by
    /// the core ([`dress_townsperson`]) and this list is worn *over* that base, the
    /// precedence ServUO's per-trade `InitOutfit` overrides have — they call
    /// `base.InitOutfit()` and add an apron, not instead of the shirt. Where the two
    /// want one layer, this list wins. A mobile with no `title` wears only this.
    pub equipment: Vec<(Graphic, Layer, Hue)>,
    /// Trained combat skills, `(skill id, value in tenths)` — Wrestling, Tactics,
    /// Anatomy and the weapon skills. Without these a creature has no `Skills`
    /// sheet, so its blows always land unscaled (the combat gate); with them it
    /// rolls to hit and scales damage like a player.
    pub skills: Vec<(u8, u16)>,
}

/// Put a mobile in the world. See `Command::SpawnMobile`.
///
/// The same bundle a player is built from — a body, a position, a facing, a
/// walker, hit points — minus the `Client`. That absence is the whole
/// difference between a creature and a person; everything that draws or moves
/// a mobile already treats "has a client" as the question, so a spawned one
/// falls out of the machinery already there.
pub fn spawn(state: &mut WorldState, spec: SpawnSpec) -> Option<EntityId> {
    let SpawnSpec {
        body,
        hue,
        hits,
        notoriety,
        damage,
        resistance,
        swing,
        sight,
        aggression,
        beat,
        wander,
        ranged,
        ranged_kind,
        position,
        facet,
        name,
        title,
        shoe,
        fame,
        karma,
        night_home,
        banker,
        vendor,
        equipment,
        skills,
    } = spec;
    // Wrapped here and carried as a `Facet` from this line down: the spec's number
    // came out of a pack, and the world's facet table is keyed by the component
    // type rather than by a bare byte.
    let facet = if state.facets.contains_key(&Facet(facet)) {
        Facet(facet)
    } else {
        warn!(facet, "unloaded facet; spawning the mobile on the default");
        state.default_facet
    };
    // Drop the mobile onto the ground, the way a client's spawner does: the
    // pack gives x/y and a rough height, and the floor it stands on — the top
    // of the static surface there, a building's raised floor and all — is the
    // map's to say. Without this a banker sinks to the given z and reads as
    // "inside a wall".
    let position = match state
        .facet_state(facet)
        .terrain
        .as_ref()
        .and_then(|t| t.spawn_z(Tile::new(position.x, position.y), i32::from(position.z)))
        .and_then(|z| i8::try_from(z).ok())
    {
        Some(z) => Point::new(position.x, position.y, z),
        None => position,
    };
    // Dress a townsperson before anything is written, because the roll decides the
    // body and the skin too — a woman is a different body graphic, not a flag. A
    // creature (no `title`) is never dressed; its body already is its appearance.
    //
    // ServUO's `BaseVendor` constructor runs `InitBody` then `InitOutfit`, and a
    // trade's own override calls `base.InitOutfit()` and *adds* to it — an apron
    // over the shirt, not instead of it. So the base is always rolled for a trade,
    // and whatever the pack sent is worn on top of it (see below, where the pack's
    // list is equipped first and the base then fills only the layers still free).
    // And only over a *human* base body. `InitOutfit` dresses a human: a shirt and
    // trousers on a dryad (`FrightenedDryad`, body 266) or a gargoyle is nonsense, and
    // rolling a gender would replace the body the pack asked for with a human one —
    // which is exactly what happened to the one non-human quest giver Britannia has.
    let dressed = title
        .as_ref()
        .filter(|_| body == crate::dress::BODY_MALE || body == crate::dress::BODY_FEMALE)
        .map(|_| dress_townsperson(&mut state.rng, shoe, None));
    let (body, hue) = match &dressed {
        Some(look) => (look.body, look.hue),
        None => (body, hue),
    };

    let (entity, serial) = match state.registry.spawn_with_serial(SerialKind::Mobile) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(?error, "out of mobile serials; not spawning");
            return None;
        }
    };
    let hits = hits.max(1);
    let facing = Facing::walking(Direction::South);
    state.registry.insert(entity, Body { id: body, hue });
    state.registry.insert(entity, Position(position));
    state.registry.insert(entity, Heading(facing));
    state.registry.insert(entity, facet);
    state.registry.insert(
        entity,
        Hitpoints {
            current: hits,
            max: hits,
        },
    );
    state.registry.insert(entity, Notoriety::from_bits(notoriety));
    state.registry.insert(entity, MeleeDamage { amount: damage });
    // Standing, only when it has any: a rat gives up nothing and a dragon a great deal,
    // and an absent component is the same as zero everywhere that reads it.
    if fame != 0 {
        state.registry.insert(entity, Fame(fame));
    }
    if karma != 0 {
        state.registry.insert(entity, Karma(karma));
    }
    // Combat skills, if the pack gave any: a sheet is what turns on the to-hit
    // roll and damage scaling for this creature (see `combat::check_hit`).
    if !skills.is_empty() {
        let mut sheet = Skills::default();
        for (id, value) in skills {
            sheet.set(id, value);
        }
        state.registry.insert(entity, sheet);
    }
    state.registry.insert(
        entity,
        Resistance {
            physical: resistance.min(100),
            ..Default::default()
        },
    );
    // Zero means "derive from dexterity", so a script that does not care about
    // pace names no number and gets the wrestling formula. A non-zero value
    // pins an exact cadence — a special creature that ignores its stats.
    if swing != 0 {
        state.registry.insert(entity, SwingSpeed { ticks: swing });
    }
    // A reach makes it an archer/mage/breather: it kites and volleys.
    if ranged > 0 {
        state.registry.insert(
            entity,
            RangedAttack {
                range: ranged,
                kind: ranged_kind,
            },
        );
    }
    // A brain only for a creature that needs one — something that hunts or
    // wanders. A pure prop (a shopkeeper standing still) gets none and never
    // enters `think`. `Combat` it earns when it first picks a fight.
    let aggression = Aggression::from_bits(aggression);
    // A brain for anything that hunts, drifts, or must answer or flee a blow —
    // which is everything but the aggressive-but-blind prop (sight 0), the old
    // meaning of "no brain".
    if sight > 0 || wander || aggression != Aggression::Aggressive {
        // Jittered like the townsfolk beat below, and for the same reason: a
        // spawner that fills a region hands every creature in it the same timer,
        // and a pack of wolves that decides, turns and lunges on one tick reads
        // as one animal with six bodies.
        let interval = if beat > 0 {
            beat
        } else {
            state.gameplay.creature_step_ticks.max(1)
        };
        let first = crate::live::first_beat(&mut state.rng, state.ticks, interval);
        state.registry.insert(
            entity,
            Brain {
                sight,
                wander,
                next_think: first,
                guard_until: 0,
                opens_doors: body_opens_doors(body),
                aggression,
                beat_ticks: beat,
            },
        );
    }
    // A name, in order of authority: what the spawn asked for, then a personal
    // name generated in front of the trade ("Rowena the blacksmith"), then the
    // creature default its body gives it ("a chicken", "a horse") — so an unnamed
    // animal or monster still reads on single-click. Nameless only when none of
    // those apply (an unlisted creature body).
    //
    // The generated form is why a whole town no longer answers to "the banker":
    // the pack sends the trade, and the person in front of it is the core's.
    let name = if let Some(name) = name {
        Some(name)
    } else if let Some(title) = &title {
        let female = dressed.as_ref().is_some_and(|look| look.female);
        Some(townsperson_name(&mut state.rng, title, female))
    } else {
        creature_name(body).map(str::to_owned)
    };
    if let Some(name) = name {
        state.registry.insert(entity, Name(name));
    }
    if vendor {
        crate::vendor::make_vendor(state, entity, serial);
    }
    if banker {
        state.registry.insert(entity, Banker);
    }
    // The trade itself, kept on the mobile: it is the key its speech table is
    // looked up by every time someone talks near it, so it cannot live only in the
    // spawn call that placed it — see `MobileRecord::title`.
    if let Some(title) = title {
        state.registry.insert(entity, Title(title));
    }
    // Every townsperson gets the base — a home to keep to and the beat that turns
    // it to face whoever comes near — not only the two with a service to sell.
    // Gating this on `banker || vendor` alone left 257 of Felucca's 738 townsfolk
    // as statues: a name, and no life at all. A declared trade is now enough, and a
    // service still is, so a pack that names neither a trade nor a service is the
    // only way to get a prop — which is what a prop should take.
    if state.registry.has::<Title>(entity) || banker || vendor {
        // The first beat is jittered across one beat's worth of ticks, the way
        // `register_spawner` jitters a fresh region's first spawn. A `Populate` places
        // seven hundred townsfolk on one tick, and a shared `next_beat` of zero puts
        // every one of their beats on the same tick — which is only a pacing
        // curiosity until they all path home at dusk on the same tick and the A*
        // bill for a whole facet lands at once.
        let first = crate::live::first_beat(&mut state.rng, state.ticks, BEAT_TICKS);
        state.registry.insert(
            entity,
            Npc {
                home: position,
                wander: TOWNSFOLK_WANDER,
                next_beat: first,
                next_greet: 0,
            },
        );
        if let Some(at) = night_home {
            state.registry.insert(entity, NightHome(at));
        }
    }
    // Dress it before the reveal, so the clothing rides in the `0x78` that
    // draws it — a naked banker is a bug that looks like nudity.
    //
    // The pack's own list goes on first and the generated base fills what is left,
    // which is the precedence ServUO's `InitOutfit` overrides have: a smith's apron
    // is a deliberate choice and the base shirt is a fallback, so where the two want
    // one layer the pack wins. `equip_worn_item` does not check the layer — it would
    // cheerfully list two items on one and leave the client drawing whichever it
    // read last — so the check is here, at the one place that equips a whole outfit.
    let mut worn: Vec<Layer> = Vec::with_capacity(8);
    for (graphic, layer, item_hue) in equipment
        .into_iter()
        .chain(dressed.into_iter().flat_map(|look| look.equipment))
    {
        if worn.contains(&layer) {
            continue;
        }
        if items::equip_worn_item(state, serial, graphic, item_hue, layer).is_some() {
            worn.push(layer);
        }
    }
    state
        .registry
        .insert(entity, Movement(Walker::new(position, facing)));
    state.facet_state_mut(facet).sectors.insert(entity, position);
    state.reveal(entity);
    // Say who and where, so a script can take control of it: the mobile
    // counterpart of `PlayerEntered`, and how `op_control` learns a serial.
    state.bus.send(MobileSpawned {
        entity,
        serial,
        position,
    });
    debug!(%serial, body = body.0, "mobile spawned");
    Some(entity)
}
