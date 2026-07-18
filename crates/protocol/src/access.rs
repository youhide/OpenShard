//! Who is allowed to run privileged commands.

use std::fmt;
use std::str::FromStr;

/// A mobile's authority: what staff commands, if any, it may run.
///
/// Ordered, so a gate is a comparison — `level >= AccessLevel::GameMaster`. Not a
/// wire type; it rides no packet. It lives here, beside [`crate::DenyReason`] and
/// the other account-shaped types, because it is the one crate the login server,
/// the world and the binary all already share, and so the one place all three can
/// name the same level without a new dependency between them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum AccessLevel {
    /// An ordinary player. The default, and what a missing or unparseable
    /// configuration falls back to — authority is never granted by accident.
    #[default]
    Player,
    /// May run the world-shaping commands: spawn, teleport, set a stat.
    GameMaster,
    /// Everything a game master may do. A seam for account or shard commands that
    /// a game master should not — kept distinct now so adding them later is not a
    /// migration.
    Administrator,
}

impl AccessLevel {
    /// Whether this level clears `required` — the whole of the gate.
    pub fn allows(self, required: AccessLevel) -> bool {
        self >= required
    }
}

impl fmt::Display for AccessLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Player => "player",
            Self::GameMaster => "gamemaster",
            Self::Administrator => "administrator",
        };
        f.write_str(name)
    }
}

/// A configured access level that names no level this build knows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnknownAccessLevel(pub String);

impl fmt::Display for UnknownAccessLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown access level {:?}", self.0)
    }
}

impl std::error::Error for UnknownAccessLevel {}

impl FromStr for AccessLevel {
    type Err = UnknownAccessLevel;

    /// Parse a configured name, case-insensitively, with the abbreviations a
    /// human actually types. Unknown is an error the caller reports rather than a
    /// silent grant — the safe direction to be wrong in.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_lowercase().as_str() {
            "player" | "" => Ok(Self::Player),
            "gamemaster" | "gm" | "game master" => Ok(Self::GameMaster),
            "administrator" | "admin" => Ok(Self::Administrator),
            other => Err(UnknownAccessLevel(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_levels_are_ordered_so_a_gate_is_a_comparison() {
        assert!(AccessLevel::GameMaster > AccessLevel::Player);
        assert!(AccessLevel::Administrator > AccessLevel::GameMaster);
        assert!(AccessLevel::Administrator.allows(AccessLevel::GameMaster));
        assert!(!AccessLevel::Player.allows(AccessLevel::GameMaster));
        assert!(AccessLevel::GameMaster.allows(AccessLevel::GameMaster));
    }

    #[test]
    fn the_default_is_no_authority() {
        assert_eq!(AccessLevel::default(), AccessLevel::Player);
    }

    #[test]
    fn names_parse_case_insensitively_with_abbreviations() {
        assert_eq!("player".parse(), Ok(AccessLevel::Player));
        assert_eq!("".parse(), Ok(AccessLevel::Player));
        assert_eq!("GM".parse(), Ok(AccessLevel::GameMaster));
        assert_eq!("GameMaster".parse(), Ok(AccessLevel::GameMaster));
        assert_eq!("  admin ".parse(), Ok(AccessLevel::Administrator));
    }

    #[test]
    fn an_unknown_name_is_an_error_not_a_grant() {
        assert_eq!(
            "wizard".parse::<AccessLevel>(),
            Err(UnknownAccessLevel("wizard".to_owned()))
        );
    }

    #[test]
    fn display_round_trips_through_parse() {
        for level in [
            AccessLevel::Player,
            AccessLevel::GameMaster,
            AccessLevel::Administrator,
        ] {
            assert_eq!(level.to_string().parse(), Ok(level));
        }
    }
}
