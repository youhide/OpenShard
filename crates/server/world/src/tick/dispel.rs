//! The three dispels: unmaking what magic made.
//!
//! Dispel and Mass Dispel ask one question — is this thing
//! [`Summoned`](openshard_state::components::Summoned) — and roll
//! [`magic::dispel_chance`] against it. Dispel Field asks the same question of a
//! *tile*, where the marker is the [`Field`] the spell that laid it left, or the
//! [`Moongate`] a Gate Travel did.
//!
//! Nothing here has a lifetime or a component of its own: a dispel is the one kind
//! of spell that only ever *removes*, which is why it needed the summon slice to
//! land first and needs nothing after it. Every exit is the one the thing already
//! had — [`npc::unsummon`]'s puff, [`World::remove_field`], [`World::close_gate`] —
//! so a dispelled creature and one whose time ran out leave the world identically,
//! and neither path can grow a step the other forgets.
//!
//! # Where the art is
//!
//! The three rows are [`SpellArt::Silent`](magic::SpellArt::Silent), and the
//! pictures are here, because a dispel has *two* outcomes and the table can carry
//! one: ServUO plays the summon's own dispel effect on the creature that goes and
//! `FixedEffect(0x3779)` on the one that holds. The same reason Mark is silent in
//! the table — a refused dispel should be as quiet as a refused mark.

use openshard_state::components::{
    Field,
    Moongate,
    SummonKind,
    Summoned,
};

use super::*;

/// "That cannot be dispelled." — ServUO's answer for a target that is not a summon
/// and not a magical field (cliloc 1005049), sent by all three spells.
const CANNOT_BE_DISPELLED: openshard_protocol::wire::ClilocId = openshard_protocol::wire::ClilocId(1_005_049);

/// "The creature resisted the attempt to dispel it!" — cliloc 1010084, the only
/// sign the caster gets that the roll happened at all.
const CREATURE_RESISTED: openshard_protocol::wire::ClilocId = openshard_protocol::wire::ClilocId(1_010_084);

/// What a creature that held against a dispel flashes — ServUO's
/// `m.FixedEffect(0x3779, 10, 20)`, the same sparkle a curse lands with.
const RESISTED_GRAPHIC: Graphic = Graphic(0x3779);

/// The sparkle a dispelled field leaves behind, and the pop that goes with it —
/// `SendLocationParticles(.., 0x376A, ..)` and `PlaySound(.., 0x201)`, the sound
/// being the one a dispelled summon makes too.
const FIELD_DISPEL_GRAPHIC: Graphic = Graphic(0x376A);
const DISPEL_SOUND: openshard_protocol::wire::SoundId = openshard_protocol::wire::SoundId(0x0201);

impl World {
    /// Dispel one aimed creature.
    ///
    /// Anything that was not summoned is refused outright and costs the caster the
    /// cast but nothing else — ServUO's `IsDispellable`, which is `Summoned` and, in
    /// a shard with necromancy, not an animated corpse. There are no animated dead
    /// here, so the marker is the whole test.
    pub(super) fn dispel_creature(&mut self, caster: EntityId, target: Serial) {
        let Some(creature) = self.state.registry.entity_of(target) else {
            return; // the cursor outlived what it was pointed at
        };
        let Some(&Summoned { kind, .. }) = self.state.registry.get::<Summoned>(creature) else {
            self.state.localized_message(caster, CANNOT_BE_DISPELLED, "");
            return;
        };
        self.roll_dispel(caster, creature, kind);
    }

    /// Dispel every summon standing within [`magic::MASS_DISPEL_RANGE`] of the aimed
    /// spot, each rolling its own chance.
    ///
    /// Each roll is separate, as ServUO's loop makes it: a caster who clears four
    /// elementals with one cast has beaten four curves, not one. Nothing that is not
    /// a summon is touched or even told — the seventh circle's answer to a field full
    /// of somebody else's creatures is not an area attack.
    pub(super) fn mass_dispel(&mut self, caster: EntityId, centre: Point) {
        let facet = self.state.facet_of(caster);
        let caught: Vec<(EntityId, SummonKind)> = self
            .state
            .facet_state(facet)
            .sectors()
            .mobiles_near(centre, magic::MASS_DISPEL_RANGE)
            .filter_map(|(entity, _)| {
                self.state
                    .registry
                    .get::<Summoned>(entity)
                    .map(|summon| (entity, summon.kind))
            })
            .collect();
        for (creature, kind) in caught {
            self.roll_dispel(caster, creature, kind);
        }
    }

    /// Roll one creature's dispel and act on it: the puff and gone, or the flash and
    /// a line to the caster.
    ///
    /// The success side is [`npc::unsummon`] and not a delete of its own, so that an
    /// expiry, a death and a dispel are one exit with one picture. ServUO says the
    /// same thing twice — `BaseCreature.Dispel` and `DispelSpell` play the identical
    /// `0x3728`/`0x201` — and here it is said once.
    fn roll_dispel(&mut self, caster: EntityId, creature: EntityId, kind: SummonKind) {
        if magic::check_dispelled(&mut self.state, caster, kind) {
            npc::unsummon(&mut self.state, creature);
        } else {
            self.location_effect(creature, RESISTED_GRAPHIC);
            self.state.localized_message(caster, CREATURE_RESISTED, "");
        }
    }

    /// Dispel the aimed field tile, or the gate a Gate Travel laid.
    ///
    /// One tile, not the whole row: each tile of a field is its own item here as it
    /// is in the reference, and `DispelFieldSpell` deletes the one that was clicked.
    /// A five-tile fire field therefore wants five casts to clear, which is what the
    /// spell has always cost.
    ///
    /// A gate goes the same way, because ServUO's `[DispellableField]` sits on
    /// `Moongate` and the spell's gates are Moongates — while the nine city ones are
    /// plain items that never were. Here that distinction needs no flag either: a
    /// spell's gate carries the [`Moongate`] component and a city gate carries
    /// nothing, being derived from where it stands. **Only the aimed end closes**,
    /// as in the reference, which leaves the far end a one-way door for the rest of
    /// its half-minute — the pair has no link field to follow, by design.
    pub(super) fn dispel_field(&mut self, caster: EntityId, target: Option<Serial>) {
        let Some(entity) = target.and_then(|serial| self.state.registry.entity_of(serial)) else {
            self.state.localized_message(caster, CANNOT_BE_DISPELLED, "");
            return;
        };
        if !self.state.registry.has::<Field>(entity) && !self.state.registry.has::<Moongate>(entity) {
            self.state.localized_message(caster, CANNOT_BE_DISPELLED, "");
            return;
        }
        // Voiced before it goes, so the sparkle has a tile to stand on and the sound
        // a position to come from — both read the entity's own `Position`.
        self.location_effect(entity, FIELD_DISPEL_GRAPHIC);
        self.state.play_sound(entity, DISPEL_SOUND);
        if self.state.registry.has::<Field>(entity) {
            self.remove_field(entity);
        } else {
            self.close_gate(entity);
        }
    }
}
