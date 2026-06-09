#!/usr/bin/env python3
"""MADO App Icon generator.

Design (MADZINE house style — ring is mandatory):
- White squircle background (macOS Big Sur+ corner radius approx 22.37% of size).
- Coral pink ring (#FF6C47, HSB H=12), outer margin ~9% of size, stroke ~7%.
- Single CJK glyph "窓" in coral pink, Noto Sans JP Light, centered in the ring,
  occupying ~62% of the ring's inner diameter.
- No borders, no shadows, no gradients.

Outputs:
- icon_1024.png
- icon_256.png
- ../../packaging/AppIcon.icns (multi-resolution)
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
import subprocess
import sys

HERE = Path(__file__).resolve().parent
FONT_PATH = HERE.parent / "fonts" / "NotoSansJP-Light.ttf"
OUT_1024 = HERE / "icon_1024.png"
OUT_256 = HERE / "icon_256.png"
ICNS_OUT = HERE.parent.parent / "packaging" / "AppIcon.icns"

CORAL = (0xFF, 0x6C, 0x47, 255)
WHITE = (0xFF, 0xFF, 0xFF, 255)
GLYPH = "窓"
# macOS Big Sur squircle corner radius ratio (approx).
CORNER_RATIO = 0.2237
# Ring geometry — absolute pixels at 1024 canvas, matched to VisionMod thin-line aesthetic.
# VisionMod: OUTER_R = 431.5, STROKE = 6 at 1024 canvas.
REF_CANVAS = 1024
RING_OUTER_R_REF = 431.5  # outer radius at 1024 canvas
RING_STROKE_REF = 6       # ring stroke in pixels at 1024 canvas
# Glyph height as fraction of the ring's inner diameter.
GLYPH_HEIGHT_RATIO = 0.62


def make_squircle_mask(size: int) -> Image.Image:
    """Return an L-mode mask with a rounded-rect (squircle approximation)."""
    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)
    radius = int(size * CORNER_RATIO)
    draw.rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)
    return mask


def fit_font_size(font_path: Path, target_h: int) -> ImageFont.FreeTypeFont:
    """Binary search font size so the cap-height of GLYPH matches target_h."""
    lo, hi = 10, target_h * 3
    best = lo
    while lo <= hi:
        mid = (lo + hi) // 2
        f = ImageFont.truetype(str(font_path), mid)
        bbox = f.getbbox(GLYPH)
        h = bbox[3] - bbox[1]
        if h <= target_h:
            best = mid
            lo = mid + 1
        else:
            hi = mid - 1
    return ImageFont.truetype(str(font_path), best)


def render_icon(size: int) -> Image.Image:
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    # White squircle background.
    bg = Image.new("RGBA", (size, size), WHITE)
    mask = make_squircle_mask(size)
    canvas.paste(bg, (0, 0), mask)

    draw = ImageDraw.Draw(canvas)

    # Coral ring — scale absolute pixel values from reference 1024 canvas.
    scale = size / REF_CANVAS
    outer_r = RING_OUTER_R_REF * scale
    stroke = max(1, int(round(RING_STROKE_REF * scale)))
    cx = size / 2.0
    cy = size / 2.0
    ring_box = (
        cx - outer_r,
        cy - outer_r,
        cx + outer_r,
        cy + outer_r,
    )
    draw.ellipse(ring_box, outline=CORAL, width=stroke)

    # Inner diameter available for the glyph.
    inner_diameter = int(round(2 * (outer_r - stroke)))
    target_h = int(inner_diameter * GLYPH_HEIGHT_RATIO)
    font = fit_font_size(FONT_PATH, target_h)
    bbox = font.getbbox(GLYPH)
    gw = bbox[2] - bbox[0]
    gh = bbox[3] - bbox[1]
    # Place so the glyph's optical bbox is centered on the canvas.
    x = (size - gw) // 2 - bbox[0]
    y = (size - gh) // 2 - bbox[1]
    draw.text((x, y), GLYPH, font=font, fill=CORAL)

    # Clip anything that escaped to the squircle outline (defensive).
    clipped = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    clipped.paste(canvas, (0, 0), mask)
    return clipped


def main() -> int:
    if not FONT_PATH.exists():
        print(f"font missing: {FONT_PATH}", file=sys.stderr)
        return 1
    OUT_1024.parent.mkdir(parents=True, exist_ok=True)
    ICNS_OUT.parent.mkdir(parents=True, exist_ok=True)

    img1024 = render_icon(1024)
    img1024.save(OUT_1024, "PNG")
    img256 = img1024.resize((256, 256), Image.LANCZOS)
    img256.save(OUT_256, "PNG")

    # Build .icns via iconutil for crisp multi-res.
    iconset = ICNS_OUT.with_suffix(".iconset")
    if iconset.exists():
        for p in iconset.iterdir():
            p.unlink()
    else:
        iconset.mkdir(parents=True)
    specs = [
        (16, "icon_16x16.png"),
        (32, "icon_16x16@2x.png"),
        (32, "icon_32x32.png"),
        (64, "icon_32x32@2x.png"),
        (128, "icon_128x128.png"),
        (256, "icon_128x128@2x.png"),
        (256, "icon_256x256.png"),
        (512, "icon_256x256@2x.png"),
        (512, "icon_512x512.png"),
        (1024, "icon_512x512@2x.png"),
    ]
    for px, name in specs:
        img1024.resize((px, px), Image.LANCZOS).save(iconset / name, "PNG")
    subprocess.run(
        ["iconutil", "-c", "icns", str(iconset), "-o", str(ICNS_OUT)],
        check=True,
    )
    # Cleanup iconset directory.
    for p in iconset.iterdir():
        p.unlink()
    iconset.rmdir()
    print(f"wrote {OUT_1024}")
    print(f"wrote {OUT_256}")
    print(f"wrote {ICNS_OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
