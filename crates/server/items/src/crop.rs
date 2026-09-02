//! Farmable crops: the cotton standing in a field, and the pick that takes it.
//!
//! ServUO's `FarmableCrop` — an immovable plant on the ground whose double-click
//! swaps it for a bare furrow, drops what it grew at its own feet and unlinks it
//! from the spawner that will regrow it. The field itself is the world's
//! (`world::crops`); this module is the plant.
//!
//! **The head of the cloth chain.** [`spin`](crate::spin) turns cotton into
//! thread, [`weave`](crate::weave) turns thread into a bolt and [`cut`](crate::cut)
//! turns the bolt into the cloth fifty-six tailoring rows eat — and until this,
//! the cotton the whole chain hangs off reached a player from a vendor's shelf
//! and nowhere else.
//!
//! No skill, no roll and no tool: picking is an item action, the shape the rest
//! of this crate's chain steps already have.

use openshard_state::components::{
    Crop,
    CropKind,
    Decays,
};

use super::*;

/// How long a picked stub stays before it is taken away — ServUO's
/// `Timer.DelayCall(TimeSpan.FromMinutes(5.0), Delete)`. The field does not wait
/// for it: the plant stops counting the moment it is picked (ServUO's `Unlink`),
/// so a harvested furrow is scenery rather than a held slot.
const WITHER_TICKS: u64 = 5 * 60 * TICKS_PER_SECOND;

/// "I can't reach that."
const CANNOT_REACH: ClilocId = ClilocId(1_019_045);

/// Put a standing plant of this crop on a tile, drawn as one of the crop's arts.
///
/// The art is drawn from the world's seeded rng, so a field is not rows of
/// identical bushes and a replay still grows the same one — ServUO picks the art
/// in the constructor for the same cosmetic reason.
pub fn plant(state: &mut WorldState, kind: CropKind, at: Point, facet: Facet) -> Option<EntityId> {
    let arts = kind.standing_arts();
    let art = arts[state.rng.below(arts.len() as u32) as usize];
    let plant = spawn_item(state, art, Hue(0), 1, false, at, facet)?;
    // A crop stands until somebody picks it. `spawn_item` gives every loose
    // ground item the ordinary rot clock, and under it a field would quietly
    // replant itself every twenty minutes whether or not anyone had been near —
    // the plant's own five-minute stub is the only clock it wants.
    state.registry.remove::<Decays>(plant);
    state.registry.insert(plant, Crop::Standing(kind));
    Some(plant)
}

/// Pick a double-clicked plant. Returns whether the item was a crop at all, so
/// the dispatch can fall through to everything else it knows.
///
/// ServUO's `OnDoubleClick` → `OnPicked`: the plant becomes the picked art, what
/// it grew lands on its tile, and the stub waits out its five minutes. Picking a
/// stub is silently nothing, which is what its `m_Picked` guard does.
pub fn pick(state: &mut WorldState, picker: EntityId, plant: EntityId) -> bool {
    let Some(crop) = state.registry.get::<Crop>(plant).copied() else {
        return false;
    };
    let Crop::Standing(kind) = crop else {
        return true;
    };
    // ServUO asks for `InRange(loc, 2)` and a line of sight; this is the reach
    // every other ground item in this crate is handled at, one tile wider and
    // without the ray, and a crop is not the place to invent a second rule.
    if !in_reach(state, plant, picker) {
        state.localized_message(picker, CANNOT_REACH, "");
        return true;
    }
    let (Some(&Position(at)), facet) = (state.registry.get::<Position>(plant), state.facet_of(plant)) else {
        return true;
    };
    // The stub before the yield, so a shard out of item serials leaves a picked
    // field rather than a plant that can be picked again for ever.
    redraw_item(state, plant, kind.picked_art());
    state.registry.insert(
        plant,
        Crop::Picked {
            withers: state.ticks + WITHER_TICKS,
        },
    );
    let (graphic, amount) = kind.yield_of();
    // On the ground at the plant's own feet, ServUO's `MoveToWorld(loc, map)`,
    // and decaying from there like anything else dropped: what a field pays is
    // an ordinary item the moment it exists.
    spawn_item(state, graphic, Hue(0), amount, true, at, facet);
    true
}

/// Take away every picked stub whose five minutes are up.
///
/// Beside [`advance_spins`](crate::advance_spins) in the tick and the same shape:
/// a scan over the handful of things mid-timer, rather than a queue of deadlines
/// that would be a second copy of `withers` to keep in step. Its own pass rather
/// than a [`Decays`] clock, because that one is an operator setting a shard can
/// turn off — and with decay off, a field of bare furrows would never grow back.
pub fn advance_crops(state: &mut WorldState) {
    let now = state.ticks;
    let withered: Vec<EntityId> = state
        .registry
        .query::<Crop>()
        .filter(|(_, crop)| matches!(crop, Crop::Picked { withers } if *withers <= now))
        .map(|(entity, _)| entity)
        .collect();
    for stub in withered {
        let Some(serial) = state.registry.serial_of(stub) else {
            continue;
        };
        remove_ground_item(state, stub, serial);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cotton_plant_is_drawn_as_one_of_its_four_arts() {
        // The picked furrow must not be among them, or a field would grow
        // plants that already look harvested.
        let arts = CropKind::Cotton.standing_arts();
        assert_eq!(arts.len(), 4);
        assert!(!arts.contains(&CropKind::Cotton.picked_art()));
    }

    #[test]
    fn cotton_is_picked_as_the_fibre_a_wheel_spins() {
        // The one place the chain could silently break: a yield the spinning
        // wheel does not recognise is a plant that pays a player in scenery.
        let (graphic, amount) = CropKind::Cotton.yield_of();
        assert_eq!(
            openshard_state::components::Fibre::from_graphic(graphic),
            Some(openshard_state::components::Fibre::Cotton)
        );
        assert_eq!(amount, 1);
    }
}
