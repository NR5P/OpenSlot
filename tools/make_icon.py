"""Render the Openslot icon (SVG source, PNGs, .ico, .icns): python3 tools/make_icon.py assets/icon

Five frequency slots; the open one (centre, orange) is the slot the QSO moves to,
with a chevron above it for the QSY.
"""
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw

BG = "#121820"
SLOT = "#33475B"
OPEN = "#E8742A"

# Geometry as fractions of the icon size
RADIUS = 0.225
BAR_W, GAP = 0.09, 0.06
BAR_TOP, BAR_BOTTOM, BAR_R = 0.32, 0.80, 0.025
CHEV_HALF, CHEV_TOP, CHEV_BOTTOM, CHEV_W = 0.085, 0.15, 0.225, 0.04


def bars():
    total = 5 * BAR_W + 4 * GAP
    x0 = (1 - total) / 2
    return [(x0 + i * (BAR_W + GAP), OPEN if i == 2 else SLOT) for i in range(5)]


def render(size: int) -> Image.Image:
    ss = 4  # supersample, then scale down for clean edges
    s = size * ss
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, s - 1, s - 1], radius=RADIUS * s, fill=BG)
    for x, color in bars():
        d.rounded_rectangle([x * s, BAR_TOP * s, (x + BAR_W) * s, BAR_BOTTOM * s], radius=BAR_R * s, fill=color)
    pts = [((0.5 - CHEV_HALF) * s, CHEV_BOTTOM * s), (0.5 * s, CHEV_TOP * s), ((0.5 + CHEV_HALF) * s, CHEV_BOTTOM * s)]
    w = CHEV_W * s
    d.line(pts, fill=OPEN, width=round(w), joint="curve")
    for px, py in (pts[0], pts[2]):
        d.ellipse([px - w / 2, py - w / 2, px + w / 2, py + w / 2], fill=OPEN)
    return img.resize((size, size), Image.LANCZOS)


def svg() -> str:
    n = 1024
    f = lambda v: f"{v * n:g}"
    rects = "\n".join(
        f'  <rect x="{f(x)}" y="{f(BAR_TOP)}" width="{f(BAR_W)}" height="{f(BAR_BOTTOM - BAR_TOP)}" rx="{f(BAR_R)}" fill="{c}"/>'
        for x, c in bars()
    )
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" width="{n}" height="{n}">
  <title>Openslot</title>
  <rect width="{n}" height="{n}" rx="{f(RADIUS)}" fill="{BG}"/>
{rects}
  <polyline points="{f(0.5 - CHEV_HALF)},{f(CHEV_BOTTOM)} {f(0.5)},{f(CHEV_TOP)} {f(0.5 + CHEV_HALF)},{f(CHEV_BOTTOM)}" fill="none" stroke="{OPEN}" stroke-width="{f(CHEV_W)}" stroke-linecap="round" stroke-linejoin="round"/>
</svg>
"""


def main(out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    (out / "openslot.svg").write_text(svg())
    for size in (16, 32, 48, 64, 128, 256, 512, 1024):
        render(size).save(out / f"openslot-{size}.png")

    # Windows .ico with the usual sizes
    render(256).save(out / "openslot.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])

    # macOS .icns: Apple's grid keeps the shape inside ~80% of the canvas
    iconset = out / "openslot.iconset"
    iconset.mkdir(exist_ok=True)
    for base in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            px = base * scale
            canvas = Image.new("RGBA", (px, px), (0, 0, 0, 0))
            body = round(px * 824 / 1024)
            off = (px - body) // 2
            canvas.paste(render(body), (off, off))
            name = f"icon_{base}x{base}{'@2x' if scale == 2 else ''}.png"
            canvas.save(iconset / name)
    subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(out / "openslot.icns")], check=True)
    for p in iconset.iterdir():
        p.unlink()
    iconset.rmdir()


if __name__ == "__main__":
    main(Path(sys.argv[1]))
