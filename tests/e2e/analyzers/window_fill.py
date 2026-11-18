#!/usr/bin/env python3
"""Check the frames of a waytape run for a window the app has not filled.

Run by waytape as an analyzer (see the `analyze` entry of the scenarios),
after the run, with the run directory in WAYTAPE_RUN_DIR. It can also be
given a run directory by hand:

    tests/e2e/analyzers/window_fill.py .waytape/runs/latest

The scenarios give the output a magenta background and the window a green
border, which sway draws at the size it has given the window. Inside the
border, magenta is window the app left uncovered; outside it, anything
that is neither magenta nor border is content that spilled out (the
pointer cursor is allowed for).

Prints a summary and the flagged frames, writes `analysis.tsv` into the
run directory, and exits with 1 if more frames are flagged than
--max-flagged allows, which fails the waytape run.
"""

import argparse
import csv
import os
import sys
from pathlib import Path

import numpy as np
from PIL import Image

# What the scenarios set in `session`.
BACKGROUND = (255, 0, 255)
BORDER = (0, 255, 0)
BORDER_WIDTH = 8
# The recording includes the pointer cursor, which is neither background nor
# border. Ignore that many stray pixels outside the window.
CURSOR_ALLOWANCE = 32 * 32


def analyze_frame(pixels):
    """Returns (window rect or None, uncovered pixel count, overflow count)."""
    background = (pixels == BACKGROUND).all(axis=2)
    border = (pixels == BORDER).all(axis=2)

    # The border is the only thing that fills whole runs of rows and columns
    # with pure green, so its extent is the outer edge of the window.
    rows = np.flatnonzero(border.sum(axis=1) >= BORDER_WIDTH)
    cols = np.flatnonzero(border.sum(axis=0) >= BORDER_WIDTH)
    if rows.size == 0 or cols.size == 0:
        return None, 0, 0

    top, bottom = rows[0] + BORDER_WIDTH, rows[-1] - BORDER_WIDTH + 1
    left, right = cols[0] + BORDER_WIDTH, cols[-1] - BORDER_WIDTH + 1
    if bottom <= top or right <= left:
        return None, 0, 0

    uncovered = int(background[top:bottom, left:right].sum())

    outside = ~(background | border)
    outside[rows[0] : rows[-1] + 1, cols[0] : cols[-1] + 1] = False
    overflow = int(outside.sum())

    return (int(left), int(top), int(right - left), int(bottom - top)), uncovered, overflow


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("run_dir", type=Path, nargs="?", default=os.environ.get("WAYTAPE_RUN_DIR"))
    parser.add_argument(
        "--max-flagged",
        type=int,
        default=0,
        help="how many flagged frames to accept before failing (default: 0)",
    )
    args = parser.parse_args()
    if args.run_dir is None:
        parser.error("give a run directory, or run as a waytape analyzer")
    run_dir = Path(args.run_dir)

    # What waytape recorded about each frame: when, and which step ran.
    with open(run_dir / "frames.tsv", newline="") as table:
        recorded = list(csv.DictReader(table, delimiter="\t"))
    if not recorded:
        print("window_fill: no frames were recorded", file=sys.stderr)
        return 1
    if not (run_dir / "frames").is_dir():
        print("window_fill: no frames/ in the run; the scenario needs `capture.frames: true`", file=sys.stderr)
        return 1

    rows, flagged = [], []
    for frame in recorded:
        index = int(frame["index"])
        time = float(frame["time_ms"]) / 1000
        pixels = np.asarray(Image.open(run_dir / "frames" / f"{index:06}.png").convert("RGB"))
        rect, uncovered, overflow = analyze_frame(pixels)
        rect_text = "-" if rect is None else "{},{} {}x{}".format(*rect)
        step = "-" if frame["step"] == "-" else f'{frame["step_index"]}:{frame["step"]}'
        rows.append(f"{index}\t{time:.3f}\t{rect_text}\t{uncovered}\t{overflow}\t{step}")

        notes = []
        if uncovered:
            notes.append(f"uncovered={uncovered}")
        if overflow > CURSOR_ALLOWANCE:
            notes.append(f"overflow={overflow}")
        if notes:
            flagged.append(f"frame {index} t={time:.3f} window={rect_text} step={step} " + " ".join(notes))

    (run_dir / "analysis.tsv").write_text(
        "frame\ttime\twindow\tuncovered_px\toverflow_px\tstep\n" + "\n".join(rows) + "\n"
    )

    first, last = float(recorded[0]["time_ms"]), float(recorded[-1]["time_ms"])
    print(f"frames: {len(recorded)}")
    print(f"duration: {(last - first) / 1000:.2f}s")
    # The flagged frames first and the count last: waytape copies the end
    # of this output into summary.txt.
    for line in flagged:
        print(line)
    print(f"frames with uncovered or overflowing pixels: {len(flagged)}")
    return 1 if len(flagged) > args.max_flagged else 0


if __name__ == "__main__":
    sys.exit(main())
