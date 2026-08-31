//! Upscale the classic bitmap fonts through an external RGBA-aware image model.
//!
//! This is deliberately an offline asset tool. It exports one padded PNG per
//! glyph, runs Real-ESRGAN's portable NCNN executable over the directory, then
//! thresholds the separately-resized alpha channel and writes another
//! `fonts.mul`. The PNGs stay beside the result so model and threshold choices
//! can be inspected rather than hidden inside the conversion.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{
    BufReader,
    BufWriter,
};
use std::path::{
    Path,
    PathBuf,
};
use std::process::{
    Command,
    ExitCode,
};

use clap::Parser;
use openshard_protocol::speech::Font;
use openshard_uofiles::color::{
    Color16,
    Rgb8,
};
use openshard_uofiles::font::{
    AsciiFonts,
    CHARS_PER_FONT,
    FONT_COUNT,
    GLYPH_BASE,
};
use openshard_uofiles::image::Image;

const PREVIEW_MARGIN: u32 = 4;
const COMPARISON_GAP: u32 = 16;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Upscale fonts.mul with Real-ESRGAN while preserving transparency"
)]
struct Cli {
    /// Ultima Online Classic install directory containing fonts.mul.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client:          PathBuf,
    /// Real-ESRGAN NCNN Vulkan executable.
    #[arg(long, value_name = "FILE")]
    upscaler:        PathBuf,
    /// Directory containing the NCNN .param and .bin model files.
    /// Defaults to `models` beside the executable.
    #[arg(long, value_name = "DIR")]
    models:          Option<PathBuf>,
    /// Output directory. Intermediates are intentionally retained here.
    #[arg(long, default_value = "font-upscale-artifacts", value_name = "DIR")]
    out:             PathBuf,
    /// NCNN model name.
    #[arg(long, default_value = "realesrgan-x4plus-anime")]
    model:           String,
    /// Model scale. The portable runner accepts 2, 3, or 4.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(2..=4))]
    scale:           u8,
    /// Alpha values below this become transparent in the binary-alpha MUL.
    #[arg(long, default_value_t = 128)]
    alpha_threshold: u8,
    /// Extra alpha thresholds to repack and place in a 2-column contact sheet.
    #[arg(long, value_delimiter = ',', default_value = "64,96,128,160")]
    compare_alpha:   Vec<u8>,
    /// Transparent context around each glyph before inference, in source pixels.
    #[arg(long, default_value_t = 8)]
    padding:         u16,
    /// Reuse already-produced glyph PNGs; useful when only changing alpha threshold.
    #[arg(long)]
    skip_inference:  bool,
    /// Text rendered once in each of the ten faces for the before/after previews.
    #[arg(long, default_value = "OpenShard: The quick brown fox 0123456789 !?")]
    sample:          String,
}

fn main() -> ExitCode {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("font upscale: {error}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let source_path = cli.client.join("fonts.mul");
    let source = AsciiFonts::load(&source_path)?;
    let input_dir = cli.out.join("glyphs-before");
    let output_dir = cli.out.join("glyphs-after");
    std::fs::create_dir_all(&input_dir)?;
    std::fs::create_dir_all(&output_dir)?;

    let exported = export_glyphs(&source, &input_dir, cli.padding)?;
    eprintln!(
        "font upscale: exported {exported} non-empty RGBA glyphs to {}",
        input_dir.display()
    );

    if !cli.skip_inference {
        let models = cli.models.clone().unwrap_or_else(|| {
            cli.upscaler
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("models")
        });
        let status = Command::new(&cli.upscaler)
            .args(["-i"])
            .arg(&input_dir)
            .args(["-o"])
            .arg(&output_dir)
            .args(["-n", &cli.model, "-s"])
            .arg(cli.scale.to_string())
            .args(["-m"])
            .arg(models)
            .args(["-f", "png"])
            .status()?;
        if !status.success() {
            return Err(format!("{} exited with {status}", cli.upscaler.display()).into());
        }
    }

    let upscaled = import_glyphs(&source, &output_dir, cli.padding, cli.scale, cli.alpha_threshold)?;
    let mul_path = cli.out.join(format!("fonts-upscaled-{}x.mul", cli.scale));
    std::fs::write(&mul_path, upscaled.encode()?)?;

    let nearest = scale_nearest(&source, cli.scale)?;
    let nearest_mul_path = cli.out.join(format!("fonts-nearest-{}x.mul", cli.scale));
    std::fs::write(&nearest_mul_path, nearest.encode()?)?;

    let before = render_sample(&source, &cli.sample, PREVIEW_MARGIN);
    let after = render_sample(&upscaled, &cli.sample, PREVIEW_MARGIN * u32::from(cli.scale));
    let nearest_preview = render_sample(&nearest, &cli.sample, PREVIEW_MARGIN * u32::from(cli.scale));
    let before_path = cli.out.join("before.png");
    let after_path = cli.out.join("after.png");
    write_png(&before_path, before.width, before.height, &before.rgba)?;
    write_png(&after_path, after.width, after.height, &after.rgba)?;
    let nearest_path = cli.out.join("nearest.png");
    write_png(
        &nearest_path,
        nearest_preview.width,
        nearest_preview.height,
        &nearest_preview.rgba,
    )?;

    let comparison = comparison(&before, &after, u32::from(cli.scale));
    let comparison_path = cli.out.join("comparison.png");
    write_png(
        &comparison_path,
        comparison.width,
        comparison.height,
        &comparison.rgba,
    )?;

    let mut threshold_previews = Vec::new();
    for threshold in &cli.compare_alpha {
        let candidate = import_glyphs(&source, &output_dir, cli.padding, cli.scale, *threshold)?;
        let candidate_mul = cli
            .out
            .join(format!("fonts-upscaled-{}x-alpha{threshold}.mul", cli.scale));
        std::fs::write(&candidate_mul, candidate.encode()?)?;
        let preview = render_sample(&candidate, &cli.sample, PREVIEW_MARGIN * u32::from(cli.scale));
        let preview_path = cli.out.join(format!("after-alpha{threshold}.png"));
        write_png(&preview_path, preview.width, preview.height, &preview.rgba)?;
        eprintln!(
            "font upscale: alpha {threshold:3} keeps {} opaque pixels",
            opaque_pixels(&candidate)
        );
        threshold_previews.push(preview);
    }
    if !threshold_previews.is_empty() {
        let sheet = contact_sheet(&threshold_previews, 2);
        write_png(
            &cli.out.join("alpha-thresholds.png"),
            sheet.width,
            sheet.height,
            &sheet.rgba,
        )?;
    }

    // A parse after the write catches record-boundary mistakes in the actual
    // artifact, not only in the in-memory value that produced it.
    let verified = AsciiFonts::load(&mul_path)?;
    if verified.len() != FONT_COUNT {
        return Err("the written file did not parse back to ten faces".into());
    }
    eprintln!(
        "font upscale: wrote {} ({} bytes), {}, {}, {}, {}, and threshold variants",
        mul_path.display(),
        std::fs::metadata(&mul_path)?.len(),
        before_path.display(),
        after_path.display(),
        nearest_path.display(),
        comparison_path.display(),
    );
    Ok(())
}

fn scale_nearest(source: &AsciiFonts, scale: u8) -> Result<AsciiFonts, Box<dyn std::error::Error>> {
    let mut result = source.clone();
    let scale_u16 = u16::from(scale);
    for font in 0..FONT_COUNT as u16 {
        for index in 0..CHARS_PER_FONT {
            let char = GLYPH_BASE.wrapping_add(index as u8);
            let glyph = source.glyph(Font(font), char).expect("the table is complete");
            let width = glyph
                .width()
                .checked_mul(scale_u16)
                .ok_or("scaled glyph width overflow")?;
            let height = glyph
                .height()
                .checked_mul(scale_u16)
                .ok_or("scaled glyph height overflow")?;
            let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height));
            for y in 0..height {
                for x in 0..width {
                    pixels.push(
                        glyph
                            .pixel(x / scale_u16, y / scale_u16)
                            .expect("nearest source coordinate is inside the glyph"),
                    );
                }
            }
            assert!(result.set_glyph(Font(font), char, Image::new(width, height, pixels)));
        }
    }
    Ok(result)
}

fn opaque_pixels(fonts: &AsciiFonts) -> usize {
    (0..FONT_COUNT as u16)
        .flat_map(|font| {
            (0..CHARS_PER_FONT).map(move |index| (Font(font), GLYPH_BASE.wrapping_add(index as u8)))
        })
        .filter_map(|(font, char)| fonts.glyph(font, char))
        .flat_map(Image::pixels)
        .filter(|pixel| !pixel.is_transparent())
        .count()
}

fn export_glyphs(
    fonts: &AsciiFonts,
    directory: &Path,
    padding: u16,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut count = 0;
    for font in 0..FONT_COUNT as u16 {
        for index in 0..CHARS_PER_FONT {
            let char = GLYPH_BASE.wrapping_add(index as u8);
            let glyph = fonts.glyph(Font(font), char).expect("the table is complete");
            if glyph.width() == 0
                || glyph.height() == 0
                || glyph.pixels().iter().all(|pixel| pixel.is_transparent())
            {
                continue;
            }
            let (width, height, rgba) = padded_rgba(glyph, padding);
            write_png(&directory.join(glyph_name(font, char)), width, height, &rgba)?;
            count += 1;
        }
    }
    Ok(count)
}

fn import_glyphs(
    source: &AsciiFonts,
    directory: &Path,
    padding: u16,
    scale: u8,
    alpha_threshold: u8,
) -> Result<AsciiFonts, Box<dyn std::error::Error>> {
    let mut result = source.clone();
    let scale_u16 = u16::from(scale);
    for font in 0..FONT_COUNT as u16 {
        for index in 0..CHARS_PER_FONT {
            let char = GLYPH_BASE.wrapping_add(index as u8);
            let glyph = source.glyph(Font(font), char).expect("the table is complete");
            let width = glyph
                .width()
                .checked_mul(scale_u16)
                .ok_or("scaled glyph width overflow")?;
            let height = glyph
                .height()
                .checked_mul(scale_u16)
                .ok_or("scaled glyph height overflow")?;

            let image =
                if width == 0 || height == 0 || glyph.pixels().iter().all(|pixel| pixel.is_transparent()) {
                    Image::new(
                        width,
                        height,
                        vec![Color16::TRANSPARENT; usize::from(width) * usize::from(height)],
                    )
                } else {
                    let path = directory.join(glyph_name(font, char));
                    let png = read_png(&path)?;
                    let crop = u32::from(padding) * u32::from(scale);
                    let expected_width = u32::from(width) + crop * 2;
                    let expected_height = u32::from(height) + crop * 2;
                    if (png.width, png.height) != (expected_width, expected_height) {
                        return Err(format!(
                            "{} is {}x{}, expected {}x{}",
                            path.display(),
                            png.width,
                            png.height,
                            expected_width,
                            expected_height,
                        )
                        .into());
                    }
                    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height));
                    for y in 0..u32::from(height) {
                        for x in 0..u32::from(width) {
                            let at = (((y + crop) * png.width + x + crop) * 4) as usize;
                            let rgba = &png.rgba[at..at + 4];
                            pixels.push(if rgba[3] < alpha_threshold {
                                Color16::TRANSPARENT
                            } else {
                                rgb_to_opaque_color16(rgba[0], rgba[1], rgba[2])
                            });
                        }
                    }
                    Image::new(width, height, pixels)
                };
            assert!(result.set_glyph(Font(font), char, image));
        }
    }
    Ok(result)
}

fn glyph_name(font: u16, char: u8) -> String {
    format!("font-{font:02}-char-{char:02X}.png")
}

/// Put useful colour under transparent pixels without changing their alpha.
/// Real-ESRGAN sees RGB, so this prevents an artificial black field from
/// becoming a dark fringe when its RGB result meets the resized alpha mask.
fn padded_rgba(glyph: &Image, padding: u16) -> (u32, u32, Vec<u8>) {
    let width = u32::from(glyph.width()) + u32::from(padding) * 2;
    let height = u32::from(glyph.height()) + u32::from(padding) * 2;
    let mut colours = vec![None; (width * height) as usize];
    let mut alpha = vec![0u8; (width * height) as usize];
    let mut queue = VecDeque::new();

    for y in 0..u32::from(glyph.height()) {
        for x in 0..u32::from(glyph.width()) {
            let pixel = glyph.pixel(x as u16, y as u16).expect("inside glyph");
            if pixel.is_transparent() {
                continue;
            }
            let Rgb8 { red, green, blue } = pixel.rgb8();
            let point = (x + u32::from(padding), y + u32::from(padding));
            let at = (point.1 * width + point.0) as usize;
            colours[at] = Some([red, green, blue]);
            alpha[at] = 255;
            queue.push_back(point);
        }
    }

    while let Some((x, y)) = queue.pop_front() {
        let colour = colours[(y * width + x) as usize].expect("queued pixels have colour");
        for (nx, ny) in neighbours(x, y, width, height) {
            let at = (ny * width + nx) as usize;
            if colours[at].is_none() {
                colours[at] = Some(colour);
                queue.push_back((nx, ny));
            }
        }
    }

    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for (index, colour) in colours.into_iter().enumerate() {
        rgba.extend_from_slice(&colour.unwrap_or([0, 0, 0]));
        rgba.push(alpha[index]);
    }
    (width, height, rgba)
}

fn neighbours(x: u32, y: u32, width: u32, height: u32) -> impl Iterator<Item = (u32, u32)> {
    [
        x.checked_sub(1).map(|nx| (nx, y)),
        (x + 1 < width).then_some((x + 1, y)),
        y.checked_sub(1).map(|ny| (x, ny)),
        (y + 1 < height).then_some((x, y + 1)),
    ]
    .into_iter()
    .flatten()
}

fn rgb_to_opaque_color16(red: u8, green: u8, blue: u8) -> Color16 {
    let red = u16::from(red >> 3);
    let green = u16::from(green >> 3);
    let blue = u16::from(blue >> 3);
    // The high bit carries no colour but makes predicted black distinct from
    // zero, which is the format's transparent sentinel.
    Color16(0x8000 | red << 10 | green << 5 | blue)
}

struct RgbaImage {
    width:  u32,
    height: u32,
    rgba:   Vec<u8>,
}

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
                .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
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

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}

fn render_sample(fonts: &AsciiFonts, text: &str, margin: u32) -> RgbaImage {
    let lines: Vec<Vec<&Image>> = (0..FONT_COUNT as u16)
        .map(|font| {
            text.bytes()
                .filter_map(|char| fonts.glyph(Font(font), char))
                .collect()
        })
        .collect();
    let content_width = lines
        .iter()
        .map(|glyphs| glyphs.iter().map(|glyph| u32::from(glyph.width())).sum())
        .max()
        .unwrap_or(0);
    let line_heights: Vec<u32> = lines
        .iter()
        .map(|glyphs| {
            glyphs
                .iter()
                .map(|glyph| u32::from(glyph.height()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let width = content_width + margin * 2;
    let height = line_heights.iter().sum::<u32>() + margin * (FONT_COUNT as u32 + 1);
    let mut rgba = vec![0; (width * height * 4) as usize];
    let mut y = margin;
    for (glyphs, line_height) in lines.iter().zip(line_heights) {
        let mut x = margin;
        for glyph in glyphs {
            blit_glyph(&mut rgba, width, x, y, glyph);
            x += u32::from(glyph.width());
        }
        y += line_height + margin;
    }
    RgbaImage { width, height, rgba }
}

fn blit_glyph(canvas: &mut [u8], canvas_width: u32, at_x: u32, at_y: u32, glyph: &Image) {
    for y in 0..u32::from(glyph.height()) {
        for x in 0..u32::from(glyph.width()) {
            let pixel = glyph.pixel(x as u16, y as u16).expect("inside glyph");
            if pixel.is_transparent() {
                continue;
            }
            let Rgb8 { red, green, blue } = pixel.rgb8();
            let at = (((at_y + y) * canvas_width + at_x + x) * 4) as usize;
            canvas[at..at + 4].copy_from_slice(&[red, green, blue, 255]);
        }
    }
}

fn comparison(before: &RgbaImage, after: &RgbaImage, scale: u32) -> RgbaImage {
    let before_width = before.width * scale;
    let before_height = before.height * scale;
    let width = before_width + COMPARISON_GAP + after.width;
    let height = before_height.max(after.height);
    let mut rgba = vec![0; (width * height * 4) as usize];
    checkerboard(&mut rgba, width, height);
    composite_nearest(&mut rgba, width, 0, 0, before, scale);
    composite_nearest(&mut rgba, width, before_width + COMPARISON_GAP, 0, after, 1);
    RgbaImage { width, height, rgba }
}

fn contact_sheet(images: &[RgbaImage], columns: u32) -> RgbaImage {
    let columns = columns.max(1);
    let cell_width = images.iter().map(|image| image.width).max().unwrap_or(1);
    let cell_height = images.iter().map(|image| image.height).max().unwrap_or(1);
    let rows = (images.len() as u32).div_ceil(columns);
    let width = cell_width * columns + COMPARISON_GAP * columns.saturating_sub(1);
    let height = cell_height * rows + COMPARISON_GAP * rows.saturating_sub(1);
    let mut rgba = vec![0; (width * height * 4) as usize];
    checkerboard(&mut rgba, width, height);
    for (index, image) in images.iter().enumerate() {
        let column = index as u32 % columns;
        let row = index as u32 / columns;
        composite_nearest(
            &mut rgba,
            width,
            column * (cell_width + COMPARISON_GAP),
            row * (cell_height + COMPARISON_GAP),
            image,
            1,
        );
    }
    RgbaImage { width, height, rgba }
}

fn checkerboard(rgba: &mut [u8], width: u32, height: u32) {
    for y in 0..height {
        for x in 0..width {
            let value = if (x / 16 + y / 16) % 2 == 0 { 38 } else { 58 };
            let at = ((y * width + x) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&[value, value, value, 255]);
        }
    }
}

fn composite_nearest(
    canvas: &mut [u8],
    canvas_width: u32,
    at_x: u32,
    at_y: u32,
    image: &RgbaImage,
    scale: u32,
) {
    for y in 0..image.height * scale {
        for x in 0..image.width * scale {
            let source = (((y / scale) * image.width + x / scale) * 4) as usize;
            if image.rgba[source + 3] == 0 {
                continue;
            }
            let target = (((at_y + y) * canvas_width + at_x + x) * 4) as usize;
            canvas[target..target + 4].copy_from_slice(&image.rgba[source..source + 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_padding_has_colour_bleed_but_keeps_zero_alpha() {
        let glyph = Image::new(2, 1, vec![Color16(0x7C00), Color16::TRANSPARENT]);
        let (width, height, rgba) = padded_rgba(&glyph, 1);
        assert_eq!((width, height), (4, 3));
        assert!(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[..3] == [255, 0, 0])
        );
        assert_eq!(rgba[((width + 1) * 4 + 3) as usize], 255);
        assert_eq!(rgba[((width + 2) * 4 + 3) as usize], 0);
    }

    #[test]
    fn opaque_black_does_not_turn_into_the_transparency_sentinel() {
        let black = rgb_to_opaque_color16(0, 0, 0);
        assert_eq!(black, Color16(0x8000));
        assert!(!black.is_transparent());
    }

    #[test]
    fn nearest_scaling_repeats_pixels_and_dimensions() {
        let bytes = {
            let mut bytes = Vec::new();
            for font in 0..FONT_COUNT {
                bytes.push(0);
                for index in 0..CHARS_PER_FONT {
                    if font == 0 && index == usize::from(b'A' - GLYPH_BASE) {
                        bytes.extend_from_slice(&[2, 1, 0]);
                        bytes.extend_from_slice(&Color16(1).0.to_le_bytes());
                        bytes.extend_from_slice(&Color16(2).0.to_le_bytes());
                    } else {
                        bytes.extend_from_slice(&[0, 0, 0]);
                    }
                }
            }
            bytes
        };
        let fonts = AsciiFonts::parse(&bytes).unwrap();
        let scaled = scale_nearest(&fonts, 3).unwrap();
        let glyph = scaled.glyph(Font(0), b'A').unwrap();
        assert_eq!((glyph.width(), glyph.height()), (6, 3));
        assert_eq!(
            glyph.pixels(),
            &[
                Color16(1),
                Color16(1),
                Color16(1),
                Color16(2),
                Color16(2),
                Color16(2),
                Color16(1),
                Color16(1),
                Color16(1),
                Color16(2),
                Color16(2),
                Color16(2),
                Color16(1),
                Color16(1),
                Color16(1),
                Color16(2),
                Color16(2),
                Color16(2),
            ]
        );
    }
}
