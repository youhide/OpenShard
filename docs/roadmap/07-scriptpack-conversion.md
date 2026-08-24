# 7. Scriptpack conversion — dropped with §5

It was a one-shot `.scp` → TS/TOML converter: read a SphereServer scriptpack once,
emit content a shard could edit as normal source. It made sense while the
destination was TypeScript on an embedded V8.

There is no TypeScript now. The destination for ported content is `data/*.json`
compiled by a `build.rs`, and the one conversion this project actually did —
ServUO's tables into `crates/*/data/` — was done with throwaway scripts whose
output was reviewed by hand and committed, which is what this section was really
asking for.

If a `.scp` pack is ever converted, the shape to copy is the migration's:
convert into JSON, put it behind a `build.rs` that rejects what the data cannot
say about itself, and prove it against the source with a test that compares
`Command`s. **`crates/server/server/src/content.rs` was that test's home**, and
`git log` has all eight of them.
