# A click that cannot be routed, and the four things that can be wrong

A walk toward an unreachable place is the same list of steps as a walk to a
reachable one, so the reason stopped being dropped. What `steer::Refusal`'s four
members map from, and the three readers that say it.


A player standing on a building's upper storey clicked the street and the body
walked into a wall. Nothing was broken in the search: the destination was past
what a 600-node plan reaches from inside a house, the client had no coarse graph
to divide the distance with, and `find_path_toward` did what it is for — walked
the body at the nearest reachable place. **Every refusal looked like that**, and
a walk toward an unreachable place is the same list of steps as a walk to a
reachable one.

So the reason stopped being dropped. `SearchExit` and `LongExit` were both
diagnostics-only — `find_path` and `find_long_path` answer `Option` and throw the
exit away — and `search_long_path` is now the pair to `search_path` that keeps
it. `steer::Refusal` is what a *person* is told, and it has four members because
they send a player four different places: `Nowhere` (round the wall, or nowhere
at all), `TooFar` (walk closer and click again), `Barred` (open the door), and
`NoGraph` (wait — this one goes away by itself).

**What the four map from is the honest part.** `LongExit::NoCorridor` is the
graph's own "there is no way": both endpoints joined it and no chain of portals
connects them, which on a facet of islands is a real answer. `OffGraph`,
`NoJoin`, `PortalsExhausted` and `Spent` are the *query* giving up and are none
of them a claim about the world, so they are all `TooFar`. A bounded search that
exhausts itself inside a house says nothing about the far side of town either —
with no graph to fall back on, that is `NoGraph` and not `Nowhere`.

Three places say it, and each is a different reader:

- **The line is dashed** and the last reachable tile gets a cross, so a route
  that does not arrive stops looking like one that does. A shut door keeps the
  solid line and its red half — that route *has* a far side.
- **The journal** gets one sentence, once per destination. A plan is remade every
  few steps and whenever the live layer moves; a client that spoke on every plan
  would fill the log while the body stood still.
- **The dev strip** keeps it for as long as the order stands, beside the graph's
  own state — `nav: none` and "too far to plot a route" are two halves of one
  story more often than not.

### The graph got a state and a button while it was there

`nav: none` / `nav: building 3s…` / `nav: 71545 nodes` on the always-there strip,
because the state a person needs is the *transient* one: a graph still being
built explains a refusal that will stop happening on its own, and a tab nobody
has open cannot say so. The World tab has the numbers, the artifact's path and a
**rebake** button — the one case a stamp cannot decide, since the artifact
validates and a person may have a reason to disbelieve it anyway.

### What is still not right about it

- **`Nowhere` is only as honest as the budget it was found in.** Inside the
  8-tile radius where the coarse graph is not asked, an exhausted search really
  did settle everything a body can stand on — but "everything" there is
  everything *within 600 nodes*, and a walled courtyard bigger than that would be
  told "there is no way" when the way is round the far side.
- **Nothing says how far the way *did* get.** A route that reaches within two
  tiles of a shut gate and one that gets a third of the way across town are the
  same sentence. The plan knows both.
- **The refusal is per destination, not per reason.** Clicking the same
  unreachable place twice says nothing the second time, which is right, but so
  does clicking it after walking somewhere the answer would have changed from.
