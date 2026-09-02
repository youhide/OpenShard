//! Shared English fallback catalogue for every localized line OpenShard's server emits.
//!
//! The UO wire carries only a `ClilocId`; an installed client table remains
//! authoritative for the player's language. This common contract supplies a
//! readable fallback when that file is unavailable and detects omissions.

use crate::wire::ClilocId;

/// One server-emitted localized line and its English fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    /// The identifier sent over the UO wire.
    pub id:       ClilocId,
    /// English text used when the installed `Cliloc.*` lacks this identifier.
    pub fallback: &'static str,
}

/// Every fixed or bounded dynamic cliloc emitted by the current server.
///
/// Generated from the server message inventory and the reference `Cliloc.enu`.
pub const SERVER_MESSAGES: &[Message] = &[
    Message {
        id:       ClilocId(500014),
        fallback: "That skill cannot be used directly.",
    },
    Message {
        id:       ClilocId(500039),
        fallback: "Failed!",
    },
    Message {
        id:       ClilocId(500118),
        fallback: "You must wait a few moments to use another skill.",
    },
    Message {
        id:       ClilocId(500119),
        fallback: "You must wait to perform another action.",
    },
    Message {
        id:       ClilocId(500134),
        fallback: "You stop meditating.",
    },
    Message {
        id:       ClilocId(500209),
        fallback: "You cannot peek into the container.",
    },
    Message {
        id:       ClilocId(500321),
        fallback: "Whom shall I examine?",
    },
    Message {
        id:       ClilocId(500323),
        fallback: "Only living things have anatomies!",
    },
    Message {
        id:       ClilocId(500324),
        fallback: "You know yourself quite well enough already.",
    },
    Message {
        id:       ClilocId(500328),
        fallback: "What animal should I look at?",
    },
    Message {
        id:       ClilocId(500329),
        fallback: "That's not an animal!",
    },
    Message {
        id:       ClilocId(500331),
        fallback: "The spirits of the dead are not the province of animal lore.",
    },
    Message {
        id:       ClilocId(500334),
        fallback: "You can't think of anything you know offhand.",
    },
    Message {
        id:       ClilocId(500343),
        fallback: "What do you wish to appraise and identify?",
    },
    Message {
        id:       ClilocId(500349),
        fallback: "What item do you wish to get information about?",
    },
    Message {
        id:       ClilocId(500352),
        fallback: "This is neither weapon nor armor.",
    },
    Message {
        id:       ClilocId(500353),
        fallback: "You are not certain...",
    },
    Message {
        id:       ClilocId(500366),
        fallback: "Select a loom to use that on.",
    },
    Message {
        id:       ClilocId(500367),
        fallback: "Try using that on a loom.",
    },
    Message {
        id:       ClilocId(500368),
        fallback: "You create some cloth and put it in your backpack.",
    },
    Message {
        id:       ClilocId(500397),
        fallback: "To whom do you wish to grovel?",
    },
    Message {
        id:       ClilocId(500398),
        fallback: "Perhaps just asking would work better.",
    },
    Message {
        id:       ClilocId(500399),
        fallback: "There is little chance of getting money from that!",
    },
    Message {
        id:       ClilocId(500401),
        fallback: "You are too far away to beg from him.",
    },
    Message {
        id:       ClilocId(500402),
        fallback: "You are too far away to beg from her.",
    },
    Message {
        id:       ClilocId(500404),
        fallback: "They seem unwilling to give you any money.",
    },
    Message {
        id:       ClilocId(500405),
        fallback: "I feel sorry for thee...",
    },
    Message {
        id:       ClilocId(500406),
        fallback: "Thou dost not look trustworthy... no gold for thee today!",
    },
    Message {
        id:       ClilocId(500407),
        fallback: "I have not enough money to give thee any!",
    },
    Message {
        id:       ClilocId(500446),
        fallback: "That is too far away.",
    },
    Message {
        id:       ClilocId(500449),
        fallback: "This sheep is not yet ready to be shorn.",
    },
    Message {
        id:       ClilocId(500450),
        fallback: "You can only skin dead creatures.",
    },
    Message {
        id:       ClilocId(500452),
        fallback: "You place the gathered wool into your backpack.",
    },
    Message {
        id:       ClilocId(500489),
        fallback: "You can't use an axe on that.",
    },
    Message {
        id:       ClilocId(500493),
        fallback: "There's not enough wood here to harvest.",
    },
    Message {
        id:       ClilocId(500495),
        fallback: "You hack at the tree for a while, but fail to produce any useable wood.",
    },
    Message {
        id:       ClilocId(500497),
        fallback: "You can't place any wood into your backpack!",
    },
    Message {
        id:       ClilocId(500498),
        fallback: "You put some logs into your backpack.",
    },
    Message {
        id:       ClilocId(500499),
        fallback: "You broke your axe.",
    },
    Message {
        id:       ClilocId(500612),
        fallback: "You play poorly, and there is no effect.",
    },
    Message {
        id:       ClilocId(500613),
        fallback: "You attempt to calm everyone, but fail.",
    },
    Message {
        id:       ClilocId(500617),
        fallback: "What instrument shall you play?",
    },
    Message {
        id:       ClilocId(500814),
        fallback: "You have been revealed!",
    },
    Message {
        id:       ClilocId(500817),
        fallback: "You can see nothing hidden there.",
    },
    Message {
        id:       ClilocId(500906),
        fallback: "What would you like to evaluate?",
    },
    Message {
        id:       ClilocId(500908),
        fallback: "It looks smarter than a rock, but dumber than a piece of wood.",
    },
    Message {
        id:       ClilocId(500910),
        fallback: "Hmm, that person looks really silly.",
    },
    Message {
        id:       ClilocId(500972),
        fallback: "You are already fishing.",
    },
    Message {
        id:       ClilocId(500974),
        fallback: "What water do you want to fish in?",
    },
    Message {
        id:       ClilocId(500976),
        fallback: "You need to be closer to the water to fish!",
    },
    Message {
        id:       ClilocId(500979),
        fallback: "You cannot see that location.",
    },
    Message {
        id:       ClilocId(501000),
        fallback: "Select what you want to examine.",
    },
    Message {
        id:       ClilocId(501001),
        fallback: "You cannot determine anything useful.",
    },
    Message {
        id:       ClilocId(501002),
        fallback: "This corpse has not been desecrated.",
    },
    Message {
        id:       ClilocId(501003),
        fallback: "You notice nothing unusual.",
    },
    Message {
        id:       ClilocId(501237),
        fallback: "You can't seem to hide right now.",
    },
    Message {
        id:       ClilocId(501240),
        fallback: "You have hidden yourself well.",
    },
    Message {
        id:       ClilocId(501241),
        fallback: "You fail to hide.",
    },
    Message {
        id:       ClilocId(501283),
        fallback: "That is locked.",
    },
    Message {
        id:       ClilocId(501587),
        fallback: "Whom do you wish to incite?",
    },
    Message {
        id:       ClilocId(501589),
        fallback: "You can't incite that!",
    },
    Message {
        id:       ClilocId(501593),
        fallback: "You can't tell someone to attack themselves!",
    },
    Message {
        id:       ClilocId(501599),
        fallback: "Your music fails to incite enough anger.",
    },
    Message {
        id:       ClilocId(501602),
        fallback: "Your music succeeds, as you start a fight.",
    },
    Message {
        id:       ClilocId(501629),
        fallback: "You inscribe the spell and put the scroll in your backpack.",
    },
    Message {
        id:       ClilocId(501630),
        fallback: "You fail to inscribe the scroll, and the scroll is ruined.",
    },
    Message {
        id:       ClilocId(501783),
        fallback: "You feel yourself resisting magical energy.",
    },
    Message {
        id:       ClilocId(501845),
        fallback: "You are busy doing something else and cannot focus.",
    },
    Message {
        id:       ClilocId(501846),
        fallback: "You are at peace.",
    },
    Message {
        id:       ClilocId(501849),
        fallback: "The mind is strong, but the body is weak.",
    },
    Message {
        id:       ClilocId(501850),
        fallback: "You cannot focus your concentration.",
    },
    Message {
        id:       ClilocId(501851),
        fallback: "You enter a meditative trance.",
    },
    Message {
        id:       ClilocId(501862),
        fallback: "You can't mine there.",
    },
    Message {
        id:       ClilocId(501942),
        fallback: "That location is blocked.",
    },
    Message {
        id:       ClilocId(501986),
        fallback: "You have no idea how to smelt this strange ore!",
    },
    Message {
        id:       ClilocId(501987),
        fallback: "There is not enough metal-bearing ore in this pile to make an ingot.",
    },
    Message {
        id:       ClilocId(501988),
        fallback: "You smelt the ore removing the impurities and put the metal in your backpack.",
    },
    Message {
        id:       ClilocId(501990),
        fallback: "You burn away the impurities but are left with less useable metal.",
    },
    Message {
        id:       ClilocId(502068),
        fallback: "What do you want to pick?",
    },
    Message {
        id:       ClilocId(502069),
        fallback: "This does not appear to be locked.",
    },
    Message {
        id:       ClilocId(502072),
        fallback: "You don't see how that lock can be manipulated.",
    },
    Message {
        id:       ClilocId(502074),
        fallback: "You broke the lockpick.",
    },
    Message {
        id:       ClilocId(502075),
        fallback: "You are unable to pick the lock.",
    },
    Message {
        id:       ClilocId(502076),
        fallback: "The lock quickly yields to your skill.",
    },
    Message {
        id:       ClilocId(502137),
        fallback: "Select the poison you wish to use.",
    },
    Message {
        id:       ClilocId(502139),
        fallback: "That is not a poison potion.",
    },
    Message {
        id:       ClilocId(502142),
        fallback: "To what do you wish to apply the poison?",
    },
    Message {
        id:       ClilocId(502145),
        fallback: "You cannot poison that! You can only poison bladed or piercing weapons, food or drink.",
    },
    Message {
        id:       ClilocId(502148),
        fallback: "You make a grave mistake while applying the poison.",
    },
    Message {
        id:       ClilocId(502366),
        fallback: "You do not know enough about locks.  Become better at picking locks.",
    },
    Message {
        id:       ClilocId(502367),
        fallback: "You are not perceptive enough.  Become better at detect hidden.",
    },
    Message {
        id:       ClilocId(502368),
        fallback: "Which trap will you attempt to disarm?",
    },
    Message {
        id:       ClilocId(502372),
        fallback: "You fail to disarm the trap...but you don't set it off.",
    },
    Message {
        id:       ClilocId(502373),
        fallback: "That doesn't appear to be trapped.",
    },
    Message {
        id:       ClilocId(502377),
        fallback: "You successfully render the trap harmless.",
    },
    Message {
        id:       ClilocId(502434),
        fallback: "What should I use these scissors on?",
    },
    Message {
        id:       ClilocId(502437),
        fallback: "Items you wish to cut must be in your backpack",
    },
    Message {
        id:       ClilocId(502440),
        fallback: "Scissors can not be used on that to produce anything.",
    },
    Message {
        id:       ClilocId(502443),
        fallback: "You fail your attempt at contacting the netherworld.",
    },
    Message {
        id:       ClilocId(502444),
        fallback: "You establish contact with the netherworld.",
    },
    Message {
        id:       ClilocId(502445),
        fallback: "You feel your contacts with the netherworld fade.",
    },
    Message {
        id:       ClilocId(502469),
        fallback: "That being cannot be tamed.",
    },
    Message {
        id:       ClilocId(502626),
        fallback: "Your hands must be free to cast spells or meditate.",
    },
    Message {
        id:       ClilocId(502655),
        fallback: "What spinning wheel do you wish to spin this on?",
    },
    Message {
        id:       ClilocId(502656),
        fallback: "That spinning wheel is being used.",
    },
    Message {
        id:       ClilocId(502658),
        fallback: "Use that on a spinning wheel.",
    },
    Message {
        id:       ClilocId(502698),
        fallback: "Which item will you attempt to steal?",
    },
    Message {
        id:       ClilocId(502704),
        fallback: "You catch yourself red-handed.",
    },
    Message {
        id:       ClilocId(502710),
        fallback: "You can't steal that.",
    },
    Message {
        id:       ClilocId(502711),
        fallback: "You can't steal that.",
    },
    Message {
        id:       ClilocId(502723),
        fallback: "You fail to steal the item.",
    },
    Message {
        id:       ClilocId(502724),
        fallback: "You successfully steal the item.",
    },
    Message {
        id:       ClilocId(502725),
        fallback: "You must hide first",
    },
    Message {
        id:       ClilocId(502726),
        fallback: "You are not hidden well enough.  Become better at hiding.",
    },
    Message {
        id:       ClilocId(502727),
        fallback: "You could not hope to move quietly wearing this much armor.",
    },
    Message {
        id:       ClilocId(502730),
        fallback: "You begin to move quietly.",
    },
    Message {
        id:       ClilocId(502731),
        fallback: "You fail in your attempt to move unnoticed.",
    },
    Message {
        id:       ClilocId(502789),
        fallback: "Tame which animal?",
    },
    Message {
        id:       ClilocId(502799),
        fallback: "It seems to accept you as master.",
    },
    Message {
        id:       ClilocId(502804),
        fallback: "That animal looks tame already.",
    },
    Message {
        id:       ClilocId(502805),
        fallback: "You seem to anger the beast!",
    },
    Message {
        id:       ClilocId(502806),
        fallback: "You have no chance of taming this creature.",
    },
    Message {
        id:       ClilocId(502807),
        fallback: "What would you like to taste?",
    },
    Message {
        id:       ClilocId(502816),
        fallback: "You feel that such an action would be inappropriate.",
    },
    Message {
        id:       ClilocId(502823),
        fallback: "You cannot discern anything about this substance.",
    },
    Message {
        id:       ClilocId(502998),
        fallback: "A dart imbeds itself in your flesh!",
    },
    Message {
        id:       ClilocId(502999),
        fallback: "You set off a trap!",
    },
    Message {
        id:       ClilocId(503000),
        fallback: "Your skin blisters from the heat!",
    },
    Message {
        id:       ClilocId(503004),
        fallback: "You are enveloped in a noxious green cloud!",
    },
    Message {
        id:       ClilocId(503033),
        fallback: "Where do you wish to dig?",
    },
    Message {
        id:       ClilocId(503040),
        fallback: "There is no metal here to mine.",
    },
    Message {
        id:       ClilocId(503041),
        fallback: "You have moved too far away to continue mining.",
    },
    Message {
        id:       ClilocId(503042),
        fallback: "Someone has gotten to the metal before you.",
    },
    Message {
        id:       ClilocId(503043),
        fallback: "You loosen some rocks but fail to find any useable ore.",
    },
    Message {
        id:       ClilocId(503171),
        fallback: "You fish a while, but fail to catch anything.",
    },
    Message {
        id:       ClilocId(503172),
        fallback: "The fish don't seem to be biting here.",
    },
    Message {
        id:       ClilocId(503174),
        fallback: "You broke your fishing pole.",
    },
    Message {
        id:       ClilocId(503176),
        fallback: "You do not have room in your backpack for a fish.",
    },
    Message {
        id:       ClilocId(1005049),
        fallback: "That cannot be dispelled.",
    },
    Message {
        id:       ClilocId(1007072),
        fallback: "You dig some iron ore and put it in your backpack.",
    },
    Message {
        id:       ClilocId(1007073),
        fallback: "You dig some dull copper ore and put it in your backpack.",
    },
    Message {
        id:       ClilocId(1007074),
        fallback: "You dig some shadow iron ore and put it in your backpack.",
    },
    Message {
        id:       ClilocId(1007075),
        fallback: "You dig some copper ore and put it in your backpack.",
    },
    Message {
        id:       ClilocId(1007076),
        fallback: "You dig some bronze ore and put it in your backpack.",
    },
    Message {
        id:       ClilocId(1007077),
        fallback: "You dig some golden ore and put it in your backpack.",
    },
    Message {
        id:       ClilocId(1007078),
        fallback: "You dig some agapite ore and put it in your backpack.",
    },
    Message {
        id:       ClilocId(1007079),
        fallback: "You dig some verite ore and put it in your backpack.",
    },
    Message {
        id:       ClilocId(1007080),
        fallback: "You dig some valorite ore and put it in your backpack.",
    },
    Message {
        id:       ClilocId(1008085),
        fallback: "You play your music and your target becomes angered.  Whom do you wish them to attack?",
    },
    // The loom's four loading lines. ServUO sends them as `1010001 + Phase++`,
    // so they must stay a consecutive run — see `items::weave`.
    Message {
        id:       ClilocId(1010001),
        fallback: "The bolt of cloth has just been started.",
    },
    Message {
        id:       ClilocId(1010002),
        fallback: "The bolt of cloth needs quite a bit more.",
    },
    Message {
        id:       ClilocId(1010003),
        fallback: "The bolt of cloth needs a little more.",
    },
    Message {
        id:       ClilocId(1010004),
        fallback: "The bolt of cloth is almost finished.",
    },
    Message {
        id:       ClilocId(1010018),
        fallback: "What do you want to use this item on?",
    },
    Message {
        id:       ClilocId(1010084),
        fallback: "The creature resisted the attempt to dispel it!",
    },
    Message {
        id:       ClilocId(1010481),
        fallback: "Your backpack is full, so the ore you mined is lost.",
    },
    Message {
        id:       ClilocId(1010516),
        fallback: "You fail to apply a sufficient dose of poison on the blade.",
    },
    Message {
        id:       ClilocId(1010517),
        fallback: "You apply the poison.",
    },
    Message {
        id:       ClilocId(1010518),
        fallback: "You fail to apply a sufficient dose of poison.",
    },
    Message {
        id:       ClilocId(1010574),
        fallback: "You put a ball of yarn in your backpack.",
    },
    Message {
        id:       ClilocId(1010576),
        fallback: "You put the balls of yarn in your backpack.",
    },
    Message {
        id:       ClilocId(1010577),
        fallback: "You put the spools of thread in your backpack.",
    },
    Message {
        id:       ClilocId(1010585),
        fallback: "Both hands must be free to steal.",
    },
    Message {
        id:       ClilocId(1010597),
        fallback: "*You start to tame the creature.*",
    },
    Message {
        id:       ClilocId(1010600),
        fallback: "You detect nothing unusual about this substance.",
    },
    Message {
        id:       ClilocId(1011441),
        fallback: "EXIT",
    },
    Message {
        id:       ClilocId(1019040),
        fallback: "You shove them out of the way.",
    },
    Message {
        id:       ClilocId(1019041),
        fallback: "You shove something invisible out of the way.",
    },
    Message {
        id:       ClilocId(1019042),
        fallback: "Being perfectly rested, you shove them out of the way.",
    },
    Message {
        id:       ClilocId(1019043),
        fallback: "Being perfectly rested, you shove something invisible out of the way.",
    },
    Message {
        id:       ClilocId(1019045),
        fallback: "I can't reach that.",
    },
    Message {
        id:       ClilocId(1028335),
        fallback: "Strength",
    },
    Message {
        id:       ClilocId(1038045),
        fallback: "That looks like they have trouble lifting small objects and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038046),
        fallback: "That looks like they have trouble lifting small objects and very clumsy.",
    },
    Message {
        id:       ClilocId(1038047),
        fallback: "That looks like they have trouble lifting small objects and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038048),
        fallback: "That looks like they have trouble lifting small objects and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038049),
        fallback: "That looks like they have trouble lifting small objects and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038050),
        fallback: "That looks like they have trouble lifting small objects and very agile.",
    },
    Message {
        id:       ClilocId(1038051),
        fallback: "That looks like they have trouble lifting small objects and extremely agile.",
    },
    Message {
        id:       ClilocId(1038052),
        fallback: "That looks like they have trouble lifting small objects and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038053),
        fallback: "That looks like they have trouble lifting small objects and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038054),
        fallback: "That looks like they have trouble lifting small objects and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038055),
        fallback: "That looks like they have trouble lifting small objects and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038056),
        fallback: "That looks rather feeble and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038057),
        fallback: "That looks rather feeble and very clumsy.",
    },
    Message {
        id:       ClilocId(1038058),
        fallback: "That looks rather feeble and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038059),
        fallback: "That looks rather feeble and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038060),
        fallback: "That looks rather feeble and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038061),
        fallback: "That looks rather feeble and very agile.",
    },
    Message {
        id:       ClilocId(1038062),
        fallback: "That looks rather feeble and extremely agile.",
    },
    Message {
        id:       ClilocId(1038063),
        fallback: "That looks rather feeble and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038064),
        fallback: "That looks rather feeble and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038065),
        fallback: "That looks rather feeble and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038066),
        fallback: "That looks rather feeble and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038067),
        fallback: "That looks somewhat weak and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038068),
        fallback: "That looks somewhat weak and very clumsy.",
    },
    Message {
        id:       ClilocId(1038069),
        fallback: "That looks somewhat weak and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038070),
        fallback: "That looks somewhat weak and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038071),
        fallback: "That looks somewhat weak and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038072),
        fallback: "That looks somewhat weak and very agile.",
    },
    Message {
        id:       ClilocId(1038073),
        fallback: "That looks somewhat weak and extremely agile.",
    },
    Message {
        id:       ClilocId(1038074),
        fallback: "That looks somewhat weak and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038075),
        fallback: "That looks somewhat weak and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038076),
        fallback: "That looks somewhat weak and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038077),
        fallback: "That looks somewhat weak and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038078),
        fallback: "That looks to be of normal strength and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038079),
        fallback: "That looks to be of normal strength and very clumsy.",
    },
    Message {
        id:       ClilocId(1038080),
        fallback: "That looks to be of normal strength and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038081),
        fallback: "That looks to be of normal strength and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038082),
        fallback: "That looks to be of normal strength and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038083),
        fallback: "That looks to be of normal strength and very agile.",
    },
    Message {
        id:       ClilocId(1038084),
        fallback: "That looks to be of normal strength and extremely agile.",
    },
    Message {
        id:       ClilocId(1038085),
        fallback: "That looks to be of normal strength and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038086),
        fallback: "That looks to be of normal strength and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038087),
        fallback: "That looks to be of normal strength and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038088),
        fallback: "That looks to be of normal strength and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038089),
        fallback: "That looks somewhat strong and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038090),
        fallback: "That looks somewhat strong and very clumsy.",
    },
    Message {
        id:       ClilocId(1038091),
        fallback: "That looks somewhat strong and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038092),
        fallback: "That looks somewhat strong and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038093),
        fallback: "That looks somewhat strong and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038094),
        fallback: "That looks somewhat strong and very agile.",
    },
    Message {
        id:       ClilocId(1038095),
        fallback: "That looks somewhat strong and extremely agile.",
    },
    Message {
        id:       ClilocId(1038096),
        fallback: "That looks somewhat strong and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038097),
        fallback: "That looks somewhat strong and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038098),
        fallback: "That looks somewhat strong and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038099),
        fallback: "That looks somewhat strong and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038100),
        fallback: "That looks very strong and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038101),
        fallback: "That looks very strong and very clumsy.",
    },
    Message {
        id:       ClilocId(1038102),
        fallback: "That looks very strong and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038103),
        fallback: "That looks very strong and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038104),
        fallback: "That looks very strong and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038105),
        fallback: "That looks very strong and very agile.",
    },
    Message {
        id:       ClilocId(1038106),
        fallback: "That looks very strong and extremely agile.",
    },
    Message {
        id:       ClilocId(1038107),
        fallback: "That looks very strong and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038108),
        fallback: "That looks very strong and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038109),
        fallback: "That looks very strong and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038110),
        fallback: "That looks very strong and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038111),
        fallback: "That looks extremely strong and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038112),
        fallback: "That looks extremely strong and very clumsy.",
    },
    Message {
        id:       ClilocId(1038113),
        fallback: "That looks extremely strong and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038114),
        fallback: "That looks extremely strong and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038115),
        fallback: "That looks extremely strong and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038116),
        fallback: "That looks extremely strong and very agile.",
    },
    Message {
        id:       ClilocId(1038117),
        fallback: "That looks extremely strong and extremely agile.",
    },
    Message {
        id:       ClilocId(1038118),
        fallback: "That looks extremely strong and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038119),
        fallback: "That looks extremely strong and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038120),
        fallback: "That looks extremely strong and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038121),
        fallback: "That looks extremely strong and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038122),
        fallback: "That looks extraordinarily strong and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038123),
        fallback: "That looks extraordinarily strong and very clumsy.",
    },
    Message {
        id:       ClilocId(1038124),
        fallback: "That looks extraordinarily strong and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038125),
        fallback: "That looks extraordinarily strong and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038126),
        fallback: "That looks extraordinarily strong and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038127),
        fallback: "That looks extraordinarily strong and very agile.",
    },
    Message {
        id:       ClilocId(1038128),
        fallback: "That looks extraordinarily strong and extremely agile.",
    },
    Message {
        id:       ClilocId(1038129),
        fallback: "That looks extraordinarily strong and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038130),
        fallback: "That looks extraordinarily strong and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038131),
        fallback: "That looks extraordinarily strong and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038132),
        fallback: "That looks extraordinarily strong and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038133),
        fallback: "That looks strong as an ox and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038134),
        fallback: "That looks strong as an ox and very clumsy.",
    },
    Message {
        id:       ClilocId(1038135),
        fallback: "That looks strong as an ox and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038136),
        fallback: "That looks strong as an ox and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038137),
        fallback: "That looks strong as an ox and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038138),
        fallback: "That looks strong as an ox and very agile.",
    },
    Message {
        id:       ClilocId(1038139),
        fallback: "That looks strong as an ox and extremely agile.",
    },
    Message {
        id:       ClilocId(1038140),
        fallback: "That looks strong as an ox and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038141),
        fallback: "That looks strong as an ox and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038142),
        fallback: "That looks strong as an ox and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038143),
        fallback: "That looks strong as an ox and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038144),
        fallback: "That looks stronger than anything you have ever seen and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038145),
        fallback: "That looks stronger than anything you have ever seen and very clumsy.",
    },
    Message {
        id:       ClilocId(1038146),
        fallback: "That looks stronger than anything you have ever seen and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038147),
        fallback: "That looks stronger than anything you have ever seen and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038148),
        fallback: "That looks stronger than anything you have ever seen and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038149),
        fallback: "That looks stronger than anything you have ever seen and very agile.",
    },
    Message {
        id:       ClilocId(1038150),
        fallback: "That looks stronger than anything you have ever seen and extremely agile.",
    },
    Message {
        id:       ClilocId(1038151),
        fallback: "That looks stronger than anything you have ever seen and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038152),
        fallback: "That looks stronger than anything you have ever seen and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038153),
        fallback: "That looks stronger than anything you have ever seen and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038154),
        fallback: "That looks stronger than anything you have ever seen and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038155),
        fallback: "That looks superhumanly strong and like they barely manage to stay standing.",
    },
    Message {
        id:       ClilocId(1038156),
        fallback: "That looks superhumanly strong and very clumsy.",
    },
    Message {
        id:       ClilocId(1038157),
        fallback: "That looks superhumanly strong and somewhat uncoordinated.",
    },
    Message {
        id:       ClilocId(1038158),
        fallback: "That looks superhumanly strong and moderately dexterous.",
    },
    Message {
        id:       ClilocId(1038159),
        fallback: "That looks superhumanly strong and somewhat agile.",
    },
    Message {
        id:       ClilocId(1038160),
        fallback: "That looks superhumanly strong and very agile.",
    },
    Message {
        id:       ClilocId(1038161),
        fallback: "That looks superhumanly strong and extremely agile.",
    },
    Message {
        id:       ClilocId(1038162),
        fallback: "That looks superhumanly strong and extraordinarily agile.",
    },
    Message {
        id:       ClilocId(1038163),
        fallback: "That looks superhumanly strong and moves like quicksilver.",
    },
    Message {
        id:       ClilocId(1038164),
        fallback: "That looks superhumanly strong and faster than anything you have ever seen.",
    },
    Message {
        id:       ClilocId(1038165),
        fallback: "That looks superhumanly strong and superhumanly agile.",
    },
    Message {
        id:       ClilocId(1038166),
        fallback: "You cannot quite judge his mental abilities.",
    },
    Message {
        id:       ClilocId(1038167),
        fallback: "You cannot quite judge her mental abilities.",
    },
    Message {
        id:       ClilocId(1038168),
        fallback: "You cannot quite judge its mental abilities.",
    },
    Message {
        id:       ClilocId(1038169),
        fallback: "He looks slightly less intelligent than a rock.",
    },
    Message {
        id:       ClilocId(1038170),
        fallback: "He looks fairly stupid.",
    },
    Message {
        id:       ClilocId(1038171),
        fallback: "He looks not the brightest.",
    },
    Message {
        id:       ClilocId(1038172),
        fallback: "He looks about average.",
    },
    Message {
        id:       ClilocId(1038173),
        fallback: "He looks moderately intelligent.",
    },
    Message {
        id:       ClilocId(1038174),
        fallback: "He looks very intelligent.",
    },
    Message {
        id:       ClilocId(1038175),
        fallback: "He looks extremely intelligent.",
    },
    Message {
        id:       ClilocId(1038176),
        fallback: "He looks extraordinarily intelligent.",
    },
    Message {
        id:       ClilocId(1038177),
        fallback: "He looks like a formidable intellect, well beyond even the extraordinary.",
    },
    Message {
        id:       ClilocId(1038178),
        fallback: "He looks like a definite genius.",
    },
    Message {
        id:       ClilocId(1038179),
        fallback: "He looks superhumanly intelligent in a manner you cannot comprehend.",
    },
    Message {
        id:       ClilocId(1038180),
        fallback: "She looks slightly less intelligent than a rock.",
    },
    Message {
        id:       ClilocId(1038181),
        fallback: "She looks fairly stupid.",
    },
    Message {
        id:       ClilocId(1038182),
        fallback: "She looks not the brightest.",
    },
    Message {
        id:       ClilocId(1038183),
        fallback: "She looks about average.",
    },
    Message {
        id:       ClilocId(1038184),
        fallback: "She looks moderately intelligent.",
    },
    Message {
        id:       ClilocId(1038185),
        fallback: "She looks very intelligent.",
    },
    Message {
        id:       ClilocId(1038186),
        fallback: "She looks extremely intelligent.",
    },
    Message {
        id:       ClilocId(1038187),
        fallback: "She looks extraordinarily intelligent.",
    },
    Message {
        id:       ClilocId(1038188),
        fallback: "She looks like a formidable intellect, well beyond even the extraordinary.",
    },
    Message {
        id:       ClilocId(1038189),
        fallback: "She looks like a definite genius.",
    },
    Message {
        id:       ClilocId(1038190),
        fallback: "She looks superhumanly intelligent in a manner you cannot comprehend.",
    },
    Message {
        id:       ClilocId(1038191),
        fallback: "It looks slightly less intelligent than a rock.",
    },
    Message {
        id:       ClilocId(1038192),
        fallback: "It looks fairly stupid.",
    },
    Message {
        id:       ClilocId(1038193),
        fallback: "It looks not the brightest.",
    },
    Message {
        id:       ClilocId(1038194),
        fallback: "It looks about average.",
    },
    Message {
        id:       ClilocId(1038195),
        fallback: "It looks moderately intelligent.",
    },
    Message {
        id:       ClilocId(1038196),
        fallback: "It looks very intelligent.",
    },
    Message {
        id:       ClilocId(1038197),
        fallback: "It looks extremely intelligent.",
    },
    Message {
        id:       ClilocId(1038198),
        fallback: "It looks extraordinarily intelligent.",
    },
    Message {
        id:       ClilocId(1038199),
        fallback: "It looks like a formidable intellect, well beyond even the extraordinary.",
    },
    Message {
        id:       ClilocId(1038200),
        fallback: "It looks like a definite genius.",
    },
    Message {
        id:       ClilocId(1038201),
        fallback: "It looks superhumanly intelligent in a manner you cannot comprehend.",
    },
    Message {
        id:       ClilocId(1038202),
        fallback: "This being is at zero percent mental strength.",
    },
    Message {
        id:       ClilocId(1038203),
        fallback: "This being is at ten percent mental strength.",
    },
    Message {
        id:       ClilocId(1038204),
        fallback: "This being is at twenty percent mental strength.",
    },
    Message {
        id:       ClilocId(1038205),
        fallback: "This being is at thirty percent mental strength.",
    },
    Message {
        id:       ClilocId(1038206),
        fallback: "This being is at forty percent mental strength.",
    },
    Message {
        id:       ClilocId(1038207),
        fallback: "This being is at fifty percent mental strength.",
    },
    Message {
        id:       ClilocId(1038208),
        fallback: "This being is at sixty percent mental strength.",
    },
    Message {
        id:       ClilocId(1038209),
        fallback: "This being is at seventy percent mental strength.",
    },
    Message {
        id:       ClilocId(1038210),
        fallback: "This being is at eighty percent mental strength.",
    },
    Message {
        id:       ClilocId(1038211),
        fallback: "This being is at ninety percent mental strength.",
    },
    Message {
        id:       ClilocId(1038212),
        fallback: "This being is at one-hundred percent mental strength.",
    },
    Message {
        id:       ClilocId(1038216),
        fallback: "This weapon might scratch your opponent slightly when you hit someone with it at short range.",
    },
    Message {
        id:       ClilocId(1038217),
        fallback: "This weapon might scratch your opponent slightly when you hit someone with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038218),
        fallback: "This weapon might scratch your opponent slightly when you stabbed with it at short range.",
    },
    Message {
        id:       ClilocId(1038219),
        fallback: "This weapon might scratch your opponent slightly when you stabbed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038220),
        fallback: "This weapon might scratch your opponent slightly when you slashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038221),
        fallback: "This weapon might scratch your opponent slightly when you slashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038222),
        fallback: "This weapon might scratch your opponent slightly when you bashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038223),
        fallback: "This weapon might scratch your opponent slightly when you bashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038224),
        fallback: "This weapon might scratch your opponent slightly when you shot someone with it at long range.",
    },
    Message {
        id:       ClilocId(1038225),
        fallback: "This weapon would do minimal damage when you hit someone with it at short range.",
    },
    Message {
        id:       ClilocId(1038226),
        fallback: "This weapon would do minimal damage when you hit someone with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038227),
        fallback: "This weapon would do minimal damage when you stabbed with it at short range.",
    },
    Message {
        id:       ClilocId(1038228),
        fallback: "This weapon would do minimal damage when you stabbed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038229),
        fallback: "This weapon would do minimal damage when you slashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038230),
        fallback: "This weapon would do minimal damage when you slashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038231),
        fallback: "This weapon would do minimal damage when you bashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038232),
        fallback: "This weapon would do minimal damage when you bashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038233),
        fallback: "This weapon would do minimal damage when you shot someone with it at long range.",
    },
    Message {
        id:       ClilocId(1038234),
        fallback: "This weapon would do some damage when you hit someone with it at short range.",
    },
    Message {
        id:       ClilocId(1038235),
        fallback: "This weapon would do some damage when you hit someone with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038236),
        fallback: "This weapon would do some damage when you stabbed with it at short range.",
    },
    Message {
        id:       ClilocId(1038237),
        fallback: "This weapon would do some damage when you stabbed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038238),
        fallback: "This weapon would do some damage when you slashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038239),
        fallback: "This weapon would do some damage when you slashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038240),
        fallback: "This weapon would do some damage when you bashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038241),
        fallback: "This weapon would do some damage when you bashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038242),
        fallback: "This weapon would do some damage when you shot someone with it at long range.",
    },
    Message {
        id:       ClilocId(1038243),
        fallback: "This weapon would probably hurt your opponent a fair amount when you hit someone with it at short range.",
    },
    Message {
        id:       ClilocId(1038244),
        fallback: "This weapon would probably hurt your opponent a fair amount when you hit someone with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038245),
        fallback: "This weapon would probably hurt your opponent a fair amount when you stabbed with it at short range.",
    },
    Message {
        id:       ClilocId(1038246),
        fallback: "This weapon would probably hurt your opponent a fair amount when you stabbed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038247),
        fallback: "This weapon would probably hurt your opponent a fair amount when you slashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038248),
        fallback: "This weapon would probably hurt your opponent a fair amount when you slashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038249),
        fallback: "This weapon would probably hurt your opponent a fair amount when you bashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038250),
        fallback: "This weapon would probably hurt your opponent a fair amount when you bashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038251),
        fallback: "This weapon would probably hurt your opponent a fair amount when you shot someone with it at long range.",
    },
    Message {
        id:       ClilocId(1038252),
        fallback: "This weapon would inflict quite a lot of damage and pain when you hit someone with it at short range.",
    },
    Message {
        id:       ClilocId(1038253),
        fallback: "This weapon would inflict quite a lot of damage and pain when you hit someone with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038254),
        fallback: "This weapon would inflict quite a lot of damage and pain when you stabbed with it at short range.",
    },
    Message {
        id:       ClilocId(1038255),
        fallback: "This weapon would inflict quite a lot of damage and pain when you stabbed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038256),
        fallback: "This weapon would inflict quite a lot of damage and pain when you slashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038257),
        fallback: "This weapon would inflict quite a lot of damage and pain when you slashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038258),
        fallback: "This weapon would inflict quite a lot of damage and pain when you bashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038259),
        fallback: "This weapon would inflict quite a lot of damage and pain when you bashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038260),
        fallback: "This weapon would inflict quite a lot of damage and pain when you shot someone with it at long range.",
    },
    Message {
        id:       ClilocId(1038261),
        fallback: "This weapon would be a superior weapon when you hit someone with it at short range.",
    },
    Message {
        id:       ClilocId(1038262),
        fallback: "This weapon would be a superior weapon when you hit someone with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038263),
        fallback: "This weapon would be a superior weapon when you stabbed with it at short range.",
    },
    Message {
        id:       ClilocId(1038264),
        fallback: "This weapon would be a superior weapon when you stabbed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038265),
        fallback: "This weapon would be a superior weapon when you slashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038266),
        fallback: "This weapon would be a superior weapon when you slashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038267),
        fallback: "This weapon would be a superior weapon when you bashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038268),
        fallback: "This weapon would be a superior weapon when you bashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038269),
        fallback: "This weapon would be a superior weapon when you shot someone with it at long range.",
    },
    Message {
        id:       ClilocId(1038270),
        fallback: "This weapon would be extraordinarily deadly when you hit someone with it at short range.",
    },
    Message {
        id:       ClilocId(1038271),
        fallback: "This weapon would be extraordinarily deadly when you hit someone with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038272),
        fallback: "This weapon would be extraordinarily deadly when you stabbed with it at short range.",
    },
    Message {
        id:       ClilocId(1038273),
        fallback: "This weapon would be extraordinarily deadly when you stabbed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038274),
        fallback: "This weapon would be extraordinarily deadly when you slashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038275),
        fallback: "This weapon would be extraordinarily deadly when you slashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038276),
        fallback: "This weapon would be extraordinarily deadly when you bashed with it at short range.",
    },
    Message {
        id:       ClilocId(1038277),
        fallback: "This weapon would be extraordinarily deadly when you bashed with it two-handed at short range.",
    },
    Message {
        id:       ClilocId(1038278),
        fallback: "This weapon would be extraordinarily deadly when you shot someone with it at long range.",
    },
    Message {
        id:       ClilocId(1038284),
        fallback: "It appears to have poison smeared on it.",
    },
    Message {
        id:       ClilocId(1038295),
        fallback: "This armor offers no defense against attackers.",
    },
    Message {
        id:       ClilocId(1038296),
        fallback: "This armor provides almost no protection.",
    },
    Message {
        id:       ClilocId(1038297),
        fallback: "This armor provides very little protection.",
    },
    Message {
        id:       ClilocId(1038298),
        fallback: "This armor offers some protection against blows.",
    },
    Message {
        id:       ClilocId(1038299),
        fallback: "This armor serves as sturdy protection.",
    },
    Message {
        id:       ClilocId(1038300),
        fallback: "This armor is a superior defense against attack.",
    },
    Message {
        id:       ClilocId(1038301),
        fallback: "This armor offers excellent protection.",
    },
    Message {
        id:       ClilocId(1038302),
        fallback: "This armor is superbly crafted to provide maximum protection.",
    },
    Message {
        id:       ClilocId(1038303),
        fallback: "This being is at zero percent endurance.",
    },
    Message {
        id:       ClilocId(1038304),
        fallback: "This being is at ten percent endurance.",
    },
    Message {
        id:       ClilocId(1038305),
        fallback: "This being is at twenty percent endurance.",
    },
    Message {
        id:       ClilocId(1038306),
        fallback: "This being is at thirty percent endurance.",
    },
    Message {
        id:       ClilocId(1038307),
        fallback: "This being is at forty percent endurance.",
    },
    Message {
        id:       ClilocId(1038308),
        fallback: "This being is at fifty percent endurance.",
    },
    Message {
        id:       ClilocId(1038309),
        fallback: "This being is at sixty percent endurance.",
    },
    Message {
        id:       ClilocId(1038310),
        fallback: "This being is at seventy percent endurance.",
    },
    Message {
        id:       ClilocId(1038311),
        fallback: "This being is at eighty percent endurance.",
    },
    Message {
        id:       ClilocId(1038312),
        fallback: "This being is at ninety percent endurance.",
    },
    Message {
        id:       ClilocId(1038313),
        fallback: "This being is at one-hundred percent endurance.",
    },
    Message {
        id:       ClilocId(1041349),
        fallback: "It appears to be:",
    },
    Message {
        id:       ClilocId(1041351),
        fallback: "You guess the value of that item at:",
    },
    Message {
        id:       ClilocId(1041352),
        fallback: "You have no idea how much it might be worth.",
    },
    Message {
        id:       ClilocId(1042001),
        fallback: "That must be in your pack for you to use it.",
    },
    Message {
        id:       ClilocId(1042404),
        fallback: "You don't have that spell!",
    },
    Message {
        id:       ClilocId(1042666),
        fallback: "You cannot quite get a sense of their physical characteristics.",
    },
    Message {
        id:       ClilocId(1042750),
        fallback: "The forensicist  ~1_NAME~ has already discovered that:",
    },
    Message {
        id:       ClilocId(1042751),
        fallback: "This person was killed by ~1_KILLER_NAME~.",
    },
    Message {
        id:       ClilocId(1042752),
        fallback: "This body has been disturbed by ~1_PLAYERS~",
    },
    Message {
        id:       ClilocId(1043297),
        fallback: "You pull out ~1_ITEM_NAME~!",
    },
    Message {
        id:       ClilocId(1044010),
        fallback: "<CENTER>CATEGORIES</CENTER>",
    },
    Message {
        id:       ClilocId(1044011),
        fallback: "<CENTER>SELECTIONS</CENTER>",
    },
    Message {
        id:       ClilocId(1044012),
        fallback: "<CENTER>NOTICES</CENTER>",
    },
    Message {
        id:       ClilocId(1044037),
        fallback: "You do not have sufficient metal to make that.",
    },
    Message {
        id:       ClilocId(1044038),
        fallback: "You have worn out your tool!",
    },
    Message {
        id:       ClilocId(1044043),
        fallback: "You failed to create the item, and some of your materials are lost.",
    },
    Message {
        id:       ClilocId(1044044),
        fallback: "PREV PAGE",
    },
    Message {
        id:       ClilocId(1044045),
        fallback: "NEXT PAGE",
    },
    Message {
        id:       ClilocId(1044053),
        fallback: "ITEM",
    },
    Message {
        id:       ClilocId(1044055),
        fallback: "<CENTER>MATERIALS</CENTER>",
    },
    Message {
        id:       ClilocId(1044056),
        fallback: "<CENTER>OTHER</CENTER>",
    },
    Message {
        id:       ClilocId(1044057),
        fallback: "Success Chance:",
    },
    Message {
        id:       ClilocId(1044058),
        fallback: "Exceptional Chance:",
    },
    Message {
        id:       ClilocId(1044059),
        fallback: "This item may hold its maker's mark",
    },
    Message {
        id:       ClilocId(1044061),
        fallback: "Anatomy",
    },
    Message {
        id:       ClilocId(1044076),
        fallback: "Eval Intelligence",
    },
    Message {
        id:       ClilocId(1044085),
        fallback: "Magery",
    },
    Message {
        id:       ClilocId(1044086),
        fallback: "Resisting Spells",
    },
    Message {
        id:       ClilocId(1044087),
        fallback: "Tactics",
    },
    Message {
        id:       ClilocId(1044090),
        fallback: "Poisoning",
    },
    Message {
        id:       ClilocId(1044103),
        fallback: "Wrestling",
    },
    Message {
        id:       ClilocId(1044106),
        fallback: "Meditation",
    },
    Message {
        id:       ClilocId(1044150),
        fallback: "BACK",
    },
    Message {
        id:       ClilocId(1044151),
        fallback: "MAKE NOW",
    },
    Message {
        id:       ClilocId(1044153),
        fallback: "You don't have the required skills to attempt this item.",
    },
    Message {
        id:       ClilocId(1044154),
        fallback: "You create the item.",
    },
    Message {
        id:       ClilocId(1044155),
        fallback: "You create an exceptional quality item.",
    },
    Message {
        id:       ClilocId(1044156),
        fallback: "You create an exceptional quality item and affix your maker's mark.",
    },
    Message {
        id:       ClilocId(1044157),
        fallback: "You fail to create the item, but no materials were lost.",
    },
    Message {
        id:       ClilocId(1044263),
        fallback: "The tool must be on your person to use.",
    },
    Message {
        id:       ClilocId(1044267),
        fallback: "You must be near an anvil and a forge to smith items.",
    },
    Message {
        id:       ClilocId(1044629),
        fallback: "There is no sand here to mine.",
    },
    Message {
        id:       ClilocId(1044630),
        fallback: "You dig for a while but fail to find any of sufficient quality for glassblowing.",
    },
    Message {
        id:       ClilocId(1044631),
        fallback: "You carefully dig up sand of sufficient quality for glassblowing.",
    },
    Message {
        id:       ClilocId(1044632),
        fallback: "Your backpack can't hold the sand, and it is lost!",
    },
    Message {
        id:       ClilocId(1046026),
        fallback: "Quest Log",
    },
    Message {
        id:       ClilocId(1048176),
        fallback: "Makes as many as possible at once",
    },
    Message {
        id:       ClilocId(1049000),
        fallback: "Confirm Quest Cancellation",
    },
    Message {
        id:       ClilocId(1049005),
        fallback: "Yes, I really want to quit this quest!",
    },
    Message {
        id:       ClilocId(1049006),
        fallback: "No, I don't want to quit.",
    },
    Message {
        id:       ClilocId(1049010),
        fallback: "Quest Offer",
    },
    Message {
        id:       ClilocId(1049073),
        fallback: "Objective:",
    },
    Message {
        id:       ClilocId(1049525),
        fallback: "Whom do you wish to calm?",
    },
    Message {
        id:       ClilocId(1049528),
        fallback: "You cannot calm that!",
    },
    Message {
        id:       ClilocId(1049531),
        fallback: "You attempt to calm your target, but fail.",
    },
    Message {
        id:       ClilocId(1049532),
        fallback: "You play hypnotic music, calming your target.",
    },
    Message {
        id:       ClilocId(1049539),
        fallback: "You play jarring music, suppressing your target's strength.",
    },
    Message {
        id:       ClilocId(1049540),
        fallback: "You attempt to disrupt your target, but fail.",
    },
    Message {
        id:       ClilocId(1049541),
        fallback: "Choose the target for your song of discordance.",
    },
    Message {
        id:       ClilocId(1049578),
        fallback: "Hits",
    },
    Message {
        id:       ClilocId(1049579),
        fallback: "Stamina",
    },
    Message {
        id:       ClilocId(1049580),
        fallback: "Mana",
    },
    Message {
        id:       ClilocId(1049581),
        fallback: "Armor Rating",
    },
    Message {
        id:       ClilocId(1049593),
        fallback: "Attributes",
    },
    Message {
        id:       ClilocId(1049611),
        fallback: "You have too many followers to tame that creature.",
    },
    Message {
        id:       ClilocId(1049645),
        fallback: "You have too many followers to summon that creature.",
    },
    Message {
        id:       ClilocId(1049655),
        fallback: "That creature cannot be tamed.",
    },
    Message {
        id:       ClilocId(1049674),
        fallback: "At your skill level, you can only lore tamed creatures.",
    },
    Message {
        id:       ClilocId(1049675),
        fallback: "At your skill level, you can only lore tamed or tameable creatures.",
    },
    Message {
        id:       ClilocId(1050039),
        fallback: "~1_NUMBER~ ~2_ITEMNAME~",
    },
    Message {
        id:       ClilocId(1050043),
        fallback: "crafted by ~1_NAME~",
    },
    Message {
        id:       ClilocId(1050045),
        fallback: "~1_PREFIX~~2_NAME~~3_SUFFIX~",
    },
    Message {
        id:       ClilocId(1060636),
        fallback: "exceptional",
    },
    Message {
        id:       ClilocId(1061646),
        fallback: "Physical",
    },
    Message {
        id:       ClilocId(1062379),
        fallback: "Est. time remaining:",
    },
    Message {
        id:       ClilocId(1062727),
        fallback: "You cannot trade with someone who is dragging something.",
    },
    Message {
        id:       ClilocId(1062779),
        fallback: "That person is already involved in a trade",
    },
    Message {
        id:       ClilocId(1062781),
        fallback: "You are already trading with someone else!",
    },
    Message {
        id:       ClilocId(1072061),
        fallback: "You hear jarring music, suppressing your strength.",
    },
    Message {
        id:       ClilocId(1072201),
        fallback: "Reward",
    },
    Message {
        id:       ClilocId(1072202),
        fallback: "Description",
    },
    Message {
        id:       ClilocId(1072204),
        fallback: "Slay",
    },
    Message {
        id:       ClilocId(1072205),
        fallback: "Obtain",
    },
    Message {
        id:       ClilocId(1072206),
        fallback: "Escort to",
    },
    Message {
        id:       ClilocId(1072207),
        fallback: "Deliver",
    },
    Message {
        id:       ClilocId(1072208),
        fallback: "All of the following",
    },
    Message {
        id:       ClilocId(1072209),
        fallback: "Only one of the following",
    },
    Message {
        id:       ClilocId(1072379),
        fallback: "Deliver to",
    },
    Message {
        id:       ClilocId(1072540),
        fallback: "You chop some ordinary logs and put them into your backpack.",
    },
    Message {
        id:       ClilocId(1072541),
        fallback: "You chop some oak logs and put them into your backpack.",
    },
    Message {
        id:       ClilocId(1072542),
        fallback: "You chop some ash logs and put them into your backpack.",
    },
    Message {
        id:       ClilocId(1072543),
        fallback: "You chop some yew logs and put them into your backpack.",
    },
    Message {
        id:       ClilocId(1072544),
        fallback: "You chop some heartwood logs and put them into your backpack.",
    },
    Message {
        id:       ClilocId(1072545),
        fallback: "You chop some bloodwood logs and put them into your backpack.",
    },
    Message {
        id:       ClilocId(1072546),
        fallback: "You chop some frostwood logs and put them into your backpack.",
    },
    Message {
        id:       ClilocId(1112698),
        fallback: "CANCEL MAKE",
    },
    Message {
        id:       ClilocId(3000087),
        fallback: "Total",
    },
    Message {
        id:       ClilocId(3000112),
        fallback: "Intelligence",
    },
    Message {
        id:       ClilocId(3000113),
        fallback: "Dexterity",
    },
    Message {
        id:       ClilocId(3000362),
        fallback: "Open",
    },
    Message {
        id:       ClilocId(3001016),
        fallback: "Miscellaneous",
    },
    Message {
        id:       ClilocId(3001030),
        fallback: "Combat Ratings",
    },
    Message {
        id:       ClilocId(3001032),
        fallback: "Lore & Knowledge",
    },
    Message {
        id:       ClilocId(3006103),
        fallback: "Buy",
    },
    Message {
        id:       ClilocId(3006104),
        fallback: "Sell",
    },
    Message {
        id:       ClilocId(3006123),
        fallback: "Open Paperdoll",
    },
    Message {
        id:       ClilocId(3006156),
        fallback: "Quest Conversation",
    },
    Message {
        id:       ClilocId(3006168),
        fallback: "Siege Bless Item",
    },
];

/// Named messages used by the Begging skill.
///
/// The text for each identifier lives in [`SERVER_MESSAGES`]; these names keep
/// the skill implementation and the client fallback on the same definition.
pub mod begging {
    use crate::wire::ClilocId;

    pub const PROMPT: ClilocId = ClilocId(500_397);
    pub const FROM_PLAYER: ClilocId = ClilocId(500_398);
    pub const FROM_A_THING: ClilocId = ClilocId(500_399);
    pub const TOO_FAR_HIM: ClilocId = ClilocId(500_401);
    pub const TOO_FAR_HER: ClilocId = ClilocId(500_402);
    pub const UNWILLING: ClilocId = ClilocId(500_404);
    pub const FEEL_SORRY: ClilocId = ClilocId(500_405);
    pub const NOT_TRUSTWORTHY: ClilocId = ClilocId(500_406);
    pub const NOT_ENOUGH_MONEY: ClilocId = ClilocId(500_407);
}

/// Return OpenShard's English fallback for a server message.
#[must_use]
pub fn fallback(id: ClilocId) -> Option<&'static str> {
    SERVER_MESSAGES
        .iter()
        .find(|message| message.id == id)
        .map(|message| message.fallback)
}

/// Whether a cliloc is in the server/client shared catalogue.
#[must_use]
pub fn contains(id: ClilocId) -> bool {
    fallback(id).is_some()
}

#[cfg(test)]
mod tests {
    use super::{
        SERVER_MESSAGES,
        contains,
        fallback,
    };
    use crate::wire::ClilocId;

    #[test]
    fn begging_uses_the_common_catalogue() {
        assert_eq!(
            fallback(super::begging::PROMPT),
            Some("To whom do you wish to grovel?")
        );
        assert!(contains(super::begging::UNWILLING));
    }

    #[test]
    fn sentinel_zero_is_not_a_message() {
        assert!(!contains(ClilocId(0)));
    }

    #[test]
    fn catalogue_is_sorted_and_has_no_duplicate_identifiers() {
        assert!(SERVER_MESSAGES.windows(2).all(|pair| pair[0].id < pair[1].id));
    }
}
