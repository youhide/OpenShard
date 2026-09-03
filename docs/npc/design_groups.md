# A party, a guild, an alliance

The model behind `crates/server/party` and `crates/server/guilds`: three ways a
set of players is a thing, and the rules that keep each of them from being the
other two. What a line addressed to one of these sets does on the wire is
[`design_speech.md`](design_speech.md).

## One split, twice

Both crates sit over a substrate in `state`, and the line is drawn in the same
place. **The substrate holds what the thing *is*** — who is in it, in what order,
who has been asked, and the relations the packet path reads to colour a mobile.
**The crate holds the rules** — the capacity, who may kick whom, who may declare
a war.

The reason is not tidiness: `0x77` carries a notoriety byte, so `state` has to
answer "what colour is this" without asking a crate above it what a rank means.

## A party is only the people in it

**The leader is the id.** A leader who leaves disbands the party rather than
handing it on, so the leader's serial is fixed for the party's whole life and is
the key — no counter, and no high-water mark to save. This is the sharpest
difference from a guild: a guild outlives its founder because it is a thing in
the world; a party is not.

**There are no ranks.** ServUO's party has exactly two rules about authority: the
leader adds and kicks, and anybody may remove *themselves*. Both are the same
packet naming a serial, which is why removal takes an actor and a target rather
than being two functions.

**Asking is what creates one**, so a leader who has asked one person and been
ignored is leading a party of one. A decline closes it again — otherwise the next
invitation silently reuses a group with a phantom member in the capacity. The
cap counts members *and* outstanding invitations, the leader included, because a
leader who is not counted could gather eleven.

**Nothing about a party is saved**, and that is the reference's behaviour rather
than an omission: a party of people who are all offline is not a party.

**Logging out leaves the party**, which the reference does not need to do —
ServUO's logged-out character stays in the world and stays in the group, and this
engine despawns the entity. Without it a party would hold a serial naming
nobody: counted against the cap, drawn on everyone else's roster as a member they
cannot see, and keeping the party alive after the last person had gone. It
follows from what a party *is*, which is also why none of it is saved.

### Two numberings under one subcommand

Everything a party does rides `0xBF` subcommand `0x06`, and the byte after it
says which of the seven the packet is — **inbound and outbound do not agree about
that byte**. `0x01` is "raise the add cursor" from a client and "here is the whole
roster" from the shard; `0x08` is "I accept" and is not an outbound number at
all. Only the two chat lines mean the same thing both ways. A decoder written
from one side reads the other's acceptance as a member list.

**The empty list is a removal.** There is no "you are in no party" packet:
ServUO's empty list is a remove-member packet with a count of zero and the
recipient's own serial in the removed slot. One type serves both.

## A guild is a thing in the world

**Five ranks, and the trap in them.** Ronin, Member, Emissary, Warlord, Leader,
with ServUO's flag set per rank. The ranks are ordered and the permissions are
**not nested**: an Emissary recruits, dismisses, promotes and titles; a Warlord
sits above it, does none of those, and declares wars the Emissary cannot.

So authority is **three separate questions**, each with its own function:

- `may` — does this rank hold the flag the operation needs.
- `outranks` — is the *target* reachable. Holding "remove players" says you may
  dismiss, not that you may dismiss *them*.
- `may_lead` — the two things no flag grants: disbanding, and handing the guild
  over.

Any of them written as a plain rank comparison gets the Emissary or the Warlord
wrong. The bit values are the reference's, kept rather than renumbered: nothing
puts them on the wire, but a table copied with its own numbers is one an auditor
can diff against the source.

A newcomer joins as a **Ronin**, which holds nothing at all — not the vote, not
guild items — and that is what a promotion is for. **Promoting stops two rungs
below the promoter** (only the Leader may reach the rank below their own),
because promoting into the rank directly under you would hand out a flag you may
not hold yourself. Demoting needs only that you outrank them, and stops at Ronin.

**An invitation is a consent**, because a guild may not conscript: an invite
leaves a candidacy the player answers. That is the *opposite* shape from a war,
and the two are deliberately not one mechanism.

### A war is two declarations, and peace is one

Guild A declares war on B and nothing changes; B declares on A and both are at
war. That is the guildstone's rule, and it is why the offers are a set separate
from the wars: a guild that declared and was ignored is not at war, and its
members must not turn orange on the strength of their own guild's opinion. There
is no accept path to keep in step with the declare path, and no way to be at war
with a guild that has not said so.

**Peace is one guild's decision**, because the alternative is a guild that cannot
stop being attacked by one that will not agree to stop.

### An alliance is a named object, not a fact about a pair

An alliance is several guilds, a leader guild, a member list and a pending list —
a guild is invited *into* it by a guild already in it, and answered by that
guild's own leader: the shape a player's own membership has, one level up.

It replaced a pairwise relation, under which A allied with B and with C left B
and C strangers, and "who is in my alliance" had no answer at all — so alliance
chat reached a set that depended on who was speaking. Four rules came with the
replacement, and each is something the pairwise model could not state:

- **The name is claimed once and belongs to the alliance**, so extending one does
  not rename it.
- **War and alliance refuse each other in all three directions** — declaring on
  an ally, inviting somebody you are at war with, and joining an alliance that
  holds a guild you are at war with — because green and orange cannot both be
  true, and the notoriety answer would otherwise depend on which question was
  asked first.
- **The leader guild leaving hands the alliance on** rather than dissolving it
  (`CalculateAllianceLeader`), which is why an alliance's id is its own and not
  that guild's.
- **An alliance that cannot field two members disbands**, handing back its whole
  membership, pending guilds included, because each has a link to unhook and the
  alliance is no longer there to be asked.

Splitting the war handshake from the alliance one is what made this possible: a
war is a thing two guilds declare at each other, an alliance is a body one is
admitted to, and keeping them one function is what made an alliance pairwise in
the first place. The permissions split with them — war status is the Warlord's,
alliance control is the Leader's.

## Notoriety is relative, and that is the architectural half

A `0x78`'s notoriety byte is not a property of the mobile. It is the answer to
"what colour does *this client* draw it in", so there are two functions: the
mobile's own standing, which combat, guards and shopkeepers read, and the wire
answer for a viewer, which the movement broadcast builds one packet per watcher
from.

**ServUO's order is kept: murderer and criminal resolve before guild**, so a red
cannot hide inside a tabard. And every operation that can move a colour
re-announces the mobiles it moved — nothing on a client asks again on its own.

## What is saved, and what the counter is for

Guilds are saved and replace-all like regions. **Membership is a character
column**, so the roster is derived from who names the guild rather than kept
twice. **The id counter is in the world row** rather than re-derived from the
table, because a disbanded guild leaves no row and the maximum id in the table is
not the maximum ever issued. Alliances are a second table on the same terms, with
a second counter, and a guild's alliance column is only a back-pointer into it.

A member's rank is saved as a number, and the schema bump that introduced it
refuses an older database rather than opening it into a shard where every
existing member — leaders included — reads as a Ronin, with no way back out.
