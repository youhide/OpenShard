//! Sitting on the chairs the UO client itself recognises.
//!
//! A seated human is a normal mobile placed at a chair's `z`, facing one of the
//! four cardinal directions.  Classic clients look for that exact relationship
//! and substitute their sit animation themselves; no non-standard packet is
//! involved.  The catalogue below is the public ClassicUO chair catalogue,
//! reduced to the four direction preferences the server needs.

use super::*;
use openshard_protocol::direction::{Direction, Facing};

/// Try to seat `player` on `chair`.
///
/// Returns `true` once the target is known to be a chair, including when the
/// request is refused.  That makes a chair a built-in interaction rather than
/// letting a generic item-use trigger reinterpret a failed attempt.
pub fn try_sit(state: &mut WorldState, player: EntityId, chair: EntityId) -> bool {
    let Some(&Drawn { id, .. }) = state.registry.get::<Drawn>(chair) else {
        return false;
    };
    let Some(directions) = chair_directions(id) else {
        return false;
    };
    let Some(&Position(chair_at)) = state.registry.get::<Position>(chair) else {
        return true;
    };
    let Some(&Position(player_at)) = state.registry.get::<Position>(player) else {
        return true;
    };

    // Mounted bodies have their saddle picture rather than the human pose the
    // client replaces, and ghosts have no sitting art.  More importantly,
    // neither should be teleported into furniture by a double-click.
    if state.registry.has::<Riding>(player) || state.registry.has::<Ghost>(player) {
        return true;
    }
    if state.facet_of(chair) != state.facet_of(player) || !in_reach(state, chair, player) {
        return true;
    }
    // A seat is a one-body surface.  Compare the actual landing point — chairs
    // on different storeys may share x/y but must not reserve one another.
    if state.mobile_occupies(state.facet_of(player), chair_at, player) {
        return true;
    }

    let facing = seated_facing(player_at, chair_at, directions);
    state.registry.insert(player, Heading(facing));
    state.registry.insert(player, Seated { chair });
    state.disrupt(player);
    if let Some(Movement(mut walker)) = state.registry.get::<Movement>(player).copied() {
        walker.facing = facing;
        state.registry.insert(player, Movement(walker));
    }
    // `move_to` updates Position, Walker, sector membership, the player's
    // camera and every watcher in one transaction.  A bare component write
    // makes the client retain its old location and is not a valid seat.
    state.move_to(player, state.facet_of(player), chair_at);
    true
}

/// Choose the cardinal direction the client uses to render a person on this
/// particular chair.  The approaching side breaks ties for chairs which can
/// face more than one way, mirroring ClassicUO's chair table logic.
fn seated_facing(from: Point, chair: Point, directions: [i8; 4]) -> Facing {
    let approach = openshard_movement::direction_toward(chair, from).unwrap_or(Direction::South);
    // ClassicUO resolves the eight possible approach directions this way before
    // selecting the two stored animation directions.  In particular, NE tries
    // east before north; treating every diagonal as its preceding cardinal
    // makes half the side entries face the wrong way.
    let (primary, alternate) = match approach {
        Direction::North => (directions[0], directions[1]),
        Direction::NorthWest => (directions[0], directions[3]),
        Direction::NorthEast => (directions[1], directions[0]),
        Direction::East => (directions[1], directions[2]),
        Direction::SouthEast => (directions[2], directions[1]),
        Direction::South => (directions[2], directions[3]),
        Direction::SouthWest => (directions[3], directions[2]),
        Direction::West => (directions[3], directions[0]),
    };
    // Every supported graphic has at least one real direction, but keep the
    // fallback total so a future catalogue row cannot turn `-1` into a wire
    // direction by accident.
    let direction = [primary, alternate]
        .into_iter()
        .find(|direction| *direction >= 0)
        .or_else(|| directions.into_iter().find(|direction| *direction >= 0))
        .unwrap_or(Direction::South.to_bits() as i8);
    Facing::walking(Direction::from_bits(direction as u8))
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
    fn classic_chair_catalogue_has_the_crafted_chairs_only() {
        assert!(chair_directions(Graphic(0x0B57)).is_some());
        assert!(chair_directions(Graphic(0x1218)).is_some());
        assert!(chair_directions(Graphic(0x2DE3)).is_some());
        assert!(chair_directions(Graphic(0x0001)).is_none());
    }

    #[test]
    fn approach_selects_the_chair_facing() {
        let chair = Point::new(10, 10, 0);
        assert_eq!(
            seated_facing(Point::new(10, 9, 0), chair, [0, 2, 4, 6]).direction,
            Direction::North
        );
        assert_eq!(
            seated_facing(Point::new(11, 10, 0), chair, [0, 2, 4, 6]).direction,
            Direction::East
        );
        assert_eq!(
            seated_facing(Point::new(11, 9, 0), chair, [-1, 2, -1, 6]).direction,
            Direction::East,
            "a north-east approach uses the east side before its north fallback"
        );
    }
}
