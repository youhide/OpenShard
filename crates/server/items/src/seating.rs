//! Walking onto the chairs the UO client itself recognises.
//!
//! A seated human is a normal mobile placed at a chair's `z`. Classic clients
//! look for that exact relationship and substitute their sit animation
//! themselves; no non-standard packet is involved. The catalogue below is the
//! public ClassicUO chair catalogue.

use super::*;

/// Record whether a mobile has just walked onto a supported chair.
///
/// Classic UO does not make chairs a double-click action: the client switches
/// to its seated pose when a human reaches the chair's own tile and height.
/// This tiny server-side marker reserves that occupied seat and lets the next
/// movement request leave it cleanly, but the movement itself stays ordinary.
pub fn occupy_chair(state: &mut WorldState, mobile: EntityId) {
    let Some(&Position(at)) = state.registry.get::<Position>(mobile) else {
        return;
    };
    // The client supplies this pose only for living human bodies. Do not give
    // a mount, ghost, or creature the movement special-case when it cannot be
    // drawn as seated in the first place.
    let human = state.registry.get::<Body>(mobile).is_some_and(|body| {
        openshard_state::components::body_type(body.id) == openshard_state::components::BodyType::Human
    });
    if !human || state.registry.has::<Ghost>(mobile) || state.registry.has::<Riding>(mobile) {
        state.registry.remove::<Seated>(mobile);
        return;
    }
    let facet = state.facet_of(mobile);
    let chair = state.registry.query::<Drawn>().find_map(|(entity, drawn)| {
        (entity != mobile
            && state.facet_of(entity) == facet
            && state
                .registry
                .get::<Position>(entity)
                .is_some_and(|position| position.0 == at)
            && chair_directions(drawn.id).is_some())
        .then_some(entity)
    });
    match chair {
        Some(chair) => {
            state.registry.insert(mobile, Seated { chair });
        }
        None => {
            state.registry.remove::<Seated>(mobile);
        }
    }
}

/// The four preferred facings (north, east, south, west) for a seat graphic.
///
/// This is deliberately graphic-based rather than name-based: localised client
/// tiledata names are not a stable game rule, while this is the exact artwork
/// the client uses to decide whether to draw a seated human.
fn chair_directions(graphic: Graphic) -> Option<[i8; 4]> {
    const CHAIRS: &[(u16, [i8; 4])] = &[
        (0x0459, [0, -1, 4, -1]),
        (0x045A, [-1, 2, -1, 6]),
        (0x045B, [0, -1, 4, -1]),
        (0x045C, [-1, 2, -1, 6]),
        (0x0A2A, [0, 2, 4, 6]),
        (0x0A2B, [0, 2, 4, 6]),
        (0x0B2C, [-1, 2, -1, 6]),
        (0x0B2D, [0, -1, 4, -1]),
        (0x0B2E, [4, 4, 4, 4]),
        (0x0B2F, [2, 2, 2, 2]),
        (0x0B30, [6, 6, 6, 6]),
        (0x0B31, [0, 0, 0, 0]),
        (0x0B32, [4, 4, 4, 4]),
        (0x0B33, [2, 2, 2, 2]),
        (0x0B4E, [2, 2, 2, 2]),
        (0x0B4F, [4, 4, 4, 4]),
        (0x0B50, [0, 0, 0, 0]),
        (0x0B51, [6, 6, 6, 6]),
        (0x0B52, [2, 2, 2, 2]),
        (0x0B53, [4, 4, 4, 4]),
        (0x0B54, [0, 0, 0, 0]),
        (0x0B55, [6, 6, 6, 6]),
        (0x0B56, [2, 2, 2, 2]),
        (0x0B57, [4, 4, 4, 4]),
        (0x0B58, [6, 6, 6, 6]),
        (0x0B59, [0, 0, 0, 0]),
        (0x0B5A, [2, 2, 2, 2]),
        (0x0B5B, [4, 4, 4, 4]),
        (0x0B5C, [0, 0, 0, 0]),
        (0x0B5D, [6, 6, 6, 6]),
        (0x0B5E, [0, 2, 4, 6]),
        (0x0B5F, [-1, 2, -1, 6]),
        (0x0B60, [-1, 2, -1, 6]),
        (0x0B61, [-1, 2, -1, 6]),
        (0x0B62, [-1, 2, -1, 6]),
        (0x0B63, [-1, 2, -1, 6]),
        (0x0B64, [-1, 2, -1, 6]),
        (0x0B65, [0, -1, 4, -1]),
        (0x0B66, [0, -1, 4, -1]),
        (0x0B67, [0, -1, 4, -1]),
        (0x0B68, [0, -1, 4, -1]),
        (0x0B69, [0, -1, 4, -1]),
        (0x0B6A, [0, -1, 4, -1]),
        (0x0B91, [4, 4, 4, 4]),
        (0x0B92, [4, 4, 4, 4]),
        (0x0B93, [2, 2, 2, 2]),
        (0x0B94, [2, 2, 2, 2]),
        (0x0CF3, [-1, 2, -1, 6]),
        (0x0CF4, [-1, 2, -1, 6]),
        (0x0CF6, [0, -1, 4, -1]),
        (0x0CF7, [0, -1, 4, -1]),
        (0x0E50, [4, 4, 4, 4]),
        (0x0E51, [4, 4, 4, 4]),
        (0x0E52, [2, 2, 2, 2]),
        (0x0E53, [2, 2, 2, 2]),
        (0x1049, [-1, 2, -1, 6]),
        (0x104A, [0, -1, 4, -1]),
        (0x11FC, [0, 2, 4, 6]),
        (0x1207, [0, -1, 4, -1]),
        (0x1208, [0, -1, 4, -1]),
        (0x1209, [0, -1, 4, -1]),
        (0x120A, [0, -1, 4, -1]),
        (0x120B, [0, -1, 4, -1]),
        (0x120C, [0, -1, 4, -1]),
        (0x1218, [4, 4, 4, 4]),
        (0x1219, [2, 2, 2, 2]),
        (0x121A, [0, 0, 0, 0]),
        (0x121B, [6, 6, 6, 6]),
        (0x1527, [2, 2, 2, 2]),
        (0x1771, [0, 2, 4, 6]),
        (0x1776, [0, 2, 4, 6]),
        (0x1779, [0, 2, 4, 6]),
        (0x1DC7, [-1, 2, -1, 6]),
        (0x1DC8, [-1, 2, -1, 6]),
        (0x1DC9, [-1, 2, -1, 6]),
        (0x1DCA, [0, -1, 4, -1]),
        (0x1DCB, [0, -1, 4, -1]),
        (0x1DCC, [0, -1, 4, -1]),
        (0x1DCD, [-1, 2, -1, 6]),
        (0x1DCE, [-1, 2, -1, 6]),
        (0x1DCF, [-1, 2, -1, 6]),
        (0x1DD0, [0, -1, 4, -1]),
        (0x1DD1, [0, -1, 4, -1]),
        (0x1DD2, [-1, 2, -1, 6]),
        (0x2A58, [4, 4, 4, 4]),
        (0x2A59, [2, 2, 2, 2]),
        (0x2A5A, [0, 2, 4, 6]),
        (0x2A5B, [0, 2, 4, 6]),
        (0x2A7F, [0, 2, 4, 6]),
        (0x2A80, [0, 2, 4, 6]),
        (0x2DDF, [0, 2, 4, 6]),
        (0x2DE0, [0, 2, 4, 6]),
        (0x2DE3, [2, 2, 2, 2]),
        (0x2DE4, [4, 4, 4, 4]),
        (0x2DE5, [6, 6, 6, 6]),
        (0x2DE6, [0, 0, 0, 0]),
        (0x2DEB, [0, 0, 0, 0]),
        (0x2DEC, [4, 4, 4, 4]),
        (0x2DED, [2, 2, 2, 2]),
        (0x2DEE, [6, 6, 6, 6]),
        (0x2DF5, [0, 2, 4, 6]),
        (0x2DF6, [0, 2, 4, 6]),
        (0x3088, [0, 2, 4, 6]),
        (0x3089, [0, 2, 4, 6]),
        (0x308A, [0, 2, 4, 6]),
        (0x308B, [0, 2, 4, 6]),
        (0x319A, [-1, 2, -1, 6]),
        (0x319B, [0, -1, 4, -1]),
        (0x35ED, [0, 2, 4, 6]),
        (0x35EE, [0, 2, 4, 6]),
        (0x3DFF, [0, -1, 4, -1]),
        (0x3E00, [-1, 2, -1, 6]),
        (0x4023, [4, 4, 4, 4]),
        (0x4024, [2, 2, 2, 2]),
        (0x4027, [4, 4, 4, 4]),
        (0x4028, [4, 4, 4, 4]),
        (0x4029, [2, 2, 2, 2]),
        (0x402A, [2, 2, 2, 2]),
        (0x4BDC, [4, 4, 4, 4]),
        (0x4C1B, [4, 4, 4, 4]),
        (0x4C1E, [2, 2, 2, 2]),
        (0x4C80, [4, 4, 4, 4]),
        (0x4C81, [2, 2, 2, 2]),
        (0x4C82, [4, 4, 4, 4]),
        (0x4C83, [4, 4, 4, 4]),
        (0x4C84, [2, 2, 2, 2]),
        (0x4C85, [2, 2, 2, 2]),
        (0x4C86, [4, 4, 4, 4]),
        (0x4C87, [4, 4, 4, 4]),
        (0x4C88, [2, 2, 2, 2]),
        (0x4C89, [2, 2, 2, 2]),
        (0x4C8A, [2, 2, 2, 2]),
        (0x4C8B, [2, 2, 2, 2]),
        (0x4C8C, [2, 2, 2, 2]),
        (0x4C8D, [4, 4, 4, 4]),
        (0x4C8E, [4, 4, 4, 4]),
        (0x4C8F, [4, 4, 4, 4]),
        (0x4DE0, [2, 2, 2, 2]),
        (0x63BC, [0, -1, 4, -1]),
        (0x63BD, [0, -1, 4, -1]),
        (0x63C3, [-1, 2, -1, 6]),
        (0x63C4, [-1, 2, -1, 6]),
        (0x996C, [4, 4, 4, 4]),
        (0x9977, [2, 2, 2, 2]),
        (0x9C57, [6, 6, 6, 6]),
        (0x9C58, [6, 6, 6, 6]),
        (0x9C59, [0, 0, 0, 0]),
        (0x9C5A, [0, 0, 0, 0]),
        (0x9C5D, [6, 6, 6, 6]),
        (0x9C5E, [6, 6, 6, 6]),
        (0x9C5F, [6, 6, 6, 6]),
        (0x9C60, [0, 0, 0, 0]),
        (0x9C61, [0, 0, 0, 0]),
        (0x9C62, [0, 0, 0, 0]),
        (0x9E8E, [0, 0, 0, 0]),
        (0x9E8F, [6, 6, 6, 6]),
        (0x9E90, [2, 2, 2, 2]),
        (0x9E91, [4, 4, 4, 4]),
        (0x9E9F, [0, 0, 0, 0]),
        (0x9EA0, [6, 6, 6, 6]),
        (0x9EA1, [4, 4, 4, 4]),
        (0x9EA2, [2, 2, 2, 2]),
        (0xA05C, [6, 6, 6, 6]),
        (0xA05D, [4, 4, 4, 4]),
        (0xA05E, [0, 0, 0, 0]),
        (0xA05F, [2, 2, 2, 2]),
        (0xA211, [0, 2, 4, 6]),
        (0xA4EA, [4, 4, 4, 4]),
        (0xA4EB, [2, 2, 2, 2]),
        (0xA586, [4, 4, 4, 4]),
        (0xA587, [2, 2, 2, 2]),
    ];
    CHAIRS
        .binary_search_by_key(&graphic.0, |(id, _)| *id)
        .ok()
        .map(|index| CHAIRS[index].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_chair_catalogue_includes_crafted_chairs_and_rejects_unrelated_items() {
        assert!(chair_directions(Graphic(0x0B57)).is_some());
        assert!(chair_directions(Graphic(0x1218)).is_some());
        assert!(chair_directions(Graphic(0x2DE3)).is_some());
        assert!(chair_directions(Graphic(0x0001)).is_none());
    }
}
