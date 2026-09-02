//! Render a declarative gump scene to a PNG without opening a client window.
//!
//! The scene describes only the composition.  Art decoding, nine-slice layout,
//! text placement and atlas packing stay in `openshard-client-render`, so this
//! preview has the same geometry as the client instead of a second UI renderer.

use std::collections::{
    BTreeMap,
    BTreeSet,
};
use std::fs::File;
use std::io::BufReader;
use std::path::{
    Path,
    PathBuf,
};
use std::process::ExitCode;

use clap::Parser;
use fontdue::{
    Font as TtfFont,
    FontSettings,
};
use openshard_client_render::atlas::FontAtlas;
use openshard_client_render::gump::{
    self,
    ArtFiles,
    GumpArt,
    GumpAtlas,
    GumpPixel,
    Picture,
};
use openshard_client_render::renderer::SPRITE_ATLAS_SIDE;
use openshard_client_render::sprite::SpriteQuad;
use openshard_client_render::text::{
    self,
    GumpLabel,
};
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_uofiles::art::Art;
use openshard_uofiles::font::{
    AsciiFonts,
    FONT_COUNT,
};
use openshard_uofiles::gumpart::Gumps;
use openshard_uofiles::hues::Hues;
use serde::Deserialize;

/// Render a `.ron` gump scene into a PNG design preview.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Declarative RON scene to render.
    #[arg(value_name = "SCENE")]
    scene: PathBuf,

    /// Ultima Online Classic install directory containing gump art and fonts.
    /// Required only by scenes that use `Gump`, `Item`, `FittedItem`, `Tile`, `Resize`, or `Label`.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: Option<PathBuf>,

    /// PNG destination. Defaults to the scene path with a `.png` extension.
    #[arg(short, long, value_name = "FILE")]
    out: Option<PathBuf>,

    /// Physical pixels per gump pixel in the PNG.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..))]
    scale: u32,
}

/// One complete preview surface, measured in unscaled gump pixels.
#[derive(Debug, Deserialize)]
struct Scene {
    width:      u32,
    height:     u32,
    #[serde(default = "default_background")]
    background: Rgb,
    /// A project-owned PNG to draw below every gump layer. Its path is relative
    /// to this scene, so one design remains runnable from any working directory.
    #[serde(default)]
    backdrop:   Option<PathBuf>,
    elements:   Vec<Element>,
}

/// An opaque colour behind the scene.
#[derive(Clone, Copy, Debug, Deserialize)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

fn default_background() -> Rgb {
    // Deliberately unlike normal gump transparency: transparent corners and
    // accidentally-black pixels must remain distinguishable in a preview.
    Rgb { r: 64, g: 0, b: 96 }
}

/// A layer in painter's order. Later entries cover earlier entries.
#[derive(Debug, Deserialize)]
enum Element {
    /// A solid, axis-aligned rectangle. This mirrors the client's `{ rect }`
    /// gump primitive and is useful for neutral frames before a skin exists.
    Rect {
        x:      i32,
        y:      i32,
        width:  u32,
        height: u32,
        colour: Rgb,
    },
    /// A cropped project asset drawn at its native size. Paths are relative to
    /// the scene, which keeps design previews independent of the working dir.
    Asset {
        asset:    PathBuf,
        source_x: u32,
        source_y: u32,
        width:    u32,
        height:   u32,
        x:        i32,
        y:        i32,
    },
    /// A cropped project asset resized with nearest-neighbour sampling. This
    /// is for compact fixed controls such as a checkbox, never for a frame.
    ScaledAsset {
        asset:         PathBuf,
        source_x:      u32,
        source_y:      u32,
        source_width:  u32,
        source_height: u32,
        x:             i32,
        y:             i32,
        width:         u32,
        height:        u32,
    },
    /// A project-owned frame whose corners stay fixed while its edges and
    /// centre tile. This is the skin primitive used by resizable panes, rows,
    /// buttons and scroll tracks.
    NineSlice {
        asset:         PathBuf,
        source_x:      u32,
        source_y:      u32,
        source_width:  u32,
        source_height: u32,
        inset_left:    u32,
        inset_top:     u32,
        inset_right:   u32,
        inset_bottom:  u32,
        /// Repeat the source's centre and edges. Use it only with art
        /// authored as seamless tiles; otherwise the centre is stretched.
        #[serde(default)]
        tile:          bool,
        x:             i32,
        y:             i32,
        width:         u32,
        height:        u32,
    },
    /// One native-size gump-art picture.
    Gump { gump: u16, x: i32, y: i32 },
    /// One static-art icon, as used by a container or `{ tilepic }`.
    Item {
        graphic: u16,
        x:       i32,
        y:       i32,
    },
    /// One static-art icon fitted proportionally and centred in its cell, as
    /// the client's `{ tilepicfit }` gump primitive does.
    FittedItem {
        graphic: u16,
        x:       i32,
        y:       i32,
        width:   u32,
        height:  u32,
        #[serde(default)]
        padding: u32,
    },
    /// A gump-art picture repeated, never scaled, over this rectangle.
    Tile {
        gump:   u16,
        x:      i32,
        y:      i32,
        width:  u32,
        height: u32,
    },
    /// The client's `resizepic`: nine gumps tiled into a scalable frame.
    Resize {
        gump:   u16,
        x:      i32,
        y:      i32,
        width:  u32,
        height: u32,
    },
    /// One line from the install's bitmap fonts.
    ///
    /// `hue` is a `hues.mul` index the way the wire spells it: `0` leaves the
    /// font's own greys alone, and anything else recolours the glyphs exactly
    /// as the client does. A preview that omits it is not a preview of the
    /// window — the classic frames write nearly every label in `0x0386`, and
    /// the raw font on that art is unreadably dark.
    Label {
        x:    i32,
        y:    i32,
        text: String,
        #[serde(default = "default_font")]
        font: u16,
        #[serde(default = "default_hue")]
        hue:  u16,
    },
    /// A UTF-8 label from a project-owned TrueType face. Unlike the classic
    /// client font files, this lets skin previews include Cyrillic without a
    /// UO installation.
    Text {
        font:   PathBuf,
        x:      i32,
        y:      i32,
        size:   f32,
        colour: Rgb,
        text:   String,
    },
}

fn default_font() -> u16 {
    1
}

/// No recolouring: the glyphs keep the font file's own pixels.
fn default_hue() -> u16 {
    Hue::NONE.0
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gump render: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let scene: Scene = ron::from_str(&std::fs::read_to_string(&cli.scene)?)?;
    if scene.width == 0 || scene.height == 0 {
        return Err("a scene canvas must be at least one pixel wide and high".into());
    }
    validate_fonts(&scene.elements)?;
    let out = cli.out.unwrap_or_else(|| cli.scene.with_extension("png"));
    let scene_dir = cli.scene.parent().unwrap_or_else(|| Path::new("."));
    let rgb = render(&scene, cli.client.as_deref(), scene_dir)?;
    let (width, height) = scaled_extent(scene.width, scene.height, cli.scale)?;
    let scaled = scale_nearest(&rgb, scene.width, scene.height, cli.scale)?;
    openshard_client_render::png::write(&out, width, height, &scaled)?;
    println!(
        "rendered {}×{} gump pixels at {}× to {}",
        scene.width,
        scene.height,
        cli.scale,
        out.display()
    );
    Ok(())
}

/// Compose the scene through the same gump-quads the client draws.
fn render(
    scene: &Scene,
    client: Option<&Path>,
    scene_dir: &Path,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let pixel_count = usize::try_from(u64::from(scene.width) * u64::from(scene.height))?;
    let mut result = [scene.background.r, scene.background.g, scene.background.b].repeat(pixel_count);
    if let Some(path) = &scene.backdrop {
        let path = scene_dir.join(path);
        let backdrop = read_png(&path)?;
        composite_image(&mut result, scene.width, scene.height, &backdrop)?;
    }
    composite_rects(&mut result, scene.width, scene.height, &scene.elements);
    composite_project_assets(&mut result, scene.width, scene.height, scene_dir, &scene.elements)?;
    let wanted = wanted_art(&scene.elements);
    if !wanted.is_empty() {
        let client = client.ok_or("this scene needs --client for its UO art")?;
        let gumps = Gumps::open(client)?;
        let art = Art::open(client)?;
        let atlas = GumpAtlas::build(
            ArtFiles {
                gumps: &gumps,
                items: &art,
            },
            wanted.iter().copied(),
        )?;
        let missing: Vec<_> = wanted
            .iter()
            .filter(|art| atlas.sprite(**art).is_none())
            .collect();
        if !missing.is_empty() {
            eprintln!(
                "gump render: {} requested art entries are absent from this client",
                missing.len()
            );
        }
        let pictures = pictures(&scene.elements, &atlas)?;
        composite(
            &mut result,
            scene.width,
            scene.height,
            &gump::collect(&pictures, &atlas),
            atlas.pixels(),
            None,
        );
    }

    if scene
        .elements
        .iter()
        .any(|element| matches!(element, Element::Label { .. }))
    {
        let client = client.ok_or("this scene needs --client for its bitmap labels")?;
        let fonts = AsciiFonts::open(client)?;
        let font_atlas = FontAtlas::build(&fonts)?;
        let labels: Vec<_> = scene
            .elements
            .iter()
            .filter_map(|element| {
                match element {
                    Element::Label {
                        x,
                        y,
                        text,
                        font,
                        hue,
                    } => {
                        Some(GumpLabel {
                            at: GumpPixel::new(*x, *y),
                            text,
                            font: Font(*font),
                            hue: Hue(*hue),
                            clip: None,
                        })
                    }
                    _ => None,
                }
            })
            .collect();
        // `hues.mul` is read only when a label actually asks for a tint, so a
        // scene of untinted captions still renders against an install that has
        // no hue file at all.
        let hues = if labels.iter().any(|label| label.hue != Hue::NONE) {
            Some(Hues::load(client.join("hues.mul"))?)
        } else {
            None
        };
        composite(
            &mut result,
            scene.width,
            scene.height,
            &text::collect_gump(&labels, &font_atlas),
            font_atlas.pixels(),
            hues.as_ref(),
        );
    }
    composite_ttf_labels(&mut result, scene.width, scene.height, scene_dir, &scene.elements)?;
    Ok(result)
}

/// Draw all simple rectangular background layers. Their use in a scene is for
/// fills and frames below art; richer assets keep their ordinary painter order.
fn composite_rects(canvas: &mut [u8], width: u32, height: u32, elements: &[Element]) {
    for element in elements {
        let Element::Rect {
            x,
            y,
            width: rect_width,
            height: rect_height,
            colour,
        } = element
        else {
            continue;
        };
        let (Ok(rect_width), Ok(rect_height)) = (i32::try_from(*rect_width), i32::try_from(*rect_height))
        else {
            continue;
        };
        let left = (*x).max(0);
        let top = (*y).max(0);
        let right = x.saturating_add(rect_width).min(width as i32);
        let bottom = y.saturating_add(rect_height).min(height as i32);
        for target_y in top..bottom {
            for target_x in left..right {
                let target = (target_y as u32 * width + target_x as u32) as usize * 3;
                canvas[target..target + 3].copy_from_slice(&[colour.r, colour.g, colour.b]);
            }
        }
    }
}

/// Draw the project atlas layers before optional classic-client art.
fn composite_project_assets(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    scene_dir: &Path,
    elements: &[Element],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut assets = BTreeMap::new();
    for element in elements {
        let (asset_path, draw) = match element {
            Element::Asset {
                asset,
                source_x,
                source_y,
                width: source_width,
                height: source_height,
                x,
                y,
            } => {
                (
                    asset,
                    ProjectDraw::Asset {
                        source_x:      *source_x,
                        source_y:      *source_y,
                        source_width:  *source_width,
                        source_height: *source_height,
                        x:             *x,
                        y:             *y,
                    },
                )
            }
            Element::NineSlice {
                asset,
                source_x,
                source_y,
                source_width,
                source_height,
                inset_left,
                inset_top,
                inset_right,
                inset_bottom,
                tile,
                x,
                y,
                width: target_width,
                height: target_height,
            } => {
                (
                    asset,
                    ProjectDraw::NineSlice {
                        source_x:      *source_x,
                        source_y:      *source_y,
                        source_width:  *source_width,
                        source_height: *source_height,
                        inset_left:    *inset_left,
                        inset_top:     *inset_top,
                        inset_right:   *inset_right,
                        inset_bottom:  *inset_bottom,
                        tile:          *tile,
                        x:             *x,
                        y:             *y,
                        target_width:  *target_width,
                        target_height: *target_height,
                    },
                )
            }
            Element::ScaledAsset {
                asset,
                source_x,
                source_y,
                source_width,
                source_height,
                x,
                y,
                width: target_width,
                height: target_height,
            } => {
                (
                    asset,
                    ProjectDraw::ScaledAsset {
                        source_x:      *source_x,
                        source_y:      *source_y,
                        source_width:  *source_width,
                        source_height: *source_height,
                        x:             *x,
                        y:             *y,
                        target_width:  *target_width,
                        target_height: *target_height,
                    },
                )
            }
            _ => continue,
        };
        let path = scene_dir.join(asset_path);
        if !assets.contains_key(&path) {
            assets.insert(path.clone(), read_png(&path)?);
        }
        let image = assets.get(&path).expect("an inserted project asset is present");
        match draw {
            ProjectDraw::Asset {
                source_x,
                source_y,
                source_width,
                source_height,
                x,
                y,
            } => {
                composite_crop(
                    canvas,
                    width,
                    height,
                    image,
                    source_x,
                    source_y,
                    source_width,
                    source_height,
                    x,
                    y,
                )?
            }
            ProjectDraw::NineSlice {
                source_x,
                source_y,
                source_width,
                source_height,
                inset_left,
                inset_top,
                inset_right,
                inset_bottom,
                tile,
                x,
                y,
                target_width,
                target_height,
            } => {
                composite_nine_slice(
                    canvas,
                    width,
                    height,
                    image,
                    NineSliceSource {
                        x: source_x,
                        y: source_y,
                        width: source_width,
                        height: source_height,
                        left: inset_left,
                        top: inset_top,
                        right: inset_right,
                        bottom: inset_bottom,
                        tile,
                    },
                    x,
                    y,
                    target_width,
                    target_height,
                )?
            }
            ProjectDraw::ScaledAsset {
                source_x,
                source_y,
                source_width,
                source_height,
                x,
                y,
                target_width,
                target_height,
            } => {
                composite_scaled_crop(
                    canvas,
                    width,
                    height,
                    image,
                    source_x,
                    source_y,
                    source_width,
                    source_height,
                    x,
                    y,
                    target_width,
                    target_height,
                )?
            }
        }
    }
    Ok(())
}

enum ProjectDraw {
    Asset {
        source_x:      u32,
        source_y:      u32,
        source_width:  u32,
        source_height: u32,
        x:             i32,
        y:             i32,
    },
    ScaledAsset {
        source_x:      u32,
        source_y:      u32,
        source_width:  u32,
        source_height: u32,
        x:             i32,
        y:             i32,
        target_width:  u32,
        target_height: u32,
    },
    NineSlice {
        source_x:      u32,
        source_y:      u32,
        source_width:  u32,
        source_height: u32,
        inset_left:    u32,
        inset_top:     u32,
        inset_right:   u32,
        inset_bottom:  u32,
        tile:          bool,
        x:             i32,
        y:             i32,
        target_width:  u32,
        target_height: u32,
    },
}

#[derive(Clone, Copy)]
struct NineSliceSource {
    x:      u32,
    y:      u32,
    width:  u32,
    height: u32,
    left:   u32,
    top:    u32,
    right:  u32,
    bottom: u32,
    tile:   bool,
}

/// Copy one native rectangle from an RGBA atlas, preserving its alpha.
#[allow(clippy::too_many_arguments)]
fn composite_crop(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    image: &RgbaImage,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
    target_x: i32,
    target_y: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_source_rect(image, source_x, source_y, source_width, source_height)?;
    for local_y in 0..source_height {
        for local_x in 0..source_width {
            composite_asset_pixel(
                canvas,
                canvas_width,
                canvas_height,
                image,
                (source_x + local_x, source_y + local_y),
                (
                    target_x + i32::try_from(local_x)?,
                    target_y + i32::try_from(local_y)?,
                ),
            );
        }
    }
    Ok(())
}

/// Copy a source rectangle into a fixed-size control with nearest sampling.
#[allow(clippy::too_many_arguments)]
fn composite_scaled_crop(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    image: &RgbaImage,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
    target_x: i32,
    target_y: i32,
    target_width: u32,
    target_height: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_source_rect(image, source_x, source_y, source_width, source_height)?;
    if target_width == 0 || target_height == 0 {
        return Err("a scaled asset must have non-zero dimensions".into());
    }
    for local_y in 0..target_height {
        let sampled_y = source_y + local_y * source_height / target_height;
        for local_x in 0..target_width {
            let sampled_x = source_x + local_x * source_width / target_width;
            composite_asset_pixel(
                canvas,
                canvas_width,
                canvas_height,
                image,
                (sampled_x, sampled_y),
                (
                    target_x + i32::try_from(local_x)?,
                    target_y + i32::try_from(local_y)?,
                ),
            );
        }
    }
    Ok(())
}

/// Nine-slice an atlas rectangle. Corners remain 1:1; the artist may choose a
/// seamless repeating centre or a single stretched interior.
#[allow(clippy::too_many_arguments)]
fn composite_nine_slice(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    image: &RgbaImage,
    source: NineSliceSource,
    target_x: i32,
    target_y: i32,
    target_width: u32,
    target_height: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_source_rect(image, source.x, source.y, source.width, source.height)?;
    let center_width = source
        .width
        .checked_sub(source.left + source.right)
        .ok_or("nine-slice horizontal insets exceed the source width")?;
    let center_height = source
        .height
        .checked_sub(source.top + source.bottom)
        .ok_or("nine-slice vertical insets exceed the source height")?;
    if center_width == 0 || center_height == 0 {
        return Err("nine-slice source needs a non-empty centre".into());
    }
    if target_width < source.left + source.right || target_height < source.top + source.bottom {
        return Err("nine-slice target is smaller than its fixed corners".into());
    }
    for local_y in 0..target_height {
        let source_y = source.y
            + nine_slice_axis(
                local_y,
                target_height,
                source.height,
                source.top,
                source.bottom,
                source.tile,
            );
        for local_x in 0..target_width {
            let source_x = source.x
                + nine_slice_axis(
                    local_x,
                    target_width,
                    source.width,
                    source.left,
                    source.right,
                    source.tile,
                );
            composite_asset_pixel(
                canvas,
                canvas_width,
                canvas_height,
                image,
                (source_x, source_y),
                (
                    target_x + i32::try_from(local_x)?,
                    target_y + i32::try_from(local_y)?,
                ),
            );
        }
    }
    Ok(())
}

fn nine_slice_axis(
    position: u32,
    target_length: u32,
    source_length: u32,
    before: u32,
    after: u32,
    tile: bool,
) -> u32 {
    if position < before {
        position
    } else if position >= target_length - after {
        source_length - (target_length - position)
    } else {
        let source_center = source_length - before - after;
        let target_center = target_length - before - after;
        if tile {
            before + (position - before) % source_center
        } else {
            before + (position - before) * source_center / target_center
        }
    }
}

fn validate_source_rect(
    image: &RgbaImage,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let right = source_x
        .checked_add(source_width)
        .ok_or("asset source x overflows u32")?;
    let bottom = source_y
        .checked_add(source_height)
        .ok_or("asset source y overflows u32")?;
    if right > image.width || bottom > image.height {
        return Err(format!(
            "asset crop {source_x},{source_y} {source_width}×{source_height} lies outside {}×{} image",
            image.width, image.height
        )
        .into());
    }
    Ok(())
}

fn composite_asset_pixel(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    image: &RgbaImage,
    source: (u32, u32),
    target: (i32, i32),
) {
    let (source_x, source_y) = source;
    let (target_x, target_y) = target;
    if target_x < 0 || target_y < 0 || target_x >= canvas_width as i32 || target_y >= canvas_height as i32 {
        return;
    }
    let source = ((source_y * image.width + source_x) * 4) as usize;
    let alpha = image.rgba[source + 3];
    if alpha == 0 {
        return;
    }
    let target = (target_y as u32 * canvas_width + target_x as u32) as usize * 3;
    blend(
        &mut canvas[target..target + 3],
        &image.rgba[source..source + 3],
        alpha,
    );
}

/// Draw Unicode labels after every project asset so inscriptions stay legible.
fn composite_ttf_labels(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    scene_dir: &Path,
    elements: &[Element],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut fonts = BTreeMap::new();
    for element in elements {
        let Element::Text {
            font,
            x,
            y,
            size,
            colour,
            text,
        } = element
        else {
            continue;
        };
        if !size.is_finite() || *size <= 0.0 {
            return Err("text size must be a positive finite number".into());
        }
        let path = scene_dir.join(font);
        if !fonts.contains_key(&path) {
            let bytes = std::fs::read(&path)?;
            let font = TtfFont::from_bytes(bytes, FontSettings::default())
                .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
            fonts.insert(path.clone(), font);
        }
        let font = fonts.get(&path).expect("an inserted font is present");
        draw_text(
            canvas,
            canvas_width,
            canvas_height,
            font,
            *x,
            *y,
            *size,
            *colour,
            text,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    font: &TtfFont,
    x: i32,
    y: i32,
    size: f32,
    colour: Rgb,
    text: &str,
) {
    let line_metrics = font.horizontal_line_metrics(size);
    let baseline_offset = line_metrics.map_or((size * 0.8).round() as i32, |metrics| {
        metrics.ascent.ceil() as i32
    });
    let line_height = line_metrics.map_or(size.ceil() as i32, |metrics| metrics.new_line_size.ceil() as i32);
    let mut pen_x = x;
    let mut baseline_y = y + baseline_offset;
    for ch in text.chars() {
        if ch == '\n' {
            pen_x = x;
            baseline_y += line_height;
            continue;
        }
        let (metrics, coverage) = font.rasterize(ch, size);
        let glyph_x = pen_x + metrics.xmin;
        let glyph_y = baseline_y - metrics.ymin - metrics.height as i32;
        for glyph_y_offset in 0..metrics.height {
            for glyph_x_offset in 0..metrics.width {
                let alpha = coverage[glyph_y_offset * metrics.width + glyph_x_offset];
                if alpha == 0 {
                    continue;
                }
                let target_x = glyph_x + glyph_x_offset as i32;
                let target_y = glyph_y + glyph_y_offset as i32;
                if target_x < 0
                    || target_y < 0
                    || target_x >= canvas_width as i32
                    || target_y >= canvas_height as i32
                {
                    continue;
                }
                let target = (target_y as u32 * canvas_width + target_x as u32) as usize * 3;
                blend(
                    &mut canvas[target..target + 3],
                    &[colour.r, colour.g, colour.b],
                    alpha,
                );
            }
        }
        pen_x += metrics.advance_width.ceil() as i32;
    }
}

/// An RGBA asset the scene can composite without a UO client installation.
struct RgbaImage {
    width:  u32,
    height: u32,
    rgba:   Vec<u8>,
}

/// Decode an 8-bit RGB/RGBA PNG supplied by the project.
fn read_png(path: &Path) -> Result<RgbaImage, Box<dyn std::error::Error>> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let size = reader
        .output_buffer_size()
        .ok_or("decoded PNG buffer is too large")?;
    let mut bytes = vec![0; size];
    let info = reader.next_frame(&mut bytes)?;
    let source = &bytes[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => source.to_vec(),
        png::ColorType::Rgb => {
            source
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], u8::MAX])
                .collect()
        }
        other => return Err(format!("{} decoded as unsupported {other:?}", path.display()).into()),
    };
    Ok(RgbaImage {
        width: info.width,
        height: info.height,
        rgba,
    })
}

/// Composite a project PNG at the scene origin.
fn composite_image(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    image: &RgbaImage,
) -> Result<(), Box<dyn std::error::Error>> {
    if (image.width, image.height) != (width, height) {
        return Err(format!(
            "backdrop is {}×{}, but the scene is {width}×{height}; scale it before putting it in the scene",
            image.width, image.height
        )
        .into());
    }
    for (target, source) in canvas
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(image.rgba.as_chunks::<4>().0)
    {
        blend(target, source, source[3]);
    }
    Ok(())
}

fn validate_fonts(elements: &[Element]) -> Result<(), Box<dyn std::error::Error>> {
    for element in elements {
        let Element::Label { font, .. } = element else {
            continue;
        };
        if usize::from(*font) >= FONT_COUNT {
            return Err(
                format!("font {font} is outside the client bitmap-font range 0..{FONT_COUNT}").into(),
            );
        }
    }
    Ok(())
}

/// Every source picture the atlas must decode before layout can inspect it.
fn wanted_art(elements: &[Element]) -> BTreeSet<GumpArt> {
    let mut wanted = BTreeSet::new();
    for element in elements {
        match element {
            Element::Gump { gump, .. } | Element::Tile { gump, .. } => {
                wanted.insert(GumpArt::Gump(Graphic(*gump)));
            }
            Element::Item { graphic, .. } | Element::FittedItem { graphic, .. } => {
                wanted.insert(GumpArt::Item(Graphic(*graphic)));
            }
            Element::Resize { gump, .. } => {
                // `gump::resize` remaps these nine ids internally, but it uses
                // every id in this contiguous range exactly once.
                for offset in 0..9 {
                    wanted.insert(GumpArt::Gump(Graphic(gump.wrapping_add(offset))));
                }
            }
            Element::Rect { .. }
            | Element::Asset { .. }
            | Element::ScaledAsset { .. }
            | Element::NineSlice { .. }
            | Element::Label { .. }
            | Element::Text { .. } => {}
        }
    }
    wanted
}

/// Translate declarative layers into the client's own picture list.
fn pictures(elements: &[Element], atlas: &GumpAtlas) -> Result<Vec<Picture>, Box<dyn std::error::Error>> {
    let mut pictures = Vec::new();
    for element in elements {
        match element {
            Element::Gump { gump, x, y } => {
                pictures.push(Picture::plain(
                    GumpArt::Gump(Graphic(*gump)),
                    GumpPixel::new(*x, *y),
                ))
            }
            Element::Item { graphic, x, y } => {
                pictures.push(Picture::plain(
                    GumpArt::Item(Graphic(*graphic)),
                    GumpPixel::new(*x, *y),
                ))
            }
            Element::FittedItem {
                graphic,
                x,
                y,
                width,
                height,
                padding,
            } => {
                let width = i32::try_from(*width)?;
                let height = i32::try_from(*height)?;
                let padding = i32::try_from(*padding)?;
                if let Some(picture) = gump::ItemCell::new(GumpPixel::new(*x, *y), width, height)
                    .padded(padding)
                    .picture(atlas, Graphic(*graphic))
                {
                    pictures.push(picture);
                }
            }
            Element::Tile {
                gump,
                x,
                y,
                width,
                height,
            } => {
                pictures.push(
                    Picture::plain(GumpArt::Gump(Graphic(*gump)), GumpPixel::new(*x, *y))
                        .tiled(i32::try_from(*width)?, i32::try_from(*height)?),
                )
            }
            Element::Resize {
                gump,
                x,
                y,
                width,
                height,
            } => {
                pictures.extend(gump::resize(
                    atlas,
                    Graphic(*gump),
                    GumpPixel::new(*x, *y),
                    i32::try_from(*width)?,
                    i32::try_from(*height)?,
                ))
            }
            Element::Rect { .. }
            | Element::Asset { .. }
            | Element::ScaledAsset { .. }
            | Element::NineSlice { .. }
            | Element::Label { .. }
            | Element::Text { .. } => {}
        }
    }
    Ok(pictures)
}

/// Draw one atlas' quads over an RGB canvas, honouring source transparency.
/// Draw sprite quads onto the canvas, tinting each through `hues` the way
/// `gump.wgsl`'s fragment stage does.
///
/// The port is deliberately literal: the ramp has 32 rungs, the rung is chosen
/// by the source pixel's red channel, and a partial hue leaves any pixel that
/// is not already grey alone. A preview that skipped this drew the classic
/// frames' captions in the font file's own near-black, which is not what any
/// player has ever seen on that art.
fn composite(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    quads: &[SpriteQuad],
    pixels: &[u8],
    hues: Option<&Hues>,
) {
    let side = SPRITE_ATLAS_SIDE as i32;
    for quad in quads {
        // `SpriteQuad::hue` carries an opacity and an atlas page above the wire
        // hue's own bits, so only the low half is a hue at all.
        let wire = Hue(quad.hue as u16);
        let left = quad.rect.x.round() as i32;
        let top = quad.rect.y.round() as i32;
        let quad_width = quad.rect.width.round() as i32;
        let quad_height = quad.rect.height.round() as i32;
        let source_x = (quad.region.u * side as f32).round() as i32;
        let source_y = (quad.region.v * side as f32).round() as i32;
        for y in 0..quad_height {
            for x in 0..quad_width {
                let (target_x, target_y) = (left + x, top + y);
                if target_x < 0 || target_y < 0 || target_x >= width as i32 || target_y >= height as i32 {
                    continue;
                }
                let source = ((source_y + y) * side + source_x + x) as usize * 4;
                let alpha = pixels[source + 3];
                if alpha == 0 {
                    continue;
                }
                let target = (target_y as u32 * width + target_x as u32) as usize * 3;
                let texel = [pixels[source], pixels[source + 1], pixels[source + 2]];
                // The renderer's own CPU port of `gump.wgsl`'s hue branch, so a
                // preview cannot disagree with the client about a colour.
                let shown = hues
                    .and_then(|hues| openshard_client_render::hue::tint(hues, wire, texel))
                    .unwrap_or(texel);
                blend(&mut canvas[target..target + 3], &shown, alpha);
            }
        }
    }
}

/// Source-over blending for antialiased art edges.
fn blend(target: &mut [u8], source: &[u8], alpha: u8) {
    for channel in 0..3 {
        target[channel] = ((u16::from(source[channel]) * u16::from(alpha)
            + u16::from(target[channel]) * u16::from(u8::MAX - alpha))
            / u16::from(u8::MAX)) as u8;
    }
}

fn scaled_extent(width: u32, height: u32, scale: u32) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    Ok((
        width.checked_mul(scale).ok_or("scaled width exceeds u32")?,
        height.checked_mul(scale).ok_or("scaled height exceeds u32")?,
    ))
}

fn scale_nearest(
    rgb: &[u8],
    width: u32,
    height: u32,
    scale: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let (scaled_width, scaled_height) = scaled_extent(width, height, scale)?;
    let pixels = usize::try_from(u64::from(scaled_width) * u64::from(scaled_height))?;
    let mut scaled = Vec::with_capacity(pixels * 3);
    for y in 0..scaled_height {
        for x in 0..scaled_width {
            let source = ((y / scale * width + x / scale) * 3) as usize;
            scaled.extend_from_slice(&rgb[source..source + 3]);
        }
    }
    Ok(scaled)
}

#[cfg(test)]
mod tests {
    use openshard_client_render::gump::{
        GumpArt,
        GumpAtlas,
        GumpPixel,
        Picture,
    };
    use openshard_protocol::wire::Graphic;
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::{
        Element,
        Scene,
        composite,
        nine_slice_axis,
        pictures,
        scale_nearest,
        wanted_art,
    };

    #[test]
    fn example_scene_parses() {
        let scene: Scene =
            ron::from_str(include_str!("../examples/admin-panel.ron")).expect("the checked-in scene parses");
        assert_eq!(scene.width, 360, "the example remains a fixed review surface");
        assert_eq!(
            scene.elements.len(),
            7,
            "the example keeps every supported visual layer represented"
        );
    }

    #[test]
    fn blacksmith_skin_scenes_parse_at_both_sizes() {
        let compact: Scene = ron::from_str(include_str!("../examples/blacksmith-skin-compact.ron"))
            .expect("the compact skin scene parses");
        let wide: Scene = ron::from_str(include_str!("../examples/blacksmith-skin-wide.ron"))
            .expect("the wide skin scene parses");
        assert_eq!((compact.width, compact.height), (1024, 640));
        assert_eq!((wide.width, wide.height), (1440, 860));
        assert!(
            compact
                .elements
                .iter()
                .any(|element| matches!(element, Element::ScaledAsset { .. })),
            "the compact scene proves checkbox state is not painted into a row"
        );
    }

    #[test]
    fn nine_slice_axis_preserves_corners_and_selects_repeat_or_stretch() {
        assert_eq!(nine_slice_axis(0, 12, 8, 2, 2, true), 0, "left corner is fixed");
        assert_eq!(nine_slice_axis(11, 12, 8, 2, 2, true), 7, "right corner is fixed");
        assert_eq!(
            nine_slice_axis(6, 12, 8, 2, 2, true),
            2,
            "tile mode repeats centre pixels"
        );
        assert_eq!(
            nine_slice_axis(6, 12, 8, 2, 2, false),
            4,
            "stretch mode advances through the centre"
        );
    }

    #[test]
    fn nearest_scale_repeats_each_source_pixel() {
        let source = [1, 2, 3, 4, 5, 6];
        let scaled = scale_nearest(&source, 2, 1, 2).expect("small dimensions cannot overflow");
        assert_eq!(
            scaled,
            [
                1, 2, 3, 1, 2, 3, 4, 5, 6, 4, 5, 6, 1, 2, 3, 1, 2, 3, 4, 5, 6, 4, 5, 6,
            ]
        );
    }

    #[test]
    fn resize_layer_packs_and_expands_all_nine_pieces() {
        let layers = [Element::Resize {
            gump:   100,
            x:      0,
            y:      0,
            width:  20,
            height: 20,
        }];
        let wanted = wanted_art(&layers);
        assert_eq!(
            wanted.len(),
            9,
            "a resize frame needs its full contiguous art range"
        );
        let atlas = GumpAtlas::pack(
            wanted
                .into_iter()
                .map(|art| (art, Image::new(1, 1, vec![Color16(0x7FFF)]))),
        )
        .expect("nine one-pixel pieces fit an atlas");
        let pictures = pictures(&layers, &atlas).expect("small dimensions fit i32");
        assert_eq!(
            pictures.len(),
            9,
            "the declarative layer reaches the renderer's nine-slice"
        );
        assert!(
            atlas.sprite(GumpArt::Gump(Graphic(100))).is_some(),
            "the base frame corner is available to the renderer"
        );
    }

    #[test]
    fn compositing_a_picture_preserves_the_atlas_colour() {
        let art = GumpArt::Gump(Graphic(100));
        let atlas = GumpAtlas::pack([(art, Image::new(1, 1, vec![Color16(0x7FFF)]))])
            .expect("one source pixel fits an atlas");
        let quads =
            openshard_client_render::gump::collect(&[Picture::plain(art, GumpPixel::new(1, 0))], &atlas);
        let mut canvas = [1, 2, 3].repeat(2);
        composite(&mut canvas, 2, 1, &quads, atlas.pixels(), None);
        assert_eq!(
            &canvas[..3],
            &[1, 2, 3],
            "the art stays at its requested position"
        );
        assert_eq!(
            &canvas[3..],
            &atlas.pixels()[..3],
            "a preview carries the exact atlas colour into the output"
        );
    }
}
