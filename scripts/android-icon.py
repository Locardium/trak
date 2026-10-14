"""Android launcher icon from the app logo.

`pnpm tauri icon` makes every other icon fine, but on Android it puts the
whole logo edge to edge on the adaptive icon's foreground over a white
background. Launchers mask that layer to a circle or a squircle showing only
its middle two thirds (72 of 108dp), so the "t" got clipped and white showed
in the corners.

This takes the opaque middle of the logo (its dark square without the
rounded corners), sizes it so the glyph's farthest corner sits at GLYPH of
the visible radius, and extends the dark background outwards by repeating
its edge pixels, so the gradient carries on with no seam whatever the mask.

Run after `pnpm tauri icon`, from the repo root:
    python scripts/android-icon.py [path/to/logo.png]
Needs Pillow and numpy (`pip install pillow numpy`). The numbers below are
measured on trak-logo.png (2000x2000): a new logo needs them measured again.
"""
import math
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
LOGO = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "app/src-tauri/icons/trak-logo.png"
RES = ROOT / "app/src-tauri/gen/android/app/src/main/res"
# Fully opaque middle of the logo, in its own pixels (the rounded corners
# are transparent further out).
OPAQUE = (200, 200, 1800, 1800)
# The glyph's bounding box in the logo (pixels brighter than the background).
GLYPH_BOX = (595, 351, 1382, 1651)
# Where the glyph's farthest corner lands, as a fraction of the visible radius.
GLYPH = 0.75
BACKGROUND = "#1d1d1d"
# Adaptive icon layers are 108dp; px per density.
SIZES = {"mdpi": 108, "hdpi": 162, "xhdpi": 216, "xxhdpi": 324, "xxxhdpi": 432}

logo = Image.open(LOGO).convert("RGBA")
cx, cy = (OPAQUE[0] + OPAQUE[2]) / 2, (OPAQUE[1] + OPAQUE[3]) / 2
far = max(
    math.hypot(x - cx, y - cy)
    for x in (GLYPH_BOX[0], GLYPH_BOX[2])
    for y in (GLYPH_BOX[1], GLYPH_BOX[3])
)
# Whole 108dp layer, in logo pixels: visible radius = far / GLYPH, and the
# visible part is 72 of the 108dp.
layer = math.ceil(2 * (far / GLYPH) * 108 / 72)
middle = np.array(logo.crop(OPAQUE))
pad_x = max(0, (layer - middle.shape[1]) // 2)
pad_y = max(0, (layer - middle.shape[0]) // 2)
full = Image.fromarray(np.pad(middle, ((pad_y, pad_y), (pad_x, pad_x), (0, 0)), mode="edge"))
full.putalpha(255)

for density, size in SIZES.items():
    out = RES / f"mipmap-{density}" / "ic_launcher_foreground.png"
    full.resize((size, size), Image.LANCZOS).save(out)
    print(f"{out.relative_to(ROOT)}  {size}px")

colors = RES / "values" / "ic_launcher_background.xml"
colors.write_text(
    '<?xml version="1.0" encoding="utf-8"?>\n'
    "<resources>\n"
    f'  <color name="ic_launcher_background">{BACKGROUND}</color>\n'
    "</resources>\n",
    encoding="utf-8",
)
print(f"{colors.relative_to(ROOT)}  {BACKGROUND}")
