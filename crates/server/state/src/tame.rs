//! What a creature takes to tame — the core's table, keyed by body.
//!
//! ServUO's `BaseCreature.MinTameSkill`/`ControlSlots`, which are per-class
//! constants, so they live here keyed by body id in the shape
//! [`creature_name`](crate::components::creature_name) and [`crate::weapon`]
//! established. A spawn may override them (the pack's own beast), and a body with
//! no row simply cannot be tamed, which is the right default: everything in
//! Britannia that is not on this list is either a person or a monster.
//!
//! **Every rideable body is tamable**, and that half is derived rather than written
//! twice: [`openshard_protocol::mounts::mount_item_for`] already knows the thirty mounts, and
//! a horse you cannot tame is a horse nobody can have. What the table below adds is
//! the creatures that are tamable and *not* rideable — the bears, the birds, the
//! pack animals — plus a taming difficulty for the mounts, which the mount table has
//! no column for.

use openshard_protocol::wire::Graphic;
use openshard_protocol::world::FollowerSlots;

use crate::components::Tamable;

/// One tamable kind: the body, the skill it takes, and the slots it fills.
struct TameData {
    body:      Graphic,
    min_skill: u16,
    slots:     FollowerSlots,
}

/// The default a rideable body takes to tame, in tenths — a horse's 29.1, which is
/// what most of the mount list is in ServUO.
const MOUNT_MIN_SKILL: u16 = 291;

/// What it takes to tame `body`, or `None` for a creature nobody tames.
#[must_use]
pub fn tamable(body: Graphic) -> Option<Tamable> {
    if let Some(row) = TAMABLE.iter().find(|row| row.body == body) {
        return Some(Tamable {
            min_skill: row.min_skill,
            slots:     row.slots,
        });
    }
    // Anything you can ride, you can tame — asked of the mount table by body, which
    // is the direction `mount_item_for` answers.
    openshard_protocol::mounts::mount_item_for(body).map(|_| {
        Tamable {
            min_skill: MOUNT_MIN_SKILL,
            slots:     FollowerSlots::ONE,
        }
    })
}

/// The classic tamables that are not mounts, from ServUO's `Scripts/Mobiles` —
/// each row's `Body`, `MinTameSkill` (in tenths) and `ControlSlots`.
///
/// Deliberately short: a body with no row cannot be tamed, and a wrong row is worse
/// than a missing one — it puts a dragon on a leash for 29 skill. The converter
/// scraping the rest per creature is the Community-Pack follow-up, the same shape
/// the creature `SetSkill` pass has.
#[rustfmt::skip]
static TAMABLE: &[TameData] = &[
    t(0x0005, 171, 1), // Eagle
    t(0x0006,   0, 1), // Bird
    t(0x00A7, 411, 1), // Brown bear
    t(0x00C9,   0, 1), // Cat
    t(0x00CB, 111, 1), // Pig
    t(0x00CD,   0, 1), // Jack rabbit
    t(0x00CF, 111, 1), // Sheep
    t(0x00D1, 111, 1), // Goat
    t(0x00D4, 591, 1), // Grizzly bear
    t(0x00D5, 351, 1), // Polar bear
    t(0x00D6, 531, 1), // Panther
    t(0x00D8, 111, 1), // Cow
    t(0x00D9,   0, 1), // Dog
    t(0x00DD, 351, 1), // Walrus
    t(0x00E1, 231, 1), // Timber wolf
    t(0x00E7, 111, 1), // Cow, the other colour
    t(0x00E8, 711, 1), // Bull
    t(0x00E9, 711, 1), // Bull, the other colour
    t(0x00EA, 591, 1), // Great hart
    t(0x00ED, 231, 1), // Hind
    t(0x0019, 531, 1), // Grey wolf
    t(0x0122, 291, 1), // Boar
    t(0x0123, 291, 1), // Pack horse
    t(0x0124, 291, 1), // Pack llama
];

/// A row, so the table reads as data.
const fn t(body: u16, min_skill: u16, slots: u8) -> TameData {
    TameData {
        body: Graphic(body),
        min_skill,
        slots: FollowerSlots::new(slots),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_rideable_is_tamable() {
        // Derived from the mount table rather than listed again: a horse you cannot
        // tame is a horse nobody can have, and two hand-kept halves of one mapping
        // is how a saved ride came back as the wrong animal once already.
        for body in [0x0074, 0x0075, 0x00C8, 0x00E2, 0x0114, 0x0317].map(Graphic) {
            assert!(
                tamable(body).is_some(),
                "0x{:04X} is rideable and must be tamable",
                body.0
            );
        }
    }

    #[test]
    fn a_monster_is_not_tamable() {
        // The default matters: a body with no row cannot be tamed at all, so the
        // table is a list of what *is* rather than a list of exceptions.
        assert!(tamable(Graphic(0x0190)).is_none(), "a person");
        assert!(tamable(Graphic(0x003B)).is_none(), "a dragon");
    }

    #[test]
    fn a_bird_is_easier_than_a_grizzly() {
        // Both numbers are ServUO's `MinTameSkill` in tenths, and the gap between
        // them is the whole shape of the skill: a tamer trains up through the woods.
        assert_eq!(tamable(Graphic(0x0006)).expect("a bird").min_skill, 0);
        assert_eq!(tamable(Graphic(0x00D4)).expect("a grizzly").min_skill, 591);
        assert_eq!(tamable(Graphic(0x00E8)).expect("a bull").min_skill, 711);
    }
}
