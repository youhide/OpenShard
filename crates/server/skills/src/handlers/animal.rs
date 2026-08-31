//! Animal Lore — the one lore skill that answers with a window rather than a line.
//!
//! ServUO's `AnimalLore` and its `AnimalLoreGump`. It was deliberately held back
//! until pets existed, because without them its gates are unreachable and its
//! window is a column of dashes: what it shows is a creature's pools, stats and
//! standing *as a pet*, and what it asks first is whether the creature is one.
//!
//! The three gates are ServUO's, and they are the skill:
//!
//! - under 100.0 you may only lore a creature somebody has already tamed;
//! - under 110.0, that or one that *could* be tamed (rolled against 80.0);
//! - above it, anything, with the wild ones rolled against 100.0.

use openshard_entities::EntityId;
use openshard_protocol::gump::{
    ButtonId,
    GumpButton,
    GumpDisplay,
    GumpId,
    GumpKey,
    GumpLayout,
    GumpPoint,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::wire::ClilocId;
use openshard_state::components::{
    Body,
    BodyType,
    Client,
    Ghost,
    Hitpoints,
    Mana,
    Pet,
    Resistance,
    Skills,
    Stamina,
    Stats,
};
use openshard_state::{
    Skill,
    WorldState,
};

use crate::check::roll_skill_band;

/// "That's not an animal!"
const NOT_AN_ANIMAL: ClilocId = ClilocId(500_329);
/// "The spirits of the dead are not the province of animal lore."
const NOT_THE_DEAD: ClilocId = ClilocId(500_331);
/// "At your skill level, you can only lore tamed creatures."
const ONLY_TAMED: ClilocId = ClilocId(1_049_674);
/// "At your skill level, you can only lore tamed or tameable creatures."
const ONLY_TAMEABLE: ClilocId = ClilocId(1_049_675);
/// "You can't think of anything you know offhand."
const NOTHING_OFFHAND: ClilocId = ClilocId(500_334);

/// The gump id the window is drawn under. Its own number, so a reply cannot be
/// confused with a quest dialog's.
pub const ANIMAL_LORE_GUMP: GumpId = openshard_protocol::gump::id::ANIMAL_LORE;

/// The skill below which only a tamed creature can be read, in tenths.
const TAMED_ONLY_BELOW: u16 = 1000;
/// And below which only a tamed or tameable one can, in tenths.
const TAMEABLE_ONLY_BELOW: u16 = 1100;

/// Animal Lore's cursor came back with something.
pub(super) fn animal_lore(state: &mut WorldState, looker: EntityId, target: EntityId) {
    if state.registry.has::<Ghost>(looker) || state.registry.has::<Ghost>(target) {
        state.localized_message(looker, NOT_THE_DEAD, "");
        return;
    }
    // A person is not an animal, and neither is anything with no body at all.
    let Some(&Body { id: body, .. }) = state.registry.get::<Body>(target) else {
        state.localized_message(looker, NOT_AN_ANIMAL, "");
        return;
    };
    if state.registry.has::<Client>(target)
        || !matches!(
            openshard_state::components::body_type(body),
            BodyType::Animal | BodyType::Monster | BodyType::Sea
        )
    {
        state.localized_message(looker, NOT_AN_ANIMAL, "");
        return;
    }

    let skill = Skill::AnimalLore;
    let value = crate::skill_value(state, looker, skill);
    let controlled = state.registry.has::<Pet>(target);
    let tameable = state
        .registry
        .get::<openshard_state::components::Tamable>(target)
        .copied()
        .or_else(|| openshard_state::tame::tamable(body))
        .is_some();

    // The ladder, in ServUO's order. A tamed creature is always readable; the rest
    // depend on how much the reader knows.
    let allowed = if controlled {
        true
    } else if value < TAMED_ONLY_BELOW {
        state.localized_message(looker, ONLY_TAMED, "");
        return;
    } else if value < TAMEABLE_ONLY_BELOW {
        if !tameable {
            state.localized_message(looker, ONLY_TAMEABLE, "");
            return;
        }
        roll_skill_band(state, looker, skill, crate::SkillBand::new(800, 1200))
    } else {
        let floor = if tameable { 800 } else { 1000 };
        roll_skill_band(state, looker, skill, crate::SkillBand::new(floor, 1200))
    };
    if !allowed {
        state.localized_message(looker, NOTHING_OFFHAND, "");
        return;
    }
    // A tamed creature read by its owner trains nothing extra; the roll above is
    // the only check, exactly as ServUO's `SendGump` calls `CheckTargetSkill` once.
    if controlled {
        let _ = roll_skill_band(state, looker, skill, crate::SkillBand::new(0, 1200));
    }
    show_window(state, looker, target);
}

/// Draw the window — ServUO's `AnimalLoreGump`, in the ML frame it uses.
///
/// Two pages rather than its five: this engine has the attributes and the combat
/// ratings, and the three it drops (AoS resistances beyond physical, per-type
/// damage, food and pack instincts) are numbers nothing in the world sets yet. A
/// column of dashes is worse than a page that is not there.
fn show_window(state: &mut WorldState, looker: EntityId, target: EntityId) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(looker) else {
        return;
    };
    let name = state
        .registry
        .get::<openshard_state::components::Name>(target)
        .map(|n| n.0.clone())
        .or_else(|| {
            state
                .registry
                .get::<Body>(target)
                .and_then(|body| openshard_state::components::creature_name(body.id))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "a creature".to_owned());
    let hits = state.registry.get::<Hitpoints>(target).copied();
    let stam = state.registry.get::<Stamina>(target).copied();
    let mana = state.registry.get::<Mana>(target).copied();
    let stats = state.registry.get::<Stats>(target).copied();
    let armour = openshard_state::armor::worn_armor_rating(state, target);
    let physical = state.registry.get::<Resistance>(target).map_or(0, |r| r.physical);
    let skills = state.registry.get::<Skills>(target).cloned();
    let loyalty = state.registry.get::<Pet>(target).map(|pet| pet.order);

    let mut gump = GumpLayout::new();
    gump.no_dispose();
    // The ML frame, piece for piece: the backdrop, three body panels and a footer.
    gump.page(0);
    gump.image(100, 100, 2080);
    gump.image(118, 137, 2081);
    gump.image(118, 207, 2081);
    gump.image(118, 277, 2081);
    gump.image(118, 347, 2083);
    gump.image(140, 138, 2091);
    gump.image(140, 335, 2091);
    gump.html(
        147,
        108,
        210,
        18,
        format!("<center><i>{name}</i></center>"),
        false,
        false,
    );
    // The close button, the one control ServUO gives it.
    gump.button(240, 77, 2093, 2093, GumpButton::Reply, 0, ButtonId::CLOSE_BOX);

    gump.page(1);
    gump.image(128, 152, 2086);
    gump.html_localized_colored(147, 150, 160, 18, ClilocId(1_049_593), 200, false, false); // Attributes
    let row = |gump: &mut GumpLayout, y: i32, label: ClilocId, value: String| {
        gump.html_localized_colored(153, y, 160, 18, label, LABEL_HUE, false, false);
        gump.html(
            280,
            y,
            75,
            18,
            format!("<div align=right>{value}</div>"),
            false,
            false,
        );
    };
    row(
        &mut gump,
        168,
        ClilocId(1_049_578),
        pool(hits.map(|h| (h.current, h.max))),
    );
    row(
        &mut gump,
        186,
        ClilocId(1_049_579),
        pool(stam.map(|s| (s.current, s.max))),
    );
    row(
        &mut gump,
        204,
        ClilocId(1_049_580),
        pool(mana.map(|m| (m.current, m.max))),
    );
    row(
        &mut gump,
        222,
        ClilocId(1_028_335),
        stat(stats.map(|s| s.strength)),
    );
    row(
        &mut gump,
        240,
        ClilocId(3_000_113),
        stat(stats.map(|s| s.dexterity)),
    );
    row(
        &mut gump,
        258,
        ClilocId(3_000_112),
        stat(stats.map(|s| s.intelligence)),
    );
    // Pre-AoS the fourth block is "Miscellaneous", and the armour rating is what
    // goes in it — the number this engine actually has.
    gump.image(128, 278, 2086);
    gump.html_localized_colored(147, 276, 160, 18, ClilocId(3_001_016), 200, false, false); // Miscellaneous
    row(&mut gump, 294, ClilocId(1_049_581), stat(Some(armour))); // Armor Rating
    row(&mut gump, 312, ClilocId(1_061_646), percent(physical)); // Physical
    gump.button(340, 358, 5601, 5605, GumpButton::Page, 2, ButtonId::UNUSED);

    gump.page(2);
    gump.image(128, 152, 2086);
    gump.html_localized_colored(147, 150, 160, 18, ClilocId(3_001_030), 200, false, false); // Combat Ratings
    let combat = [
        (168, ClilocId(1_044_103), Skill::Wrestling),
        (186, ClilocId(1_044_087), Skill::Tactics),
        (204, ClilocId(1_044_086), Skill::MagicResist),
        (222, ClilocId(1_044_061), Skill::Anatomy),
        (240, ClilocId(1_044_090), Skill::Poisoning),
        (276, ClilocId(1_044_085), Skill::Magery),
        (294, ClilocId(1_044_076), Skill::EvalInt),
        (312, ClilocId(1_044_106), Skill::Meditation),
    ];
    for (y, label, skill) in combat {
        let value = skills.as_ref().map_or(0, |s| s.get(skill));
        row(&mut gump, y, label, tenths(value));
    }
    gump.image(128, 260, 2086);
    gump.html_localized_colored(147, 258, 160, 18, ClilocId(3_001_032), 200, false, false); // Lore & Knowledge
    // A tamed creature says what it was last told, which is the one thing this
    // window can say about loyalty that is true.
    if let Some(order) = loyalty {
        gump.html(
            153,
            330,
            200,
            18,
            format!("<div align=right>{order:?}</div>"),
            false,
            false,
        );
    }
    gump.button(317, 358, 5603, 5607, GumpButton::Page, 1, ButtonId::UNUSED);

    let (layout, lines) = gump.finish();
    let packet = ServerPacket::GumpDisplay(GumpDisplay {
        // Keyed on the dialog's own id rather than on a mobile: the window is
        // read-only and answers nothing, so the key only has to be a number the
        // client can hang the window on — which is exactly what a `GumpKey` is.
        serial:  GumpKey(ANIMAL_LORE_GUMP.0),
        gump_id: ANIMAL_LORE_GUMP,
        at:      GumpPoint::new(250, 50),
        layout:  layout.to_owned(),
        lines:   lines.to_vec(),
    });
    state.send_packet(connection, &packet);
}

/// The hue ServUO draws a label in.
const LABEL_HUE: u32 = 0x24E5;

/// A pool as "current/max", or the dashes ServUO shows for one that is not there.
fn pool(value: Option<(u16, u16)>) -> String {
    match value {
        Some((current, max)) if max > 0 => format!("{current}/{max}"),
        _ => "---".to_owned(),
    }
}

/// A whole-number stat, or dashes.
fn stat(value: Option<u16>) -> String {
    match value {
        Some(value) if value > 0 => value.to_string(),
        _ => "---".to_owned(),
    }
}

/// A skill in tenths, drawn the way the client shows one.
fn tenths(value: u16) -> String {
    if value < 100 {
        return "---".to_owned();
    }
    format!("{}.{}", value / 10, value % 10)
}

/// A resistance as a percentage, or dashes for none.
fn percent(value: u8) -> String {
    if value == 0 {
        "---".to_owned()
    } else {
        format!("{value}%")
    }
}
