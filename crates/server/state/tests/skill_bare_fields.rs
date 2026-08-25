//! The gate for the `Skill` newtype sweep (`docs/roadmap.md`'s "server/common/
//! render newtype hunt" backlog, `Skill` entry): every remaining bare
//! `skill: u8` in the workspace is on an explicit, reasoned allowlist, not
//! left behind by omission.
//!
//! Same shape as `crates/common/protocol/tests/facet_bare_fields.rs`, which
//! this borrows from almost verbatim — `Skill` is one name, not a class
//! hierarchy, so the check is "does `skill: u8` appear in this fixed list of
//! files, exactly this many times" rather than a type-shape matcher. It lives
//! here rather than in `protocol` because `Skill` is defined in this crate.

use std::fs;
use std::path::{Path, PathBuf};

/// Every file with a bare `skill: u8` left, how many times, and why. A file
/// not on this list with a hit is a violation; a listed count that no longer
/// matches is stale and the entry should have moved or been deleted with the
/// fix that changed it.
const ALLOWLIST: &[(&str, usize, &str)] = &[
    (
        "crates/server/world/src/tick/command.rs",
        3,
        "the Command queue: CastSpell/SetSkill/UseSkill cross it unchecked (N3's \"the queue is a delivery, not a checkpoint\"); the function that first reads it (cast_spell, set_skill, set_skill_cap, use_skill) promotes with Skill::from_id",
    ),
    (
        "crates/server/skills/src/lib.rs",
        3,
        "set_skill/set_skill_cap/use_skill: the public doors the Command queue's bare skill first reaches; each promotes with Skill::from_id, the same shape as set_skill_lock",
    ),
    (
        "crates/server/world/src/tick/tests.rs",
        1,
        "a local test helper that takes a literal id for readability and promotes internally",
    ),
];

/// How many times a bare `skill: u8` appears in `text`, independent of how
/// rustfmt-able whitespace is placed around its punctuation.
fn count_bare_skill(text: &str) -> usize {
    let compact: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    compact.match_indices("skill:u8").count()
}

#[test]
fn the_skill_counter_does_not_have_a_whitespace_escape_hatch() {
    assert_eq!(count_bare_skill("skill: u8"), 1);
    assert_eq!(count_bare_skill("skill : u8"), 1);
    assert_eq!(count_bare_skill("skill:\n    u8"), 1);
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
            && path.file_name().is_none_or(|name| name != "skill_bare_fields.rs")
        {
            out.push(path);
        }
    }
}

#[test]
fn every_bare_skill_field_is_on_the_allowlist() {
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
        let count = count_bare_skill(&text);
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
        "the Skill newtype gate disagrees with what's actually in the workspace.\n\
         \n\
         Found (file, count): {found:#?}\n\
         \n\
         Allowlisted (file, count): {allowed:#?}\n\
         \n\
         A file found but not (or not at the right count) allowlisted is a bare `skill: u8` \
         that either needs converting to `Skill`, or a new/updated allowlist entry with a \
         reason. A stale allowlist entry with nothing found means the field was already fixed \
         and the entry should have been deleted with it.",
    );
}
