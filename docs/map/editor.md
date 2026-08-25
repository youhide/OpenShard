# The map editor

The editor is the operator-facing end of
[`new_map_representation/`](new_map_representation/README.md): a Game Master
builds one draft against the facet revision they received, previews its sparse
projection, and commits the same canonical operations as one durable patch.

## Boundary

- It lives in our client and uses the client's existing `egui`, renderer,
  picking and shard connection.
- A brush is not a map operation. Paint, raise, lower, flatten and smooth
  compile into `SetLand`; placing and removing furniture compile into
  `AddStatic` and `RemoveStatic`.
- The draft is local until Commit. A commit names its parent revision and the
  server refuses a stale parent instead of merging terrain silently.
- The server supplies the author from the authenticated session, checks Game
  Master authority and applies the patch through `mapedit::commit`.

## Slices

1. **Mode.** A Game Master can open and leave an editor workspace. Its shell
   shows the active tool, brush, draft size, revision and commit state.
2. **Catalogue.** Land and static art can be searched by decimal or hexadecimal
   id and by tiledata name, selected and previewed without decoding all art at
   startup.
3. **Tools.** The first set is land paint, raise, lower and flatten, plus static
   placement and removal. A brush has a shape and radius; height tools also have
   strength or a target height.
4. **Draft.** Gestures edit a private preview, coalesce repeated writes to the
   same land tile, expose dirty chunks and support exact undo and redo. Discard
   returns to the chosen parent revision.
5. **Commit.** One bounded request carries canonical operations. The shard
   validates authority, facet, parent, coordinates and request size, then
   performs the existing apply-log-announce transaction. Accepted and refused
   replies leave the editor in explicit states.
6. **Proof.** Unit tests cover catalogue lookup, brush footprints, operation
   compilation, undo/redo and hostile packets. An end-to-end test commits a
   mixed land/static draft, observes the connected client update and proves the
   edit survives restart.

## First usable cut

The cut is done when a Game Master selects a land or static, paints with a
radius, raises or flattens ground, undoes and redoes locally, commits once, and
sees the connected world move to the accepted revision. Smooth, stamps,
rectangle selection, commit history UI and house-to-terrain conversion follow
that cut.

The first cut currently uses click-sized brush dabs and a coloured geometry
overlay for draft terrain. Continuous drag strokes, art-composited static
preview, smooth, stamps and rebase remain follow-up work.
