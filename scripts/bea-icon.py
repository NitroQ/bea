"""Render the Bea logo (pixel rects in src/BeaAvatar.tsx) to app-icon PNGs.

Source of truth stays in BeaAvatar.tsx: this script parses the PALETTE hexes
and the BASE + logo-variant rects, then draws them with Pillow (no other deps).

Outputs:
  src-tauri/icons/bea-source.png  1024x1024 master (feed to `tauri icon`)
  public/bea-icon.png              256x256 web favicon

Regenerate:  python scripts/bea-icon.py
Then:        npx tauri icon src-tauri/icons/bea-source.png
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
AVATAR = ROOT / "src" / "BeaAvatar.tsx"

GRID_W, GRID_H = 48, 56
BG = "#101820"
RING = "#5bd0ac"


def extract_block(source: str, start_marker: str) -> str:
    """Return text of the first non-empty `[ ... ]` block after start_marker.

    Skips the type annotation's empty brackets (`PixelRect[]`) and works for
    both `= [...]` and `: [...]` assignments.
    """
    start = source.index(start_marker)
    begin = source.index("[", start)
    while source[begin : begin + 2] == "[]":
        begin = source.index("[", begin + 2)
    depth = 0
    for i in range(begin, len(source)):
        if source[i] == "[":
            depth += 1
        elif source[i] == "]":
            depth -= 1
            if depth == 0:
                return source[begin : i + 1]
    raise ValueError(f"unbalanced brackets after {start_marker!r}")


RECT_RE = re.compile(r"\[\s*(\d+),\s*(\d+),\s*(\d+),\s*(\d+),\s*'(\w+)'\s*\]")


def parse_rects(block: str) -> list[tuple[int, int, int, int, str]]:
    return [(int(x), int(y), int(w), int(h), c) for x, y, w, h, c in RECT_RE.findall(block)]


def main() -> None:
    source = AVATAR.read_text(encoding="utf-8")

    palette = dict(re.findall(r"(\w+):\s*'(#[0-9a-fA-F]{6})'", source))
    if "dress" not in palette or "skin" not in palette:
        sys.exit("could not parse PALETTE from BeaAvatar.tsx")

    base = parse_rects(extract_block(source, "const BASE"))
    eyes = parse_rects(extract_block(source, "const SHINY_EYES"))
    blush = parse_rects(extract_block(source, "const SOFT_BLUSH"))
    smile = parse_rects(extract_block(source, "const SOFT_SMILE"))
    logo_block = extract_block(source, "logo: {")
    # logo block holds `label` + `rects: [...]`; the first bracket pair is the
    # label string's... no — label is a string, so first [...] is the rects
    # array only if no other brackets precede it. Spreads (`...SHINY_EYES`)
    # contribute no brackets, so parse_rects on the whole block is exact for
    # the logo-specific literal rects.
    logo_specific = parse_rects(logo_block)

    rects = base + eyes + blush + smile + logo_specific
    print(f"parsed {len(rects)} rects ({len(base)} base + {len(logo_specific)} logo-specific)")

    # Draw Bea at integer scale on a transparent layer (crisp pixels).
    scale = 16
    bea = Image.new("RGBA", (GRID_W * scale, GRID_H * scale), (0, 0, 0, 0))
    draw = ImageDraw.Draw(bea)
    for x, y, w, h, color in rects:
        draw.rectangle(
            [x * scale, y * scale, (x + w) * scale - 1, (y + h) * scale - 1],
            fill=palette[color],
        )

    def compose(size: int, ring_width: int) -> Image.Image:
        canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        bg = ImageDraw.Draw(canvas)
        bg.rounded_rectangle([0, 0, size - 1, size - 1], radius=int(size * 0.24), fill=BG)
        fitted = bea.resize((int(size * 0.72), int(size * 0.72 * GRID_H / GRID_W)), Image.NEAREST)
        canvas.alpha_composite(fitted, (int((size - fitted.width) / 2), int((size - fitted.height) / 2)))
        # Teal ring on top so it stays visible over dark taskbars/titlebars.
        ring = ImageDraw.Draw(canvas)
        inset = ring_width // 2
        ring.rounded_rectangle(
            [inset, inset, size - 1 - inset, size - 1 - inset],
            radius=int(size * 0.24),
            outline=RING,
            width=ring_width,
        )
        return canvas

    master_path = ROOT / "src-tauri" / "icons" / "bea-source.png"
    compose(1024, 26).save(master_path)
    print(f"wrote {master_path}")

    favicon_path = ROOT / "public" / "bea-icon.png"
    favicon_path.parent.mkdir(exist_ok=True)
    compose(256, 7).save(favicon_path)
    print(f"wrote {favicon_path}")


if __name__ == "__main__":
    main()
