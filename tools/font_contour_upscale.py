#!/usr/bin/env python3
"""Use a local neural upscaler only for font contours, never for glyph RGB.

The tool wraps an external local model in two explicit steps:

    ./tools/font_contour_upscale.py prepare glyphs-before mask-inputs

    # Run the selected local NCNN model over mask-inputs, then:
    ./tools/font_contour_upscale.py combine \
        --source-dir glyphs-before \
        --mask-dir mask-outputs \
        --out-dir glyphs-after-contour \
        --scale 4 --rgb-filter nearest

``prepare`` writes each padded source alpha as an opaque black/white image.
The neural model therefore sees only the silhouette.  ``combine`` uses the
model's luminance as the new alpha but scales the original colour separately,
without any neural texture/detail synthesis.  Output filenames and padded
dimensions match ``openshard-font-upscale``'s normal ``glyphs-after`` directory,
so the result can be repacked with ``--skip-inference``.

Requires Pillow.  Generated client-derived images belong under the ignored
``font-upscale-artifacts/`` tree, not in Git.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image


def pngs(directory: Path) -> list[Path]:
    return sorted(path for path in directory.glob("*.png") if path.is_file())


def prepare(input_dir: Path, out_dir: Path) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    files = pngs(input_dir)
    if not files:
        raise SystemExit(f"no PNG files in {input_dir}")
    for path in files:
        alpha = Image.open(path).convert("RGBA").getchannel("A")
        alpha.convert("RGB").save(out_dir / path.name)
    print(f"prepared {len(files)} opaque silhouette inputs")


def combine(args: argparse.Namespace) -> None:
    args.out_dir.mkdir(parents=True, exist_ok=True)
    files = pngs(args.source_dir)
    if not files:
        raise SystemExit(f"no PNG files in {args.source_dir}")

    resampling = {
        "nearest": Image.Resampling.NEAREST,
        "lanczos": Image.Resampling.LANCZOS,
    }[args.rgb_filter]
    written = partial = 0
    for source_path in files:
        mask_path = args.mask_dir / source_path.name
        if not mask_path.exists() and args.only_existing:
            continue
        if not mask_path.exists():
            raise FileNotFoundError(mask_path)

        source = Image.open(source_path).convert("RGBA")
        size = (source.width * args.scale, source.height * args.scale)
        mask = Image.open(mask_path).convert("L")
        if mask.size != size:
            raise ValueError(f"{mask_path} is {mask.size}, expected {size}")
        if args.threshold is not None:
            threshold = args.threshold
            mask = mask.point(lambda value: 255 if value >= threshold else 0)
        else:
            histogram = mask.histogram()
            partial += sum(histogram[1:255])

        # Transparent source pixels already contain nearest-colour bleed from
        # openshard-font-upscale's exporter.  Ignoring source alpha here is
        # intentional: the neural silhouette supplies the replacement alpha.
        colour = source.convert("RGB").resize(size, resampling)
        output = colour.convert("RGBA")
        output.putalpha(mask)
        output.save(args.out_dir / source_path.name)
        written += 1

    if not written:
        raise SystemExit("no complete source/mask pairs found")
    print(
        f"combined {written} contour-upscaled glyphs with {args.rgb_filter} RGB; "
        f"partial alpha pixels: {partial}"
    )


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)

    prep = commands.add_parser("prepare", help="extract padded alpha as RGB silhouettes")
    prep.add_argument("input_dir", type=Path)
    prep.add_argument("out_dir", type=Path)

    comb = commands.add_parser("combine", help="attach neural silhouettes to original RGB")
    comb.add_argument("--source-dir", type=Path, required=True)
    comb.add_argument("--mask-dir", type=Path, required=True)
    comb.add_argument("--out-dir", type=Path, required=True)
    comb.add_argument("--scale", type=int, default=4, choices=(2, 3, 4))
    comb.add_argument("--rgb-filter", choices=("nearest", "lanczos"), default="nearest")
    comb.add_argument("--threshold", type=int, choices=range(256))
    comb.add_argument(
        "--only-existing",
        action="store_true",
        help="skip source glyphs whose neural mask output is absent",
    )
    return result


def main() -> None:
    args = parser().parse_args()
    if args.command == "prepare":
        prepare(args.input_dir, args.out_dir)
    else:
        combine(args)


if __name__ == "__main__":
    main()
