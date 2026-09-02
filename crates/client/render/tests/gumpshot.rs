//! A laid-out window, written out where a person can look at it.
//!
//! [`artshot`](../artshot.rs)'s twin, one layer up. That tool answers "what did
//! the artist draw"; this one answers "where did we put it" — which is the only
//! way to argue about a window's layout at all. A paperdoll's frame, its doll
//! and its buttons are three pictures whose *relationship* is the whole subject,
//! and a hex id says nothing about it.
//!
//! What is composited is [`gump::collect`]'s own quads over
//! [`GumpAtlas`]'s own pixels — the list the GPU pass draws, sampled from the
//! texture it samples — so a placement bug shows up here exactly as it shows up
//! on the screen. The glyphs are composited from the font atlas beside them and
//! every quad goes through
//! [`hue::tint`](openshard_client_render::hue::tint), the CPU port of the
//! pass's own fragment branch: the classic frames write nearly every caption in
//! `0x0386`, and a picture drawn without the lookup is a picture of an
//! unreadable window nobody has seen in play. Each line's origin is still
//! marked, because where a line starts is the layout fact this tool is for.
//!
//! Ignored and gated on `OPENSHARD_CLIENT`, like every other test that reads an
//! install:
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo test -p openshard-client-render --test gumpshot \
//!     -- --ignored --nocapture
//! ```
//!
//! The pictures land under `target/gumps/`, or wherever `OPENSHARD_GUMP_OUT`
//! points, at `OPENSHARD_GUMP_SCALE` screen pixels per gump pixel.

use std::collections::BTreeSet;
use std::fs;
use std::path::{
    Path,
    PathBuf,
};

use openshard_client_model::Skill as SkillLine;
use openshard_client_render::atlas::FontAtlas;
use openshard_client_render::gump::{
    self,
    ArtFiles,
    GumpArt,
    GumpAtlas,
    GumpPixel,
    Picture,
    Scissor,
};
use openshard_client_render::mobiles::EquipmentLayer;
use openshard_client_render::paperdoll::{
    self,
    Wearer,
    Whose,
};
use openshard_client_render::skills::{
    self,
    Standing,
    Tree,
};
use openshard_client_render::text::{
    self,
    GumpLabel,
};
use openshard_client_render::{
    container,
    renderer,
};
use openshard_protocol::containers::{
    ContainedItem,
    GridSlot,
};
use openshard_protocol::gump::layout::parse;
use openshard_protocol::gump::{
    ButtonId,
    GumpButton,
    GumpLayout,
    GumpPoint,
    SwitchId,
};
use openshard_protocol::serial::Serial;
use openshard_protocol::skill::SkillLock;
use openshard_protocol::wire::{
    Graphic,
    Hue,
    Layer,
};
use openshard_tiles::TileData;
use openshard_uofiles::art::Art;
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::font::AsciiFonts;
use openshard_uofiles::gumpart::Gumps;
use openshard_uofiles::skillgrp::SkillGroups;
use openshard_uofiles::skills::Skills as SkillNames;

/// The colour behind the window: one no gump pixel can be, so that "transparent"
/// and "black" are two different things in the picture — [`artshot`]'s own
/// backdrop, for the same reason.
const BACKDROP: [u8; 3] = [64, 0, 96];

/// A male human and a female one.
const MALE: u16 = 0x0190;
const FEMALE: u16 = 0x0191;

/// The files every scene below is drawn out of, or `None` where no client is
/// installed.
struct Client {
    gumps:      Gumps,
    art:        Art,
    equip_conv: EquipConv,
    tiledata:   TileData,
    /// `fonts.mul`, packed: the face a window's own text is drawn in.
    fonts:      FontAtlas,
    /// `hues.mul`, for the tint the GPU pass applies in its fragment stage.
    ///
    /// Without it every caption here draws in the font file's own near-black,
    /// which on the classic frames is a picture of an unreadable window that
    /// nobody has ever seen in play — see [`shoot`].
    hues:       openshard_uofiles::hues::Hues,
}

fn client() -> Option<Client> {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    let fonts = AsciiFonts::open(&dir).expect("fonts.mul");
    Some(Client {
        gumps:      Gumps::open(&dir).expect("gumpartLegacyMUL.uop"),
        art:        Art::open(&dir).expect("artLegacyMUL.uop"),
        // Optional in an install, and its absence resolves nothing — which is
        // the ordinary case for most rows anyway.
        equip_conv: EquipConv::load(dir.join("Equipconv.def")).unwrap_or_default(),
        tiledata:   openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul"),
        fonts:      FontAtlas::build(&fonts).expect("ten faces of small glyphs fit an atlas"),
        hues:       openshard_uofiles::hues::Hues::load(dir.join("hues.mul")).expect("hues.mul"),
    })
}

impl Client {
    fn files(&self) -> ArtFiles<'_> {
        ArtFiles {
            gumps: &self.gumps,
            items: &self.art,
        }
    }

    /// The `AnimID` a worn item draws by, which is what a paperdoll layer
    /// carries: the wire graphic is `tiledata`'s key and never reaches one.
    fn worn(&self, layer: Layer, graphic: u16) -> EquipmentLayer {
        EquipmentLayer {
            graphic: self.tiledata.static_tile(graphic).anim_id,
            hue: Hue::NONE,
            layer,
        }
    }
}

fn out_dir() -> PathBuf {
    match std::env::var_os("OPENSHARD_GUMP_OUT") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/gumps"),
    }
}

/// Screen pixels per gump pixel. Two by default: gump art is small, and a
/// one-pixel seam between two pieces of a background is the defect this tool
/// exists to show.
fn scale() -> u32 {
    std::env::var("OPENSHARD_GUMP_SCALE")
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(2)
}

/// Every window in this file, side by side on disk.
#[test]
#[ignore = "reads a real install and writes pictures for a person"]
fn what_the_layout_puts_where() {
    let Some(client) = client() else {
        return;
    };
    let out = out_dir();
    fs::create_dir_all(&out).expect("a place to write");

    paperdolls(&client, &out);
    bag(&client, &out);
    dialog(&client, &out);
    skill_window(&client, &out);
    status_windows(&client, &out);
}

/// Both status frames, out of the same character.
///
/// Two pictures rather than one because the whole point of the pair is that
/// they say the same eleven facts in two shapes: a number that landed in the
/// wrong column on one of them is only visible next to the other. The arrows
/// are stated — a frame drawn with `locks: None` is the honest picture of a
/// client that has had no `0xBF 0x19` yet, and it is also a picture with no
/// arrows in it to check.
fn status_windows(client: &Client, out: &Path) {
    use openshard_client_model::Status;
    use openshard_client_render::status::{
        self,
        Form,
    };
    use openshard_protocol::mobile::{
        AosStatus,
        DamageRange,
        Resistances,
        StatLockBits,
        Vitals,
    };

    let status = Status {
        name:          "Lord British".to_owned(),
        female:        false,
        strength:      100,
        dexterity:     50,
        intelligence:  75,
        stamina:       Vitals {
            current: 49,
            max:     50,
        },
        gold:          1_234,
        armor:         42,
        weight:        12,
        max_weight:    450,
        stat_cap:      225,
        followers:     0,
        followers_max: 5,
        // Distinct values throughout, so a field drawn from its neighbour is
        // visible rather than plausible.
        resistances:   Resistances {
            fire:   12,
            cold:   8,
            poison: 3,
            energy: 5,
        },
        luck:          140,
        damage:        DamageRange { min: 5, max: 11 },
        tithing:       40,
        aos:           AosStatus {
            max_physical:         70,
            max_fire:             71,
            max_cold:             72,
            max_poison:           73,
            max_energy:           74,
            defense_chance:       15,
            max_defense_chance:   45,
            hit_chance:           20,
            swing_speed:          25,
            damage_increase:      30,
            lower_reagent_cost:   35,
            spell_damage:         40,
            faster_cast_recovery: 4,
            faster_casting:       2,
            lower_mana_cost:      8,
        },
    };
    // One arrow of each face, so all three graphics are in the picture.
    let numbers = status::Numbers {
        status: &status,
        hits:   Vitals {
            current: 98,
            max:     100,
        },
        mana:   Vitals {
            current: 72,
            max:     75,
        },
        locks:  Some(StatLockBits {
            strength:     SkillLock::Up,
            dexterity:    SkillLock::Down,
            intelligence: SkillLock::Locked,
        }),
    };
    for (form, name) in [(Form::Old, "status-old"), (Form::Modern, "status-modern")] {
        let window = status::window(
            form,
            numbers,
            |text, font| text::gump_width(text, font, &client.fonts),
            GumpPixel::new(0, 0),
        );
        let lines: Vec<(GumpLabel<'_>, Option<Scissor>)> =
            window.lines.iter().map(|line| (line.label(), None)).collect();
        shoot(client, &window.pictures, &lines, &window.rule_quads(), out, name);
    }
}

/// Our own doll and a stranger's, dressed the same.
///
/// The two frames differ — `0x07D0` has room down its right-hand side for the
/// buttons a player gets over their own — and drawing both is what makes the
/// difference visible rather than asserted.
fn paperdolls(client: &Client, out: &Path) {
    // A shirt, a pair of pants, a robe, boots and the backpack every character
    // wears: enough layers that an ordering mistake shows.
    let equipment = [
        client.worn(Layer::SHIRT, 0x1517),
        client.worn(Layer::PANTS, 0x152E),
        client.worn(Layer::SHOES, 0x170B),
        client.worn(Layer::ROBE, 0x1F03),
        client.worn(Layer::BACKPACK, 0x0E75),
    ];
    for (body, whose, who, name) in [
        (
            MALE,
            Whose::Own { war: false },
            "Lord British",
            "paperdoll-male-own",
        ),
        (
            FEMALE,
            Whose::Another,
            "Sarah the tailor",
            "paperdoll-female-another",
        ),
    ] {
        let wearer = Wearer {
            body:      Graphic(body),
            hue:       Hue::NONE,
            equipment: &equipment,
        };
        let at = GumpPixel::new(0, 0);
        let doll = paperdoll::window(
            Some(&wearer),
            whose,
            // Nothing held: what a pressed button looks like is a question for
            // the app, which is the only thing that has a mouse.
            None,
            None,
            None,
            &client.equip_conv,
            &client.gumps,
            at,
        );
        shoot(
            client,
            &doll.pictures,
            &[(paperdoll::title(who, at), None)],
            &[],
            out,
            name,
        );
    }
}

/// A container: one background out of `gumpart` with the world's own art on it.
fn bag(client: &Client, out: &Path) {
    const BACKPACK: Graphic = Graphic(0x003C);
    let item = |serial: u32, graphic: u16, x: i32, y: i32| {
        ContainedItem {
            serial:  Serial::new(serial).unwrap(),
            graphic: Graphic(graphic),
            amount:  openshard_protocol::items::ItemAmount(1),
            at:      GumpPoint::new(x, y),
            grid:    GridSlot(0),
            hue:     Hue::NONE,
        }
    };
    let contents = [
        item(0x4000_0001, 0x0E75, 40, 40),
        item(0x4000_0002, 0x0F0E, 90, 60),
        item(0x4000_0003, 0x0EED, 60, 90),
    ];
    let pictures = container::window(BACKPACK, &contents, GumpPixel::new(0, 0));
    shoot(client, &pictures, &[], &[], out, "container-backpack");

    // The same bag with its client-side action under it, in both faces. The
    // button hangs off the background's bottom edge and its caption off the
    // button's right edge, so where those two land is arithmetic over two art
    // sizes and neither is written down — which is the whole reason to look.
    let mut atlas = GumpAtlas::build(
        client.files(),
        [
            GumpArt::Gump(BACKPACK),
            GumpArt::Gump(container::ACTION_UP),
            GumpArt::Gump(container::ACTION_DOWN),
        ],
    )
    .expect("the bag and both button faces");
    let button =
        container::stack_all_button(&atlas, BACKPACK, GumpPixel::new(0, 0)).expect("both are packed");
    // `shoot` grows an atlas of its own from the pictures; this one only
    // answered the two sizes the placement is made of.
    atlas.take_dirty();
    for (lit, name) in [(false, "container-action-up"), (true, "container-action-down")] {
        let pictures = container::window_with_action(
            BACKPACK,
            &contents,
            GumpPixel::new(0, 0),
            None,
            Some((button, container::action_face(lit))),
        );
        let caption = GumpLabel {
            at:   button.label_at(),
            hue:  Hue(0x0386),
            clip: None,
            text: container::STACK_ALL_LABEL,
            font: openshard_protocol::speech::Font(1),
        };
        shoot(client, &pictures, &[(caption, None)], &[], out, name);
    }
}

/// A `0xB0` dialog, laid out through the same path the shard's own reach: a
/// background to nine-slice, buttons, a switch and a `{ tilepic }`.
fn dialog(client: &Client, out: &Path) {
    let mut layout = GumpLayout::new();
    layout.background(0, 0, 300, 270, 5054);
    layout.label(105, 14, 2100, "Admin");
    layout.button(30, 54, 4005, 4007, GumpButton::Reply, 0, ButtonId(13));
    layout.label(66, 56, 1153, "Populate Felucca");
    layout.button(30, 84, 4005, 4007, GumpButton::Reply, 0, ButtonId(14));
    layout.label(66, 86, 1153, "Wipe spawners");
    layout.check(30, 120, 210, 211, false, SwitchId(1));
    layout.label(66, 122, 1153, "Include dungeons");
    layout.item(200, 150, Graphic(0x0E75), Hue::NONE);
    layout.cropped_label(30, 200, 120, 20, 1153, "cropped to its own box");
    let (string, lines) = layout.finish();
    let lines: Vec<String> = lines.to_vec();
    let elements = parse(string);

    // Every page's art, packed before the window is laid out: where a
    // background's edges go is decided by how big its corners turned out to be.
    let mut atlas = GumpAtlas::build(client.files(), gump::art_of(&elements)).expect("the layout's art");
    let window = gump::window(&elements, GumpPixel::new(0, 0), 0, &BTreeSet::new(), None, &atlas);
    // `shoot` grows an atlas of its own from the pictures; this one only
    // answered the nine-slice's sizes.
    atlas.take_dirty();
    // A caption names a *row* of the table that arrived beside the layout, and
    // resolving one is the caller's job — see `gump::Caption`. This is that
    // resolution, and in the app it is the same three lines.
    let text: Vec<(GumpLabel<'_>, Option<Scissor>)> = window
        .captions
        .iter()
        .map(|caption| {
            let text = match caption.source {
                gump::CaptionSource::Line(line) => lines[line].as_str(),
                // This synthetic layout only ever uses `label`/`cropped_label`.
                gump::CaptionSource::Cliloc(_) => unreachable!("no cliloc element in this layout"),
            };
            (
                GumpLabel {
                    at: caption.at,
                    hue: caption.hue,
                    clip: caption.clip,
                    text,
                    font: gump::CAPTION_FONT,
                },
                None,
            )
        })
        .collect();
    shoot(client, &window.pictures, &text, &[], out, "dialog-admin");
}

/// The skill window, scrolled off its first row.
///
/// Scrolled *on purpose*: at the top of the list every row is whole, and the
/// one thing this window does that no other does — cut a row in half at the
/// viewport's edge — cannot be seen at all. The picture is the argument for the
/// scissor, so it has to show one.
fn skill_window(client: &Client, out: &Path) {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("gated by `client()`"));
    let names = SkillNames::open(&dir).expect("Skills.idx and skills.mul");
    let groups = SkillGroups::open(&dir).expect("skillgrp.mul");
    let mut tree = Tree::default();
    let content = skills::content_height(&names, &groups, &tree);
    // Half a row down, and one heading shut, so the picture carries both of the
    // things the tree can do.
    tree.toggle(SkillGroups::MISC);
    tree.scroll_to(30, content);
    let at = GumpPixel::new(0, 0);
    let sheet = skills::window(
        &names,
        &groups,
        &tree,
        // A value that says which row it is on rather than a flat number: a
        // column drawn from the wrong skill looks right until the numbers
        // differ.
        |id| {
            Some(Standing {
                skill: SkillLine {
                    value: u16::from(id.0) * 10,
                    base:  0,
                    lock:  SkillLock::Up,
                    cap:   1000,
                },
                lock:  match id.0 % 3 {
                    0 => SkillLock::Up,
                    1 => SkillLock::Down,
                    _ => SkillLock::Locked,
                },
            })
        },
        |text, font| text::gump_width(text, font, &client.fonts),
        at,
    );
    let lines: Vec<(GumpLabel<'_>, Option<Scissor>)> = sheet
        .lines
        .iter()
        .map(|line| (line.label(), line.scissor))
        .collect();
    shoot(client, &sheet.pictures, &lines, &[], out, "skills");
}

/// Composite a laid-out window and write it out.
///
/// The pictures are placed by whoever laid the window out, so the extent drawn
/// here is theirs too: the bounding box of every quad, plus a margin, and *not*
/// a size anything asked the window for — see `container::size`'s docs for why
/// a window has no size to ask about.
///
/// The text is composited from the *other* atlas, which is the one thing here
/// that a single GPU pass could not do either: two textures, two draw calls,
/// and the letters over the art in both.
fn shoot(
    client: &Client,
    pictures: &[Picture],
    lines: &[(GumpLabel<'_>, Option<Scissor>)],
    plates: &[openshard_client_render::sprite::SpriteQuad],
    out: &Path,
    name: &str,
) {
    let mut atlas = GumpAtlas::empty();
    atlas
        .add(client.files(), pictures.iter().map(|picture| picture.graphic))
        .expect("the window's art");
    let mut quads = gump::collect(pictures, &atlas);
    assert!(!quads.is_empty(), "{name} drew nothing at all");
    // Over the art and under the text, which is where the pass draws them. A
    // plate carries no region at all (`gump::plate`), so the loop below paints
    // it rather than sampling an atlas that has nothing at those coordinates.
    quads.extend(plates.iter().copied());
    // A line at a time, because a box is a line's own: the skill window cuts
    // its rows to the list and writes its total outside it, and a single
    // `collect_gump` over the lot could only apply one box or none — which is
    // exactly the mistake the app made before it drew the window and looked.
    let glyphs: Vec<_> = lines
        .iter()
        .flat_map(|(line, scissor)| {
            let mut quads = text::collect_gump(std::slice::from_ref(line), &client.fonts);
            if let Some(scissor) = scissor {
                scissor.cut(&mut quads);
            }
            quads
        })
        .collect();
    let art_quads = quads.len();
    quads.extend(glyphs);

    const MARGIN: i32 = 8;
    let (mut left, mut top, mut right, mut bottom) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for quad in &quads {
        left = left.min(quad.rect.x as i32);
        top = top.min(quad.rect.y as i32);
        right = right.max((quad.rect.x + quad.rect.width) as i32);
        bottom = bottom.max((quad.rect.y + quad.rect.height) as i32);
    }
    let (left, top) = (left - MARGIN, top - MARGIN);
    let (width, height) = ((right - left + MARGIN) as u32, (bottom - top + MARGIN) as u32);

    let mut rgb = vec![BACKDROP; (width * height) as usize];
    let side = renderer::SPRITE_ATLAS_SIDE;
    for (index, quad) in quads.iter().enumerate() {
        // A plate: no region at all, and `u` carrying its shade instead of a
        // texture coordinate. `gump.wgsl` paints it; so does this.
        if quad.region.du == 0.0 && quad.region.dv == 0.0 {
            let shade = (quad.region.u * 255.0).round() as u8;
            for row in 0..quad.rect.height as i32 {
                for column in 0..quad.rect.width as i32 {
                    let x = quad.rect.x as i32 + column - left;
                    let y = quad.rect.y as i32 + row - top;
                    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                        continue;
                    }
                    rgb[(y as u32 * width + x as u32) as usize] = [shade, shade, shade];
                }
            }
            continue;
        }
        // Which texture this quad samples: the art up to `art_quads`, the font
        // after it. The GPU says the same thing by binding one and then the
        // other, and the split is what a draw call is.
        let texels = match index < art_quads {
            true => atlas.pixels(),
            false => client.fonts.pixels(),
        };
        // The region is normalised over the atlas; the quad is drawn one texel
        // to one gump pixel, which is what the pass does with `Nearest` and no
        // camera. So the copy is a rectangle, not a sample.
        let (u, v) = (
            (quad.region.u * side as f32).round() as i32,
            (quad.region.v * side as f32).round() as i32,
        );
        for row in 0..quad.rect.height as i32 {
            for column in 0..quad.rect.width as i32 {
                let at = ((v + row) as usize * side as usize + (u + column) as usize) * 4;
                // Transparent where nothing was packed: the pass discards those
                // texels rather than blending them.
                if texels[at + 3] == 0 {
                    continue;
                }
                let x = quad.rect.x as i32 + column - left;
                let y = quad.rect.y as i32 + row - top;
                if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                    continue;
                }
                let texel = [texels[at], texels[at + 1], texels[at + 2]];
                // The pass's own fragment stage, on the CPU — `hue::tint`. A
                // picture drawn without it is a picture of the atlas rather
                // than of the window: the classic frames write nearly every
                // caption in `0x0386`, and the raw font is near-black.
                let shown = openshard_client_render::hue::tint(&client.hues, Hue(quad.hue as u16), texel)
                    .unwrap_or(texel);
                rgb[(y as u32 * width + x as u32) as usize] = shown;
            }
        }
    }

    // The corner every line of text was placed from, so that a caption sitting
    // a pixel off its box can be seen to be doing so rather than read as the
    // font's own bearing.
    for (line, _) in lines {
        mark(
            &mut rgb,
            width,
            height,
            line.at.x - left,
            line.at.y - top,
            [255, 255, 64],
        );
    }

    let scale = scale();
    let (sw, sh) = (width * scale, height * scale);
    let mut scaled = Vec::with_capacity((sw * sh * 3) as usize);
    for y in 0..sh {
        for x in 0..sw {
            scaled.extend_from_slice(&rgb[((y / scale) * width + (x / scale)) as usize]);
        }
    }
    let path = out.join(format!("{name}.png"));
    openshard_client_render::png::write(&path, sw, sh, &scaled).expect("the picture writes");
    println!("{name}: {width}x{height} gump pixels -> {}", path.display());
}

/// One pixel of overlay, in gump coordinates already shifted to the picture.
fn mark(rgb: &mut [[u8; 3]], width: u32, height: u32, x: i32, y: i32, colour: [u8; 3]) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    rgb[(y as u32 * width + x as u32) as usize] = colour;
}
