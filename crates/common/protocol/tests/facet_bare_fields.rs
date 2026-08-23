//! `docs/facet_newtype.md`'s gate: every remaining bare `facet: u8` in the
//! workspace is on an explicit, reasoned allowlist, not left behind by
//! omission. Added at the end of the sweep's `world`/`scripting` stage, per
//! that plan's F6 — early stages would have spent more effort maintaining a
//! mostly-red gate than the gate was worth.
//!
//! Simpler than [`bare_integer_fields.rs`](bare_integer_fields.rs), which this
//! borrows its shape from: `Facet` is one name, not a class hierarchy such as
//! N10 pins down, so the check is "does `facet: u8` (or `facet:u8`) appear in
//! this fixed list of files, exactly this many times" rather than a
//! type-shape matcher walking every struct and enum in a crate. `protocol` is
//! where `Facet` is defined, so the test lives here and reaches out to the
//! rest of the workspace by a path relative to `CARGO_MANIFEST_DIR`, the same
//! way the plan's own survey did.

use std::fs;
use std::path::{Path, PathBuf};

/// Every file with a bare `facet: u8` left, how many times, and why —
/// `docs/facet_newtype.md`'s F2 and F4 carve-outs, plus the two examples that
/// "follow their crate's fix" (F3's survey table). A file not on this list
/// with a hit is a violation; a listed count that no longer matches is stale
/// and the entry should have moved or been deleted with the fix that changed
/// it.
const ALLOWLIST: &[(&str, usize, &str)] = &[
    (
        "crates/server/persistence/src/record.rs",
        9,
        "F2: the disk seam — a saved facet is a SQL column, not a live component",
    ),
    (
        "crates/server/persistence/src/sqlite.rs",
        1,
        "F2: same disk seam, the SQLite row struct",
    ),
    (
        "crates/server/persistence/src/pg.rs",
        3,
        "F2: same disk seam, the PostgreSQL row decode",
    ),
    (
        "crates/common/uofiles/src/map.rs",
        1,
        "F2: load_facet indexes FACET_SHAPES and formats a client filename — the number itself, not a domain value",
    ),
    (
        "crates/common/uofiles/examples/tile_probe.rs",
        1,
        "follows uofiles::map's own carve-out: the same raw number, read from argv",
    ),
    (
        "crates/client/artscan/src/bin/openshard-interiors-bake.rs",
        1,
        "follows tile_probe's carve-out: `--facet` as argv, widened to Facet at its first use",
    ),
    (
        "crates/client/artscan/src/bin/openshard-interiors-inspect.rs",
        1,
        "the same `--facet` argument on the inspector beside it",
    ),
    (
        "crates/common/movement/examples/coarse_bench.rs",
        1,
        "the same `--facet` argument on the coarse-router probe; widened at `Facet(cli.facet)`",
    ),
    (
        "crates/common/movement/examples/span_census.rs",
        1,
        "the same `--facet` argument on the span census, coarse_bench's shape: a command line takes a number and the map reader is handed `cli.facet`",
    ),
    (
        "crates/common/movement/examples/span_index.rs",
        1,
        "the same `--facet` argument on the span bake's own oracle, span_census's shape and beside it",
    ),
    (
        "crates/common/movement/examples/span_check.rs",
        1,
        "the same `--facet` argument on the span step rule's own oracle, span_index's shape and beside it",
    ),
    (
        "crates/client/render/examples/shard/mod.rs",
        1,
        "a standalone diagnostic tool reading a SQL column directly, the record.rs shape, in a crate with no protocol dependency",
    ),
    (
        "crates/client/render/tests/shard.rs",
        1,
        "the shard reader's SQL fixture row mirrors a database column and is intentionally raw at that test boundary",
    ),
    (
        "crates/server/state/build.rs",
        1,
        "the RegionSet that data/regions.json deserializes into: a build script cannot depend on the protocol crate, so the number is widened to Facet in the expression it emits",
    ),
    (
        "crates/server/world/build.rs",
        3,
        "the set-level facet of data/{spawns,deco,townsfolk}.json, the state/build.rs shape for the same reason: a build script has no protocol dependency, and every expression it emits carries a Facet",
    ),
];

/// How many times `facet: u8` or `facet:u8` appears in `text`, counting
/// overlapping whitespace variants once each — a plain substring count, since
/// the field name is one word and not a type a comment could plausibly
/// mention without meaning this one.
fn count_bare_facet(text: &str) -> usize {
    text.match_indices("facet: u8").count() + text.match_indices("facet:u8").count()
}

/// Every `.rs` file under `dir`, walked recursively. No `target/` ever
/// appears under `crates/`, so there is nothing to skip.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|error| panic!("could not read {dir:?}: {error}"));
    for entry in entries {
        let entry = entry.expect("directory entry must be readable");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            // This file's own prose names the pattern it looks for, which
            // would otherwise count as hits on itself.
            && path.file_name().is_none_or(|name| name != "facet_bare_fields.rs")
        {
            out.push(path);
        }
    }
}

#[test]
fn every_bare_facet_field_is_on_the_allowlist() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let crates_dir = workspace_root.join("crates");
    assert!(
        crates_dir.is_dir(),
        "expected {crates_dir:?} to exist — the gate is reading the wrong directory",
    );

    let mut files = Vec::new();
    collect_rs_files(&crates_dir, &mut files);
    assert!(
        files.len() > 100,
        "found only {} .rs files under {crates_dir:?} — the walk is broken",
        files.len(),
    );

    let mut found: Vec<(String, usize)> = Vec::new();
    for path in &files {
        let text = fs::read_to_string(path).expect("workspace source file must be readable");
        let count = count_bare_facet(&text);
        if count > 0 {
            let relative = path
                .strip_prefix(&workspace_root)
                .expect("every scanned file is under workspace_root")
                .to_str()
                .expect("workspace paths are valid UTF-8")
                .replace('\\', "/");
            found.push((relative, count));
        }
    }
    found.sort();

    let mut allowed: Vec<(String, usize)> = ALLOWLIST
        .iter()
        .map(|(file, count, _reason)| ((*file).to_owned(), *count))
        .collect();
    allowed.sort();

    assert_eq!(
        found, allowed,
        "docs/facet_newtype.md's gate disagrees with what's actually in the workspace.\n\
         \n\
         Found (file, count): {found:#?}\n\
         \n\
         Allowlisted (file, count): {allowed:#?}\n\
         \n\
         A file found but not (or not at the right count) allowlisted is a bare `facet: u8` \
         that either needs converting to `Facet`, or a new/updated allowlist entry with a \
         reason. A stale allowlist entry with nothing found means the field was already fixed \
         and the entry should have been deleted with it.",
    );
}
