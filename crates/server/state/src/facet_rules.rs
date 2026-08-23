//! What a facet *allows*, as against where it is and what it is made of.
//!
//! A [`Facet`] in this engine is an id and nothing else — the key a map is filed
//! under, the byte a client is told at login. ServUO's `Map` carries a
//! `MapRules` beside its id (`Server/Map.cs:121`), and the two shipped
//! combinations are the Trammel/Felucca split every player knows by feel:
//! `TrammelRules` is `FreeMovement | BeneficialRestrictions |
//! HarmfulRestrictions`, and `FeluccaRules` is `None`.
//!
//! # Only the flag that has a reader
//!
//! `MapRules` has four flags and this type has one. The other three are named
//! here rather than built, because a flag nothing asks about is a flag nothing
//! keeps honest:
//!
//! - `Internal` marks ServUO's holding pen for dragged items and commodity
//!   deeds. This engine has no such map — an item being dragged is
//!   [`crate::runtime::HeldItem`], which is a component and not a place.
//! - `BeneficialRestrictions` and `HarmfulRestrictions` are the same question
//!   asked about a *spell* rather than a step: whether a heal may land on a
//!   murderer, whether a fireball may land on an innocent. They belong here when
//!   the spell path asks, and the field they will be is the reason this is a
//!   struct rather than a bare `bool` on [`crate::runtime::FacetState`].

use openshard_protocol::world::Facet;

/// The rules that hold on one facet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FacetRules {
    /// Whether bodies walk through each other here, freely and for nothing.
    ///
    /// ServUO's `MapRules.FreeMovement`, and the first branch of
    /// `Mobile.CheckShove` (`Server/Mobile.cs:3518`): with it set the whole
    /// shove rule does not run — no stamina, no message, no refusal, and no
    /// reveal. Off, a body in the way is a body in the way, and getting past one
    /// costs ten stamina.
    ///
    /// It is not "may I walk through walls". A facet with free movement still
    /// has ground, doors and houses; what it stops having is *people* as
    /// obstacles.
    pub free_movement: bool,
}

impl FacetRules {
    /// The rules the retail shard ran on the facet numbered `facet`.
    ///
    /// **A default that matches what the client already believes**, which is the
    /// whole reason it is derived from a number this crate otherwise treats as
    /// opaque. ClassicUO decides the same question with `_world.Map.Index == 0`
    /// — hardcoded, on the client's side of the wire, where this shard cannot
    /// reach it. A shard that gives facet 0 free movement is a shard whose
    /// players walk through each other on screen and are snapped back by the
    /// server a moment later; a shard that refuses it on facet 1 draws the
    /// mirror of that. Either way the disagreement is felt as a stutter and read
    /// as a bug in the walk.
    ///
    /// So this is a *statement about the client*, not an inference about the
    /// map, and it is the only thing in this engine that reads meaning into a
    /// facet number. An operator who loads Felucca into slot three is free to
    /// say so — see `world.free_movement` in the config — and is then also
    /// choosing the stutter, knowingly.
    #[must_use]
    pub const fn classic(facet: Facet) -> Self {
        Self {
            // `FeluccaRules = None` against `TrammelRules`, which carries the
            // flag. Every facet after the first is a Trammel-ruleset facet in
            // retail — Ilshenar, Malas, Tokuno and Ter Mur all have it.
            free_movement: facet.0 != FELUCCA.0,
        }
    }
}

/// The one facet whose number means something, and it means it to the *client*.
///
/// Named so [`FacetRules::classic`] does not spell a bare `0` in the middle of
/// the one comparison that is not arithmetic.
pub const FELUCCA: Facet = Facet(0);

#[cfg(test)]
mod tests {
    use super::*;

    /// The split, at the boundary the client hardcodes. Both sides are asserted
    /// because "everything but the first" is the half a lazy reading gets
    /// backwards.
    #[test]
    fn felucca_is_the_facet_where_a_body_is_in_the_way() {
        assert!(
            !FacetRules::classic(FELUCCA).free_movement,
            "facet 0 is Felucca, and a body there is an obstacle"
        );
        for facet in 1..=5 {
            assert!(
                FacetRules::classic(Facet(facet)).free_movement,
                "facet {facet} runs Trammel rules, where nobody is in anybody's way"
            );
        }
    }
}
