//! The staff command vocabulary: `.add`, `.go`, `.skill` — the words a game
//! master types into the speech box and the world runs instead of saying.
//!
//! # Why the names live in `common` and not in the world
//!
//! Both ends of the wire read this list. The world dispatches on it
//! (`openshard_world::gm::run`), and the client offers it as the line is typed
//! — a player who has to remember twenty-five verbs and their argument order
//! has no way to find out what a shard can do. Two lists would be one list and
//! a copy of it going stale: a command added to the world would keep working
//! and simply stop being offered, which is the failure nobody notices.
//!
//! So the list is an **enum**, not a table of strings. The world matches on
//! [`StaffCommand`] exhaustively, so adding a variant here does not compile
//! until the world has an arm for it, and the client completes from
//! [`StaffCommand::ALL`], so the same variant is offered the moment it exists.
//! Neither end can drift from the other without the compiler saying so.
//!
//! # Who may type them, and where that is decided
//!
//! The **decision** is the world's and stays there: `World::say` gates on the
//! actor's authority before `gm::run` is entered at all, and nothing a client
//! says about itself is read. What lives here is the *threshold* that gate
//! compares against — [`StaffCommand::AUTHORITY`] — for the same reason the
//! names do: two statements of one rule are a pair that can drift, and this is
//! the one crate both the gate and the completer can read.
//!
//! So [`StaffCommand::matching`] takes the level the shard holds the typist at
//! (a client learns it from `openshard_protocol::access::AuthorityNotice`) and
//! offers nothing to somebody who may run nothing. It offered all twenty-five to
//! everybody until that packet existed, on the argument that a word the shard
//! refuses reads the same as a mistyped one — which is true of the *answer* and
//! false of the *offer*: a completer that lists verbs nobody may use is teaching
//! them.

use openshard_protocol::access::AccessLevel;

/// The character that turns speech into a command.
///
/// Sphere's convention, kept, and written here rather than in either end: the
/// world strips it before dispatching and the client reads it to know that the
/// line being typed is a command at all.
pub const PREFIX: char = '.';

/// One staff command.
///
/// Alphabetical, because [`ALL`](Self::ALL) is the order a person reads them in
/// when the client offers them, and the house verbs (`hban`, `hcoowner`, …)
/// land next to each other for free.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum StaffCommand {
    /// `.add <graphic> [amount]`
    Add,
    /// `.addgold <amount>`
    AddGold,
    /// `.addstatic <graphic> [z]`
    AddStatic,
    /// `.admin`
    Admin,
    /// `.boat <multi id>`
    Boat,
    /// `.deed <multi id>`
    Deed,
    /// `.dummy [hits|off]`
    Dummy,
    /// `.gm [on|off]`
    Gm,
    /// `.go <x> <y> [z] [facet]`
    Go,
    /// `.hban`
    HBan,
    /// `.hcoowner`
    HCoOwner,
    /// `.hdemolish [house serial]`
    HDemolish,
    /// `.hdesign <multi id>`
    HDesign,
    /// `.hdrop`
    HDrop,
    /// `.hfriend`
    HFriend,
    /// `.house <multi id> [x y z]`
    House,
    /// `.hunban`
    HUnban,
    /// `.key <value>`
    Key,
    /// `.poison <level 0-4>`
    Poison,
    /// `.quests`
    Quests,
    /// `.rmstatic [nth]`
    RmStatic,
    /// `.sail <direction|stop> [fast]`
    Sail,
    /// `.save`
    Save,
    /// `.set <str|dex|int> <value>`
    Set,
    /// `.setland <tile id> [z]`
    SetLand,
    /// `.sight`
    Sight,
    /// `.skill <name> <value>`
    Skill,
    /// `.spellbook`
    Spellbook,
    /// `.tele`
    Tele,
    /// `.tile`
    Tile,
    /// `.trap <kind> [power]`
    Trap,
    /// `.where`
    Where,
}

impl StaffCommand {
    /// Every command there is, alphabetically.
    ///
    /// An array and not a `Vec`: it is a constant, and the client walks it on
    /// every keystroke while a command is being typed.
    pub const ALL: [Self; 32] = [
        Self::Add,
        Self::AddGold,
        Self::AddStatic,
        Self::Admin,
        Self::Boat,
        Self::Deed,
        Self::Dummy,
        Self::Gm,
        Self::Go,
        Self::HBan,
        Self::HCoOwner,
        Self::HDemolish,
        Self::HDesign,
        Self::HDrop,
        Self::HFriend,
        Self::House,
        Self::HUnban,
        Self::Key,
        Self::Poison,
        Self::Quests,
        Self::RmStatic,
        Self::Sail,
        Self::Save,
        Self::Set,
        Self::SetLand,
        Self::Sight,
        Self::Skill,
        Self::Spellbook,
        Self::Tele,
        Self::Tile,
        Self::Trap,
        Self::Where,
    ];

    /// The word itself, without [`PREFIX`], lowercase — which is how it is
    /// written, matched and offered.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::AddGold => "addgold",
            Self::AddStatic => "addstatic",
            Self::Admin => "admin",
            Self::Boat => "boat",
            Self::Deed => "deed",
            Self::Dummy => "dummy",
            Self::Gm => "gm",
            Self::Go => "go",
            Self::HBan => "hban",
            Self::HCoOwner => "hcoowner",
            Self::HDemolish => "hdemolish",
            Self::HDesign => "hdesign",
            Self::HDrop => "hdrop",
            Self::HFriend => "hfriend",
            Self::House => "house",
            Self::HUnban => "hunban",
            Self::Key => "key",
            Self::Poison => "poison",
            Self::Quests => "quests",
            Self::RmStatic => "rmstatic",
            Self::Sail => "sail",
            Self::Save => "save",
            Self::Set => "set",
            Self::SetLand => "setland",
            Self::Sight => "sight",
            Self::Skill => "skill",
            Self::Spellbook => "spellbook",
            Self::Tele => "tele",
            Self::Tile => "tile",
            Self::Trap => "trap",
            Self::Where => "where",
        }
    }

    /// What follows the word: `<required>`, `[optional]`, empty for a command
    /// that takes nothing.
    ///
    /// The same shape the world's own `Usage:` replies are written in, because
    /// it is the same sentence: a person who reads it in the completion popup
    /// and a person who reads it after mistyping must not be told two different
    /// things.
    #[must_use]
    pub const fn arguments(self) -> &'static str {
        match self {
            Self::Add => "<graphic> [amount]",
            Self::AddGold => "<amount>",
            Self::AddStatic => "<graphic> [z]",
            Self::RmStatic => "[nth]",
            Self::SetLand => "<tile id> [z]",
            Self::Boat | Self::Deed | Self::HDesign => "<multi id>",
            Self::Dummy => "[hits|off]",
            Self::House => "<multi id|@template> [x y z]",
            Self::Gm => "[on|off]",
            Self::HDemolish => "[house serial]",
            Self::Go => "<x> <y> [z] [facet]",
            Self::Key => "<value>",
            Self::Poison => "<level 0-4>",
            Self::Sail => "<n|ne|e|se|s|sw|w|nw|stop> [fast]",
            Self::Set => "<str|dex|int> <value>",
            Self::Skill => "<name> <value>",
            Self::Trap => "<magic|explosion|dart|poison> [power]",
            Self::Admin
            | Self::HBan
            | Self::HCoOwner
            | Self::HDrop
            | Self::HFriend
            | Self::HUnban
            | Self::Quests
            | Self::Save
            | Self::Sight
            | Self::Spellbook
            | Self::Tele
            | Self::Tile
            | Self::Where => "",
        }
    }

    /// One line, in the imperative, for the completion popup.
    ///
    /// Short enough to sit beside the name on a chat line at any of
    /// `desk::ChatScale`'s sizes — a summary that wraps is a summary that
    /// covers the world.
    #[must_use]
    pub const fn summary(self) -> &'static str {
        match self {
            Self::Add => "drop an item at your feet",
            Self::AddGold => "put gold in your pack",
            Self::AddStatic => "build a static into the map under you",
            Self::Admin => "open the administration menu",
            Self::Boat => "put a ship on the water at your feet",
            Self::Deed => "put a house deed in your pack",
            Self::Dummy => "stand a scarecrow beside you to shoot at",
            Self::Gm => "turn staff mode on or off",
            Self::Go => "jump to coordinates",
            Self::HBan => "click whom to ban from this house",
            Self::HCoOwner => "click whom to make a co-owner",
            Self::HDemolish => "pull down a named house, or the one under you",
            Self::HDesign => "give this house another multi's shape",
            Self::HDrop => "click whom to strip of standing here",
            Self::HFriend => "click whom to make a friend of this house",
            Self::House => "put a classic or imported house at your feet",
            Self::HUnban => "click whom to unban from this house",
            Self::Key => "put a key in your pack, and lock what you click",
            Self::Poison => "drop a bottle of poison at your feet",
            Self::Quests => "open your quest log",
            Self::RmStatic => "take a static out of the map under you",
            Self::Sail => "steer the ship you are standing on",
            Self::Save => "save the world now",
            Self::Set => "change one of your stats",
            Self::SetLand => "change the ground under you, for good",
            Self::Sight => "click a spot, and say what the ray to it meets",
            Self::Skill => "set one of your skills, in whole points",
            Self::Spellbook => "drop a full spellbook into your pack",
            Self::Tele => "click a spot, and jump to it",
            Self::Tile => "say what the map holds under you",
            Self::Trap => "click a container, and trap it",
            Self::Where => "say where you are standing",
        }
    }

    /// The command a typed word names, or `None` for a word no command has.
    ///
    /// Case-insensitive: `.GM` and `.gm` are the same command, which is what
    /// the world's own `to_lowercase` did before this existed. ASCII only, and
    /// deliberately — every name here is ASCII, and a Turkish locale's dotless
    /// `ı` must not decide whether `.SKILL` is a command.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|command| command.name().eq_ignore_ascii_case(word))
    }

    /// The authority a shard requires of anybody typing one of these.
    ///
    /// **One threshold for the whole vocabulary**, because that is what the
    /// world enforces: `World::say` asks `WorldState::staff_authority` once,
    /// before dispatching, and no arm of `gm::run` asks for more. A per-command
    /// level would be a client hiding words the shard would in fact have run, or
    /// offering ones it would refuse — which is the same drift two lists have,
    /// one rung down.
    ///
    /// [`AccessLevel::Administrator`] exists and is deliberately not used here:
    /// it is a seam for account or shard commands that a game master should not
    /// have, and the day one of these needs it, this constant becomes a method
    /// on the command and both ends follow it at once.
    pub const AUTHORITY: AccessLevel = AccessLevel::GameMaster;

    /// Every command whose name begins with `prefix`, in [`ALL`](Self::ALL)'s
    /// order — or nothing at all, for somebody `level` does not let command.
    ///
    /// An empty prefix matches everything, which is what an empty `.` should
    /// offer *to staff*: the whole vocabulary, which is the only way to find out
    /// it exists. To everybody else an empty prefix matches nothing, which is
    /// the honest answer to "what may I run" and not a refusal to answer.
    ///
    /// A client learns `level` from
    /// [`AuthorityNotice`](openshard_protocol::access::AuthorityNotice) and
    /// holds [`AccessLevel::Player`] until one arrives — so a shard that never
    /// sends one offers no completions rather than offering words at random,
    /// which is the safe direction for a guess about authority to be wrong in.
    #[must_use]
    pub fn matching(prefix: &str, level: AccessLevel) -> Vec<Self> {
        if !level.allows(Self::AUTHORITY) {
            return Vec::new();
        }
        Self::ALL
            .into_iter()
            .filter(|command| {
                command.name().len() >= prefix.len()
                    && command.name()[..prefix.len()].eq_ignore_ascii_case(prefix)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::access::AccessLevel;

    use super::{PREFIX, StaffCommand};

    #[test]
    fn every_command_is_in_all_exactly_once() {
        let mut names: Vec<&str> = StaffCommand::ALL.iter().map(|command| command.name()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "two commands share a name");
    }

    /// `ALL` is the order the client offers them in, so it is sorted here rather
    /// than sorted again at every keystroke.
    #[test]
    fn all_is_alphabetical() {
        let names: Vec<&str> = StaffCommand::ALL.iter().map(|command| command.name()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn a_name_round_trips_through_parse_whatever_its_case() {
        for command in StaffCommand::ALL {
            assert_eq!(StaffCommand::parse(command.name()), Some(command));
            assert_eq!(StaffCommand::parse(&command.name().to_uppercase()), Some(command));
        }
        assert_eq!(StaffCommand::parse("nosuchthing"), None);
        assert_eq!(StaffCommand::parse(""), None);
    }

    #[test]
    fn a_prefix_offers_what_begins_with_it() {
        let staff = AccessLevel::GameMaster;
        assert_eq!(
            StaffCommand::matching("hd", staff),
            vec![
                StaffCommand::HDemolish,
                StaffCommand::HDesign,
                StaffCommand::HDrop
            ]
        );
        assert_eq!(StaffCommand::matching("skill", staff), vec![StaffCommand::Skill]);
        assert!(StaffCommand::matching("zzz", staff).is_empty());
        assert_eq!(
            StaffCommand::matching("", staff).len(),
            StaffCommand::ALL.len(),
            "a lone '{PREFIX}' offers the whole vocabulary"
        );
    }

    /// A prefix longer than the name it is a prefix of matches nothing, and in
    /// particular does not panic slicing past the end.
    #[test]
    fn a_prefix_past_the_end_of_a_name_matches_nothing() {
        assert!(StaffCommand::matching("wherever", AccessLevel::GameMaster).is_empty());
    }

    /// A player is offered nothing, however much of a command they type: the
    /// shard would refuse every one of these words, and a completer that listed
    /// them would be teaching verbs that do not work.
    #[test]
    fn an_ordinary_player_is_offered_nothing_at_all() {
        for prefix in ["", "h", "hd", "save", "zzz"] {
            assert!(
                StaffCommand::matching(prefix, AccessLevel::Player).is_empty(),
                "a player was offered something for {prefix:?}"
            );
        }
    }

    /// And every level at or above the threshold is offered the same list —
    /// there is one gate, so there is one vocabulary.
    #[test]
    fn every_level_that_may_command_is_offered_the_whole_vocabulary() {
        assert!(AccessLevel::GameMaster.allows(StaffCommand::AUTHORITY));
        assert_eq!(
            StaffCommand::matching("", AccessLevel::Administrator).len(),
            StaffCommand::ALL.len()
        );
        assert_eq!(
            StaffCommand::matching("h", AccessLevel::Administrator),
            StaffCommand::matching("h", AccessLevel::GameMaster)
        );
    }

    /// The summaries are read on one chat line beside the name and its
    /// arguments, so their length is a property worth pinning rather than a
    /// matter of taste that drifts one command at a time.
    #[test]
    fn a_summary_fits_on_a_line() {
        for command in StaffCommand::ALL {
            let width = command.name().len() + command.arguments().len() + command.summary().len();
            assert!(width <= 80, "{} reads too wide: {width}", command.name());
        }
    }
}
