//! Client versions and the era they belong to.

use std::fmt;
use std::str::FromStr;

/// A UO client version, e.g. 7.0.45.65.
///
/// Ordering is lexicographic across the four fields, which matches how the
/// client itself versions: 4.0.7.0 is newer than 4.0.5.0 is newer than 3.0.8.4.
/// Every feature gate in [`crate::Feature`] is a comparison against one of
/// these.
///
/// # Old-style versions
///
/// Clients before 5.0.6.5 were named `3.0.7b`, where the trailing letter is the
/// patch: `a` is 1, `b` is 2, and so on. [`ClientVersion::from_str`] accepts
/// both that and the modern dotted form, so `"3.0.7b"` and `"3.0.7.2"` parse to
/// the same value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ClientVersion {
    /// Major version. `7` for all modern clients.
    pub major: u8,
    /// Minor version.
    pub minor: u8,
    /// Revision.
    pub revision: u8,
    /// Patch. In old-style versions this is the trailing letter, `a` = 1.
    pub patch: u8,
}

impl ClientVersion {
    /// Build a version from its four parts.
    pub const fn new(major: u8, minor: u8, revision: u8, patch: u8) -> Self {
        Self {
            major,
            minor,
            revision,
            patch,
        }
    }

    /// The oldest client the server will consider talking to.
    pub const OLDEST: Self = Self::new(0, 0, 0, 0);

    /// 1.26.4 — The Second Age.
    pub const T2A: Self = Self::new(1, 26, 4, 0);
    /// 3.0.7b (= 3.0.7.2) — Lord Blackthorn's Revenge.
    pub const LBR: Self = Self::new(3, 0, 7, 2);
    /// 4.0.0 — Age of Shadows. Sphere's MINCLIVER_AOS is 4000000, not 4000001.
    pub const AOS: Self = Self::new(4, 0, 0, 0);
    /// 4.0.5 — Samurai Empire.
    pub const SE: Self = Self::new(4, 0, 5, 0);
    /// 5.0.0 — Mondain's Legacy.
    pub const ML: Self = Self::new(5, 0, 0, 0);
    /// 7.0.0.0 — Stygian Abyss.
    pub const SA: Self = Self::new(7, 0, 0, 0);
    /// 7.0.9.0 — High Seas.
    pub const HS: Self = Self::new(7, 0, 9, 0);
    /// 7.0.45.65 — Time of Legends.
    pub const TOL: Self = Self::new(7, 0, 45, 65);

    /// Which expansion era this version belongs to.
    pub const fn era(self) -> Era {
        Era::of(self)
    }

    /// Whether this client supports `feature`.
    ///
    /// This is the only place version numbers should be compared in gameplay
    /// code. Ask what a client *can do*, never what version it is — that keeps
    /// the era table in one place instead of smeared across every packet
    /// handler.
    pub fn supports(self, feature: crate::Feature) -> bool {
        self >= feature.since()
    }

    /// Version of the 0x11 status packet this client expects.
    ///
    /// Returns 1 through 6. The status packet grew a new tail with each era and
    /// clients reject the wrong length outright, so this cannot be a feature
    /// flag — it is a shape, not a capability.
    pub fn status_packet_version(self) -> u8 {
        const VERSIONS: [(u8, ClientVersion); 5] = [
            (6, ClientVersion::new(7, 0, 30, 0)),
            (5, ClientVersion::new(5, 0, 0, 0)),
            (4, ClientVersion::new(4, 0, 0, 0)),
            (3, ClientVersion::new(3, 0, 8, 10)),
            (2, ClientVersion::new(3, 0, 8, 4)),
        ];
        VERSIONS
            .iter()
            .find(|(_, since)| self >= *since)
            .map_or(1, |(version, _)| *version)
    }
}

impl fmt::Display for ClientVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.major, self.minor, self.revision, self.patch
        )
    }
}

impl fmt::Debug for ClientVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ClientVersion({self})")
    }
}

/// A client version string could not be parsed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseVersionError {
    input: String,
}

impl fmt::Display for ParseVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a client version", self.input)
    }
}

impl std::error::Error for ParseVersionError {}

impl FromStr for ClientVersion {
    type Err = ParseVersionError;

    /// Parse `"7.0.45.65"`, `"3.0.7b"`, `"5.0.9"`, or `"4.0"`.
    ///
    /// The client sends this string in the 0xBD seed packet, so it is untrusted
    /// input from the network: anything unparseable is an error, never a
    /// best-effort guess.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let fail = || ParseVersionError {
            input: s.to_owned(),
        };

        let fields: Vec<&str> = s.trim().split('.').map(str::trim).collect();
        // Two fields is the shortest thing worth calling a version; a bare "7"
        // is far more likely to be garbage.
        if fields.len() < 2 || fields.len() > 4 {
            return Err(fail());
        }

        let mut parts = [0u8; 4];
        for (index, field) in fields.iter().enumerate() {
            // An old-style trailing letter encodes the patch, and only ever sat
            // on the revision of a three-field version ("3.0.7b"). Anywhere
            // else it is junk, so it must not be quietly accepted.
            if fields.len() == 3 && index == 2 {
                if let Some(&letter) = field.as_bytes().last() {
                    if letter.is_ascii_alphabetic() {
                        parts[2] = field[..field.len() - 1].parse::<u8>().map_err(|_| fail())?;
                        // 'a' is patch 1, matching Sphere's 3.0.7b == 3000702.
                        parts[3] = letter.to_ascii_lowercase() - b'a' + 1;
                        return Ok(Self::new(parts[0], parts[1], parts[2], parts[3]));
                    }
                }
            }
            parts[index] = field.parse::<u8>().map_err(|_| fail())?;
        }

        Ok(Self::new(parts[0], parts[1], parts[2], parts[3]))
    }
}

/// A UO expansion era.
///
/// Eras exist for coarse decisions — which map set to load, whether housing is
/// customisable — not for packet gating. For anything a client either can or
/// cannot do, use [`ClientVersion::supports`]: features did not arrive in neat
/// era-sized batches, and treating them as if they did is how you end up sending
/// a packet a client silently drops.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum Era {
    /// Before The Second Age.
    PreT2A,
    /// The Second Age.
    T2A,
    /// Lord Blackthorn's Revenge.
    Lbr,
    /// Age of Shadows.
    Aos,
    /// Samurai Empire.
    Se,
    /// Mondain's Legacy.
    Ml,
    /// Stygian Abyss.
    Sa,
    /// High Seas.
    Hs,
    /// Time of Legends and later.
    Tol,
}

impl Era {
    /// The era a version belongs to.
    pub const fn of(version: ClientVersion) -> Self {
        // Newest first: each arm is "at least this".
        if ge(version, ClientVersion::TOL) {
            Self::Tol
        } else if ge(version, ClientVersion::HS) {
            Self::Hs
        } else if ge(version, ClientVersion::SA) {
            Self::Sa
        } else if ge(version, ClientVersion::ML) {
            Self::Ml
        } else if ge(version, ClientVersion::SE) {
            Self::Se
        } else if ge(version, ClientVersion::AOS) {
            Self::Aos
        } else if ge(version, ClientVersion::LBR) {
            Self::Lbr
        } else if ge(version, ClientVersion::T2A) {
            Self::T2A
        } else {
            Self::PreT2A
        }
    }

    /// The lowest version in this era.
    pub const fn min_version(self) -> ClientVersion {
        match self {
            Self::PreT2A => ClientVersion::OLDEST,
            Self::T2A => ClientVersion::T2A,
            Self::Lbr => ClientVersion::LBR,
            Self::Aos => ClientVersion::AOS,
            Self::Se => ClientVersion::SE,
            Self::Ml => ClientVersion::ML,
            Self::Sa => ClientVersion::SA,
            Self::Hs => ClientVersion::HS,
            Self::Tol => ClientVersion::TOL,
        }
    }
}

impl fmt::Display for Era {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::PreT2A => "Pre-T2A",
            Self::T2A => "The Second Age",
            Self::Lbr => "Lord Blackthorn's Revenge",
            Self::Aos => "Age of Shadows",
            Self::Se => "Samurai Empire",
            Self::Ml => "Mondain's Legacy",
            Self::Sa => "Stygian Abyss",
            Self::Hs => "High Seas",
            Self::Tol => "Time of Legends",
        };
        f.write_str(name)
    }
}

/// `a >= b` in a const context, since `PartialOrd` is not const.
const fn ge(a: ClientVersion, b: ClientVersion) -> bool {
    if a.major != b.major {
        return a.major > b.major;
    }
    if a.minor != b.minor {
        return a.minor > b.minor;
    }
    if a.revision != b.revision {
        return a.revision > b.revision;
    }
    a.patch >= b.patch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_is_field_by_field() {
        assert!(ClientVersion::new(4, 0, 7, 0) > ClientVersion::new(4, 0, 5, 0));
        assert!(ClientVersion::new(7, 0, 0, 0) > ClientVersion::new(6, 99, 99, 99));
        assert!(ClientVersion::new(3, 0, 8, 10) > ClientVersion::new(3, 0, 8, 4));
        assert!(ClientVersion::AOS > ClientVersion::LBR);
        assert!(ClientVersion::TOL > ClientVersion::HS);
    }

    #[test]
    fn const_ge_matches_the_derived_ord() {
        // `ge` is hand-written for const contexts; it must not drift from Ord.
        let samples = [
            ClientVersion::OLDEST,
            ClientVersion::T2A,
            ClientVersion::LBR,
            ClientVersion::AOS,
            ClientVersion::SE,
            ClientVersion::ML,
            ClientVersion::SA,
            ClientVersion::HS,
            ClientVersion::TOL,
            ClientVersion::new(3, 0, 8, 4),
            ClientVersion::new(7, 0, 30, 0),
            ClientVersion::new(255, 255, 255, 255),
        ];
        for a in samples {
            for b in samples {
                assert_eq!(ge(a, b), a >= b, "ge({a}, {b}) disagrees with Ord");
            }
        }
    }

    #[test]
    fn parses_modern_dotted_versions() {
        assert_eq!(
            "7.0.45.65".parse::<ClientVersion>().unwrap(),
            ClientVersion::new(7, 0, 45, 65)
        );
        assert_eq!(
            "5.0.9".parse::<ClientVersion>().unwrap(),
            ClientVersion::new(5, 0, 9, 0)
        );
        assert_eq!(
            "4.0".parse::<ClientVersion>().unwrap(),
            ClientVersion::new(4, 0, 0, 0)
        );
        assert_eq!(
            " 7.0.45.65 ".parse::<ClientVersion>().unwrap(),
            ClientVersion::new(7, 0, 45, 65),
            "the seed packet is whitespace-padded in the wild"
        );
    }

    #[test]
    fn parses_old_style_letter_versions() {
        // Sphere encodes 3.0.7b as 3000702, so 'b' is patch 2.
        assert_eq!(
            "3.0.7b".parse::<ClientVersion>().unwrap(),
            ClientVersion::new(3, 0, 7, 2)
        );
        assert_eq!(
            "4.0.0a".parse::<ClientVersion>().unwrap(),
            ClientVersion::new(4, 0, 0, 1)
        );
        assert_eq!(
            "3.0.8j".parse::<ClientVersion>().unwrap(),
            ClientVersion::new(3, 0, 8, 10)
        );
        assert_eq!(
            "3.0.7B".parse::<ClientVersion>().unwrap(),
            ClientVersion::new(3, 0, 7, 2),
            "case must not matter"
        );
        assert_eq!(
            "3.0.7b".parse::<ClientVersion>().unwrap(),
            "3.0.7.2".parse::<ClientVersion>().unwrap(),
            "both spellings mean the same client"
        );
    }

    #[test]
    fn rejects_junk() {
        // This string arrives from the network in the 0xBD packet.
        for junk in [
            "",
            "   ",
            "7",
            "abc",
            "7.x",
            "7.0.0.0.0",
            "999.0.0.0",
            "-1.0",
            "7..0",
            "3.0b.7",
            "7.0.0b.0",
            "3.0.7bb",
        ] {
            assert!(
                junk.parse::<ClientVersion>().is_err(),
                "{junk:?} must not parse"
            );
        }
    }

    #[test]
    fn display_round_trips_through_parse() {
        for version in [
            ClientVersion::T2A,
            ClientVersion::LBR,
            ClientVersion::AOS,
            ClientVersion::TOL,
        ] {
            let text = version.to_string();
            assert_eq!(text.parse::<ClientVersion>().unwrap(), version, "{text}");
        }
    }

    #[test]
    fn eras_bracket_their_versions() {
        assert_eq!(Era::of(ClientVersion::new(1, 0, 0, 0)), Era::PreT2A);
        assert_eq!(Era::of(ClientVersion::T2A), Era::T2A);
        assert_eq!(Era::of(ClientVersion::new(2, 0, 3, 0)), Era::T2A);
        assert_eq!(Era::of(ClientVersion::LBR), Era::Lbr);
        assert_eq!(Era::of(ClientVersion::AOS), Era::Aos);
        assert_eq!(Era::of(ClientVersion::new(4, 0, 4, 0)), Era::Aos);
        assert_eq!(Era::of(ClientVersion::SE), Era::Se);
        assert_eq!(Era::of(ClientVersion::ML), Era::Ml);
        assert_eq!(Era::of(ClientVersion::new(6, 0, 14, 2)), Era::Ml);
        assert_eq!(Era::of(ClientVersion::SA), Era::Sa);
        assert_eq!(Era::of(ClientVersion::HS), Era::Hs);
        assert_eq!(Era::of(ClientVersion::TOL), Era::Tol);
        assert_eq!(Era::of(ClientVersion::new(7, 0, 95, 0)), Era::Tol);
    }

    #[test]
    fn era_boundaries_are_exact() {
        // One patch below each boundary must fall in the previous era.
        assert_eq!(Era::of(ClientVersion::new(3, 255, 255, 255)), Era::Lbr);
        assert_eq!(Era::of(ClientVersion::new(4, 0, 4, 99)), Era::Aos);
        assert_eq!(Era::of(ClientVersion::new(6, 99, 99, 99)), Era::Ml);
        assert_eq!(Era::of(ClientVersion::new(7, 0, 8, 99)), Era::Sa);
        assert_eq!(Era::of(ClientVersion::new(7, 0, 45, 64)), Era::Hs);
    }

    #[test]
    fn era_ordering_follows_history() {
        assert!(Era::PreT2A < Era::T2A);
        assert!(Era::T2A < Era::Lbr);
        assert!(Era::Aos < Era::Se);
        assert!(Era::Hs < Era::Tol);
    }

    #[test]
    fn min_version_round_trips() {
        for era in [
            Era::PreT2A,
            Era::T2A,
            Era::Lbr,
            Era::Aos,
            Era::Se,
            Era::Ml,
            Era::Sa,
            Era::Hs,
            Era::Tol,
        ] {
            assert_eq!(Era::of(era.min_version()), era, "{era}");
        }
    }

    #[test]
    fn status_packet_versions_match_the_era_table() {
        // Sphere: v2 at 3.0.8d, v3 at 3.0.8j, v4 at 4.0.0a, v5 at 5.0.0a,
        // v6 at 7.0.30.0.
        assert_eq!(ClientVersion::new(3, 0, 8, 3).status_packet_version(), 1);
        assert_eq!(
            ClientVersion::new(3, 255, 255, 255).status_packet_version(),
            3
        );
        assert_eq!(ClientVersion::new(3, 0, 8, 4).status_packet_version(), 2);
        assert_eq!(ClientVersion::new(3, 0, 8, 9).status_packet_version(), 2);
        assert_eq!(ClientVersion::new(3, 0, 8, 10).status_packet_version(), 3);
        assert_eq!(ClientVersion::new(4, 0, 0, 0).status_packet_version(), 4);
        assert_eq!(ClientVersion::new(4, 9, 9, 9).status_packet_version(), 4);
        assert_eq!(ClientVersion::new(5, 0, 0, 0).status_packet_version(), 5);
        assert_eq!(ClientVersion::new(7, 0, 29, 9).status_packet_version(), 5);
        assert_eq!(ClientVersion::new(7, 0, 30, 0).status_packet_version(), 6);
        assert_eq!(ClientVersion::TOL.status_packet_version(), 6);
    }

    #[test]
    fn status_packet_version_never_goes_backwards() {
        let mut previous = 0;
        for major in 0..8u8 {
            for revision in [0u8, 8, 30, 45] {
                let version = ClientVersion::new(major, 0, revision, 10);
                let current = version.status_packet_version();
                assert!(
                    current >= previous,
                    "{version} dropped from v{previous} to v{current}"
                );
                previous = current;
            }
        }
    }
}
