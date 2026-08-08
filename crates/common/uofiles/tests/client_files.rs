//! What the readers do on a real client install, rather than on a fixture.
//!
//! Every test here skips unless `OPENSHARD_CLIENT` points at a UO client
//! directory. No client files enter this repository — they are copyrighted, and
//! a path that is right on one machine is wrong on every other.
//!
//! # Why a synthetic fixture is not enough
//!
//! A fixture is written by the same understanding that wrote the parser, so the
//! two agree by construction. Every mistake this suite exists to catch — a facet
//! whose shape the block count cannot name, a tiledata layout guessed from a
//! size, a container whose entries are not in the order they look like they are
//! in — is a mistake a fixture reproduces faithfully. These are the assertions
//! only a shipped file can settle.
//!
//! The install these numbers were taken from is client 7.0.116.0.

use openshard_protocol::speech::Font;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_uofiles::anim::{Anim, BodyKind, DIRECTIONS};
use openshard_uofiles::art::Art;
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::font::{AsciiFonts, CHARS_PER_FONT, FONT_COUNT, GLYPH_BASE};
use openshard_uofiles::hues::Hues;
use openshard_uofiles::map::Map;
use openshard_uofiles::texmaps::{TEXTURE_COUNT, TexMaps, TextureId};
use openshard_uofiles::tiledata::{LAND_TILE_COUNT, TileData, TileDataFormat};

/// The client directory, or `None` to skip.
///
/// Read at runtime rather than compile time so that setting the variable does
/// not need a rebuild.
fn client_dir() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    dir.join("tiledata.mul").exists().then_some(dir)
}

fn tiledata() -> Option<TileData> {
    let dir = client_dir()?;
    Some(TileData::load(dir.join("tiledata.mul")).expect("a client ships a readable tiledata.mul"))
}

/// Every facet a client ships, and what it must come out as.
const FACETS: [(u8, u32, u32, &str); 6] = [
    (0, 7168, 4096, "Felucca/Trammel (post-ML)"),
    (1, 7168, 4096, "Felucca/Trammel (post-ML)"),
    (2, 2304, 1600, "Ilshenar"),
    (3, 2560, 2048, "Malas"),
    (4, 1448, 1448, "Tokuno"),
    (5, 1280, 4096, "Ter Mur"),
];

#[test]
fn every_facet_loads_as_the_facet_it_actually_is() {
    // The regression this suite was written for. Malas and Ter Mur are both
    // 81,920 blocks, so before the facet number reached the size decision,
    // `load_facet(dir, 5)` returned a 2560x2048 map named "Malas" — 256 blocks
    // per column instead of 512, which transposes everything past the first
    // column and reports no error at all.
    //
    // Nothing inside either file can catch this: the maps are the same length,
    // the staidx files are the same length, and every block in both is a
    // well-formed 196 bytes. Only the number the caller asked for can.
    let Some(dir) = client_dir() else {
        return;
    };
    for (facet, width, height, name) in FACETS {
        let map = Map::load_facet(&dir, facet).unwrap_or_else(|e| panic!("map{facet} should load: {e}"));
        assert_eq!(
            (map.width(), map.height()),
            (width, height),
            "map{facet} came out the wrong shape"
        );
        assert_eq!(map.facet_name(), name);
        assert!(map.static_count() > 1000, "map{facet} has almost no statics");
    }
}

#[test]
fn a_facets_far_corner_is_on_the_map_and_one_past_it_is_not() {
    // The bounds follow from the shape, so this is the shape again from the
    // other side: on a facet loaded as the wrong one, the corner it claims and
    // the corner it has would disagree.
    let Some(dir) = client_dir() else {
        return;
    };
    for (facet, width, height, _) in FACETS {
        let map = Map::load_facet(&dir, facet).unwrap();
        let (far_x, far_y) = ((width - 1) as u16, (height - 1) as u16);
        assert!(
            map.land(far_x, far_y).is_some(),
            "map{facet} is short of its corner"
        );
        assert!(
            !map.contains(width as u16, far_y),
            "map{facet} is wider than it says"
        );
        assert!(
            !map.contains(far_x, height as u16),
            "map{facet} is taller than it says"
        );
    }
}

#[test]
fn the_statics_a_facet_reports_are_the_statics_its_index_describes() {
    // An independent count, from the raw `staidx`/`statics` pair rather than
    // through the loader: 12-byte index entries, 7-byte statics, and the two
    // sentinels the loader treats as "nothing here". If the loader ever dropped
    // a block — a wrong block count, an off-by-one in the index walk — the two
    // numbers part company. It is a re-derivation rather than a second source,
    // which is worth saying out loud, but it is a different walk over the same
    // bytes and it catches a loader that silently loses blocks.
    let Some(dir) = client_dir() else {
        return;
    };
    let facet = 0u8;
    let index = std::fs::read(dir.join(format!("staidx{facet}.mul"))).unwrap();
    let data = std::fs::read(dir.join(format!("statics{facet}.mul"))).unwrap();

    let mut expected = 0usize;
    for entry in index.chunks_exact(12) {
        let offset = u32::from_le_bytes(entry[0..4].try_into().unwrap());
        let length = u32::from_le_bytes(entry[4..8].try_into().unwrap());
        if offset == u32::MAX || length == u32::MAX || length == 0 {
            continue;
        }
        let (offset, length) = (offset as usize, length as usize);
        if offset + length > data.len() {
            continue;
        }
        expected += length / 7;
    }

    let map = Map::load_facet(&dir, facet).unwrap();
    assert_eq!(map.static_count(), expected, "the loader and the index disagree");
    assert!(
        expected > 1_000_000,
        "Felucca holds millions of statics, not {expected}"
    );
}

#[test]
fn a_statics_coordinates_land_inside_its_own_block() {
    // `load_statics` recovers a block's world origin by inverting the
    // column-major block formula, and `block_index` applies that formula
    // forward. They are written out twice, in two places, and nothing makes
    // them agree. If they ever disagree, a static asks to be drawn in a
    // different block from the one it is stored in — and `statics_at` filters
    // by exact coordinates, so the symptom is furniture that quietly vanishes.
    let Some(dir) = client_dir() else {
        return;
    };
    let map = Map::load_facet(&dir, 0).unwrap();

    let mut found = 0;
    // Britain: dense enough that a sweep this small finds thousands.
    for y in 1500..1560u16 {
        for x in 1450..1510u16 {
            for item in map.statics_at(x, y) {
                assert_eq!((item.x, item.y), (x, y));
                found += 1;
            }
        }
    }
    // A sweep over empty ocean would satisfy every assertion above without
    // examining a single static.
    assert!(found > 500, "only {found} statics in the middle of Britain");
}

#[test]
fn the_shipped_tiledata_is_the_high_seas_layout() {
    // There is no version field. The layout is decided by which of the two
    // divides the file exactly, and this is that arithmetic against a file
    // rather than against a constant someone typed.
    let Some(dir) = client_dir() else {
        return;
    };
    let bytes = std::fs::read(dir.join("tiledata.mul")).unwrap();
    let data = TileData::parse(&bytes).expect("a shipped tiledata.mul parses");
    assert_eq!(
        data.format(),
        TileDataFormat::HighSeas,
        "a 7.0.x client is High Seas"
    );

    // The detection has to be exact, not merely satisfied. Drop one byte and
    // neither layout divides the file any more, so the answer must become "this
    // is not tiledata.mul" rather than the other layout — a detection that
    // rounded would pick the wrong stride and read every tile's flags from the
    // middle of its neighbour.
    assert!(
        TileData::parse(&bytes[..bytes.len() - 1]).is_none(),
        "a file one byte short still parsed, so the layout was not decided by arithmetic"
    );
}

#[test]
fn the_land_table_reads_as_names_and_not_as_bytes_from_the_next_field() {
    // The whole entry stride, checked by its consequence. One byte out and
    // every name picks up the tail of the field before it — which is exactly
    // how a wrong stride announces itself, and exactly what a synthetic fixture
    // cannot show, because a fixture is laid out by the same arithmetic.
    let Some(data) = tiledata() else {
        return;
    };
    for (id, name) in [
        (0x0003u16, "grass"),
        (0x0006, "grass"),
        (0x0016, "sand"),
        (0x00A8, "water"),
        (0x03A4, "snow"),
    ] {
        assert_eq!(data.land(id).name, name, "land {id:#06X}");
    }

    let named = (0..LAND_TILE_COUNT)
        .filter(|id| !data.land(*id as u16).name.is_empty())
        .count();
    // Roughly a quarter of the table is named on 7.0.116.0; the rest is
    // genuinely blank. A stride that had slipped would leave far fewer names
    // intact, and a count of zero would mean this test read nothing.
    assert!(named > 3000, "only {named} land tiles have a name");
}

#[test]
fn water_is_water_across_the_run_the_client_ships() {
    // Movement asks exactly this question, on every step, of every tile. The
    // four ids are one contiguous run because that is how the file stores the
    // ocean, and a flags field read at the wrong width or offset would not give
    // four in a row.
    let Some(data) = tiledata() else {
        return;
    };
    for id in 0x00A8u16..=0x00AB {
        let tile = data.land(id);
        assert_eq!(tile.name, "water", "land {id:#06X}");
        assert!(tile.flags.is_water(), "land {id:#06X} is not water");
        assert!(tile.flags.is_blocking(), "water blocks a walker");
        assert!(!tile.flags.is_platform(), "water is not something to stand on");
    }

    // And the ground beside it is not water, so the assertion above is not
    // simply true of everything.
    assert!(!data.land(0x0003).flags.is_water(), "grass is not water");
    assert!(!data.land(0x0003).flags.is_blocking(), "grass is walkable");
}

#[test]
fn the_static_entry_layout_lands_on_weight_layer_and_height() {
    // Height at 20 and name at 21, from Sphere's `CUOItemTypeRec_HS`. One byte
    // out and the height byte becomes the first character of the name — which
    // is the tell, and which needs a real entry to show, because a fixture puts
    // the bytes wherever the parser expects them.
    let Some(data) = tiledata() else {
        return;
    };

    let crate_tile = data.static_tile(0x0E3D);
    assert_eq!(crate_tile.name, "crate");
    assert_eq!(crate_tile.height, 3, "a crate is three units tall");
    assert_eq!(
        crate_tile.weight, 255,
        "255 is immovable, and a crate is furniture"
    );
    assert!(crate_tile.flags.is_blocking());

    // Stairs: the run at 1006 is what the climb rule is built on. Their height
    // is the full height; the terrain code halves it.
    let stairs = data.static_tile(1006);
    assert_eq!(stairs.name, "stone stairs");
    assert!(stairs.flags.is_climbable(), "1006 is the first climbable tile");
    assert_eq!(stairs.height, 10);
}

#[test]
fn a_real_tiledata_name_carries_the_plural_marker_the_client_resolves() {
    // `pluralize_name` was written for a bug report — "bolt%s% of cloth"
    // reaching the client verbatim — and tested on that string. This is the
    // marker in a shipped file, which is what says the parser preserves it
    // rather than eating the `%` as a name terminator.
    let Some(data) = tiledata() else {
        return;
    };
    let board = data.static_tile(0x1BD7);
    assert_eq!(board.name, "board%s", "the marker survives the name field");
    assert_eq!(
        openshard_uofiles::tiledata::pluralize_name(&board.name, true),
        "boards"
    );
    assert_eq!(
        openshard_uofiles::tiledata::pluralize_name(&board.name, false),
        "board"
    );
}

#[test]
fn land_tile_zero_is_a_dummy_and_sets_no_movement_bit() {
    // A quirk of the shipped file, pinned so it is not mistaken for a parser
    // bug the next time somebody prints the land table. Record 0 is the only
    // one written in the pre-High-Seas 26-byte shape: its name sits six bytes
    // into the entry, so read at the modern offsets it comes out as flags
    // 0x4E55_0000_0000_0000 and the name "ED" — the tail of "UNUSED".
    //
    // It is left alone rather than special-cased because the bits that land in
    // that flag word are all above bit 32, and every flag movement asks about
    // is below it. What matters is that this junk cannot make tile 0 walkable,
    // water, or a floor — and that is what is asserted.
    let Some(data) = tiledata() else {
        return;
    };
    let dummy = data.land(0);
    assert!(!dummy.flags.is_water());
    assert!(!dummy.flags.is_blocking());
    assert!(!dummy.flags.is_platform());
    assert!(!dummy.flags.is_climbable());
    assert!(
        !dummy.flags.has(openshard_uofiles::tiledata::TileFlags::FLOOR),
        "the dummy record must not read as a floor"
    );
    // The neighbours are ordinary records, which is what makes record 0 a quirk
    // of the file rather than a stride that is wrong everywhere.
    assert_eq!(data.land(2).name, "NODRAW");
    assert_eq!(data.land(3).name, "grass");
}

#[test]
fn the_corner_of_felucca_is_ocean_and_britain_is_not() {
    // Block 0 and a block deep inside the file, so that "the map loaded" means
    // more than "the first block parsed". A container concatenated in offset
    // order passes at (0,0) and fails here.
    let Some(dir) = client_dir() else {
        return;
    };
    let map = Map::load_facet(&dir, 0).unwrap();
    let data = tiledata().unwrap();

    let corner = map.land(0, 0).expect("(0,0) is on the map");
    assert!(
        data.land(corner.tile).flags.is_water(),
        "the north-west corner of Felucca is ocean, not tile {}",
        corner.tile
    );

    let britain = map.land(1495, 1629).expect("Britain is on the map");
    assert!(
        !data.land(britain.tile).flags.is_water(),
        "the middle of Britain came out as water, so the blocks are misplaced"
    );
}

// The art and the palettes. Everything above reads a file the server needs;
// everything below reads one only a renderer does.

#[test]
fn the_shipped_hues_are_three_thousand_ramps() {
    // The count is not in the file — it is the length divided by the group
    // size — so this is that division against a real hues.mul.
    let Some(dir) = client_dir() else {
        return;
    };
    let hues = Hues::load(dir.join("hues.mul")).expect("a client ships a readable hues.mul");
    assert_eq!(hues.count(), 3000);

    // The one-based index, at both ends of the table.
    assert!(hues.get(Hue::NONE).is_none(), "Hue(0) is no tint, not row zero");
    assert!(hues.get(Hue(1)).is_some(), "the first hue is Hue(1)");
    assert!(
        hues.get(Hue(3000)).is_some(),
        "the last hue is Hue(3000), not Hue(2999)"
    );
    assert!(hues.get(Hue(3001)).is_none());
}

#[test]
fn a_real_hue_ramp_runs_dark_to_light_in_one_channel() {
    // A hue is a gradient, and the file stores it darkest first. If the entry
    // stride were wrong the 32 colours would be a slice across several hues —
    // still 32 colours, still plausible on their own, and not a ramp.
    let Some(dir) = client_dir() else {
        return;
    };
    let hues = Hues::load(dir.join("hues.mul")).unwrap();
    let blue = hues.get(Hue(2)).expect("hue 2 is shipped");

    let mut climbed = 0;
    for pair in blue.colors.windows(2) {
        assert!(
            pair[1].blue() >= pair[0].blue(),
            "the ramp goes backwards: {:?} then {:?}",
            pair[0],
            pair[1]
        );
        if pair[1].blue() > pair[0].blue() {
            climbed += 1;
        }
    }
    // A ramp of one flat colour would satisfy every assertion above.
    assert!(climbed > 10, "only {climbed} steps of the ramp actually climb");
    assert!(blue.colors[31].blue() > blue.colors[0].blue());

    // And it is blue: a channel order read backwards would make this the red
    // ramp and nothing else in the file would complain.
    assert_eq!(blue.colors[31].red(), 0);
    assert_eq!(blue.colors[31].green(), 0);
}

#[test]
fn land_art_is_a_diamond_with_empty_corners() {
    // The shape is the format. Row 0 draws two pixels in the middle and the
    // middle row draws all 44 — a reader that laid the same bytes out as a
    // rectangle would fill the corners and lose the widest rows.
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).expect("a client ships artLegacyMUL.uop");
    let grass = art
        .land(Graphic(3))
        .unwrap()
        .expect("land tile 3 is grass and every client has it");

    assert_eq!((grass.width(), grass.height()), (44, 44));

    // The corners are outside the diamond and are never written.
    for (x, y) in [(0u16, 0u16), (43, 0), (0, 43), (43, 43), (20, 0), (23, 0)] {
        assert!(
            grass.pixel(x, y).unwrap().is_transparent(),
            "({x},{y}) is outside the diamond and should not be drawn"
        );
    }
    // The two pixels row 0 does draw, and the full width at the waist.
    assert!(!grass.pixel(21, 0).unwrap().is_transparent());
    assert!(!grass.pixel(22, 0).unwrap().is_transparent());
    assert!(!grass.pixel(0, 21).unwrap().is_transparent());
    assert!(!grass.pixel(43, 21).unwrap().is_transparent());
}

#[test]
fn land_art_reads_to_the_end_of_the_diamond_and_not_into_the_padding() {
    // Every land entry is 2,048 bytes and the picture is 2,024 of them. Starting
    // 24 bytes late — the obvious way to be wrong about which end the padding is
    // on — leaves the last row unwritten, and an unwritten row is transparent
    // rather than an error. The bottom point of the diamond is where that shows.
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).unwrap();
    let grass = art.land(Graphic(3)).unwrap().unwrap();

    assert!(!grass.pixel(21, 43).unwrap().is_transparent(), "the bottom point");
    assert!(!grass.pixel(22, 43).unwrap().is_transparent(), "the bottom point");

    // And the tile is painted rather than mostly empty: the diamond is 1,012
    // pixels and grass fills nearly all of them. A handful genuinely are colour
    // zero, so this is a floor and not an equality.
    let drawn = grass.pixels().iter().filter(|p| !p.is_transparent()).count();
    assert!(
        drawn > 1000,
        "only {drawn} of the diamond's 1012 pixels are painted"
    );
}

#[test]
fn the_channel_order_is_what_a_shipped_grass_tile_says() {
    // Nothing in any file says which five bits are red. Reversed, every value
    // still decodes to a colour and the world comes out in a different palette —
    // which reads as a stylistic choice, not a bug. Grass settles it: whatever
    // else it is, it is greener than it is blue.
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).unwrap();
    let grass = art.land(Graphic(3)).unwrap().unwrap();

    let mut sampled = 0;
    let (mut red, mut green, mut blue) = (0u32, 0u32, 0u32);
    for pixel in grass.pixels().iter().filter(|p| !p.is_transparent()) {
        red += u32::from(pixel.red());
        green += u32::from(pixel.green());
        blue += u32::from(pixel.blue());
        sampled += 1;
    }
    assert!(sampled > 1000, "only {sampled} pixels were sampled");
    assert!(green > red, "grass: green {green} should beat red {red}");
    assert!(green > blue, "grass: green {green} should beat blue {blue}");
}

#[test]
fn every_land_tile_the_client_ships_decodes() {
    // Totality over the real index space. A land slot is either a tile or it is
    // absent — about a quarter are filled — and neither may be an error or a
    // panic, because this is what a renderer walks to draw a screen.
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).unwrap();

    let mut present = 0;
    for id in 0..0x4000u16 {
        if art
            .land(Graphic(id))
            .unwrap_or_else(|e| panic!("land {id:#06X}: {e}"))
            .is_some()
        {
            present += 1;
        }
    }
    // 4,244 on 7.0.116.0. A floor rather than the number, so a client of another
    // age still runs the sweep — but high enough that a container that resolved
    // nothing would fail instead of passing quietly.
    assert!(present > 3000, "only {present} land tiles decoded");
}

#[test]
fn every_static_sprite_below_the_land_boundary_decodes() {
    // The run-length decoder against sixteen thousand real sprites, which is the
    // only way to find out whether its bounds checks are right about anything.
    // Every failure mode it guards — a row index past the data, a run longer
    // than its row, a sprite shorter than its header — is a shape a hand-written
    // fixture only has because someone thought to write it.
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).unwrap();

    let mut present = 0;
    let mut painted = 0;
    for id in 0..0x4000u16 {
        let Some(image) = art
            .static_art(Graphic(id))
            .unwrap_or_else(|e| panic!("static {id:#06X}: {e}"))
        else {
            continue;
        };
        present += 1;
        assert!(image.width() > 0 && image.height() > 0);
        assert_eq!(
            image.pixels().len(),
            image.width() as usize * image.height() as usize,
            "static {id:#06X} is not the buffer its own size implies"
        );
        if image.pixels().iter().any(|p| !p.is_transparent()) {
            painted += 1;
        }
    }
    assert!(present > 15_000, "only {present} static sprites decoded");
    // A decoder that produced correctly sized, entirely empty buffers would pass
    // everything above.
    assert!(
        painted > 15_000,
        "only {painted} of {present} sprites drew anything"
    );
}

#[test]
fn a_static_sprite_is_the_size_its_own_header_claims() {
    // The crate tiledata already vouches for, so the same graphic is checked
    // through two different files: tiledata says it is three units tall and
    // immovable, and the art says how many pixels that is.
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).unwrap();
    let crate_art = art
        .static_art(Graphic(0x0E3D))
        .unwrap()
        .expect("0x0E3D is a crate and every client has it");

    assert_eq!((crate_art.width(), crate_art.height()), (44, 60));
    assert_eq!(crate_art.pixels().len(), 44 * 60);

    let drawn = crate_art.pixels().iter().filter(|p| !p.is_transparent()).count();
    assert!(drawn > 500, "a crate is a solid object, not {drawn} pixels");
    assert!(
        drawn < 44 * 60,
        "a sprite that fills its whole box is not run-length encoded"
    );
}

#[test]
fn every_texture_the_client_ships_is_one_of_the_two_squares() {
    // Totality over the texture index, the same sweep the art gets. The size is
    // not in the entry — it follows from the length — so a length this reader
    // does not recognise has to be an error rather than a texture read at the
    // wrong side, which is a picture and looks like corrupt terrain.
    let Some(dir) = client_dir() else {
        return;
    };
    let texmaps = TexMaps::open(&dir).expect("a client ships texidx.mul and texmaps.mul");

    let (mut small, mut large) = (0, 0);
    for id in 0..TEXTURE_COUNT as u16 {
        let Some(image) = texmaps
            .texture(TextureId(id))
            .unwrap_or_else(|e| panic!("texture {id}: {e}"))
        else {
            continue;
        };
        assert_eq!(image.width(), image.height(), "texture {id} is not square");
        match image.width() {
            64 => small += 1,
            128 => large += 1,
            side => panic!("texture {id} came out {side} on a side"),
        }
        assert_eq!(image.pixels().len(), usize::from(image.width()).pow(2));
    }

    // 3,561 and 555 on 7.0.116.0. Both sizes have to be there: a reader that
    // took every entry for a 64 would decode the small ones perfectly and
    // quietly read a quarter of each large one.
    assert!(small > 3000, "only {small} small textures");
    assert!(large > 400, "only {large} large textures");
    assert_eq!(texmaps.present(), small + large);
}

#[test]
fn a_land_tiles_texture_is_the_same_terrain_as_its_art() {
    // The assertion that pins the `texture` field's offset in `tiledata`, and the
    // only one that can. Read two bytes early or late, every land tile still
    // names *a* texture and the ground is still textured — with somebody else's
    // terrain, which looks like a seasonal variant rather than like a bug.
    //
    // The two pictures of one tile are drawn from the same terrain, so their
    // average colours are close. That is not a threshold anyone can defend on
    // its own, so it is not one: the same measurement is taken against a
    // *shifted* pairing, and the claim is that the real pairing is much closer
    // than a wrong one. A file, and not a number chosen here, decides.
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).unwrap();
    let texmaps = TexMaps::open(&dir).unwrap();
    let tiles = tiledata().expect("client_dir() said there is one");

    /// The mean colour of everything drawn, as 0..=31 per channel.
    fn mean(image: &openshard_uofiles::image::Image) -> (f64, f64, f64) {
        let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
        for pixel in image.pixels().iter().filter(|p| !p.is_transparent()) {
            r += u64::from(pixel.red());
            g += u64::from(pixel.green());
            b += u64::from(pixel.blue());
            n += 1;
        }
        let n = n.max(1) as f64;
        (r as f64 / n, g as f64 / n, b as f64 / n)
    }

    fn distance(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
        (a.0 - b.0).abs() + (a.1 - b.1).abs() + (a.2 - b.2).abs()
    }

    // Every land graphic that has both pictures, in index order.
    let mut pairs = Vec::new();
    for id in 0..LAND_TILE_COUNT as u16 {
        let texture = tiles.land(id).texture;
        let (Some(land), Some(texture)) = (art.land(Graphic(id)).unwrap(), texmaps.texture(texture).unwrap())
        else {
            continue;
        };
        pairs.push((mean(&land), mean(&texture)));
    }
    assert!(
        pairs.len() > 2000,
        "only {} land tiles have both an art tile and a texture",
        pairs.len()
    );

    let matched: f64 = pairs.iter().map(|(art, tex)| distance(*art, *tex)).sum::<f64>() / pairs.len() as f64;
    // The control: the same textures against the wrong tiles. Rotating by a
    // third keeps every picture in the comparison, so the two numbers differ
    // only in *which* texture went with which tile.
    let shift = pairs.len() / 3;
    let mismatched: f64 = pairs
        .iter()
        .enumerate()
        .map(|(i, (art, _))| distance(*art, pairs[(i + shift) % pairs.len()].1))
        .sum::<f64>()
        / pairs.len() as f64;

    // On 7.0.116.0 the real pairing averages 5.5 and the shifted one 18.4,
    // across 3,806 tiles that have both pictures.
    assert!(
        matched * 2.0 < mismatched,
        "a tile's own texture ({matched:.2}) is barely closer to its art than a stranger's \
         ({mismatched:.2}); the texture id is being read from the wrong place",
    );
}

/// A human body's standing animation exists, in every stored direction, and
/// looks like a person.
///
/// The index into `anim.mul` is arithmetic — see `BodyKind` — and arithmetic is
/// exactly what a fixture cannot check: the wrong base constant lands on
/// another creature's frames, which decode perfectly and draw a plausible
/// animal. What a shipped file can settle is that the numbers a *client* uses
/// find a body where a human is supposed to be, at a size a human is.
#[test]
fn a_humans_standing_animation_is_where_the_index_says() {
    let Some(dir) = client_dir() else {
        return;
    };
    let mut anim = Anim::open(&dir).expect("a client ships anim.idx and anim.mul");

    // 400 is the male human body, and 4 is `PeopleAnimationGroup.Stand`.
    for direction in 0..DIRECTIONS {
        let frames = anim
            .frames(400, 4, direction)
            .expect("a well-formed entry")
            .unwrap_or_else(|| panic!("body 400 has no standing animation facing {direction}"));
        assert!(
            !frames.is_empty(),
            "the standing animation facing {direction} decoded to no frames",
        );

        for (index, frame) in frames.iter().enumerate() {
            let (width, height) = (frame.image.width(), frame.image.height());
            // A person on a 44-pixel tile: taller than wide, and neither
            // dimension anywhere near a full-screen picture. A wrong index
            // constant lands on a dragon or on a piece of another frame, and
            // both fail this before anything is drawn.
            assert!(
                (10..80).contains(&width) && (20..100).contains(&height),
                "frame {index} facing {direction} is {width}x{height}, which is not a person",
            );
            // And it is mostly not there: a human silhouette leaves the
            // corners of its own box empty. A frame that came out solid is a
            // run placement that filled rows it should not have.
            let drawn = frame
                .image
                .pixels()
                .iter()
                .filter(|color| !color.is_transparent())
                .count();
            let area = usize::from(width) * usize::from(height);
            assert!(
                drawn * 4 < area * 3 && drawn * 10 > area,
                "frame {index} facing {direction} covers {drawn} of {area} pixels",
            );
        }
    }
}

/// The index is mostly empty, and that is the file rather than a bug.
///
/// Worth an assertion because "no frames" is the answer this reader gives for
/// every kind of failure — an out-of-range group, an absent body, a truncated
/// entry — and a reader that had silently started returning it for *everything*
/// would leave the suite above as the only thing standing between it and an
/// empty world. This says the emptiness is where it belongs: bodies the client
/// draws have frames, and the gaps between them do not.
#[test]
fn the_animation_index_is_sparse_but_the_bodies_that_exist_are_dense() {
    let Some(dir) = client_dir() else {
        return;
    };
    let anim = Anim::open(&dir).expect("anim.idx and anim.mul");

    // Standing, facing 0, for every body the file could hold.
    let present = (0..1000u16)
        .filter(|body| {
            let group = match BodyKind::of(*body) {
                BodyKind::Monster => 1,
                BodyKind::Animal => 2,
                BodyKind::Human => 4,
            };
            anim.has_frames(*body, group, 0)
        })
        .count();
    assert!(
        (50..900).contains(&present),
        "{present} of the first 1,000 bodies stand; the index is being read at the wrong stride",
    );
}

/// `Equipconv.def` is text, not one of this crate's binary formats, so there
/// is no arithmetic to pin the way there is for `tiledata`'s layout — this
/// only checks that a real file parses to something, the same floor
/// `fonts.mul` gets until a client install is on hand to read expected
/// `(body, graphic)` pairs off.
#[test]
fn a_real_equipconv_parses_to_a_nonempty_table() {
    let Some(dir) = client_dir() else {
        return;
    };
    let path = dir.join("Equipconv.def");
    if !path.exists() {
        return;
    }
    let table = EquipConv::load(&path).expect("a client ships a readable Equipconv.def");
    assert!(!table.is_empty(), "Equipconv.def parsed to no entries at all");
}

/// `fonts.mul` against a real file, the counterpart `font.rs`'s module doc
/// says has been missing: every face's glyph headers are read at the offset
/// the previous glyph's own width and height say they end at, with no
/// resync point, so a wrong assumption about the record layout desyncs
/// every glyph after the first divergence rather than failing outright — a
/// synthetic fixture, built by the same layout it is checked against, cannot
/// show that.
#[test]
fn a_real_fonts_mul_parses_to_ten_plausible_faces() {
    let Some(dir) = client_dir() else {
        return;
    };
    let path = dir.join("fonts.mul");
    if !path.exists() {
        return;
    }
    let bytes = std::fs::read(&path).expect("a client ships a readable fonts.mul");
    let fonts = AsciiFonts::parse(&bytes).expect("a shipped fonts.mul parses");
    assert_eq!(fonts.len(), FONT_COUNT);

    // A desynced read does not fail — it produces glyphs, just the wrong
    // ones — so the check is on the shape of what came out: every glyph a
    // real client draws is a handful of pixels on a 44-pixel tile, never a
    // fraction of the whole file's worth of "pixels" read from the middle of
    // the next face.
    let mut widest = 0usize;
    let mut sampled = 0usize;
    for font in 0..FONT_COUNT as u16 {
        // `GLYPH_BASE` upward: the table has no record at all below it, so a
        // code point down there is the one input `glyph` is supposed to
        // refuse rather than sample.
        for offset in 0..CHARS_PER_FONT as u16 {
            let char = (u16::from(GLYPH_BASE) + offset) as u8;
            let glyph = fonts
                .glyph(Font(font), char)
                .unwrap_or_else(|| panic!("font {font} character {char:#04X} missing from the table"));
            assert!(
                usize::from(glyph.width()) < 64 && usize::from(glyph.height()) < 64,
                "font {font} character {char:#04X} came out {}x{}, which is not a glyph",
                glyph.width(),
                glyph.height(),
            );
            widest = widest.max(glyph.width() as usize);
            sampled += 1;
        }
    }
    assert_eq!(sampled, FONT_COUNT * CHARS_PER_FONT);
    // A desync that happened to keep every width under 64 by luck would still
    // have to explain a table with nothing wide in it at all.
    assert!(widest > 5, "the widest glyph read was only {widest} pixels");

    // `'A'` (0x41) and space (0x20) are drawn in every face; a stride that had
    // slipped would make at least one of the ten faces' `'A'` come out
    // zero-sized or absurdly large, the way a control code's glyph looks.
    //
    // This is also the assertion `GLYPH_BASE` exists for. The table's real
    // base is `0x20`, not code point `0`: read without the offset, `glyph(3,
    // 0x41)` silently returns table index 65, which is the record for
    // `'a'` (`0x61 - GLYPH_BASE`) and not `'A'` at all — width 8, height 19
    // on this client, versus 12x21 for the real `'A'`. Both are "some glyph,
    // not zero-sized", which is why width alone cannot catch this: it takes
    // knowing that `'A'` is drawn *wider* than space, not merely drawn.
    for font in 0..FONT_COUNT as u16 {
        let letter = fonts.glyph(Font(font), 0x41).unwrap();
        let space = fonts.glyph(Font(font), 0x20).unwrap();
        assert!(
            letter.width() > 0 && letter.height() > 0,
            "font {font}'s 'A' is {}x{}, not drawn at all",
            letter.width(),
            letter.height()
        );
        assert!(
            letter.width() > space.width(),
            "font {font}'s 'A' ({}px) is not wider than its space ({}px) — GLYPH_BASE is off",
            letter.width(),
            space.width(),
        );
    }

    // Font 3 (`Font::DEFAULT`, what every stock line of speech draws in) on
    // this client's 7.0.116.0 `fonts.mul`, pinned exactly: the regression this
    // whole test exists for is a systematic 32-record (`GLYPH_BASE`) shift,
    // and a shift that size changes both numbers on every printable
    // character, so an exact pin here is a much sharper trip wire than the
    // shape checks above.
    let a = fonts.glyph(Font(3), 0x41).unwrap();
    assert_eq!((a.width(), a.height()), (12, 21), "font 3's 'A'");
    let space = fonts.glyph(Font(3), 0x20).unwrap();
    assert_eq!((space.width(), space.height()), (6, 20), "font 3's space");
}
