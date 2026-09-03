# A quest

The model behind `crates/server/quests`: what a quest is, how progress moves,
what a giver is, and why the window is a port down to its button numbers.

## The model is here, the content is content's

A quest is data — a `QuestDef` in `state/data/quests.json` — and a giver is bound
by the placement that spawns it. What is *in* the engine is the model: what an
objective is, how progress moves, when a quest may be offered again, and what the
log looks like on a client. That is the same split `magic::spells` and
`combat::weapons` use, and it is the reason this is a crate at all rather than
the pack owning everything.

It was written the other way first — five thin seams and an opaque blob the
engine only stored — and that did not survive a client. Three things were wrong,
and each is a rule now:

- **A packet cannot be answered by content.** The paperdoll's Quest button sends
  `0xD7` subcommand `0x32`, not a gump reply, so nothing pack-side could route
  it. A player could accept a quest and then had no way to see it, track it or
  resign it.
- **A binding held in script memory dies at the first restart.** The restore path
  emits no spawn event — it would re-stock every vendor and duplicate its crate —
  so a giver bound on that event worked exactly once, on the boot where the
  populate ran, and never again, silently. The binding is a **saved component**
  now, so the engine knows without asking, and the restore announces a *distinct*
  "restored" event for readers that need one.
- **The right window was not writable pack-side even in principle**: no switches
  on a gump reply, so no radio dialog; no server-side close; no private message;
  no per-player sound.

## Four objectives, and each is found rather than announced

| Kind | What it asks | What moves it |
|---|---|---|
| `Slay` | kill *n* of a body | the deaths combat already announces |
| `Obtain` | carry *n* of a graphic at once | a diffing pass over the backpack |
| `Deliver` | take *n* of a graphic to a named NPC | talking to the destination |
| `Escort` | walk somebody to a named region | a point query against the regions |

**Nothing in combat, items or movement knows quests exist.** A kill objective
reads the event; an escort reads where the escorted NPC is standing; a timer
reads the tick counter.

**`Obtain` counts the backpack, and that is deliberate rather than a gap.** The
engine emits nothing at all when an item changes hands, and the obvious fixes —
an `ItemMoved` event, or a `quests::notice()` beside every insert — are exactly
the pattern the persistence rule warns decays: the first system that moves an
item without knowing quests exist breaks a quest silently, and no test without a
quest in it catches that. **A pass that looks cannot be forgotten.** It costs one
walk of the backpack per player who actually has an obtain objective, twice a
second, which is the same bargain the status bar already makes. Progress
therefore goes *down* when the items are dropped, which is the reference's
behaviour too.

Two field choices follow from restarts rather than from taste. A `Deliver`
destination is **a name, not a serial**, because it is written before anything
has been spawned and a name still means the same thing after a restart. A `Slay`
body is matched against the victim's, so any creature drawn as that body counts.

## When a quest may be taken again

`done_once` outranks everything: a character who has finished it is never offered
it again. Otherwise `restart_delay_secs` is the wait, with zero meaning
immediately. `all_objectives` decides whether every objective must be met or any
one of them is enough.

## The offer, in ServUO's order

`MondainQuester.OnTalk`, kept in order because the order is the behaviour: finish
a delivery, then show a quest already in progress, then check the cap, then offer
a new one. A giver you already have a quest from must talk about *that* quest
rather than offering another.

Underneath it, the double-click event fires for **every** mobile — a shop no
longer swallows it, because in ServUO a quester *is* a vendor.

## One window, and its numbers are copied

The quest gump is a port of `MondainQuestGump`: the same frame art, the same
eight sections, the same button ids, the same page order, the same four sounds.
It is one window rather than six because the paperdoll button, an offer and a
turn-in all end up looking at the same frame with a different middle, and sharing
the id means one reply handler instead of six that must agree.

**The button ids are copied rather than chosen** because nothing on the wire says
what a button means — the client sends back the number the layout gave it.
Keeping the reference's numbering costs nothing and means the reply handler can
be read against `MondainQuestGump.OnResponse` line by line, which is the only way
to be sure a page chain is right.

**A reply is matched against what the server remembers drawing.** The page a
button was clicked on comes from that memory, never from the client, which is
free to send any number it likes; and a reply for a window this side never opened
does nothing.

The layout itself is built through a typed `GumpLayout` in `protocol` whose
keywords come from the reference's own gump sources, rather than by writing the
command string by hand.

## Reaching the window at all

The `0xB9` feature mask is what makes a client *draw* the Quest button, so the
`[gameplay] expansion` setting sends the expansion bits that turn it on. A staff
command and a "Quest Log" context-menu entry reach the same window either way, so
a shard configured below that expansion is not locked out of its own quests.

**Filed and closed as a non-bug**: on an already-accepted quest's Description,
Objectives or Rewards page, the button drawn with the close-box art is
"close *quest*", not "close" — it redraws the Main section rather than closing
the window, exactly as `MondainQuestGump.OnResponse` does. The window is
no-close throughout, so a real close is reachable only from Main. Confusing, and
retail-accurate; kept for parity rather than "fixed".

## The escortable

An escort is a quest whose objective is a place and whose subject is an NPC. What
makes it feel like a person rather than a marker is that **its lines are spoken
rather than shown as private system messages**: `BaseEscortable` is one of the
few NPC classes the reference does give lines, so the ask, the thanks and the "I
seem to have lost my master" are *heard*. That is what makes sixty of them
scattered across a facet findable, and what tells a bystander an escort has just
set out. The ask rides the greeting seam and stops once somebody is leading it.
