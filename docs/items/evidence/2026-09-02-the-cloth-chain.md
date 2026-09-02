# The cloth chain: a wheel, a loom, and a pair of scissors

> **This is a record.** It was written as part of `docs/crafting.md` and is kept
> as it was written. The model it describes as built is
> [`../design_crafting.md`](../design_crafting.md) — where the two differ, the
> design is right — and what is still open is ranked in
> [`../README.md`](../README.md), which is the only status page for this domain.
>
> **Its section numbers are that document's, not this file's.** §1–§4 are
> [`../design_crafting.md`](../design_crafting.md); §5, §7 and the numbered
> review are the three sibling records beside this one.

## 6. The cloth chain (2026-09-02)

Cloth (`0x1766`) is eaten by fifty-six tailoring rows, ten carpentry ones and
one smithing one, and until this slice **nothing on the shard made any** — the
leather gap of #11 again, one material over. ServUO's answer is not a craft at
all: it is two house addons a player uses items *on*, and a pair of scissors at
the end.

```
cotton (0xDF9) ─┐
flax  (0x1A9C) ─┴─► [spinning wheel, 6 s] ─► 6 × spool of thread (0xFA0)
wool   (0xDF8) ────► [spinning wheel, 6 s] ─► 3 × dark yarn (0xE1D)
tainted wool (0x101F) ► [    ″    ] ─► 1 × dark yarn

thread or yarn ×5 ─► [loom: 4 load, the 5th weaves] ─► bolt of cloth (0xF95)
bolt ─► [scissors] ─► 50 × cloth (0x1766)          ← what a tailor spends
```

- **Six new addons, no new machinery.** `LoomEast/South`,
  `SpinningWheelEast/South` and `ElvenSpinningWheelEast/South` are
  [`AddonKind`] variants with deed kinds 115-120, and they install through the
  path #3 and #5 built for the ovens: registered typed deeds on the shared
  scroll art, `AddonPart` grouping, whole-addon release, refund on collapse.
  The four carpentry rows that make them **already existed** in
  `carpentry.json`, generated from `DefCarpentry.cs` and crafting an inert
  `0x14F0` scroll because they carried no `kind` and no `addon`; giving them
  both is the whole of the data change. The loom's two-tile geometry comes from
  the generated `decoration::ADDON_COMPONENTS`, like the stone oven's; a wheel
  is one tile and reads its own resting art from `wheel_arts` rather than
  restating it (#5's defect, refused a second time).
- **The wheel is the first timed thing in `items`.** `Spinning` sits on the
  addon's root component; `advance_spins` runs beside `advance_crafts` in the
  tick. Every tile of the wheel redraws to its turning art and back — the
  `set_door` redraw, forget-then-reveal — and a turning wheel refuses a second
  pile (cliloc 502656). The art pairs are ServUO's own and **do not all move the
  same way**: the classic wheel counts up off its resting graphic and both elven
  ones count down, so a single "+1 while busy" rule would have drawn the elven
  wheels as different furniture entirely.
- **Hue is the through-line.** Cotton carries a dye, the thread keeps it, the
  bolt takes it from the *fifth* material rather than the four already loaded
  (ServUO's own choice), and the cut keeps it again. Nothing in the chain is a
  registered kind with a material family: a dye is not a grade, so these stay
  legacy art plus hue, which is also what the vendors already stock them as.
- **Where things land.** The wheel's yield goes to the spinner's pack, the
  loom's bolt to the weaver's pack, and the cut's cloth to whatever container
  the bolt was in — `ScissorHelper`'s parent rule, the same one the hides
  follow. A spinner who logged out inside the six seconds gets the thread on the
  wheel's own tile rather than nowhere: the fibre is already spent, and this
  engine's logged-out character has no pack to reach.
- **Still bought, not grown** — *as this slice landed*. Cotton, flax and wool
  reached a player from a vendor's shelf and nowhere else; `FarmableCotton`,
  `FarmableFlax` and shearing a sheep were a world slice of their own, and none
  of them is what made cloth unreachable. Two of the three grew a day later;
  see §7.
