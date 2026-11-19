#!/usr/bin/env python3
"""Check the shapes the probe app draws with PROBE_SHAPES=1.

Run by waytape as an analyzer of the `shapes` scenario, with the run
directory in WAYTAPE_RUN_DIR. It can also be given a run directory by hand:

    tests/e2e/analyzers/shapes.py .waytape/runs/shapes

Reads the `shapes` screenshot. The positions and colours mirror
`ShapesPainter` in `app/lib/main.dart`, in pixels from the top left corner
of the window, which is found by the border sway draws around it.

Prints one line per check, and exits with 1 if any failed.
"""

import argparse
import os
import sys
from pathlib import Path

import numpy as np
from PIL import Image

BORDER = (0, 255, 0)
BORDER_WIDTH = 8

WHITE = (255, 255, 255)
BLUE = (0, 0, 255)
RED = (255, 0, 0)
GREEN = (0, 128, 0)
PURPLE = (128, 0, 128)

# How far a channel may be from the colour it is expected to have.
TOLERANCE = 24

# (what is checked, x, y, colour)
POINTS = [
    ("non-zero star: middle filled", 80, 80, BLUE),
    ("non-zero star: point filled", 80, 30, BLUE),
    ("non-zero star: corner of its bounds empty", 26, 26, WHITE),
    ("even-odd star: middle empty", 220, 80, WHITE),
    ("even-odd star: point filled", 220, 30, RED),
    ("even-odd star: corner of its bounds empty", 166, 26, WHITE),
    ("rounded rectangle clip: middle drawn", 360, 80, GREEN),
    ("rounded rectangle clip: edge drawn", 302, 80, GREEN),
    ("rounded rectangle clip: corner clipped", 304, 24, WHITE),
    ("circle clip: middle drawn", 500, 80, PURPLE),
    ("circle clip: corner clipped", 446, 26, WHITE),
]

# (what is checked, x, y, width, height): regions with the edges of a black
# square rotated by 30 degrees on white. Antialiasing blends the edges into
# shades of grey; without it, every pixel is black or white.
EDGES = [
    ("antialiased edges", 20, 180, 240, 240),
    ("antialiased edges in a layer", 300, 180, 240, 240),
]
# A 160 pixel square has 640 pixels of edge. Each of them is grey with
# antialiasing, unless the edge happens to run exactly along pixels.
MIN_GREY = 300


def find_window(pixels):
    """The top left corner of the window, inside sway's border."""
    border = (pixels == BORDER).all(axis=2)
    rows = np.flatnonzero(border.sum(axis=1) >= BORDER_WIDTH)
    cols = np.flatnonzero(border.sum(axis=0) >= BORDER_WIDTH)
    if rows.size == 0 or cols.size == 0:
        return None
    return int(cols[0]) + BORDER_WIDTH, int(rows[0]) + BORDER_WIDTH


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("run_dir", type=Path, nargs="?", default=os.environ.get("WAYTAPE_RUN_DIR"))
    args = parser.parse_args()
    if args.run_dir is None:
        parser.error("give a run directory, or run as a waytape analyzer")

    shot = Path(args.run_dir) / "shots" / "shapes.png"
    if not shot.exists():
        print(f"shapes: {shot} is missing", file=sys.stderr)
        return 1
    pixels = np.asarray(Image.open(shot).convert("RGB")).astype(int)
    origin = find_window(pixels)
    if origin is None:
        print("shapes: the window's border is not in the screenshot", file=sys.stderr)
        return 1
    left, top = origin

    failed = 0

    def report(ok, what, detail):
        nonlocal failed
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'} {what}: {detail}")

    for what, x, y, colour in POINTS:
        actual = tuple(int(c) for c in pixels[top + y, left + x])
        ok = all(abs(a - e) <= TOLERANCE for a, e in zip(actual, colour))
        report(ok, what, f"{actual} at {x},{y}, expected {colour}")

    for what, x, y, width, height in EDGES:
        region = pixels[top + y : top + y + height, left + x : left + x + width]
        lightness = region.mean(axis=2)
        grey = int(((lightness > 24) & (lightness < 232)).sum())
        report(grey >= MIN_GREY, what, f"{grey} grey pixels, expected at least {MIN_GREY}")

    # The count last: waytape copies the end of this output into summary.txt.
    print(f"failed checks: {failed}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
