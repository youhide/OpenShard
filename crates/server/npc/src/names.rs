//! A townsperson's name: a personal name and the title of its trade.
//!
//! # Two halves, from two places
//!
//! ServUO builds a vendor's label out of a `Name` drawn from `Data/names.xml`
//! (`NameList.RandomName("male")`) and a `Title` its class fixes — "the blacksmith",
//! "the banker" — and the client draws the two together. That split is why a town
//! reads as a town: everybody has a trade *and* a name.
//!
//! What the shard had instead was one string, and only the title was being sent,
//! so all thirty-eight bankers in Felucca were called "the banker". The title
//! comes with the spawn (it knows the profession); the personal name is generated
//! here off the world's seeded [`Rng`], so a shard names the same town twice.
//!
//! # This is the list, and it is deliberately not the whole of ServUO's
//!
//! `Data/names.xml` holds 1,500 male and 2,132 female names. Those belong to the
//! operator's own ServUO checkout, not in this repository — the same reason no
//! client files are here. So `data/names.json` carries a spread of them wide
//! enough that a full Felucca does not read as repetitive.
//!
//! It used to say "and a pack that wants the whole list overrides this one",
//! naming a `speech::registered_name` as what would serve it. **That function was
//! never written.** The script pack did register its 3,632 names, `Dialogue` did
//! store them, and nothing ever read them — every townsperson has been named from
//! this file the entire time. The dead half of `Dialogue` is gone and this is the
//! one place names come from; a shard that wants ServUO's full lists edits
//! `data/names.json`.

use openshard_state::rng::Rng;

// The two lists are `data/names.json`, eight names to a line; `build.rs` emits
// them as `const`s before this crate compiles. The order there is the order the
// roll indexes into, so a shard names the same town twice.
include!(concat!(env!("OUT_DIR"), "/names.rs"));

/// A personal name for a townsperson, from `data/names.json`.
///
/// The gender picks the list, as `BaseVendor.InitBody` does.
#[must_use]
pub fn personal_name(rng: &mut Rng, female: bool) -> &'static str {
    let list = if female { FEMALE_NAMES } else { MALE_NAMES };
    list[rng.below(list.len() as u32) as usize]
}

/// A townsperson's full label: a personal name and its trade, e.g.
/// "Rowena the blacksmith".
///
/// `title` is what the pack sends — already in ServUO's form, with the leading
/// "the". An empty title gives the bare name, so a nameless-trade NPC still reads.
#[must_use]
pub fn townsperson_name(rng: &mut Rng, title: &str, female: bool) -> String {
    let name = personal_name(rng, female);
    let title = title.trim();
    if title.is_empty() {
        name.to_owned()
    } else {
        format!("{name} {title}")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn a_full_name_carries_the_trade_after_the_person() {
        // The order matters: the client draws the label as one string, and
        // "the blacksmith Rowena" is not how UO reads.
        let mut rng = Rng::new(0x51ED);
        let name = townsperson_name(&mut rng, "the blacksmith", false);
        assert!(name.ends_with(" the blacksmith"), "{name}");
        assert!(!name.starts_with("the "), "{name}");
    }

    #[test]
    fn the_same_seed_names_the_same_townsperson() {
        let mut a = Rng::new(9);
        let mut b = Rng::new(9);
        assert_eq!(
            townsperson_name(&mut a, "the banker", true),
            townsperson_name(&mut b, "the banker", true)
        );
    }

    #[test]
    fn a_tradeless_npc_keeps_a_bare_name() {
        let mut rng = Rng::new(3);
        let name = townsperson_name(&mut rng, "  ", false);
        assert!(!name.contains(' '), "{name}");
        assert!(!name.is_empty());
    }

    #[test]
    fn the_lists_are_wide_enough_for_a_whole_facet() {
        // 738 townsfolk are placed at once. A list of twenty — which is what this
        // replaced — puts the same six names on every street in Britain.
        assert!(MALE_NAMES.len() >= 100, "{}", MALE_NAMES.len());
        assert!(FEMALE_NAMES.len() >= 100, "{}", FEMALE_NAMES.len());
        assert_eq!(
            MALE_NAMES.len(),
            MALE_NAMES.iter().collect::<HashSet<_>>().len(),
            "a duplicate name wastes a slot in the roll"
        );
        assert_eq!(
            FEMALE_NAMES.len(),
            FEMALE_NAMES.iter().collect::<HashSet<_>>().len()
        );
        assert!(
            MALE_NAMES
                .iter()
                .chain(FEMALE_NAMES)
                .all(|n| !n.is_empty() && !n.contains(' ')),
            "a personal name is one word, or the title runs into it"
        );
    }

    #[test]
    fn the_two_lists_do_not_name_the_same_person() {
        // Not a correctness rule, a variety one: an overlap means a hue-and-body
        // female NPC can be called a name every male NPC also uses.
        let men: HashSet<_> = MALE_NAMES.iter().collect();
        let overlap = FEMALE_NAMES.iter().filter(|n| men.contains(n)).count();
        assert_eq!(overlap, 0, "{overlap} names appear on both lists");
    }
}
