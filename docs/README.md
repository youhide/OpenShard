# Documentation

This tree is the **as-built** description of OpenShard: what exists today, why
it is shaped that way, and what is open. Plans — intent, ordering, the work not
yet done — live in [`../plans/`](../plans/README.md), outside this tree, on purpose.

## Roles: a document's role decides where it lives

Every document has exactly one role. The role is not a label in the text; it is
the place in the tree. A file that changes role gets moved, not renamed in
place.

| Role | Where | Holds | Must not hold |
|---|---|---|---|
| **topic canon** | `<domain>/<topic>/README.md` | status and boundaries; the current picture; decisions and invariants; what is open; pointers to material; a compressed history | a second copy of the status |
| **design** | `<topic>/design_*.md` | the stable model, formulas, invariants — as built | status, ordering, "amendments forced by …" |
| **research** | `<topic>/research_*.md` | alternatives compared, reference engines read, what was rejected and why | a `NEXT` of its own |
| **evidence** | `<topic>/evidence/<date>-<slug>.md` | a measurement, a phase report, a closed handoff as fact, a trap that cost a day | an active `NEXT`, a queue |
| **reference** | `<topic>/reference/` | file formats, artifact contracts, version tables, external sources with their licences | — |
| **runbook** | `<domain>/runbook_*.md` | commands, environment, diagnostics | — |
| **plan** | `../plans/<domain>/<topic>/PLAN.md` | the goal, the options, the order, the criteria for moving on | launch status, a session log, the topic's backlog |

Three consequences, and they are the point of the split:

- **Design carries no history.** "It used to be X, then we understood Y" is
  evidence. Design says "it is Y, because Z; for X see `research_*`."
- **A plan carries no progress.** Progress is evidence plus one status line in
  the topic README. A plan is edited only when the plan itself is revised.
- **A closed plan does not stay in `docs/`.** Closing is one commit: the topic
  README is brought to as-built, the decisions go to design, the remainder goes
  to the README's "what is open", and the plan is `git mv`'d to `plans/…/done/`
  or, after a domain revision, to `docs/archive/`.

There is no permanent handoff document. A live worktree and its next step
belong to the topic README; the facts a handoff established belong to
`evidence/`.

**A status is a line of text, never a glyph in a heading.** A heading is an
address: every link written to it breaks the moment a ✅ is appended, silently,
because the anchor grows a trailing dash and nothing checks it until a link
audit runs. Sixty of the sixty-one broken links the `world` migration found were
exactly that. Say "built" in the body — a record does anyway — and leave the
heading alone.

## Domains

A domain is roughly a group of crates, so "where does this go" has an objective
answer: where the code lives. A topic is a subsystem with a canon of its own.

| Domain | Crates | State |
|---|---|---|
| `protocol/` | `common/protocol`, `common/movement` (wire) | not migrated |
| `server/` | `server/server`, `gateway`, `login`, `persistence`, `state` | not migrated |
| [`world/`](world/README.md) | `server/world`, `common/movement` (search), `common/map`, `common/basemap`, `common/tiles`, `common/uofiles` | **migrated** — the consolidation it replaces is in [`archive/world/`](archive/world/README.md) |
| `items/` | `server/items`, `server/crafting` | not migrated |
| `combat/` | `server/combat`, `skills`, `magic` | not migrated |
| `housing/` | `server/housing`, boats | not migrated |
| `npc/` | `server/npc`, `ai`, `quests`, `guilds`, chat | not migrated |
| `client/` | `client/net`, `client/model`, `client/app` | not migrated |
| [`render/`](render/README.md) | `client/render`, `client/artscan`, `client/pathtrace` | **migrated** — twelve superseded documents in [`archive/render/`](archive/render/README.md) |

`render` is a domain separate from `client` on purpose: it is the majority of
this corpus by volume and it has an oracle of its own. `world/` holds the
documents of three readers that live in `client/render` — the radar raster, the
building flood, the roof cutaway — because what each of them asks is a question
about the map.

Two directories sit outside the domain grid:

- `archive/<domain>/` — only after a domain has been revised; each carries a
  README listing what is in it and why it stopped being current.
- `prototypes/` — throwaway experiments, kept for the record.

Engine-wide documents stay at the top: [`architecture.md`](architecture.md),
[`style.md`](style.md), and the development runbook.

## Until the migration lands

The tree is mid-move. Files still sitting flat in `docs/` are pre-migration
documents that have not had a role assigned yet; the domain table above says
which of them have. The order of the move and its criteria are in
[`../plans/`](../plans/README.md), not here.
