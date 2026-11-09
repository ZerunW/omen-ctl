"""Draw the omenctl app icon: a black diamond on a dark rounded tile, lit from
behind by a warm glow (magenta lower left, red, orange upper right).
usage: python3 -I make-icon.py OUTDIR   (needs Pillow)"""
import math, os, sys
from PIL import Image, ImageChops, ImageDraw, ImageFilter

S = 2048                      # work size, downsampled at the end
out = sys.argv[1]
os.makedirs(out, exist_ok=True)


def blob(cx, cy, rx, ry, color, blur, alpha=255):
    layer = Image.new("RGB", (S, S), (0, 0, 0))
    d = ImageDraw.Draw(layer)
    c = tuple(int(v * alpha / 255) for v in color)
    d.ellipse([cx - rx, cy - ry, cx + rx, cy + ry], fill=c)
    return layer.filter(ImageFilter.GaussianBlur(blur))


def screen(a, b):
    return ImageChops.screen(a, b)


# glow: positions relative to the tile, the diamond sits in the middle
glow = Image.new("RGB", (S, S), (0, 0, 0))
for args in [
    (0.40, 0.66, 0.20, 0.17, (214, 22, 92), 150),     # magenta, lower left
    (0.50, 0.50, 0.25, 0.23, (210, 18, 24), 160),     # red core behind the diamond
    (0.60, 0.36, 0.20, 0.16, (255, 70, 10), 150),     # orange, upper right
    (0.70, 0.26, 0.14, 0.11, (190, 120, 20), 170),    # faint amber, far upper right
]:
    cx, cy, rx, ry, col, blur = args
    glow = screen(glow, blob(cx * S, cy * S, rx * S, ry * S, col, blur))

# rounded dark tile
tile = Image.new("RGB", (S, S), (9, 9, 11))
tile = screen(tile, glow)
mask = Image.new("L", (S, S), 0)
pad = int(S * 0.04)
ImageDraw.Draw(mask).rounded_rectangle([pad, pad, S - pad, S - pad], radius=int(S * 0.20), fill=255)

# black diamond (square rotated 45 degrees), half the tile wide
d = ImageDraw.Draw(tile)
c, h = S / 2, S * 0.255
d.polygon([(c, c - h), (c + h, c), (c, c + h), (c - h, c)], fill=(0, 0, 0))

icon = Image.new("RGBA", (S, S), (0, 0, 0, 0))
icon.paste(tile, (0, 0), mask)

for size in (16, 24, 32, 48, 64, 128, 256, 512, 1024):
    icon.resize((size, size), Image.LANCZOS).save(os.path.join(out, f"omenctl-gui-{size}.png"), optimize=True)
# raw RGBA for the window icon (no image decoder needed in the app)
open(os.path.join(out, "omenctl-gui-256.rgba"), "wb").write(icon.resize((256, 256), Image.LANCZOS).tobytes())
print("done")
