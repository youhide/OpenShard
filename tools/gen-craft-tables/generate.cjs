#!/usr/bin/env node
//
// Turn ServUO's craft tables into this engine's recipe data.
//
// A one-shot build tool, not an engine feature: it runs once, its output is
// committed under `crates/server/crafting/data/` as `*.json`, and from then on
// those files are edited as ordinary data. The same bargain the Community Pack's
// `convert-servuo.cjs` makes, and the roadmap's own instruction for the recipes.
//
// The JSON is what `crates/server/crafting/build.rs` turns into `const` tables
// before the crate compiles — so nothing downstream of here knows this ran, and
// re-running it against a newer ServUO shows a diff of what changed rather than
// a rewritten wall of Rust.
//
//   node tools/gen-craft-tables/generate.cjs [path-to-servuo] [--dry]
//
// The one hard part is that ServUO names a crafted item by its **C# type** and
// this engine needs a **graphic**. So the script indexes every class under
// `Scripts/` and walks up the inheritance chain to whatever constructor finally
// passes a literal item id — `RingmailGloves` to `BaseArmor` to `base(0x13eb)`.
// A type that will not resolve is **dropped and printed**, never guessed: the
// `resolveBody` lesson from the creature converter, where a silent fallback put
// the wrong art on the map and nobody noticed for a month.

'use strict';

const fs = require('fs');
const path = require('path');

const SERVUO = process.argv[2] && !process.argv[2].startsWith('--')
  ? process.argv[2]
  : path.join(process.env.HOME, 'Git', 'ServUO');
const DRY = process.argv.includes('--dry');
// A table that already exists has been *edited as data* since it was generated —
// typed `kind`/`addon` rows, the hand-written dough row, per-recipe chance
// floors — and none of that is in the C#. Writing over it silently loses the
// work, which is what a first run of this tool for a new trade did to seven
// tables. Adding a trade therefore only writes the file that is missing; a
// deliberate re-port against a newer ServUO says `--force` and reads the diff.
const FORCE = process.argv.includes('--force');
const OUT = path.join(__dirname, '..', '..', 'crates', 'server', 'crafting', 'data');

// The expansions this shard can be set to. `[gameplay] expansion` tops out at
// ML, so anything gated above it names content the engine does not have.
const SUPPORTED_EXPANSIONS = new Set(['AOS', 'SE', 'ML', 'LBR', 'UOR', 'T2A', 'None']);
const UNSUPPORTED_EXPANSIONS = ['SA', 'HS', 'TOL', 'EJ'];

// The tables this slice ports, and the constants each `Def*.cs` header
// carries that are not in its recipe list.
const SYSTEMS = [
  { file: 'DefBlacksmithy', module: 'blacksmithy', skill: 'Blacksmith' },
  { file: 'DefTailoring', module: 'tailoring', skill: 'Tailoring' },
  { file: 'DefCarpentry', module: 'carpentry', skill: 'Carpentry' },
  { file: 'DefTinkering', module: 'tinkering', skill: 'Tinkering' },
  { file: 'DefAlchemy', module: 'alchemy', skill: 'Alchemy' },
  {
    file: 'DefBowFletching',
    module: 'fletching',
    skill: 'Fletching',
    // The combat table currently knows the three classic ranged weapons. Keep
    // the material chain and those weapons; darts and expansion bows would be
    // craftable props until their combat rows are ported.
    types: new Set(['Kindling', 'Shaft', 'Arrow', 'Bolt', 'Bow', 'Crossbow', 'HeavyCrossbow']),
    // ServUO's colored-item table includes BaseWeapon, not these resources.
    // Special wood therefore colors a bow but still makes ordinary shafts.
    plainTypes: new Set(['Kindling', 'Shaft', 'Arrow', 'Bolt']),
  },
  { file: 'DefCooking', module: 'cooking', skill: 'Cooking' },
  {
    file: 'DefInscription',
    module: 'inscription',
    skill: 'Inscribe',
    // The one table that does not write its rows as `AddCraft`: sixty-four
    // Magery scrolls go through an `AddSpell` helper reading two fields set
    // between the circles (`m_Circle` picks the skill band and the group,
    // `m_Mana` the mana the scribe pays). `expand` rewrites those calls into
    // the statements the parser below already understands, so there is one
    // parser and not two, and the numbers still come out of the C#.
    expand: expandInscription,
    // A Magery scroll is exactly a row whose art falls in the classic run the
    // spellbook reads, `0x1F2D + spell`. What that leaves out is content this
    // engine has not reached rather than rows worth shipping inert: the
    // necromancy scrolls (no such spells here, and their reagents grow
    // nowhere), and the Mondain's Legacy artifact books, whose own materials
    // do not exist. The two books that stay are the two this shard already has
    // items for.
    keep: (recipe) =>
      (recipe.graphic >= 0x1f2d && recipe.graphic <= 0x1f2d + 63)
      || recipe.type === 'Runebook'
      || recipe.type === 'Spellbook',
  },
];

// ---------------------------------------------------------------------------
// Reading C#

/** Strip `//` and block comments without touching string literals. */
function stripComments(src) {
  let out = '';
  let i = 0;
  while (i < src.length) {
    const two = src.substr(i, 2);
    if (two === '//') {
      while (i < src.length && src[i] !== '\n') i++;
    } else if (two === '/*') {
      i += 2;
      while (i < src.length && src.substr(i, 2) !== '*/') i++;
      i += 2;
    } else if (src[i] === '"') {
      out += src[i++];
      while (i < src.length && src[i] !== '"') {
        if (src[i] === '\\') out += src[i++];
        out += src[i++];
      }
      out += src[i++];
    } else {
      out += src[i++];
    }
  }
  return out;
}

/** Every `.cs` file under a directory. */
function allSources(dir, found = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) allSources(full, found);
    else if (entry.name.endsWith('.cs')) found.push(full);
  }
  return found;
}

/** The `{...}` block starting at or after `from`, as text. */
function blockAt(src, from) {
  const open = src.indexOf('{', from);
  if (open < 0) return null;
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === '{') depth++;
    else if (src[i] === '}') {
      depth--;
      if (depth === 0) return { body: src.slice(open + 1, i), end: i + 1 };
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Class index: type name -> item graphic

/** name -> { base, flip, src } for every class in the tree. */
const classes = new Map();

function indexClasses() {
  const files = allSources(path.join(SERVUO, 'Scripts'));
  const decl = /(?:^|\n)\s*(?:\[[^\]]*\]\s*)*(?:public|internal|private|protected)?\s*(?:sealed\s+|abstract\s+|static\s+|partial\s+)*class\s+(\w+)\s*(?::\s*([^{]+))?\{/g;
  for (const file of files) {
    const src = stripComments(fs.readFileSync(file, 'utf8'));
    let match;
    decl.lastIndex = 0;
    while ((match = decl.exec(src)) !== null) {
      const name = match[1];
      if (classes.has(name)) continue; // first definition wins; duplicates are partials
      const bases = (match[2] || '')
        .split(',')
        .map((s) => s.trim().replace(/<.*/, ''))
        .filter(Boolean);
      const block = blockAt(src, match.index + match[0].length - 1);
      // A `[Flipable(a, b)]` above the class is the fallback graphic: several
      // items set their id that way and never pass one to a constructor.
      const before = src.slice(Math.max(0, match.index - 300), match.index + match[0].length);
      const flip = /\[Flip(?:able|ableAttribute|Attribute)?\s*\(\s*(0x[0-9A-Fa-f]+|\d+)/.exec(before);
      classes.set(name, {
        base: bases[0] || null,
        flip: flip ? Number(flip[1]) : null,
        body: block ? block.body : '',
      });
    }
  }
}

/**
 * The handful of types whose art no constructor states.
 *
 * Each of these computes its item id in an override (`BaseBeverage.ComputeItemID`
 * returns a literal buried in a method; `Key` takes it from a `KeyType` enum), so
 * the walk up the constructors finds nothing. Three items is not worth teaching
 * the resolver to read method bodies — but a **silent** fallback would be, which
 * is why they are named here rather than defaulted.
 */
const GRAPHIC_OVERRIDES = new Map([
  ['Goblet', 0x099a], // BaseBeverage.ComputeItemID
  ['PewterMug', 0x0fff], // the same
  ['Key', 0x1010], // KeyType.Iron, the default a tinker makes
  // The two books state their art through a *chained* constructor, and the walk
  // below follows `base(…)` only: `Spellbook() : this((ulong)0)` reaches
  // `this(content, 0xEFA)`, and `Runebook()` reaches a `base(Core.AOS ? id :
  // 0xEFA)` whose AoS arm is a defaulted parameter, `int id = 0x22C5`. Both ids
  // are the ones `openshard_state` already holds as `SPELLBOOK_GRAPHIC` and
  // `RUNEBOOK_GRAPHIC`, which is what makes naming them here safe.
  ['Spellbook', 0x0efa],
  ['Runebook', 0x22c5],
]);

const graphicCache = new Map();

/**
 * The item graphic a C# type draws as, or null.
 *
 * Looks for a literal first argument to `base(...)` in a constructor, then an
 * `ItemID = ...` in the body, then the class's `[Flipable]`, then the same
 * questions of its base class. Constructors are tried shortest-first because a
 * `[Constructable]` no-argument one is the shape a player-made item takes.
 */
function graphicOf(name, seen = new Set()) {
  if (GRAPHIC_OVERRIDES.has(name)) return GRAPHIC_OVERRIDES.get(name);
  if (graphicCache.has(name)) return graphicCache.get(name);
  if (seen.has(name)) return null;
  seen.add(name);
  const info = classes.get(name);
  if (!info) return null;

  const ctors = [];
  const ctor = new RegExp(`\\b${name}\\s*\\(([^)]*)\\)\\s*:\\s*base\\s*\\(([^)]*)\\)`, 'g');
  let match;
  while ((match = ctor.exec(info.body)) !== null) {
    ctors.push({ params: match[1].trim(), args: match[2] });
  }
  ctors.sort((a, b) => a.params.length - b.params.length);
  for (const { params, args } of ctors) {
    if (/\bSerial\b/.test(params)) continue; // the deserialising constructor
    for (const arg of splitArgs(args)) {
      const literal = /^(0x[0-9A-Fa-f]+|\d+)$/.exec(arg.trim());
      if (literal) {
        const id = Number(literal[1]);
        // Small numbers are amounts, hues and resource enums, not art. Every
        // real item id in the client's tiledata is above this.
        if (id > 0x40) return remember(name, id);
      }
    }
  }
  const assigned = /\bItemID\s*=\s*(0x[0-9A-Fa-f]+|\d+)/.exec(info.body);
  if (assigned && Number(assigned[1]) > 0x40) return remember(name, Number(assigned[1]));
  if (info.flip) return remember(name, info.flip);
  if (info.base) {
    const inherited = graphicOf(info.base, seen);
    if (inherited) return remember(name, inherited);
  }
  return null;
}

function remember(name, id) {
  graphicCache.set(name, id);
  return id;
}

/** Split an argument list on top-level commas. */
function splitArgs(text) {
  const out = [];
  let depth = 0;
  let current = '';
  for (const ch of text) {
    if (ch === '(' || ch === '<') depth++;
    else if (ch === ')' || ch === '>') depth--;
    if (ch === ',' && depth === 0) {
      out.push(current);
      current = '';
    } else current += ch;
  }
  if (current.trim()) out.push(current);
  return out;
}

// ---------------------------------------------------------------------------
// Materials: type -> (graphic, hue)

/** Resource type name -> { graphic, hue } from `Misc/ResourceInfo.cs`. */
const resourceHues = new Map();

function indexResources() {
  const src = stripComments(
    fs.readFileSync(path.join(SERVUO, 'Scripts', 'Misc', 'ResourceInfo.cs'), 'utf8'),
  );
  // `new CraftResourceInfo(0x973, 1053108, "Dull Copper", …, typeof(DullCopperIngot), …)`.
  // The AoS leather table is preferred over the pre-AoS one where both name a
  // type, because `[gameplay] expansion` defaults to ML; the pre-AoS hues are a
  // deferred refinement, noted in the roadmap.
  const aosLeather = src.indexOf('m_AOSLeatherInfo');
  const row = /new CraftResourceInfo\(\s*(0x[0-9A-Fa-f]+|\d+)\s*,\s*\d+\s*,\s*"[^"]*"\s*,[^,]*,\s*CraftResource\.\w+\s*((?:,\s*typeof\(\w+\))+)/g;
  let match;
  while ((match = row.exec(src)) !== null) {
    const hue = Number(match[1]);
    const preferred = match.index > aosLeather;
    for (const type of match[2].matchAll(/typeof\((\w+)\)/g)) {
      const name = type[1];
      if (resourceHues.has(name) && !preferred) continue;
      resourceHues.set(name, hue);
    }
  }
}

/** A crafting material as this engine holds one: a graphic and a hue. */
function materialOf(type) {
  const graphic = graphicOf(type);
  if (graphic === null) return null;
  return { graphic, hue: resourceHues.get(type) || 0 };
}

// ---------------------------------------------------------------------------
// Parsing one Def*.cs

/** Drop `if (Core.SA) { … }` blocks whole, keeping any `else`. */
function dropUnsupported(src) {
  for (;;) {
    const guard = new RegExp(`if\\s*\\(\\s*(?:!\\s*)?(?:Core\\.(?:${UNSUPPORTED_EXPANSIONS.join('|')}))[^)]*\\)`).exec(src);
    if (!guard) break;
    const negated = /\(\s*!/.test(guard[0]);
    const block = blockAt(src, guard.index + guard[0].length);
    if (!block) {
      src = src.slice(0, guard.index) + src.slice(guard.index + guard[0].length);
      continue;
    }
    // `if (!Core.SA)` is the branch a pre-SA shard takes, so keep its body.
    const keep = negated ? block.body : '';
    src = src.slice(0, guard.index) + keep + src.slice(block.end);
  }
  return src;
}

/** Split a method body into `;`-terminated statements, ignoring braces. */
function statements(src) {
  const out = [];
  let depth = 0;
  let current = '';
  for (const ch of src) {
    if (ch === '(') depth++;
    else if (ch === ')') depth--;
    if (ch === ';' && depth === 0) {
      out.push(current.trim());
      current = '';
    } else if (ch === '{' || ch === '}') {
      if (current.trim()) out.push(current.trim());
      current = '';
    } else current += ch;
  }
  if (current.trim()) out.push(current.trim());
  return out.filter(Boolean);
}

/** `1044036` or `"a string"` — ServUO's TextDefinition union. */
function text(arg) {
  const trimmed = arg.trim();
  const literal = /^"(.*)"$/.exec(trimmed);
  if (literal) return { str: literal[1] };
  if (/^-?\d+$/.test(trimmed)) return { cliloc: Number(trimmed) };
  return null;
}

/** A numeric argument, resolving `Core.AOS ? a : b` to the AoS branch. */
function number(arg) {
  const ternary = /\?\s*([^:]+):/.exec(arg);
  if (ternary) arg = ternary[1];
  const value = /-?(?:0x[0-9A-Fa-f]+|\d+(?:\.\d+)?)/.exec(arg);
  return value ? Number(value[0]) : null;
}

/** Skills are in tenths here and doubles there. */
function tenths(arg) {
  const value = number(arg);
  return value === null ? null : Math.round(value * 10);
}

// ---------------------------------------------------------------------------
// DefInscription's scroll helper

// The clilocs `AddSpell` computes rather than states. Each is a base plus the
// `Reg` enum's own value, so they are transcribed once here and indexed the way
// the C# indexes them.
const SPELL_NAME_BASE = 1044381; // `1044381 + m_Index++`, one per scroll in order
const REG_NAME_BASE = 1044353; // `1044353 + (int)reg`
const REG_MESSAGE_BASE = 1044361; // `1044361 + (int)reg`
const BLANK_SCROLL_NAME = 1044377;
const BLANK_SCROLL_MESSAGE = 1044378;

/**
 * The eight circles' skill bands and group clilocs, read out of `AddSpell`'s own
 * switch rather than copied: the numbers are irregular (-25.0, -10.8, 3.5, …)
 * and a transcription error in them would be invisible — a scroll that wants
 * the wrong skill still crafts.
 */
function inscriptionCircles(src) {
  const circles = [];
  const row = /case\s+(\d+):\s*minSkill\s*=\s*(-?[\d.]+);\s*maxSkill\s*=\s*(-?[\d.]+);\s*cliloc\s*=\s*(\d+);/g;
  let match;
  while ((match = row.exec(src)) !== null) {
    circles[Number(match[1])] = {
      min: Number(match[2]),
      max: Number(match[3]),
      group: Number(match[4]),
    };
  }
  if (circles.length !== 8 || circles.some((c) => !c)) {
    throw new Error(`DefInscription: found ${circles.length} spell circles, expected 8`);
  }
  return circles;
}

/**
 * The `Reg` enum in its declared order, checked against the `m_RegTypes` table
 * beside it. The enum's *value* is what both reagent clilocs are computed from
 * and the type at the same index is what the row consumes, so the two lists
 * agreeing is the whole reason `AddSpell` can name a reagent once.
 */
function inscriptionReagents(src) {
  const names = /private\s+enum\s+Reg\s*\{([^}]*)\}/.exec(src);
  const table = /m_RegTypes\s*=\s*new\s+Type\[\]\s*\{([^}]*)\}/.exec(src);
  if (!names || !table) throw new Error('DefInscription: no Reg enum or m_RegTypes table');
  const enumerated = names[1].split(',').map((s) => s.trim()).filter(Boolean);
  const types = [...table[1].matchAll(/typeof\(\s*(\w+)\s*\)/g)].map((m) => m[1]);
  if (enumerated.length !== types.length) {
    throw new Error(`DefInscription: ${enumerated.length} Reg names against ${types.length} types`);
  }
  for (let i = 0; i < types.length; i++) {
    if (enumerated[i] !== types[i]) {
      throw new Error(`DefInscription: Reg.${enumerated[i]} is m_RegTypes[${i}] = ${types[i]}`);
    }
  }
  return enumerated;
}

/**
 * Rewrite `AddSpell(typeof(HealScroll), Reg.Garlic, …)` into the `AddCraft` /
 * `AddRes` / `SetManaReq` statements every other table writes by hand.
 *
 * `AddNecroSpell` and `AddMysticSpell` are deliberately left alone: they are
 * two more families of scroll for spells this shard does not have, and a row
 * expanded here would only be dropped by `keep` a moment later — with the
 * difference that a reader of the drop list would have to work out why.
 */
function expandInscription(lines, src) {
  const circles = inscriptionCircles(src);
  const reagents = inscriptionReagents(src);
  const out = [];
  let circle = 0;
  let mana = 0;
  let index = 0;
  let expanded = 0;
  for (const line of lines) {
    const setCircle = /^m_Circle\s*=\s*(\d+)$/.exec(line);
    if (setCircle) {
      circle = Number(setCircle[1]);
      continue;
    }
    const setMana = /^m_Mana\s*=\s*(\d+)$/.exec(line);
    if (setMana) {
      mana = Number(setMana[1]);
      continue;
    }
    const spell = /^AddSpell\(([\s\S]*)\)$/.exec(line);
    if (!spell) {
      out.push(line);
      continue;
    }
    const args = splitArgs(spell[1]).map((a) => a.trim());
    const type = /typeof\((\w+)\)/.exec(args[0]);
    const regs = args.slice(1).map((arg) => {
      const named = /^Reg\.(\w+)$/.exec(arg);
      const at = named ? reagents.indexOf(named[1]) : -1;
      if (at < 0) throw new Error(`DefInscription: ${line} names no known reagent in ${arg}`);
      return at;
    });
    if (!type || regs.length === 0) throw new Error(`DefInscription: cannot expand ${line}`);
    const band = circles[circle];
    const name = SPELL_NAME_BASE + index++;
    const first = regs[0];
    out.push(
      `index = AddCraft(typeof(${type[1]}), ${band.group}, ${name}, `
      + `${band.min}, ${band.max}, typeof(${reagents[first]}), ${REG_NAME_BASE + first}, 1, `
      + `${REG_MESSAGE_BASE + first})`,
    );
    for (const reg of regs.slice(1)) {
      out.push(`AddRes(index, typeof(${reagents[reg]}), ${REG_NAME_BASE + reg}, 1, ${REG_MESSAGE_BASE + reg})`);
    }
    out.push(`AddRes(index, typeof(BlankScroll), ${BLANK_SCROLL_NAME}, 1, ${BLANK_SCROLL_MESSAGE})`);
    out.push(`SetManaReq(index, ${mana})`);
    expanded++;
  }
  if (expanded !== 64) {
    throw new Error(`DefInscription: expanded ${expanded} Magery scrolls, expected 64`);
  }
  return out;
}

const dropped = [];

function parseSystem(spec) {
  const file = path.join(SERVUO, 'Scripts', 'Services', 'Craft', `${spec.file}.cs`);
  let src = stripComments(fs.readFileSync(file, 'utf8'));
  const init = src.indexOf('InitCraftList');
  const body = blockAt(src, init);
  if (!body) throw new Error(`no InitCraftList in ${spec.file}`);
  const plain = statements(dropUnsupported(body.body));
  // A table whose rows are written through a helper hands the parser the
  // statements that helper stands for; every other table is already in that
  // shape. The whole file is passed along because the helper's own constants
  // live outside `InitCraftList`.
  const lines = spec.expand ? spec.expand(plain, src) : plain;

  const groups = [];
  const recipes = [];
  const byVar = new Map();
  let subRes = null;

  const groupIndex = (definition) => {
    const key = JSON.stringify(definition);
    const found = groups.findIndex((g) => JSON.stringify(g) === key);
    if (found >= 0) return found;
    groups.push(definition);
    return groups.length - 1;
  };

  for (const line of lines) {
    const call = /(?:(\w+)\s*=\s*)?\b(AddCraft|AddRes|AddSkill|SetSubRes|SetSubRes2|AddSubRes|AddSubRes2|SetItemHue|SetUseAllRes|SetMinSkillOffset|ForceNonExceptional|ForceExceptional|SetNeedHeat|SetNeedOven|SetNeedMill|SetNeedWater|SetNeedMaker|AddRecipe|SetNeededThemePack|SetRequiresBasketWeaving|SetRequireResTarget|AddCreateItem|AddCraftAction|SetData|SetDisplayID|SetUseSubRes2|SetNeededExpansion|SetForceSuccess|SetManaReq)\s*\(([\s\S]*)\)\s*$/.exec(line);
    if (!call) continue;
    const [, assigned, name, rawArgs] = call;
    const args = splitArgs(rawArgs);

    switch (name) {
      case 'AddCraft': {
        const type = /typeof\((\w+)\)/.exec(args[0]);
        if (!type) break;
        // The 10-argument overload names the skill; the 9-argument one means
        // the system's own.
        const named = /SkillName\.\w+/.test(args[3] || '');
        const at = named ? 1 : 0;
        const graphic = graphicOf(type[1]);
        const recipe = {
          type: type[1],
          graphic,
          group: groupIndex(text(args[1])),
          name: text(args[2]),
          skills: [],
          resources: [],
          amount: 1,
          hue: 0,
          retainColor: !spec.plainTypes?.has(type[1]),
          useAllRes: false,
          minSkillOffset: 0,
          markable: markable(type[1]),
          mana: 0,
          neverExceptional: false,
          alwaysExceptional: false,
          needs: {},
          drop: graphic === null ? `no graphic for ${type[1]}` : null,
        };
        const mainMin = tenths(args[3 + at]);
        const mainMax = tenths(args[4 + at]);
        recipe.skills.push({ skill: named ? csSkill(args[3]) : spec.skill, min: mainMin, max: mainMax });
        const resType = /typeof\((\w+)\)/.exec(args[5 + at] || '');
        if (resType) {
          const material = materialOf(resType[1]);
          if (!material) recipe.drop = recipe.drop || `no graphic for material ${resType[1]}`;
          else {
            recipe.resources.push({
              type: resType[1],
              graphic: material.graphic,
              hue: material.hue,
              amount: number(args[7 + at]) ?? 1,
              name: text(args[6 + at]) || { cliloc: 0 },
              message: text(args[8 + at]) || { cliloc: 502925 },
            });
          }
        }
        recipes.push(recipe);
        if (assigned) byVar.set(assigned, recipe);
        break;
      }
      case 'AddRes': {
        const recipe = byVar.get(args[0].trim()) || recipes[recipes.length - 1];
        const resType = /typeof\((\w+)\)/.exec(args[1] || '');
        if (!recipe || !resType) break;
        const material = materialOf(resType[1]);
        if (!material) {
          recipe.drop = recipe.drop || `no graphic for material ${resType[1]}`;
          break;
        }
        recipe.resources.push({
          type: resType[1],
          graphic: material.graphic,
          hue: material.hue,
          amount: number(args[3]) ?? 1,
          name: text(args[2]) || { cliloc: 0 },
          message: text(args[4]) || { cliloc: 502925 },
        });
        break;
      }
      case 'AddSkill': {
        const recipe = byVar.get(args[0].trim()) || recipes[recipes.length - 1];
        if (!recipe) break;
        recipe.skills.push({
          skill: csSkill(args[1]),
          min: tenths(args[2]),
          max: tenths(args[3]),
        });
        break;
      }
      case 'SetItemHue':
        withRecipe(args[0], (r) => { r.hue = number(args[1]) ?? 0; });
        break;
      case 'SetUseAllRes':
        withRecipe(args[0], (r) => { r.useAllRes = /true/.test(args[1]); });
        break;
      case 'SetMinSkillOffset':
        withRecipe(args[0], (r) => { r.minSkillOffset = tenths(args[1]) ?? 0; });
        break;
      // ServUO's `SetManaReq`, the mana a scribe pays for the scroll on top of
      // its reagents. Checked twice like every other gate and spent only when
      // the item is actually made.
      case 'SetManaReq':
        withRecipe(args[0], (r) => { r.mana = number(args[1]) ?? 0; });
        break;
      case 'ForceNonExceptional':
        withRecipe(args[0], (r) => { r.neverExceptional = true; });
        break;
      case 'ForceExceptional':
        withRecipe(args[0], (r) => { r.alwaysExceptional = true; });
        break;
      case 'SetNeedHeat':
        withRecipe(args[0], (r) => { r.needs.heat = true; });
        break;
      case 'SetNeedOven':
        withRecipe(args[0], (r) => { r.needs.oven = true; });
        break;
      case 'SetNeedMill':
        withRecipe(args[0], (r) => { r.needs.mill = true; });
        break;
      case 'SetNeedWater':
        withRecipe(args[0], (r) => { r.needs.water = true; });
        break;
      // Each of these names a subsystem this slice does not have. The recipe is
      // dropped rather than silently shipped without its gate — an unlockable
      // recipe with no lock is a rare item every new character can make.
      case 'AddRecipe':
        withRecipe(args[0], (r) => { r.drop = r.drop || 'recipe-scroll gated'; });
        break;
      case 'SetNeededThemePack':
        withRecipe(args[0], (r) => { r.drop = r.drop || 'theme pack'; });
        break;
      case 'SetRequiresBasketWeaving':
        withRecipe(args[0], (r) => { r.drop = r.drop || 'basket weaving'; });
        break;
      case 'SetNeededExpansion':
        withRecipe(args[0], (r) => {
          const expansion = /Expansion\.(\w+)/.exec(args[1]);
          if (expansion && !SUPPORTED_EXPANSIONS.has(expansion[1])) {
            r.drop = r.drop || `expansion ${expansion[1]}`;
          }
        });
        break;
      case 'SetRequireResTarget':
      case 'AddCreateItem':
      case 'AddCraftAction':
      case 'SetData':
      case 'SetDisplayID':
        withRecipe(args[0], (r) => { r.drop = r.drop || 'custom craft'; });
        break;
      case 'SetUseSubRes2':
        withRecipe(args[0], (r) => { r.drop = r.drop || 'scales axis'; });
        break;
      case 'SetSubRes': {
        const type = /typeof\((\w+)\)/.exec(args[0]);
        const material = type && materialOf(type[1]);
        if (material) subRes = { graphic: material.graphic, name: text(args[1]), entries: [] };
        break;
      }
      case 'AddSubRes': {
        if (!subRes) break;
        const type = /typeof\((\w+)\)/.exec(args[0]);
        const material = type && materialOf(type[1]);
        if (!material) break;
        subRes.entries.push({
          type: type[1],
          hue: material.hue,
          name: text(args[1]),
          reqSkill: tenths(args[2]) ?? 0,
          message: text(args[4]) || text(args[3]) || { cliloc: 1044268 },
        });
        break;
      }
      default:
        break;
    }
  }

  function withRecipe(arg, act) {
    const recipe = byVar.get(arg.trim()) || recipes[recipes.length - 1];
    if (recipe) act(recipe);
  }

  const kept = [];
  for (const recipe of recipes) {
    if (spec.types && !spec.types.has(recipe.type)) {
      recipe.drop = recipe.drop || 'no gameplay row';
    }
    if (spec.keep && !spec.keep(recipe)) {
      recipe.drop = recipe.drop || 'no gameplay row';
    }
    if (recipe.drop) {
      dropped.push(`${spec.file}: ${recipe.type} — ${recipe.drop}`);
      continue;
    }
    // The material axis substitutes into whichever line names its graphic.
    if (subRes) {
      for (const res of recipe.resources) {
        if (res.graphic === subRes.graphic) res.fromAxis = true;
      }
    }
    kept.push(recipe);
  }
  // A dropped recipe must not leave a hole in the group numbering, but groups
  // are referenced by index, so they are kept whole and unused ones simply draw
  // an empty page. Renumbering would be one more thing to get wrong.
  return { spec, groups, recipes: kept, subRes, total: recipes.length };
}

/** `SkillName.Blacksmith` -> `Blacksmith`. */
function csSkill(arg) {
  const match = /SkillName\.(\w+)/.exec(arg || '');
  return match ? match[1] : null;
}

/**
 * Whether an exceptional one carries its maker's name — ServUO's `IsMarkable`,
 * which is a list of base classes rather than a flag.
 */
// Not all of `m_MarkableTable`, which also names three dozen concrete furniture
// and container types: only the bases the trades ported so far actually make,
// plus the two books, which the table names by their own type and which are the
// only rows here whose maker's mark a player ever sees.
const MARKABLE_BASES = [
  'BaseArmor', 'BaseWeapon', 'BaseClothing', 'BaseJewel', 'BaseTool',
  'BaseHarvestTool', 'BaseInstrument', 'BaseQuiver', 'DragonBardingDeed',
  'Spellbook', 'Runebook',
];
function markable(type) {
  const seen = new Set();
  let name = type;
  while (name && !seen.has(name)) {
    if (MARKABLE_BASES.includes(name)) return true;
    seen.add(name);
    name = classes.get(name)?.base || null;
  }
  return false;
}

// ---------------------------------------------------------------------------
// Emitting Rust

/** ServUO skill names that differ from this engine's `Skill` variants. */
const SKILL_RENAMES = {
  Blacksmith: 'Blacksmith',
  Tailoring: 'Tailoring',
  Carpentry: 'Carpentry',
  Tinkering: 'Tinkering',
  Alchemy: 'Alchemy',
  Magery: 'Magery',
  Mining: 'Mining',
  ArmsLore: 'ArmsLore',
  Fletching: 'Fletching',
  Inscribe: 'Inscribe',
  Cartography: 'Cartography',
  Cooking: 'Cooking',
  Tasteid: 'TasteId',
  TasteID: 'TasteId',
  Lumberjacking: 'Lumberjacking',
  Musicianship: 'Musicianship',
  Poisoning: 'Poisoning',
};

// A `TextDefinition` as the data file writes it: a bare number is a cliloc, a
// quoted string is the literal arm.
function jsonText(value) {
  if (!value) return '0';
  if (value.cliloc !== undefined) return String(value.cliloc);
  return JSON.stringify(value.str);
}

// Only the requirements that are actually set — the key is absent on 484 of the
// 485 rows, which is the whole reason the data file is readable.
function jsonNeeds(needs) {
  const set = Object.keys(needs).filter((k) => needs[k]);
  if (set.length === 0) return null;
  return `{ ${set.map((k) => `"${k}": true`).join(', ')} }`;
}

// The trade's `data/*.json`, in exactly the shape `crates/server/crafting/build.rs`
// deserialises — and in exactly the layout the committed files already have, so
// re-running this against a newer ServUO shows a diff of what *changed* rather
// than a diff of the whole table.
//
// Every field the build script defaults is left out when it holds the default.
// `deny_unknown_fields` on the far side means a key misspelt here fails the
// build rather than being ignored.
function emit(parsed) {
  const { groups, recipes, subRes } = parsed;
  const out = [];
  out.push('{');
  out.push(`  "groups": [${groups.map(jsonText).join(', ')}],`);

  if (subRes && subRes.entries.length) {
    out.push('  "sub_res": {');
    out.push(`    "graphic": "${hex(subRes.graphic)}",`);
    out.push(`    "name": ${jsonText(subRes.name)},`);
    out.push('    "entries": [');
    const entries = subRes.entries.map(
      (e) =>
        `      { "hue": "${hex(e.hue)}", "name": ${jsonText(e.name)}, ` +
        `"req_skill": ${e.reqSkill}, "message": ${jsonText(e.message)} }`,
    );
    out.push(entries.join(',\n'));
    out.push('    ]');
    out.push('  },');
  }

  out.push('  "recipes": [');
  const rows = recipes.map((recipe) => {
    const row = [];
    row.push('    {');
    row.push(`      "graphic": "${hex(recipe.graphic)}",`);
    row.push(`      "name": ${jsonText(recipe.name)},`);
    row.push(`      "group": ${recipe.group},`);
    // `amount` is 1 for everything this tool produces; the field exists for the
    // rows a shard edits by hand afterwards.
    if (recipe.hue) row.push(`      "hue": "${hex(recipe.hue)}",`);
    if (!recipe.retainColor) row.push('      "retain_color": false,');
    if (recipe.useAllRes) row.push('      "use_all_res": true,');
    if (recipe.minSkillOffset) row.push(`      "min_skill_offset": ${recipe.minSkillOffset},`);
    if (recipe.mana) row.push(`      "mana": ${recipe.mana},`);
    if (recipe.markable) row.push('      "markable": true,');
    if (recipe.neverExceptional) row.push('      "never_exceptional": true,');
    if (recipe.alwaysExceptional) row.push('      "always_exceptional": true,');
    const needs = jsonNeeds(recipe.needs);
    if (needs) row.push(`      "needs": ${needs},`);

    const skills = recipe.skills.map((s) => {
      const variant = SKILL_RENAMES[s.skill] || s.skill;
      return `{ "skill": "${variant}", "min": ${s.min}, "max": ${s.max} }`;
    });
    row.push(`      "skills": [${skills.join(', ')}],`);

    if (recipe.resources.length) {
      const lines = recipe.resources.map((r) => {
        const parts = [`"graphic": "${hex(r.graphic)}"`];
        if (r.hue) parts.push(`"hue": "${hex(r.hue)}"`);
        parts.push(`"amount": ${r.amount}`);
        parts.push(`"name": ${jsonText(r.name)}`);
        parts.push(`"message": ${jsonText(r.message)}`);
        if (r.fromAxis) parts.push('"from_axis": true');
        return `        { ${parts.join(', ')} }`;
      });
      row.push(`      "resources": [\n${lines.join(',\n')}\n      ]`);
    } else {
      row.push('      "resources": []');
    }
    row.push('    }');
    return row.join('\n');
  });
  out.push(rows.join(',\n'));
  out.push('  ]');
  out.push('}');
  return `${out.join('\n')}\n`;
}

function hex(value) {
  return `0x${(value || 0).toString(16).toUpperCase().padStart(4, '0')}`;
}

// ---------------------------------------------------------------------------

function main() {
  process.stdout.write('indexing ServUO classes… ');
  indexClasses();
  indexResources();
  console.log(`${classes.size} classes, ${resourceHues.size} materials`);

  for (const spec of SYSTEMS) {
    const parsed = parseSystem(spec);
    const table = emit(parsed);
    const file = path.join(OUT, `${spec.module}.json`);
    const kept = fs.existsSync(file) && !FORCE;
    if (!DRY && !kept) {
      fs.mkdirSync(OUT, { recursive: true });
      fs.writeFileSync(file, table);
    }
    const axis = parsed.subRes ? `${parsed.subRes.entries.length} materials` : 'no axis';
    console.log(
      `${spec.module.padEnd(12)} ${String(parsed.recipes.length).padStart(4)}/${parsed.total} recipes, ` +
      `${parsed.groups.length} groups, ${axis}` +
      (kept ? '  — kept: edited as data since (--force to re-port)' : ''),
    );
  }

  console.log(`\n${dropped.length} recipes dropped:`);
  const reasons = new Map();
  for (const line of dropped) {
    const reason = line.split('— ')[1];
    reasons.set(reason, (reasons.get(reason) || 0) + 1);
  }
  for (const [reason, count] of [...reasons].sort((a, b) => b[1] - a[1])) {
    console.log(`  ${String(count).padStart(4)}  ${reason}`);
  }
  if (process.argv.includes('--verbose')) for (const line of dropped) console.log(`    ${line}`);
}

main();
