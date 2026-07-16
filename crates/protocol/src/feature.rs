//! What a given client can actually do.
//!
//! # Why this is a table and not an era check
//!
//! UO features did not arrive in neat era-sized batches. Tooltips landed at
//! 4.0.0a with AoS, but stat locks came at 4.0.1a, tooltip hashes at 4.0.5a, and
//! the new damage packet at 4.0.7a — all inside "AoS". A client at 4.0.3
//! wants tooltips and stat locks but *not* tooltip hashes, and sending it the
//! 0xDC packet gets you silence, not an error.
//!
//! So gameplay code asks [`ClientVersion::supports`], never `era == Era::Aos`.
//! The version boundaries live here, once, and nowhere else.
//!
//! The boundaries themselves come from SphereServer's `MINCLIVER_*` table in
//! `common/sphereproto.h`, which encodes two decades of finding out the hard way
//! which client breaks on what. That table is the one part of Sphere worth
//! keeping — it is observed protocol behaviour, not architecture.

use crate::version::ClientVersion;

/// A capability a client either has or does not have.
///
/// Ask via [`ClientVersion::supports`]:
///
/// ```
/// use openshard_protocol::{ClientVersion, Feature};
///
/// let aos = ClientVersion::new(4, 0, 3, 0);
/// assert!(aos.supports(Feature::Tooltips));      // since 4.0.0
/// assert!(!aos.supports(Feature::TooltipHash));  // not until 4.0.5
/// assert!(!aos.supports(Feature::Buffs));        // not until 5.0.2b
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Feature {
    // -- expansion packet sets -------------------------------------------
    /// Lord Blackthorn's Revenge packets. Since 3.0.7b.
    LbrPackets,
    /// Age of Shadows packets. Since 4.0.0a.
    AosPackets,
    /// Samurai Empire packets. Since 4.0.5a.
    SePackets,
    /// Mondain's Legacy packets. Since 5.0.0a.
    MlPackets,
    /// Stygian Abyss packets. Since 7.0.0.0.
    SaPackets,
    /// High Seas packets. Since 7.0.9.0.
    HsPackets,
    /// Time of Legends packets. Since 7.0.45.65.
    TolPackets,

    // -- object properties -----------------------------------------------
    /// Object property tooltips (0xD6). Since 4.0.0a.
    Tooltips,
    /// Tooltip revision hashes (0xDC), which cut tooltip traffic hard. Since 4.0.5a.
    TooltipHash,
    /// Buff and debuff icons (0xDF). Since 5.0.2b.
    Buffs,
    /// Stat lock state (0xBF.0x19.0x02). Since 4.0.1a.
    StatLocks,
    /// Skill caps in the 0x3A skills packet. Since 4.0.0a.
    SkillCaps,
    /// The `noto_invul` (yellow) health bar. Since 4.0.0a.
    NotorietyInvulnerable,

    // -- packet shapes ---------------------------------------------------
    /// Damage numbers via 0xBF.0x22. Since 4.0.0a.
    DamagePacketExtended,
    /// Damage numbers via the newer 0x0B packet. Since 4.0.7a.
    DamagePacket,
    /// Spellbook contents via 0xBF.0x1B. Since 4.0.0a.
    SpellbookPacket,
    /// Custom (player-designed) house packets. Since 4.0.0a.
    CustomMulti,
    /// Books via the newer 0xD4 packet. Since 5.0.0a.
    NewBook,
    /// zlib-compressed gumps. Since 5.0.0a.
    CompressedGumps,
    /// Container item grid indices. Since 6.0.1.7.
    ItemGrid,
    /// Context menus via 0xBF.0x14.0x02 rather than 0x01. Since 6.0.0.0.
    NewContextMenu,
    /// A 4-byte rather than 2-byte feature mask in 0xB9. Since 6.0.14.2.
    ExtraFeatureMask,
    /// Mobile animations via 0xE2. Since 7.0.0.0.
    NewMobileAnimation,
    /// Smooth boat movement (0xF6). Since 7.0.9.0.
    SmoothShip,
    /// Map display via 0xF5. Since 7.0.13.0.
    NewMapDisplay,
    /// Extra fields in the login start info. Since 7.0.13.0.
    ExtraStartInfo,
    /// Mobile spawn via the newer 0x78 packet. Since 7.0.33.1.
    NewMobileIncoming,

    // -- systems ---------------------------------------------------------
    /// The post-2011 chat system, classic client. Since 7.0.4.1.
    NewChatSystem,
    /// Cross-shard global chat. Since 7.0.62.2.
    GlobalChat,
    /// Virtual gold and platinum in the trade window. Since 7.0.45.65.
    NewSecureTrade,
    /// Map waypoints on the classic client. Since 7.0.84.0.
    MapWaypoints,

    // -- behaviours ------------------------------------------------------
    /// Shard IPs in the 0xA8 list are sent reversed. Since 4.0.0.
    ///
    /// Reversed, for the *newer* clients. That is not a typo and it is not what
    /// the 0x8C relay does — see `encode_shard_list` and `encode_relay`.
    ReversedShardIp,
    /// The 0xA9 character list carries a trailing u32 flags field.
    /// Since 1.26.0.1.
    CharacterListFlags,
    /// The character list must be padded to at least five slots. Since 3.0.0a.
    PaddedCharacterList,
    /// Closing a dialog server-side does not echo a client response. Since 4.0.4.0.
    SilentCloseDialog,
}

impl Feature {
    /// The oldest client version that has this feature.
    ///
    /// Mirrors SphereServer's `MINCLIVER_*` constants.
    pub const fn since(self) -> ClientVersion {
        match self {
            // MINCLIVER_LBR 3000702
            Self::LbrPackets => ClientVersion::new(3, 0, 7, 2),
            // MINCLIVER_AOS 4000000
            Self::AosPackets => ClientVersion::new(4, 0, 0, 0),
            // MINCLIVER_SE 4000500
            Self::SePackets => ClientVersion::new(4, 0, 5, 0),
            // MINCLIVER_ML 5000000
            Self::MlPackets => ClientVersion::new(5, 0, 0, 0),
            // MINCLIVER_SA 7000000
            Self::SaPackets => ClientVersion::new(7, 0, 0, 0),
            // MINCLIVER_HS 7000900
            Self::HsPackets => ClientVersion::new(7, 0, 9, 0),
            // MINCLIVER_TOL 7004565
            Self::TolPackets => ClientVersion::new(7, 0, 45, 65),

            // MINCLIVER_TOOLTIP 4000000
            Self::Tooltips => ClientVersion::new(4, 0, 0, 0),
            // MINCLIVER_TOOLTIPHASH 4000500
            Self::TooltipHash => ClientVersion::new(4, 0, 5, 0),
            // MINCLIVER_BUFFS 5000202
            Self::Buffs => ClientVersion::new(5, 0, 2, 2),
            // MINCLIVER_STATLOCKS 4000100
            Self::StatLocks => ClientVersion::new(4, 0, 1, 0),
            // MINCLIVER_SKILLCAPS 4000000
            Self::SkillCaps => ClientVersion::new(4, 0, 0, 0),
            // MINCLIVER_NOTOINVUL 4000000
            Self::NotorietyInvulnerable => ClientVersion::new(4, 0, 0, 0),

            // MINCLIVER_DAMAGE 4000000
            Self::DamagePacketExtended => ClientVersion::new(4, 0, 0, 0),
            // MINCLIVER_NEWDAMAGE 4000700
            Self::DamagePacket => ClientVersion::new(4, 0, 7, 0),
            // MINCLIVER_SPELLBOOK 4000000
            Self::SpellbookPacket => ClientVersion::new(4, 0, 0, 0),
            // MINCLIVER_CUSTOMMULTI 4000000
            Self::CustomMulti => ClientVersion::new(4, 0, 0, 0),
            // MINCLIVER_NEWBOOK 5000000
            Self::NewBook => ClientVersion::new(5, 0, 0, 0),
            // MINCLIVER_COMPRESSDIALOG 5000000
            Self::CompressedGumps => ClientVersion::new(5, 0, 0, 0),
            // MINCLIVER_ITEMGRID 6000107
            Self::ItemGrid => ClientVersion::new(6, 0, 1, 7),
            // MINCLIVER_NEWCONTEXTMENU 6000000
            Self::NewContextMenu => ClientVersion::new(6, 0, 0, 0),
            // MINCLIVER_EXTRAFEATURES 6001402
            Self::ExtraFeatureMask => ClientVersion::new(6, 0, 14, 2),
            // MINCLIVER_NEWMOBILEANIM 7000000
            Self::NewMobileAnimation => ClientVersion::new(7, 0, 0, 0),
            // MINCLIVER_SMOOTHSHIP 7000900
            Self::SmoothShip => ClientVersion::new(7, 0, 9, 0),
            // MINCLIVER_NEWMAPDISPLAY 7001300
            Self::NewMapDisplay => ClientVersion::new(7, 0, 13, 0),
            // MINCLIVER_EXTRASTARTINFO 7001300
            Self::ExtraStartInfo => ClientVersion::new(7, 0, 13, 0),
            // MINCLIVER_NEWMOBINCOMING 7003301
            Self::NewMobileIncoming => ClientVersion::new(7, 0, 33, 1),

            // MINCLIVER_NEWCHATSYSTEM 7000401
            Self::NewChatSystem => ClientVersion::new(7, 0, 4, 1),
            // MINCLIVER_GLOBALCHAT 7006202
            Self::GlobalChat => ClientVersion::new(7, 0, 62, 2),
            // MINCLIVER_NEWSECURETRADE 7004565
            Self::NewSecureTrade => ClientVersion::new(7, 0, 45, 65),
            // MINCLIVER_MAPWAYPOINT 7008400
            Self::MapWaypoints => ClientVersion::new(7, 0, 84, 0),

            // MAXCLIVER_REVERSEIP 4000000, inverted.
            //
            // Sphere states this one as a *maximum*: below 4.0.0 it takes the
            // other branch. Expressed here as a "since" like everything else in
            // this table, which flips which branch the name refers to — hence
            // "reversed since 4.0.0" rather than Sphere's "reverse below it".
            //
            // Do not trust Sphere's inline comments on this one. They say the
            // reverseIp branch sends "in reverse" and the other sends "in
            // order", and both are the wrong way round for the bytes that
            // actually leave: `s_addr` is already network order, so the branch
            // that reverses the dword un-reverses the address. Read the shifts.
            Self::ReversedShardIp => ClientVersion::new(4, 0, 0, 0),
            // send.cpp writes the 0xA9 flags only when the reported version is
            // strictly greater than 1260000, so the boundary is one patch above.
            Self::CharacterListFlags => ClientVersion::new(1, 26, 0, 1),
            // MINCLIVER_PADCHARLIST 3000010
            Self::PaddedCharacterList => ClientVersion::new(3, 0, 0, 10),
            // MINCLIVER_CLOSEDIALOG 4000400
            Self::SilentCloseDialog => ClientVersion::new(4, 0, 4, 0),
        }
    }

    /// Every feature, oldest client requirement first.
    ///
    /// Exists so tooling (the dashboard's compatibility view, the tests below)
    /// can enumerate the table rather than hard-coding a list that drifts.
    pub const ALL: &'static [Feature] = &[
        Feature::CharacterListFlags,
        Feature::PaddedCharacterList,
        Feature::LbrPackets,
        Feature::AosPackets,
        Feature::ReversedShardIp,
        Feature::Tooltips,
        Feature::SkillCaps,
        Feature::NotorietyInvulnerable,
        Feature::DamagePacketExtended,
        Feature::SpellbookPacket,
        Feature::CustomMulti,
        Feature::StatLocks,
        Feature::SilentCloseDialog,
        Feature::SePackets,
        Feature::TooltipHash,
        Feature::DamagePacket,
        Feature::MlPackets,
        Feature::NewBook,
        Feature::CompressedGumps,
        Feature::Buffs,
        Feature::NewContextMenu,
        Feature::ItemGrid,
        Feature::ExtraFeatureMask,
        Feature::SaPackets,
        Feature::NewMobileAnimation,
        Feature::NewChatSystem,
        Feature::HsPackets,
        Feature::SmoothShip,
        Feature::NewMapDisplay,
        Feature::ExtraStartInfo,
        Feature::NewMobileIncoming,
        Feature::TolPackets,
        Feature::NewSecureTrade,
        Feature::GlobalChat,
        Feature::MapWaypoints,
    ];
}

/// Every feature a specific client has, resolved once.
///
/// Resolve this at login and hang it off the connection. Packet encoders then
/// branch on a bool instead of re-walking the table on every send.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FeatureSet {
    version: ClientVersion,
    /// Bit `i` is `Feature::ALL[i]`.
    bits: u64,
}

impl FeatureSet {
    /// Resolve the table for `version`.
    ///
    /// # Panics
    /// If [`Feature::ALL`] outgrows 64 entries, which the test below catches
    /// long before a release does.
    pub fn resolve(version: ClientVersion) -> Self {
        assert!(
            Feature::ALL.len() <= 64,
            "FeatureSet needs a wider bitmask than u64"
        );
        let mut bits = 0u64;
        for (index, feature) in Feature::ALL.iter().enumerate() {
            if version >= feature.since() {
                bits |= 1 << index;
            }
        }
        Self { version, bits }
    }

    /// The client this was resolved for.
    pub const fn version(self) -> ClientVersion {
        self.version
    }

    /// Whether the client has `feature`.
    pub fn has(self, feature: Feature) -> bool {
        Feature::ALL
            .iter()
            .position(|f| *f == feature)
            .is_some_and(|index| self.bits & (1 << index) != 0)
    }

    /// Every feature the client has.
    pub fn iter(self) -> impl Iterator<Item = Feature> {
        Feature::ALL
            .iter()
            .enumerate()
            .filter(move |(index, _)| self.bits & (1u64 << *index) != 0)
            .map(|(_, feature)| *feature)
    }

    /// How many features the client has.
    pub const fn count(self) -> u32 {
        self.bits.count_ones()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::Era;

    /// Named so that adding a `Feature` variant fails to compile here.
    ///
    /// Rust cannot enumerate enum variants, so a feature left out of
    /// [`Feature::ALL`] silently vanishes from every [`FeatureSet`] with no
    /// error anywhere. This match is the guard: it is exhaustive, so a new
    /// variant stops the build until someone writes it down — and the assertion
    /// below then checks it also reached `ALL`.
    ///
    /// **Adding a feature? Add it to `Feature::ALL` and to this match.**
    fn is_listed_in_all(feature: Feature) -> bool {
        match feature {
            Feature::LbrPackets
            | Feature::AosPackets
            | Feature::SePackets
            | Feature::MlPackets
            | Feature::SaPackets
            | Feature::HsPackets
            | Feature::TolPackets
            | Feature::Tooltips
            | Feature::TooltipHash
            | Feature::Buffs
            | Feature::StatLocks
            | Feature::SkillCaps
            | Feature::NotorietyInvulnerable
            | Feature::DamagePacketExtended
            | Feature::DamagePacket
            | Feature::SpellbookPacket
            | Feature::CustomMulti
            | Feature::NewBook
            | Feature::CompressedGumps
            | Feature::ItemGrid
            | Feature::NewContextMenu
            | Feature::ExtraFeatureMask
            | Feature::NewMobileAnimation
            | Feature::SmoothShip
            | Feature::NewMapDisplay
            | Feature::ExtraStartInfo
            | Feature::NewMobileIncoming
            | Feature::NewChatSystem
            | Feature::GlobalChat
            | Feature::NewSecureTrade
            | Feature::MapWaypoints
            | Feature::ReversedShardIp
            | Feature::CharacterListFlags
            | Feature::PaddedCharacterList
            | Feature::SilentCloseDialog => Feature::ALL.contains(&feature),
        }
    }

    #[test]
    fn all_lists_every_feature_exactly_once() {
        for feature in Feature::ALL {
            assert!(
                is_listed_in_all(*feature),
                "{feature:?} is missing from Feature::ALL"
            );
        }

        let mut sorted = Feature::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            Feature::ALL.len(),
            "Feature::ALL contains a duplicate"
        );
    }

    #[test]
    fn all_is_ordered_by_version() {
        for pair in Feature::ALL.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(
                a.since() <= b.since(),
                "{a:?} ({}) must not come after {b:?} ({})",
                a.since(),
                b.since()
            );
        }
    }

    #[test]
    fn all_fits_the_bitmask() {
        assert!(
            Feature::ALL.len() <= 64,
            "FeatureSet::bits is a u64; widen it before adding feature {}",
            Feature::ALL.len() + 1
        );
    }

    #[test]
    fn every_boundary_is_exact() {
        // The whole point of the table. Borrowing across fields matters here:
        // most boundaries sit at patch 0, so a naive `patch - 1` would skip
        // nearly every feature and prove almost nothing.
        for feature in Feature::ALL {
            let since = feature.since();
            assert!(
                since.supports(*feature),
                "{feature:?} is off at its own boundary {since}"
            );

            let Some(before) = one_below(since) else {
                continue;
            };
            assert!(
                !before.supports(*feature),
                "{feature:?} claims {before} but wants {since}"
            );
        }
    }

    /// The version immediately below `version` in the total order.
    fn one_below(version: ClientVersion) -> Option<ClientVersion> {
        let ClientVersion {
            major,
            minor,
            revision,
            patch,
        } = version;
        if patch > 0 {
            Some(ClientVersion::new(major, minor, revision, patch - 1))
        } else if revision > 0 {
            Some(ClientVersion::new(major, minor, revision - 1, u8::MAX))
        } else if minor > 0 {
            Some(ClientVersion::new(major, minor - 1, u8::MAX, u8::MAX))
        } else if major > 0 {
            Some(ClientVersion::new(major - 1, u8::MAX, u8::MAX, u8::MAX))
        } else {
            None
        }
    }

    /// Decode a SphereServer `MINCLIVER_*` constant into a version.
    ///
    /// Sphere packs a version into one decimal number: `major * 1_000_000 +
    /// minor * 10_000 + revision * 100 + patch`. So 3000702 is 3.0.7.2, which
    /// its own comment spells "3.0.7b" — the trailing letter *is* the patch.
    fn from_sphere_number(packed: u32) -> ClientVersion {
        ClientVersion::new(
            (packed / 1_000_000) as u8,
            ((packed / 10_000) % 100) as u8,
            ((packed / 100) % 100) as u8,
            (packed % 100) as u8,
        )
    }

    #[test]
    fn sphere_number_decoding_is_right() {
        // The one Sphere constant whose comment and number provably agree.
        assert_eq!(
            from_sphere_number(3_000_702),
            ClientVersion::new(3, 0, 7, 2)
        );
        assert_eq!(
            from_sphere_number(3_000_702),
            "3.0.7b".parse::<ClientVersion>().unwrap(),
            "Sphere's packed form and the old-style string must agree"
        );
        assert_eq!(
            from_sphere_number(7_004_565),
            ClientVersion::new(7, 0, 45, 65)
        );
    }

    #[test]
    fn every_boundary_matches_spheres_constant_verbatim() {
        // Pinned to the *numbers* in Sphere's sphereproto.h, not to the prose in
        // its comments. The two disagree: MINCLIVER_AOS is 4000000, which is
        // 4.0.0.0, while the comment beside it says "4.0.0a", which would be
        // 4.0.0.1. Trusting the comment over the number is exactly how this
        // table drifts a patch out of true, silently.
        let table = [
            (Feature::LbrPackets, 3_000_702),
            (Feature::AosPackets, 4_000_000),
            (Feature::SePackets, 4_000_500),
            (Feature::MlPackets, 5_000_000),
            (Feature::SaPackets, 7_000_000),
            (Feature::HsPackets, 7_000_900),
            (Feature::TolPackets, 7_004_565),
            (Feature::Tooltips, 4_000_000),
            (Feature::TooltipHash, 4_000_500),
            (Feature::Buffs, 5_000_202),
            (Feature::StatLocks, 4_000_100),
            (Feature::SkillCaps, 4_000_000),
            (Feature::NotorietyInvulnerable, 4_000_000),
            (Feature::DamagePacketExtended, 4_000_000),
            (Feature::DamagePacket, 4_000_700),
            (Feature::SpellbookPacket, 4_000_000),
            (Feature::CustomMulti, 4_000_000),
            (Feature::NewBook, 5_000_000),
            (Feature::CompressedGumps, 5_000_000),
            (Feature::ItemGrid, 6_000_107),
            (Feature::NewContextMenu, 6_000_000),
            (Feature::ExtraFeatureMask, 6_001_402),
            (Feature::NewMobileAnimation, 7_000_000),
            (Feature::SmoothShip, 7_000_900),
            (Feature::NewMapDisplay, 7_001_300),
            (Feature::ExtraStartInfo, 7_001_300),
            (Feature::NewMobileIncoming, 7_003_301),
            (Feature::NewChatSystem, 7_000_401),
            (Feature::GlobalChat, 7_006_202),
            (Feature::NewSecureTrade, 7_004_565),
            (Feature::MapWaypoints, 7_008_400),
            (Feature::ReversedShardIp, 4_000_000), // MAXCLIVER_REVERSEIP, inverted
            (Feature::CharacterListFlags, 1_260_001), // send.cpp: `> 1'26'00'00`
            (Feature::PaddedCharacterList, 3_000_010),
            (Feature::SilentCloseDialog, 4_000_400),
        ];
        assert_eq!(
            table.len(),
            Feature::ALL.len(),
            "every feature needs its Sphere constant pinned here"
        );
        for (feature, packed) in table {
            assert_eq!(
                feature.since(),
                from_sphere_number(packed),
                "{feature:?} drifted from MINCLIVER {packed}"
            );
        }
    }

    #[test]
    fn features_within_one_era_are_not_all_or_nothing() {
        // The reason gameplay code must never branch on Era.
        let early_aos = ClientVersion::new(4, 0, 3, 0);
        assert_eq!(early_aos.era(), Era::Aos);

        assert!(early_aos.supports(Feature::Tooltips), "4.0.0a");
        assert!(early_aos.supports(Feature::StatLocks), "4.0.1a");
        assert!(!early_aos.supports(Feature::SilentCloseDialog), "4.0.4.0");
        assert!(!early_aos.supports(Feature::TooltipHash), "4.0.5a");
        assert!(!early_aos.supports(Feature::DamagePacket), "4.0.7a");
    }

    #[test]
    fn a_t2a_client_gets_nothing_modern() {
        let t2a = ClientVersion::T2A;
        assert!(!t2a.supports(Feature::Tooltips));
        assert!(!t2a.supports(Feature::LbrPackets));
        assert!(!t2a.supports(Feature::Buffs));
        assert!(!t2a.supports(Feature::ItemGrid));

        // CharacterListFlags is the one thing this old: the 0xA9 flags dword
        // arrived at 1.26.0.1, a hair after T2A itself.
        assert_eq!(
            FeatureSet::resolve(t2a).iter().collect::<Vec<_>>(),
            vec![Feature::CharacterListFlags]
        );
    }

    #[test]
    fn a_current_client_gets_everything() {
        let latest = ClientVersion::new(7, 0, 95, 0);
        for feature in Feature::ALL {
            assert!(latest.supports(*feature), "{feature:?} missing on {latest}");
        }
        assert_eq!(
            FeatureSet::resolve(latest).count() as usize,
            Feature::ALL.len()
        );
    }

    #[test]
    fn feature_sets_grow_monotonically_with_version() {
        // No client may lose a feature its predecessor had.
        let ladder = [
            ClientVersion::T2A,
            ClientVersion::LBR,
            ClientVersion::AOS,
            ClientVersion::SE,
            ClientVersion::ML,
            ClientVersion::SA,
            ClientVersion::HS,
            ClientVersion::TOL,
            ClientVersion::new(7, 0, 95, 0),
        ];
        for pair in ladder.windows(2) {
            let (older, newer) = (FeatureSet::resolve(pair[0]), FeatureSet::resolve(pair[1]));
            for feature in older.iter() {
                assert!(
                    newer.has(feature),
                    "{} has {feature:?} but {} does not",
                    pair[0],
                    pair[1]
                );
            }
            assert!(newer.count() >= older.count());
        }
    }

    #[test]
    fn resolve_agrees_with_supports() {
        // FeatureSet is a cache; it must never disagree with the source table.
        let versions = [
            ClientVersion::OLDEST,
            ClientVersion::T2A,
            ClientVersion::new(4, 0, 3, 0),
            ClientVersion::new(5, 0, 2, 2),
            ClientVersion::new(6, 0, 14, 2),
            ClientVersion::TOL,
            ClientVersion::new(7, 0, 95, 0),
        ];
        for version in versions {
            let set = FeatureSet::resolve(version);
            assert_eq!(set.version(), version);
            for feature in Feature::ALL {
                assert_eq!(
                    set.has(*feature),
                    version.supports(*feature),
                    "{version} disagrees about {feature:?}"
                );
            }
        }
    }

    #[test]
    fn iter_yields_exactly_the_features_held() {
        let set = FeatureSet::resolve(ClientVersion::new(4, 0, 3, 0));
        let listed: Vec<_> = set.iter().collect();
        assert_eq!(listed.len(), set.count() as usize);
        for feature in &listed {
            assert!(set.has(*feature));
        }
        assert!(listed.contains(&Feature::Tooltips));
        assert!(!listed.contains(&Feature::TooltipHash));
    }
}
