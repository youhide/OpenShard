# A line, and who hears it

The model behind `crates/server/chat` and the routing that sits on it: what a
spoken line is on the wire, how far it carries, who is allowed to hear it, and
the three kinds of line that reach a *set of people* instead of a place.

What a townsperson answers with is
[`design_townsfolk.md`](design_townsfolk.md); the sets themselves — a party, a
guild, an alliance — are [`design_groups.md`](design_groups.md).

## Why speech is a crate

`chat` depends only on the state below it, never on `world` above. It is a plain
function over the shared `WorldState`: `say` and `speak` read the speaker's
position, draw the words over its head for everyone in earshot, and emit
`MobileSpoke`. The world's tick calls them and does not reach inside. The event
lives here too — domain events live with the crate that owns the rule — and
`world` re-exports it for the reader that does not know `chat` by name.

## Four packets, and the encoder is chosen by content

| Direction | Packet | When |
|---|---|---|
| in | `0x03` | the classic ASCII talk packet |
| in | `0xAD` | what a modern client actually sends: UTF-16, plain or keyword-encoded |
| out | `0x1C` | pure-ASCII text, universally understood |
| out | `0xAE` | anything Latin-1 cannot carry, as big-endian UTF-16 |

The outbound choice is made on the **content**, not on a declared client
version — which the game connection never states. A player could only have typed
text needing `0xAE` through `0xAD` to begin with, so the content test doubles as
the capability test: type "olá" and the accent comes back intact.

`0x03` alone left live chat silent for every ClassicUO client, which is what
made `0xAD` the fix rather than an addition.

## Range is a mode, and a mode can mean "not a place at all"

`speech_range` is one function over the mode byte the client already sends, and
its numbers are Sphere's defaults, made operator settings:

- **Whisper** (`;`) — three tiles.
- **Regular, emote, label and a spell's mantra** — eighteen, the screen. A mantra
  carries exactly as far as a sentence: ServUO's `SayMantra` goes out through the
  same door ordinary speech uses, and the mode changes only how the client draws
  it.
- **Yell** (`!`) — thirty-one tiles.
- **Guild, alliance, and anything unrecognised — zero.**

That last row is load-bearing. `World::say` branches on the mode **before**
anything measures a distance, because a guild line picks its listeners by
membership and a line that fell through to the broadcast would be a private one
said out loud in the street. `speech_range` answering zero means even a routing
failure is silence rather than that.

## The living do not hear the dead

A ghost is drawn only to other ghosts and to staff. Without a gate on the
*hearing* side it would be invisible and audible at once, which reads as a client
bug and is an engine one — so listeners are filtered through
`can_hear_mobile`, which is the same question `can_see_mobile` asks plus Spirit
Speak: a living mobile that has contacted the netherworld catches the voice
without the speaker becoming visible. One choke point rather than a second rule
that can drift from the first.

## Three lines that go to a set of people

They share one mechanism and differ in what draws them:

- **Party** — its own packet, drawn in its own colour, carrying no position. None
  of the speech machinery applies: no distance, no whisper radius, no line over a
  head. What it shares with speech is only that a player typed it. This is the
  first tenant of the "send to a set" router, and the reason party was built
  before guild chat rather than beside it.
- **Guild** (mode `0x0D`) and **alliance** (mode `0x0E`) — ordinary `0xAD` in and
  ordinary `0xAE` out with the mode preserved, so the client draws them as a line
  from a named speaker in the guild colour and *not* over anybody's head. The
  listeners are the roster.

On our own client a channel is a selector cycled with Tab and drawn in the
prompt, rather than a `/` prefix: a channel is a property of the line, not of its
first character, and a prefix hides the state it sets.

## The staff layer is speech that never reaches a screen

A `.`-prefixed line from a privileged mobile is split off in the `Command::Say`
handler and run as a command instead of being said. An ordinary player saying
`.hello` just talks, so there is no leak and no surprise.

The gate lives in the world, not in the command module: `gm` trusts that a call
means the actor cleared it, and only parses and acts. Authority is an
`AccessLevel` on the account — player, game master, administrator — looked up at
login and carried into the world as a component, **re-derived every login and
never saved with the character**, so a demotion takes effect.

Everything a command does is a world mutation the tick is already the right place
for, so it is applied like any other: server-authoritative, no client round-trip.
And each command leans on the system that owns its rule — `items` spawns the
item, `skills` re-caps the stat — rather than reaching into the registry itself.
The actor is answered privately with a system line.

## What the event is for

`MobileSpoke` is the hook everything conversational hangs off: a townsperson's
keyword table, a banker's service, a guard being called, a pet taking an order, a
quest keyword. Each is a reader of the event rather than a call inside `speak`,
which is why adding one of them changes nothing about speech.
