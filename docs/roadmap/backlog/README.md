# Backlog

This is the single queue for unfinished roadmap work. Phase files describe what
was built and retain the technical context; task state and prioritisation live
here. Order, not dates.

## Queue

### Protocol

- [ ] Add packet tests against captures from real clients.
- [ ] Revisit login encryption only when a client that cannot disable it must
      be supported.

### World and map

- [ ] Give `World` a controlled way to publish live map patches.
- [ ] Measure and address the remaining R2–R4 map findings below.
- [ ] Benchmark packed land cells before deciding whether to remove the
      alignment byte.

### Gameplay

- [ ] Complete classic-client per-weapon and per-body animations.
- [ ] Finish the remaining usable skills and exact skill/spell presentation.
- [ ] Add summons, the deferred spell subsystems, and adjacent-tile quarry
      pathing.
- [ ] Complete boats B3–B4.
- [ ] Complete house customisation C3–C4.
- [ ] Resolve the data-table and Felucca-converter findings below.

### Red at HEAD

Two suites fail on a clean tree, and neither is anybody's current work. They are
here because "all four commands are expected to be silent" stops meaning
anything the moment two of them are not.

- [ ] `openshard-movement --test walk_scenes`: `a_villa_stair_carries_a_body_to_its_first_floor`
      and `a_route_climbs_from_a_villas_ground_floor_to_its_first_floor` — the
      storey above pulls a body up through its own floor, and the route up a
      villa stair takes a step that is not the one the scene asserts.
- [ ] `openshard-protocol --test bare_integer_fields`: the N10 allowlist has
      drifted behind `src/`. Four bare fields are unlisted —
      `HarvestPreview::action`, `HarvestPreview::cycles`, and `WeatherChange`'s
      `intensity` and `temperature`. Each wants a type or a reasoned entry, and
      the check cannot be trusted to catch a fifth while it is red for four.
- [ ] `cargo fmt --all` also has pre-existing drift in `server/housing/src/lib.rs`,
      `server/housing/src/tests.rs` and `server/world/src/tick/houses.rs`.

### Operations

- [ ] Add metrics, tracing, Prometheus, and health endpoints.
- [ ] Add plugin lifecycle and enable/disable support.
- [ ] Add the REST/JWT administration API.
- [ ] Build the dashboard, launcher, and map editor.
- [ ] Add the in-world operator shutdown command from `shutdown.md` S7.
- [ ] Add licence policy enforcement and third-party notices to releases.

### Client

- [ ] Complete client milestones M2–M5.
- [ ] Resolve the client, rendering, navigation, and instrumentation findings
      below.
- [ ] Add `verdata.mul` support and close the remaining client-version
      boundaries.

### Later

- [ ] LLM NPCs, quest generation, GM assistant, and Discord integration — only
      after the engine stands on its own.

## Detailed records

- [Protocol](protocol.md)
- [World and map](world-and-map.md)
- [Gameplay](gameplay.md)
- [Operations](operations.md)
- [Client](client/README.md)
- [Client compatibility](client-compatibility.md)
- [Later ideas](later.md)
