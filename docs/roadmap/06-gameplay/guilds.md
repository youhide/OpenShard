# Guilds

[Gameplay index](README.md) · [Roadmap](../README.md) · [Backlog](../backlog/README.md)

- [x] `guilds` — **built, with ServUO's five ranks.** Founding, invitations,
  leaving, dismissal, titles, promotion, leadership, disbanding, and the war and
  alliance handshake, reached from the paperdoll's Guild button (`0xD7`/`0x28`).
  - **Notoriety became relative, which is the architectural half.** A `0x78`'s
    notoriety byte is not a property of the mobile — it is the answer to "what
    colour does *this client* draw it in". `notoriety_of` stays the mobile's own
    standing (combat, guards, shopkeepers); `notoriety_toward(viewer, target)` is
    the wire answer, and `broadcast_move` builds one `0x77` per watcher. ServUO's
    order is kept: murderer and criminal resolve **before** guild, so a red
    cannot hide inside a tabard.
  - **A war takes two declarations** — the guildstone's rule. A guild that
    declared and was ignored is *not* at war, which is why `war_offers` is a set
    separate from `wars`: its members must not turn orange on the strength of
    their own guild's opinion. Peace, though, is one guild's decision, because
    the alternative is a guild that cannot stop being attacked by one that will
    not agree to stop.
  - **An invitation is a consent**: a guild may not conscript, so `invite` leaves
    a `GuildCandidate` the player answers.
  - Every operation that can move a colour re-announces the mobiles it moved.
    Nothing on a client asks again on its own.
  - **Saved, schema v26.** The guilds replace-all like the regions; membership is
    a character column, so the roster is derived from who names the guild; and the
    id counter is in the world row rather than re-derived, because a disbanded
    guild leaves no row and the maximum id in the table is not the maximum ever
    issued. The alliances are a second table on the same terms, with a second
    counter, and the guild's `alliance` column is only a back-pointer into it.
  - **Ranks, and the trap in them.** Ronin, Member, Emissary, Warlord, Leader,
    with ServUO's flag set per rank (`Scripts/Misc/Guild.cs`). The ranks are
    ordered and the permissions are **not nested**: an Emissary recruits,
    dismisses, promotes and titles; a Warlord sits above it and does none of
    those, and declares wars the Emissary cannot. So authority is three separate
    questions and each has its own function — `may` for the flag, `outranks` for
    whether the *target* is reachable, and `may_lead` for the two things no flag
    grants (disbanding, and handing the guild over). Any of them written as a
    plain rank comparison gets the Emissary or the Warlord wrong.

    A newcomer joins as a **Ronin**, which holds nothing at all — not the vote,
    not guild items. That is ServUO's `AddMember`, and it is what a promotion is
    for. Promoting stops **two** rungs below the promoter (only the Leader may
    reach the rank below their own), because promoting into the rank directly
    under you would hand out a flag you may not hold yourself; demoting needs
    only that you outrank them, and stops at Ronin. Saved as a number, schema
    v25 — which refuses an older database rather than opening it into a shard
    where every existing member, leaders included, reads as a Ronin and no guild
    has a way back out of that.
  - **Named alliances, replacing a pairwise `Relation::Ally`.** An alliance is a
    named object — several guilds, a leader guild, a member list and a pending
    list — a guild is invited *into* by a guild already in it, and answered by
    that guild's own leader: the shape a player's own membership has, one level
    up. It replaced this engine's own simplification, in which being allied was a
    fact about a *pair*, and A allied with B and with C left B and C strangers.
    That model had no answer to "who is in my alliance", so alliance chat reached
    a set that depended on who was speaking.

    Four rules came with it, and each is a thing the pairwise model could not
    state. The name is claimed once and belongs to the alliance, so extending one
    does not rename it. War and alliance refuse each other in **all three**
    directions — declaring on an ally, inviting somebody you are at war with, and
    joining an alliance that holds a guild you are at war with — because green
    and orange cannot both be true and the notoriety answer would otherwise
    depend on which question was asked first. The leader guild leaving hands the
    alliance on rather than dissolving it (ServUO's `CalculateAllianceLeader`),
    which is why an alliance's id is its own and not that guild's. And an
    alliance that cannot field two members disbands, handing back its whole
    membership — pending guilds included — because each has a link to unhook and
    the alliance is no longer there to be asked.

    Splitting `propose` in half was the point of it: a war is a thing two guilds
    declare at each other, an alliance is a body one is admitted to, and keeping
    them one function is what made an alliance pairwise in the first place. The
    permissions split with it — `CONTROL_WAR_STATUS` for the war, which is the
    Warlord's, and `ALLIANCE_CONTROL` for the alliance, which is the Leader's.
  - **Guild chat, and alliance chat.** A guild line is
    not a command or a prefix — it is ordinary `0xAD` speech with the mode byte
    set to `0x0D` (`0x0E` for the alliance), and it goes back out as an ordinary
    `0xAE` with the same mode so the client draws it in its own colour. What
    matters is that `World::say` branches on the mode **before** anything
    measures a distance: these two pick listeners by membership, and a line that
    fell through to the broadcast would be a private one said out loud in the
    street. `speech_range` answers zero for both, so even a routing failure is
    silence rather than that.

    Our own client can now speak on either: `chat::Channel` is a selector cycled
    with Tab and drawn in the prompt, rather than a `/` prefix — a channel is a
    property of the line, not of its first character, and a prefix hides the
    state it sets. See [`client.md`](../../client.md).

    An alliance line reaches the alliance's members, which is now one set rather
    than one per speaker — see the entry above for what it used to be.
  - Deferred: the guildstone as a placeable item.
  - Client-side: the window renders, the health bars take their hue from the
    byte, and the **tooltip** now shows here too — the `[ABBR]` suffix and the
    "Warlord, The Silver Serpent" line both. The `0xD6`/`0xDC` half this client
    had never had landed with the guild work rather than after it; see
    "Tooltips, and the half that was never written" in [`client.md`](../../client.md).
