#!/usr/bin/env python3
"""Run DeepVecFont's font-specific SR checkpoint on binary glyph masks.

Input images are grayscale masks with white foreground and black background.
DeepVecFont was trained on black glyphs over white, so polarity is inverted only
for inference. The output is converted back to a white-foreground soft mask.

The official 128->256 model first enlarges its 128px input to 256px and then
refines it with a Pix2Pix U-Net. ``--zoom 4`` makes tiny UO glyphs occupy a
training-like fraction of the canvas; the network result is then reduced to the
requested logical scale (4x by default).
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("input", type=Path, help="mask PNG or directory of mask PNGs")
    result.add_argument("output", type=Path, help="output PNG or directory")
    result.add_argument(
        "--repo",
        type=Path,
        default=Path("font-upscale-artifacts/runners/deepvecfont"),
        help="official DeepVecFont checkout",
    )
    result.add_argument(
        "--checkpoint",
        type=Path,
        help="generator checkpoint; defaults to REPO/experiments/image_sr/latest_net_G.pth",
    )
    result.add_argument("--zoom", type=int, choices=(2, 4), default=4)
    result.add_argument("--scale", type=int, default=4, help="logical output scale")
    result.add_argument("--threshold", type=int, choices=range(256))
    return result


def input_files(path: Path) -> list[Path]:
    if path.is_file():
        return [path]
    if not path.is_dir():
        raise SystemExit(f"input does not exist: {path}")
    files = sorted(path.glob("*.png"))
    if not files:
        raise SystemExit(f"no PNG files in: {path}")
    return files


def load_network(repo: Path, checkpoint: Path):
    sys.path.insert(0, str(repo.resolve()))
    import torch
    from models.imgsr import networks

    network = networks.define_G(
        1,
        1,
        64,
        "unet_256",
        norm="instance",
        use_dropout=True,
        gpu_ids=[],
    )
    state = torch.load(checkpoint, map_location="cpu", weights_only=True)
    network.load_state_dict(state)
    network.eval()
    return torch, network


def infer_mask(torch, network, source: Image.Image, zoom: int, scale: int) -> Image.Image:
    mask = source.convert("L")
    width, height = mask.size
    enlarged_width, enlarged_height = width * zoom, height * zoom
    if enlarged_width > 128 or enlarged_height > 128:
        raise ValueError(
            f"{source.filename}: {width}x{height} at zoom {zoom} exceeds 128x128"
        )

    # Nearest keeps the low-resolution observation exact. DeepVecFont sees the
    # polarity and canvas geometry used by its font dataset.
    enlarged = mask.resize((enlarged_width, enlarged_height), Image.Resampling.NEAREST)
    canvas = Image.new("L", (128, 128), 255)
    left = (128 - enlarged_width) // 2
    top = (128 - enlarged_height) // 2
    canvas.paste(Image.eval(enlarged, lambda value: 255 - value), (left, top))

    values = np.asarray(canvas, dtype=np.float32) / 255.0
    tensor = torch.from_numpy(values)[None, None]
    tensor = tensor * 2.0 - 1.0
    tensor = torch.nn.functional.interpolate(
        tensor, size=(256, 256), mode="bilinear", align_corners=False
    )
    with torch.inference_mode():
        prediction = network(tensor)

    prediction = prediction[0, 0].clamp(-1.0, 1.0)
    prediction = ((prediction + 1.0) * 0.5).cpu().numpy()
    alpha = 1.0 - prediction

    crop_left = left * 2
    crop_top = top * 2
    crop = alpha[
        crop_top : crop_top + enlarged_height * 2,
        crop_left : crop_left + enlarged_width * 2,
    ]
    crop_image = Image.fromarray(np.round(crop * 255.0).astype(np.uint8), mode="L")
    target_size = (width * scale, height * scale)
    if crop_image.size != target_size:
        crop_image = crop_image.resize(target_size, Image.Resampling.LANCZOS)
    return crop_image


def main() -> None:
    args = parser().parse_args()
    checkpoint = args.checkpoint or args.repo / "experiments/image_sr/latest_net_G.pth"
    if not checkpoint.is_file():
        raise SystemExit(f"checkpoint does not exist: {checkpoint}")

    files = input_files(args.input)
    if len(files) == 1 and args.input.is_file() and args.output.suffix.lower() == ".png":
        outputs = [args.output]
    else:
        args.output.mkdir(parents=True, exist_ok=True)
        outputs = [args.output / item.name for item in files]

    torch, network = load_network(args.repo, checkpoint)
    for source_path, output_path in zip(files, outputs, strict=True):
        source = Image.open(source_path)
        result = infer_mask(torch, network, source, args.zoom, args.scale)
        if args.threshold is not None:
            result = result.point(lambda value: 255 if value >= args.threshold else 0)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        result.save(output_path)
        print(f"{source_path} -> {output_path} ({result.width}x{result.height})")


if __name__ == "__main__":
    main()
