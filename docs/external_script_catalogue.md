# External shard and script catalogue

An index of public UO shard code worth **reading** for content, behaviour, and
tools.  It is deliberately a catalogue, not a dependency list: OpenShard does
not vendor these packs and does not promise source compatibility with their
engines.  The snapshot was checked on 2026-08-26.

## Import rule

Before taking anything beyond an observation, record its upstream path and
licence.  A missing licence means **do not copy**.  Even a compatible licence
does not make an old script a design: first extract the player-visible rule,
then express and test it in OpenShard's data and Rust model.

## Housing and multis

| Source | What it contains | Good leads for OpenShard | Licence / use |
|---|---|---|---|
| [SphereServer Scripts-X](https://github.com/Sphereserver/Scripts-X) | Current official Sphere X scriptpack.  Its [housing](https://github.com/Sphereserver/Scripts-X/tree/main/housing) package covers signs, access, decay and house speech; [multis](https://github.com/Sphereserver/Scripts-X/tree/main/multis) contains stock houses, foundations, stairs and ships. | [Custom foundation definitions](https://github.com/Sphereserver/Scripts-X/blob/main/multis/m_foundations.scp): every rectangular 2-storey foundation has bounds, sign offset, price, storage and vendor capacity.  [House functions](https://github.com/Sphereserver/Scripts-X/blob/main/housing/house_functions.scp) are a useful cross-check for owner/co-owner/friend/guild access and decay cadence. | Apache-2.0 upstream.  May be read and, if ever copied, retain notices; prefer extracting facts and data rather than SphereScript. |
| [POL ModernDistro](https://github.com/polserver/ModernDistro) | Maintained, basic POL distribution.  It has ordinary [house multis](https://github.com/polserver/ModernDistro/tree/master/pkg/multis/house), a more elaborate [static-housing](https://github.com/polserver/ModernDistro/tree/master/pkg/systems/staticHousing) system, and a [custom-housing](https://github.com/polserver/ModernDistro/tree/master/pkg/systems/customHousing) package. | The custom deed [chooses a two- or three-storey foundation, derives its dimensions and price, checks placement, creates keys/sign/components, then marks the multi custom](https://github.com/polserver/ModernDistro/blob/master/pkg/systems/customHousing/scripts/customeHouseDeed.src).  Static housing's [footage test](https://github.com/polserver/ModernDistro/blob/master/pkg/systems/staticHousing/include/staticHousing.inc) is a concrete alternate model: several 3-D boxes define the interior. | No repository licence declared.  Research only until provenance is clarified. |
| [SE UO shard](https://github.com/vitorfdl/se-uo-shard) | A full POL 100 roleplay shard, including [`pkg/multis`](https://github.com/vitorfdl/se-uo-shard/tree/master/pkg/multis), a base [house package](https://github.com/vitorfdl/se-uo-shard/tree/master/pkg/multis/house), and an [architect system](https://github.com/vitorfdl/se-uo-shard/tree/master/pkg/systems/architect) with named parts, rectangles and sets. | Best candidate for examining how a real POL shard composes housing, construction content, economy, commands and world data instead of treating houses as an isolated feature. | Apache-2.0 repository.  Audit third-party package origins before reuse. |
| [Angel Island / Siege Perilous server](https://github.com/Luke-Tomasello/Angel-Island-Server) | Full RunUO-derived shard and world files.  The project documents a preview area and 56 purchasable custom houses. | A useful product/content reference: inspect its house catalogue, preview flow and how a shard lets players compare templates before purchase.  World saves and custom art are data to inspect, not assets to import. | GPL-3.0: observations only; no code or assets. |
| [Awesome UO Scripts](https://github.com/kaktaknet/awesome-uo-scripts) | A reorganised archive of 750+ legacy Sphere 0.55/0.56 scripts.  The [Housing](https://github.com/kaktaknet/awesome-uo-scripts/tree/main/ultima_online_scripts/Housing) section includes placement, rental, security, interior commands, crafted houses, preset multis and many `.wsc` designs. | Fastest idea bank for a house-template gallery: start with [placement](https://github.com/kaktaknet/awesome-uo-scripts/tree/main/ultima_online_scripts/Housing/House_Placement_v1), [rental](https://github.com/kaktaknet/awesome-uo-scripts/tree/main/ultima_online_scripts/Housing/House_Rental_v1), [security](https://github.com/kaktaknet/awesome-uo-scripts/tree/main/ultima_online_scripts/Housing/House_Security_v1), and legacy design files. | No repository licence; sources are archival and mixed-origin.  Strictly a quarantine/read-only source. |

### Concrete variants worth prototyping

1. **Foundation catalogue plus a preview lot.**  Offer a finite set of
   dimensioned foundations or prebuilt multis, show the footprint/cost/capacity
   before purchase, and hand the selected `multi_id` to the existing placement
   path.  Scripts-X provides the parameters; Angel Island validates that the
   preview-lot presentation works for players.
2. **Static/preset homes.**  Let a staff-authored building have a sign, owner,
   roles, interior volume and decor without requiring the AOS customisation
   protocol.  POL static housing is the strongest reference for this distinct
   feature.
3. **Custom-built homes.**  Keep the existing customisation design as the
   canonical protocol work, but use POL's explicit foundation/deed/sign/component
   lifecycle as a checklist.  Do not import eScript or its data blindly.
4. **House-adjacent content.**  Re-deeding, rentals, décor/deeds, crafted signs,
   telepads, vendors and security menus are independent, opt-in features.  They
   should be catalogued separately from placement so that a useful house system
   does not become a monolith.

## Other high-value script packs

| Source | Useful areas | Licence / use |
|---|---|---|
| [SphereServer Scripts-X](https://github.com/Sphereserver/Scripts-X) | [World decoration](https://github.com/Sphereserver/Scripts-X/tree/main/functions/worldgen/decoration), map-specific spawns, the region-creator dialog, and `static → multi`/centred-multi export tools.  Strong sources for making map content editable and reproducible. | Apache-2.0; still treat it as an observed content pipeline, not an architecture template. |
| [SphereServer Scripts](https://github.com/Sphereserver/Scripts) | The older official pack: `systems/`, `add-on/`, maps, NPCs, speech, stones and web/admin material.  Good for historical feature discovery and comparing behaviour against Scripts-X. | No repository licence declared; research only. |
| [POL ModernDistro](https://github.com/polserver/ModernDistro) | Packages for boats, deeds, decorations, spawns, commands and gumps.  Its boat package is especially useful as an independent implementation to compare with `docs/housing/design_boats.md`. | No repository licence declared; research only. |
| [RunUO custom scripts](https://github.com/felladrin/runuo-custom-scripts) | Small, separated gameplay additions with installation notes: a good source of feature boundaries and admin/player UX. | GPL-2.0: observations only. |

## Verified custom gump leads

The links below are concrete, player-visible or staff-facing gumps inspected in
the 2026-08-26 snapshot.  They are commit-pinned so an investigation can start
at the exact source examined.  They remain **references for behaviour and UX**:
the licence and import rules above still apply, and no layouts, art, or code
should be copied wholesale.

| Source | Gump | What is worth observing | Licence / use |
|---|---|---|---|
| [POL ModernDistro](https://github.com/polserver/ModernDistro) | [Boat navigator](https://github.com/polserver/ModernDistro/blob/def6509db8c6414239919f4775cc1043ebe65278/pkg/multis/boat/navigator/navigator.src) | A compact directional pad with turn, stop, drydock, anchor and speed controls.  Useful for deciding which boat actions deserve a single direct-manipulation window instead of speech commands. | No repository licence declared; research only. |
| POL ModernDistro | [House-sign gallery](https://github.com/polserver/ModernDistro/blob/def6509db8c6414239919f4775cc1043ebe65278/pkg/systems/staticHousing/include/signSelectionFunctions.inc) | A paged tile gallery for choosing a static house sign.  It is a good small reference for presenting a finite set of server-owned visual choices. | No repository licence declared; research only. |
| POL ModernDistro | [Guild menu](https://github.com/polserver/ModernDistro/blob/def6509db8c6414239919f4775cc1043ebe65278/pkg/systems/guilds/doGuildGump.src) | A charter plus icon, member/master actions, tooltips and paginated option rows.  Its role-sensitive menu is relevant to OpenShard's guild gump, although the underlying POL script is not reusable. | No repository licence declared; research only. |
| [SE UO shard](https://github.com/vitorfdl/se-uo-shard) | [Architect](https://github.com/vitorfdl/se-uo-shard/blob/342a39135d57445d8d7a3290c06754a314ee0111/pkg/systems/architect/textcmd/player/architect.src) | A persistent construction palette: categories, part preview, nudge controls, create/line/tile/ring actions, marking, undo, save/load and export.  Strong reference for staff construction UX, not for its POL implementation. | Apache-2.0 repository; audit package provenance before any reuse. |
| SE UO shard | [Character-creation wizard](https://github.com/vitorfdl/se-uo-shard/blob/342a39135d57445d8d7a3290c06754a314ee0111/pkg/systems/charactercreation/gumpcriacao.src) | A five-step flow for character, class, background, attributes and appearance, with progress rail and back/next navigation. | Apache-2.0 repository; audit package provenance before any reuse. |
| SE UO shard | [Hotbar](https://github.com/vitorfdl/se-uo-shard/blob/342a39135d57445d8d7a3290c06754a314ee0111/pkg/systems/charactercreation/hotbar/hotbar.src) and [talent book](https://github.com/vitorfdl/se-uo-shard/blob/342a39135d57445d8d7a3290c06754a314ee0111/pkg/systems/charactercreation/talentbook.src) | The hotbar is movable but non-closeable/non-disposable and binds skills, items and cooldown display; the book is a focused inspect-and-buy talent panel.  Both show shard-specific player UI rather than stock windows. | Apache-2.0 repository; audit package provenance before any reuse. |
| [Angel Island / Siege Perilous server](https://github.com/Luke-Tomasello/Angel-Island-Server) | [Leaderboard](https://github.com/Luke-Tomasello/Angel-Island-Server/blob/7cbbf405aa4182b90a6fb47f48bbd93c75f06792/scripts/Engines/Leaderboard/LeaderboardGump.cs) | A tabbed, paginated leaderboard whose tab width adapts to the number of visible tabs.  A useful UI pattern for rankings without a client extension. | GPL-3.0; observations only. |
| Angel Island / Siege Perilous server | [Township panel](https://github.com/Luke-Tomasello/Angel-Island-Server/blob/7cbbf405aa4182b90a6fb47f48bbd93c75f06792/scripts/Engines/Township/Gumps/TownshipGump.cs) | A role-sensitive multi-page settlement dashboard: upkeep, treasury history, NPCs, stockpile, permits, enemies and pack-up.  Good evidence that housing-adjacent administration benefits from a coherent navigation model. | GPL-3.0; observations only. |
| [RunUO custom scripts](https://github.com/felladrin/runuo-custom-scripts) | [Boat-control pad](https://github.com/felladrin/runuo-custom-scripts/blob/d8adb01ec64123eb7721f7ab7a13a40c842f4619/BoatControl%20Command/BoatControl.cs), [history viewer](https://github.com/felladrin/runuo-custom-scripts/blob/d8adb01ec64123eb7721f7ab7a13a40c842f4619/History%20Command/History.cs), and [hair-style picker](https://github.com/felladrin/runuo-custom-scripts/blob/d8adb01ec64123eb7721f7ab7a13a40c842f4619/ChangeHairStyle%20Command/ChangeHairStyle.cs) | Three intentionally small examples: command pad, paged scrollable log, and a selectable visual grid.  They are useful for isolating UX patterns from large server systems. | GPL-2.0; observations only. |

### Initial OpenShard-oriented shortlist

1. **Housing: sign/template gallery.**  Prototype a paged, server-owned choice
   gallery from the ModernDistro sign picker; pair each preview with dimensions,
   price and capacity from the foundation catalogue.
2. **Staff map/build tooling: architect palette.**  Extract the interaction
   model — category, preview, place/line/fill, undo and save/load — and express
   it over OpenShard's own map-edit commands and data.
3. **Player progression: talent book or character-creation wizard.**  These
   are clear candidates only if the corresponding gameplay model is added;
   neither UI should lead the design.
4. **Later, when settlements exist: township navigation.**  Its page structure
   is a useful reference for grouping funds, permissions, NPCs and history, but
   it would be premature to port before those systems exist.

## Triage workflow

1. Create one issue per candidate with an upstream permalink, commit hash,
   licence and a short statement of the player-visible behaviour.
2. Decide whether it is a protocol fact, a content/data idea, or a gameplay
   policy.  Only the first category can justify a compatibility port.
3. For a content/data candidate, translate it into OpenShard-owned JSON/TOML or
   Rust data and add a focused test.  Do not add an interpreter or a legacy-pack
   loader.
4. For no-licence or copyleft sources, retain only independently written notes
   and tests derived from observed behaviour.

The project-wide rules and the established engine references remain in
[`findings.md`](findings.md); this file is the discovery queue beside them.
