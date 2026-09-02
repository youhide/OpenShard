# The channel selector, and the whole of `0xBF`

Two findings from one afternoon: that `0xBF` — nine of the enum's variants — had
no decode arm at all, and that a guild line is a mode byte rather than a prefix
character.


Landed 2026-08-15, and it turned out to be two things.

### `0xBF` had no decoder at all

Not the party family — *the family*. `ServerPacket::decode` matches on the id
byte, and for `0xBF` the id byte is not a key: nine of the enum's variants share
it, and which one a packet is lives two bytes further in. There was no arm, so
every extended command the shard has ever sent reached this client as
`Undecoded` — the context menu, the spellbook's contents, the stat-lock arrows,
the map change and the close-gump, along with everything party.

The fix is a second dispatch (`decode_extended`) and, for party, a third
(`decode_party`), because party's four packets share a subcommand *and* differ by
a further byte. Every decoder reads that subcommand back and refuses a body that
is not its own — twice on purpose, once to pick and once to be sure, because
getting it wrong is not a malformed packet but a well-formed one read as the
wrong thing.

**Four of the nine are still undecoded and that is deliberate.** The context menu
(`0x14`), the spellbook (`0x1B`), the stat locks (`0x19`) and the map change
(`0x08`) each need a reader on this end that wants them, and a decoder written
before there is one only moves the packet from "undecoded" to "decoded and
dropped". Their subcommands are listed in `decode_extended`'s own doc so the next
person has them rather than a search.

### A channel, not a prefix

A guild line is ordinary `0xAD` speech with a different **mode byte** — it is a
property of the line, not of its first character. So `talk::say` takes a
`TalkMode` and `chat::Channel` is the selector: `say`, `guild`, `alliance`,
`party`, cycled with **Tab** while the line has the keyboard, drawn in the prompt
that already said `say:`.

The alternative considered and rejected was reserving `/` or `\` at the front of
the line, ClassicUO-style. It makes a character unsayable, and it hides the state
it sets — a player who typed a `/` and then deleted it has no way to know what
channel they are on. The prompt says, always, and the channel survives a send:
one that reset itself would make a conversation on any channel four keystrokes a
sentence.

**Party is not speech**, so the `Party` channel does not go through `say` at all
— it is `0xBF 0x06 0x04`, and putting it through the speech path would say it out
loud to the street.

### The party windows are egui, and that is a decision

Two of them: an invitation prompt and the roster, both `egui::Window`s in the
shell, following the stack-split prompt that was already there. Not gump art, and
not a shard-sent `0xB0`:

- **Not a gump**, because the roster window would be a thousand lines of render
  code (`client/render/skills.rs` is the measure) for a list of names and two
  buttons.
- **Not a shard-sent dialog**, which was the tempting shortcut — it would work on
  both clients with no client code at all — because ClassicUO draws its *own*
  invitation prompt off the `0x07`, and a shard that sent both would give
  ClassicUO users two.

Both windows are drawn straight off `WorldView::party` rather than from local
state, so neither can go stale: the question exists because the shard said so and
stops existing when the shard says otherwise.

**Members are shown by serial.** A `0x78` carries no name — a mobile is named by
a single click or by a tooltip, and this client may have done neither to the
person inviting it. A number beats a guess, and the fix is to ask for the
tooltip, which is now possible and is not wired here.

