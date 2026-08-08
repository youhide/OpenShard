//! Which way the keyboard is asking to walk.
//!
//! # Why a key is held rather than pressed
//!
//! A step used to be sent from the key event itself, which made the *operating
//! system's* auto-repeat the walking speed: nothing for half a second, then
//! thirty steps a second. Neither is a pace a mobile has. The fast half is worse
//! than it looks — `WalkPace` refuses a sustained flood as a speedhack, so the
//! shard answers with a `0x21` and the body is pulled back to where it really
//! is, which reads as the walk stuttering rather than as the client asking for
//! too much.
//!
//! So the key is *state* and the clock is ours: a direction is held or it is
//! not, and [`crate::steer::Steering`] is what decides when the next step
//! leaves. This file answers one question — which way is the keyboard pointing
//! right now — and owns no timer, because the mouse asks for steps at the same
//! rate and two clocks would take two steps per beat.

use openshard_protocol::direction::{Direction, Facing};
use winit::keyboard::KeyCode;

/// The arrow keys currently down, and whether shift is.
#[derive(Clone, Debug, Default)]
pub struct Held {
    /// Every arrow currently down, in the order they went down.
    ///
    /// A stack and not a single direction: pressing right without letting go of
    /// up walks east, and letting go of right again walks north-west rather than
    /// stopping. The last one pressed is the one obeyed, which is what every
    /// game does with a keyboard that reports two keys.
    down: Vec<Direction>,
    /// Whether either shift is down. The whole of "run" on this keyboard.
    running: bool,
}

impl Held {
    /// The direction an arrow key means, or `None` for any other key.
    ///
    /// The four arrows are the four diagonals of the map, which are the four
    /// *straight* directions on screen: UO's grid is turned 45 degrees, so "up"
    /// is north-west and there is no key that moves one axis alone.
    pub fn direction_of(code: KeyCode) -> Option<Direction> {
        match code {
            KeyCode::ArrowUp => Some(Direction::NorthWest),
            KeyCode::ArrowDown => Some(Direction::SouthEast),
            KeyCode::ArrowLeft => Some(Direction::SouthWest),
            KeyCode::ArrowRight => Some(Direction::NorthEast),
            _ => None,
        }
    }

    /// An arrow went down. Answers whether the direction being obeyed changed.
    ///
    /// `false` is the operating system repeating a key that is already the one
    /// obeyed, and its repeat rate is not a walking speed — see the module docs.
    pub fn press(&mut self, direction: Direction) -> bool {
        if self.down.last() == Some(&direction) {
            return false;
        }
        self.down.retain(|held| *held != direction);
        self.down.push(direction);
        true
    }

    /// An arrow came up.
    ///
    /// Releasing a key nobody is holding is not an error — a window that lost
    /// focus mid-step gets the release and never got the press.
    pub fn release(&mut self, direction: Direction) {
        self.down.retain(|held| *held != direction);
    }

    /// Shift went down or came up.
    pub fn set_running(&mut self, running: bool) {
        self.running = running;
    }

    /// Whether shift is down, which is what the mouse's own steps read to decide
    /// their pace as well.
    pub const fn running(&self) -> bool {
        self.running
    }

    /// Everything is up.
    ///
    /// The window losing focus is a key release that never arrives, and without
    /// this the character walks into the wall behind whatever the player alt-tabbed
    /// to.
    pub fn clear(&mut self) {
        self.down.clear();
    }

    /// Which way the keyboard is asking to walk, if it is asking at all.
    pub fn asking(&self) -> Option<Facing> {
        let direction = *self.down.last()?;
        Some(match self.running {
            true => Facing::running(direction),
            false => Facing::walking(direction),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defect this module was written for: the OS repeats a held key at its
    /// own rate, and that rate is not a walking speed. The repeat changes
    /// nothing, so nothing upstream re-arms a clock from it.
    #[test]
    fn the_operating_systems_repeat_changes_nothing() {
        let mut held = Held::default();
        assert!(held.press(Direction::NorthWest));
        for _ in 1..30 {
            assert!(!held.press(Direction::NorthWest));
        }
    }

    #[test]
    fn the_last_key_pressed_is_the_one_obeyed_and_releasing_it_falls_back() {
        let mut held = Held::default();
        held.press(Direction::NorthWest);
        assert!(held.press(Direction::NorthEast));
        assert_eq!(held.asking(), Some(Facing::walking(Direction::NorthEast)));
        held.release(Direction::NorthEast);
        assert_eq!(
            held.asking(),
            Some(Facing::walking(Direction::NorthWest)),
            "the key still down keeps walking"
        );
    }

    #[test]
    fn shift_is_the_running_flag() {
        let mut held = Held::default();
        held.press(Direction::SouthEast);
        held.set_running(true);
        assert_eq!(held.asking(), Some(Facing::running(Direction::SouthEast)));
    }

    #[test]
    fn nothing_held_asks_for_nothing() {
        let mut held = Held::default();
        held.press(Direction::West);
        held.release(Direction::West);
        assert_eq!(held.asking(), None);
        // A release nobody pressed is not an error.
        held.release(Direction::West);

        held.press(Direction::South);
        held.clear();
        assert_eq!(held.asking(), None);
    }
}
