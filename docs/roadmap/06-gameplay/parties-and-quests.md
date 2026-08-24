# Parties and quests

[Gameplay index](README.md) · [Roadmap](../README.md) · [Backlog](../backlog/README.md)

- [x] `parties` — **built.** Inviting, accepting, leaving, kicking, the chat and
  the loot flag, all on `0xBF` subcommand `0x06`. Ported from ServUO's
  `PartyCommands` and `Scripts/Services/Party/`.
  - **Two numberings under one subcommand.** The byte *after* `0x0006` says which
    of the seven a packet is, and inbound and outbound do not agree about it:
    `0x01` is "raise the add cursor" from a client and "here is the whole roster"
    from the shard, `0x08` is "I accept" and is not an outbound number at all.
    Only `0x03`/`0x04` — the two chat lines — mean the same thing both ways. A
    decoder written from one side reads the other's acceptance as a member list.
  - **The empty list is a removal.** There is no "you are in no party" packet:
    ServUO's `PartyEmptyList` is a `0x02` with a member count of zero and the
    recipient's own serial in the removed slot, which is `PartyRemoveMember`'s
    layout with the list empty. One type serves both.
  - **The leader is the id.** A leader who leaves disbands the party rather than
    handing it on, so the leader's serial is fixed for the party's whole life and
    is the key — no counter, and no high-water mark to save. This is the sharpest
    difference from a guild: a guild outlives its founder because it is a thing in
    the world, and a party is only the people in it.
  - **Asking is what creates one**, so a leader who has asked one person and been
    ignored is leading a party of one. `decline` closes it again — otherwise the
    next invitation silently reuses a group with a phantom member in the cap.
    The cap (10) counts members *and* outstanding invitations, the leader
    included.
  - **`tell_party` is the router the whole thing is for.** "A line goes to a set
    of people who are not the ones standing nearby" is one mechanism, and guild
    chat is its second tenant — which is why party was built first rather than
    beside it.
  - Not saved, and that is the reference's behaviour rather than an omission:
    ServUO's `Party` has no serialization, and a party of people who are all
    offline is not a party.
  - **Logging out leaves the party**, which the reference does not need to do:
    ServUO's logged-out `PlayerMobile` stays in the world and stays in the group,
    and this engine despawns the entity. Without `on_logout` a party would hold a
    serial naming nobody — counted against the cap, drawn on everyone else's
    roster as a member they cannot see, and keeping the party alive after the
    last person in it had gone. It follows from what a party is, which is also
    why none of it is saved.
  - **The loot flag has no consumer yet.** `WorldState::party_may_loot` answers,
    and nothing asks: corpses on this shard are open to anybody, because there is
    no criminal-act rule on looting one to exempt a party from. That rule is the
    missing half, and it belongs with the criminal system rather than here.
  - Client-side: **built**. Our own client decodes the four outbound packets,
    holds the roster and the invitation on its `WorldView`, sends five of the
    seven requests, and draws an invitation prompt and a roster window. It also
    turned up that `0xBF` had **no decoder at all** on that end — nine variants
    share the id byte and there was no arm — so the whole family was arriving as
    `Undecoded`. See "The channel selector, and the whole of `0xBF`" in
    [`client.md`](../../client.md).
- [x] `quests` — **a core system now, ServUO's Mondain's Legacy model, with the
  content left to the pack.** It was built pack-first (five thin seams and an
  opaque JSON blob the engine only stored) and that did not survive a client.
  Three things were wrong:
  - **No quest log.** The paperdoll's Quest button sends `0xD7` subcommand
    `0x32` — a packet, not a gump reply, so nothing pack-side could answer it.
    The id sat in the length table with nothing routing it. A player could accept
    a quest and then had no way to see it, track it or resign it.
  - **Givers went inert at the first restart.** `restore_mobiles` emits no
    `MobileSpawned` (it would re-stock every vendor and duplicate its crate) and
    the pack bound a giver only on that event, so the shard's quests worked
    exactly once — on the boot where `.admin` Populate ran — and never again,
    silently.
  - **The right window was not writable pack-side.** The script `GumpAnswered`
    dropped `switches` (no radio dialog), and there was no server-side gump close,
    no private message and no per-player sound.

  What landed: `crates/server/quests` owns the model (`QuestDef`, objectives Slay /
  Obtain / Deliver / Escort, rewards, `all_objectives`, `done_once`, restart
  delays), the progress passes, the turn-in and the window; the pack owns the
  quests, registered as data through `op_register_quests` and bound to an NPC
  with `op_bind_quest_giver` / `op_make_escortable`. Progress is **found, not
  announced**: kills off `combat::MobileDied`, escorts a point query against
  `Regions`, timers off the tick counter, and Obtain a diffing pass over the
  backpack twice a second — because nothing in the engine says an item moved, and
  a call beside every insert is the pattern the persistence rule warns decays.
  The gump is a port of `MondainQuestGump` (same frame art, same eight sections,
  same button ids, same four sounds) built through a new typed `GumpLayout` in
  `protocol` whose keywords come from ServUO's `Gump*.cs`; a reply is matched
  against what the server remembers drawing, so a `0xB1` for a window this side
  never opened does nothing. Underneath: `MobileUsed` fires for **every**
  double-clicked mobile (a shop no longer swallows it — in ServUO a
  `MondainQuester` *is* a `BaseVendor`), `restore_mobiles` announces a distinct
  `MobileRestored`, and the bindings are saved components (schema v13, replacing
  the v11 blob with structured `quests`/`done_quests`). The `0xB9` mask is what
  makes the client *draw* the button at all, so `[gameplay] expansion`
  (`"aos"`/`"se"`/`"ml"`, ML by default) sends ServUO's `ExpansionML` bits; a
  staff `.quests` and a "Quest Log" context entry reach the same window either
  way. Deferred: quest chains, `ApprenticeObjective`, the question-and-answer
  objective, reward *choice*, the staff force-complete button, and a converter
  pass over ServUO's own `BaseQuest` subclasses now that the model matches theirs.
  Filed and closed as a non-bug: on an already-accepted quest's Description /
  Objectives / Rewards page, the button drawn with the close-box art
  (`0x2EEC`/`0x2EEE`) is `CLOSE_QUEST`, not `CLOSE` — it redraws the Main
  section rather than closing the window, same as `MondainQuestGump.OnResponse`.
  The window is `no_close()` throughout (a right click never dismisses it), so
  a real close is reachable only from Main. Confusing, but retail-accurate;
  kept for parity rather than "fixed".
