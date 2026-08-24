# Client compatibility backlog

[Backlog](README.md) · [Roadmap](../README.md)

## Which client versions to support — see [`client_versions.md`](../../client_versions.md)

That document holds the evidence: which clients people actually play (7.0.x on
the big shards, 5.0.8.3 on the T2A/Renaissance ones), what changes between
versions in the files and on the wire, and how to obtain a set of files legally.

The backlog it leaves us, in order of size:

- **`verdata.mul` support.** Mandatory below 5.0.0a and entirely absent:
      `grep -rn verdata --include='*.rs' crates` finds nothing. `uo-rust-libs`
      `src/map/diff.rs` (MIT) is worth reading first for the sibling
      `mapdif`/`stadif` format, whose `*difl` lookup does not announce itself.
- [x] **A version-driven map width.** `MapSize::for_client` (`crates/common/protocol/src/world.rs`)
      clamps Felucca and Trammel to 6144 wide for a client below
      `ClientVersion::WIDE_MAP` (4.0.11d, sourced from ClassicUO's `CV_4011D` —
      Sphere's own `grayproto.h` has no MINCLIVER constant for map width at all,
      so this is not a `Feature`, which every entry of that table is pinned to
      one for). Wired at both places a map size reaches the wire: world entry
      (`0x1B`) and a mid-session facet change (`0x76`), the latter reading the
      traveller's version off the connection row.
- **The lower half of two protocol boundaries.** `Feature::NewContextMenu`
      (6.0.0.0) gates the *new* `0xBF.0x14.0x02` form, so nothing stops us
      sending the old form to a client with no popup menus at all. Same gap for
      cliloc: `Feature::Tooltips` (4.0.0a) covers OPL, the plain localized
      message `0xC1` has no entry.
- **The AoS boundary is Sphere's, not the client's.** `MINCLIVER_AOS` is
      4.0.0.0 while the client gained AoS features at 3.0.8z, so every client in
      `[3.0.8z, 4.0.0)` is told it has no AoS support when it does.
