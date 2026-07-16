//! Facing, and the eight directions a mobile can step in.

use std::fmt;

/// Which way a mobile faces, and steps.
///
/// Zero is north and the values run clockwise. That is the client's own
/// numbering, not a choice: it appears raw in the `0x02` walk request and in
/// every packet carrying a facing.
///
/// # The names are tile-space, not screen-space
///
/// UO draws its map rotated 45°, so what a player calls "north" is up-and-left
/// on the tile grid. These names follow the *tiles*, matching Sphere's
/// `sm_Moves` and the client's own numbering: [`Direction::North`] is
/// `(0, -1)`, a pure `-y` step, and it renders as a diagonal.
///
/// So the four "cardinal" directions here are the ones that look diagonal, and
/// the four "diagonals" are the ones that look straight. Reasoning from what
/// the screen shows is how the signs come out wrong; use [`Direction::step`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Direction {
    /// 0
    North,
    /// 1
    NorthEast,
    /// 2
    East,
    /// 3
    SouthEast,
    /// 4
    South,
    /// 5
    SouthWest,
    /// 6
    West,
    /// 7
    NorthWest,
}

/// The bit the client sets in a direction byte to mean "running".
///
/// The direction and the run flag share one byte, which is why every packet
/// carrying a facing needs [`Facing`] rather than a bare [`Direction`].
pub const RUNNING_BIT: u8 = 0x80;

/// The mask that recovers a direction from a direction byte.
const DIRECTION_MASK: u8 = 0x07;

impl Direction {
    /// Every direction, clockwise from north.
    pub const ALL: [Direction; 8] = [
        Direction::North,
        Direction::NorthEast,
        Direction::East,
        Direction::SouthEast,
        Direction::South,
        Direction::SouthWest,
        Direction::West,
        Direction::NorthWest,
    ];

    /// Read a direction from its wire value, ignoring the running bit.
    ///
    /// Total: the low three bits can only be 0 to 7, so every byte the client
    /// can send names a direction. The high bits are not ours to interpret —
    /// `0x80` is running and the rest are unused, so masking is the whole of it.
    pub const fn from_bits(bits: u8) -> Self {
        match bits & DIRECTION_MASK {
            0 => Self::North,
            1 => Self::NorthEast,
            2 => Self::East,
            3 => Self::SouthEast,
            4 => Self::South,
            5 => Self::SouthWest,
            6 => Self::West,
            _ => Self::NorthWest,
        }
    }

    /// The wire value, without the running bit.
    pub const fn to_bits(self) -> u8 {
        self as u8
    }

    /// How x and y change taking one step this way.
    ///
    /// Verbatim from Sphere's `sm_Moves` in `common/CPointBase.cpp`.
    pub const fn step(self) -> (i32, i32) {
        match self {
            Self::North => (0, -1),
            Self::NorthEast => (1, -1),
            Self::East => (1, 0),
            Self::SouthEast => (1, 1),
            Self::South => (0, 1),
            Self::SouthWest => (-1, 1),
            Self::West => (-1, 0),
            Self::NorthWest => (-1, -1),
        }
    }

    /// Whether a step this way moves on both axes at once.
    ///
    /// Diagonals cost the same time as cardinals in UO, unlike most games, so
    /// this is not about movement cost — it is about what a step has to be able
    /// to squeeze past.
    pub const fn is_diagonal(self) -> bool {
        let (x, y) = self.step();
        x != 0 && y != 0
    }

    /// The direction 180° away.
    pub const fn opposite(self) -> Self {
        Self::from_bits(self.to_bits().wrapping_add(4))
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::North => "N",
            Self::NorthEast => "NE",
            Self::East => "E",
            Self::SouthEast => "SE",
            Self::South => "S",
            Self::SouthWest => "SW",
            Self::West => "W",
            Self::NorthWest => "NW",
        };
        f.write_str(name)
    }
}

/// A direction plus whether the mobile is running.
///
/// One byte on the wire. Splitting them into separate fields here means no
/// packet encoder has to remember to mask, and none can accidentally send a
/// facing of `0x84` where it meant "south, running".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Facing {
    /// Which way.
    pub direction: Direction,
    /// Whether the mobile is running rather than walking.
    pub running: bool,
}

impl Facing {
    /// A walking facing.
    pub const fn walking(direction: Direction) -> Self {
        Self {
            direction,
            running: false,
        }
    }

    /// A running facing.
    pub const fn running(direction: Direction) -> Self {
        Self {
            direction,
            running: true,
        }
    }

    /// Read a facing from its wire byte.
    pub const fn from_bits(bits: u8) -> Self {
        Self {
            direction: Direction::from_bits(bits),
            running: bits & RUNNING_BIT != 0,
        }
    }

    /// The wire byte.
    pub const fn to_bits(self) -> u8 {
        let bits = self.direction.to_bits();
        if self.running {
            bits | RUNNING_BIT
        } else {
            bits
        }
    }
}

impl fmt::Display for Facing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.running {
            write!(f, "{} running", self.direction)
        } else {
            write!(f, "{}", self.direction)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_are_the_clients_own() {
        // These numbers appear raw in 0x02 and every facing field. They are not
        // ours to renumber.
        assert_eq!(Direction::North.to_bits(), 0);
        assert_eq!(Direction::East.to_bits(), 2);
        assert_eq!(Direction::South.to_bits(), 4);
        assert_eq!(Direction::West.to_bits(), 6);
        assert_eq!(Direction::NorthWest.to_bits(), 7);
    }

    #[test]
    fn every_byte_the_client_can_send_names_a_direction() {
        // No validation needed and none possible: three bits, eight values.
        for bits in 0..=u8::MAX {
            let direction = Direction::from_bits(bits);
            assert_eq!(
                direction.to_bits(),
                bits & DIRECTION_MASK,
                "0x{bits:02X} round-trips through the mask"
            );
        }
    }

    #[test]
    fn the_running_bit_does_not_disturb_the_direction() {
        for direction in Direction::ALL {
            let running = Facing::running(direction);
            assert_eq!(running.to_bits(), direction.to_bits() | RUNNING_BIT);
            assert_eq!(Facing::from_bits(running.to_bits()), running);

            let walking = Facing::walking(direction);
            assert_eq!(walking.to_bits(), direction.to_bits());
            assert_eq!(Facing::from_bits(walking.to_bits()), walking);
        }
    }

    #[test]
    fn a_facing_round_trips_from_any_byte() {
        for bits in 0..=u8::MAX {
            let facing = Facing::from_bits(bits);
            // Only the low three bits and 0x80 mean anything; the rest are
            // dropped, which is what the client does too.
            assert_eq!(facing.to_bits(), bits & (DIRECTION_MASK | RUNNING_BIT));
        }
    }

    #[test]
    fn steps_match_spheres_table_verbatim() {
        // sm_Moves in common/CPointBase.cpp. Copied, not derived: the whole
        // table is eight lines and the signs are exactly what a fresh
        // derivation gets wrong.
        assert_eq!(Direction::North.step(), (0, -1));
        assert_eq!(Direction::NorthEast.step(), (1, -1));
        assert_eq!(Direction::East.step(), (1, 0));
        assert_eq!(Direction::SouthEast.step(), (1, 1));
        assert_eq!(Direction::South.step(), (0, 1));
        assert_eq!(Direction::SouthWest.step(), (-1, 1));
        assert_eq!(Direction::West.step(), (-1, 0));
        assert_eq!(Direction::NorthWest.step(), (-1, -1));
    }

    #[test]
    fn every_step_is_exactly_one_tile() {
        for direction in Direction::ALL {
            let (x, y) = direction.step();
            assert!((-1..=1).contains(&x), "{direction} moves {x} on x");
            assert!((-1..=1).contains(&y), "{direction} moves {y} on y");
            assert_ne!((x, y), (0, 0), "{direction} must move somewhere");
        }
    }

    #[test]
    fn every_step_is_distinct() {
        // Two directions with the same vector would mean one of them is a typo.
        let mut steps: Vec<(i32, i32)> = Direction::ALL.iter().map(|d| d.step()).collect();
        steps.sort_unstable();
        steps.dedup();
        assert_eq!(steps.len(), 8, "two directions share a step vector");
    }

    #[test]
    fn opposites_cancel_out() {
        for direction in Direction::ALL {
            let (x, y) = direction.step();
            let (ox, oy) = direction.opposite().step();
            assert_eq!((x + ox, y + oy), (0, 0), "{direction} and its opposite");
            assert_eq!(direction.opposite().opposite(), direction);
        }
    }

    #[test]
    fn four_directions_are_diagonal() {
        let diagonal = Direction::ALL.iter().filter(|d| d.is_diagonal()).count();
        assert_eq!(diagonal, 4);
        assert!(Direction::NorthEast.is_diagonal());
        assert!(!Direction::North.is_diagonal());
    }

    #[test]
    fn all_lists_each_direction_once_in_wire_order() {
        for (index, direction) in Direction::ALL.iter().enumerate() {
            assert_eq!(direction.to_bits() as usize, index);
        }
    }
}
