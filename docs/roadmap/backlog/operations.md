# Operations backlog

[Backlog](README.md) · [Roadmap](../README.md)

## Licensing — backlog

The repository shipped a GPL-3.0 `LICENSE` while `Cargo.toml` declared
`MIT OR Apache-2.0`, so every crate's metadata contradicted the file for as
long as both existed. Resolved in favour of the metadata; the reasoning is the
`## Licence` section of the README. Two things it left open:

- **A licence gate in CI.** Nothing currently notices when a dependency
  arrives under terms the workspace cannot take. `cargo-deny` with a `[licenses]
  allow` list is the usual answer, and it belongs beside the three commands CI
  already runs. Today's audit of the tree, for the record: no dependency is
  copyleft-only except `cooked-waker` (MPL-2.0, pulled in by `deno_core`);
  `self_cell` offers `Apache-2.0 OR GPL-2.0-only` and `r-efi` offers an MIT
  option, so both are takeable, and no package is missing a licence field.
- **The MPL notice on a binary release.** MPL-2.0 is file-level copyleft and
  §3.3 explicitly allows a Larger Work under other terms, so `cooked-waker`
  constrains nothing about our own licence — but a distributed binary still owes
  its recipients the notice and an offer of that crate's source. Whatever builds
  the release artefacts should generate a third-party notices file rather than
  leaving this to be remembered.
