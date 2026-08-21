# A map we can change

We want a world we can edit. Today the map is read from an installed UO
client's files, so a shard can put a house or an item *on* the world but cannot
move the coastline, raise the ground, or knock down a wall — not without every
player editing their own install. So the map becomes our data: land (one
material and a height per tile) and statics (the fixed things above it),
addressed as `(map_id, facet, x, y, z)`. UO's own files stay an **importer** —
one way to create a starting world, not the runtime source.

Everything below is a consequence of that map being large. Nothing below is a
consequence of anything else, and that is the point of keeping this document
short.

## What size buys us, and all it buys

A facet is millions of tiles, so two mechanics, and no more than two:

- **Chunks.** The map loads, caches and invalidates in fixed blocks — 64×64
  tiles is the current guess, to be measured on Felucca, not argued about here.
  Nobody reloads a facet to move a rock.
- **Patches.** An edit is a small ordered record against an immutable base, not
  a rewritten facet. What we read is `resolved = base + published patches`,
  which is what makes an edit attributable, revertible and conflict-checkable.

That is the whole shape:

```text
importers (UO map, editor, generator)
    -> base chunks (immutable)
    -> ordered durable patches
    -> resolved chunks  ->  server (authority) + client (drawing, local planning)
```

## What size does *not* buy: saving bytes

The classic format, and most of the cleverness this document used to contain,
was designed when bandwidth was scarce. It isn't any more. So we spend bytes
freely and keep the mechanism dumb:

- a resolved chunk travels as a whole self-contained blob, never as a stream of
  operations the client could end up half-way through;
- the client keeps a disk cache and re-fetches on a hash mismatch; a cache that
  re-downloads too much beats a delta scheme that can silently desync;
- baking the map into the client is *pleasant*, not required. With an honest
  cache and background download the player sees the same thing. If shipping a
  baked pack turns out to be easier, we ship one; it is not a design pillar.

## Transport: leave the classic protocol alone

None of this has to ride the classic UO protocol. If our own encoding
(protobuf, say) fits inside it, good; if it doesn't, we open a second socket
alongside for our own system traffic. This is deliberately not load-bearing —
the map format must not know how it travels, and the choice can be made late.

An unmodified 2D client keeps reading its own files and simply will not see
terrain patches. Accepted; the patched world is for our client.

## What holds regardless of any later decision

- **The server is the authority.** A matching client chunk speeds up drawing
  and local path planning. It never authorises a step.
- **One geometry underneath.** Movement, LoS, pathfinding, interiors and the
  renderer ask the same resolved map plus a live overlay of entities. A private
  map reader inside one consumer is how the minimap ends up disagreeing with
  the pathfinder.
- **Live entities are not map data.** Mobiles, items, doors, boats and player
  houses have serials and their own persistence. Publishing one into the map is
  an explicit operation, not a drift.

## Order of work

Format and importer → the server reads it instead of UO files → our client
reads it → patches and materialisation → streaming and an editor on top.
Each step ends with a world that runs.

## Decided while building, not here

Chunk size, the codec, our own material/asset ids and how assets are packed,
the exact patch operations, which socket carries chunks, what the editor
validates, how derived data (navigation, minimap, occluders) rebuilds
incrementally. These are real questions with real trade-offs, and settling them
on paper before the code exists is what made the previous draft of this
document seven hundred lines long.
