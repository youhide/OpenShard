# Plans

Intent, options, ordering and the criteria for moving on. Everything here is
work **not yet built**; what is built is described in [`../docs/`](../docs/README.md).

Layout mirrors the documentation domains:

```
plans/<domain>/<topic>/PLAN.md
plans/<domain>/<topic>/done/          closed plans, kept for the record
plans/roadmap/PLAN.md                 the order across domains, not inside one
```

A plan is edited only when the plan itself is revised. Progress does not belong
here — a measurement or a phase report is evidence and goes to
`docs/<domain>/<topic>/evidence/`; the single live "what is next" is the status
section of the topic's README. Closing a plan is one commit: the topic README is
brought to as-built, the decisions move into its `design_*`, the remainder into
its "what is open", and the plan is `git mv`'d into `done/`.

The roles this splits against are defined in
[`../docs/README.md`](../docs/README.md).
