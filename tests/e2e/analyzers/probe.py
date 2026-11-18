"""Read a waytape run of the probe app (tests/e2e/app) back from its frames.

Shared by the `probe_*.py` analyzers. The probe app draws, in every frame,
a square in each corner and two rows of cells that spell out the size the
frame was laid out for, a frame counter and a tap counter (see
`app/lib/main.dart`: the constants here mirror the ones there). Together
with the border sway draws around the window, that says for every frame
the compositor displayed:

- where the window was (`window`),
- where the app's picture was (`content`), and whether all of it was there,
- which size Flutter had laid that picture out for (`encoded`),
- which frame of the app it was (`counter`).
"""

import csv
import json
import os
from dataclasses import asdict, dataclass, field
from pathlib import Path

import numpy as np
from PIL import Image

# What the scenarios set in `session`.
BACKGROUND = (255, 0, 255)
BORDER = (0, 255, 0)
BORDER_WIDTH = 8

# What the probe app draws, in physical pixels.
CORNER = 16
CELL = 8
CODE_LEFT = 24
CODE_TOP = 4
ROW_PITCH = CELL + 4
SIZE_BITS = 13
FRAME_BITS = 16
TAP_BITS = 8

TOP_LEFT = (255, 0, 0)
TOP_RIGHT = (0, 0, 255)
BOTTOM_LEFT = (255, 255, 0)
BOTTOM_RIGHT = (0, 255, 255)
SYNC = (255, 128, 0)
ONE = (255, 255, 255)
ZERO = (0, 0, 0)


@dataclass
class Frame:
    """One frame of the recording, and what could be read from it."""

    index: int
    time: float  # seconds since the start of the run
    step: str  # "<index>:<name>" of the step that was running, or "-"
    section: str  # the text of the last `mark` step before the frame
    # Rectangles are (x, y, width, height) in pixels of the recording.
    window: tuple | None = None  # inside sway's border
    content: tuple | None = None  # the app's picture, from its corners
    encoded: tuple | None = None  # (width, height) the picture says it is
    counter: int | None = None
    taps: int | None = None
    # What is wrong with the frame, in words. Empty when nothing is.
    problems: list = field(default_factory=list)

    @property
    def in_steps(self):
        return self.step != "-"


def run_dir_argument(parser):
    parser.add_argument("run_dir", type=Path, nargs="?", default=os.environ.get("WAYTAPE_RUN_DIR"))
    parser.add_argument(
        "--scale",
        type=float,
        default=1.0,
        help="the output's scale in the scenario (default: 1)",
    )


def _bounding_box(mask):
    rows = np.flatnonzero(mask.any(axis=1))
    cols = np.flatnonzero(mask.any(axis=0))
    if rows.size == 0:
        return None
    # x0, y0, x1, y1 with x1 and y1 exclusive.
    return int(cols[0]), int(rows[0]), int(cols[-1]) + 1, int(rows[-1]) + 1


def _is(pixels, colour):
    """Where `pixels` is exactly `colour`."""
    # One comparison of packed pixels is much faster than one per channel.
    packed = pixels.view(np.uint32)[..., 0] if pixels.shape[2] == 4 else None
    if packed is None:
        return (pixels == colour).all(axis=2)
    return packed == np.frombuffer(bytes((*colour, 255)), dtype=np.uint32)[0]


def find_window(pixels, border_width):
    """The rectangle inside sway's border, or None if there is no border."""
    border = _is(pixels, BORDER)
    # The border is the only thing that fills whole runs of rows and columns
    # with pure green, so its extent is the outer edge of the window.
    rows = np.flatnonzero(border.sum(axis=1) >= border_width)
    cols = np.flatnonzero(border.sum(axis=0) >= border_width)
    if rows.size == 0 or cols.size == 0:
        return None
    left, top = int(cols[0]) + border_width, int(rows[0]) + border_width
    width = int(cols[-1]) - border_width + 1 - left
    height = int(rows[-1]) - border_width + 1 - top
    if width <= 0 or height <= 0:
        return None
    return left, top, width, height


def _read_bits(pixels, x, y, count):
    value = 0
    for _ in range(count):
        colour = tuple(int(c) for c in pixels[y, x][:3])
        if colour == ONE:
            value = value * 2 + 1
        elif colour == ZERO:
            value = value * 2
        else:
            return None, x
        x += CELL
    return value, x


def _read_row(pixels, origin, row, fields):
    """The values of one code row, or None if it is not all there."""
    height, width = pixels.shape[:2]
    x = origin[0] + CODE_LEFT + CELL // 2
    y = origin[1] + CODE_TOP + row * ROW_PITCH + CELL // 2
    cells = 2 + sum(fields)
    if y >= height or x + (cells - 1) * CELL >= width:
        return None
    if tuple(int(c) for c in pixels[y, x][:3]) != SYNC:
        return None
    x += CELL
    values = []
    for count in fields:
        value, x = _read_bits(pixels, x, y, count)
        if value is None:
            return None
        values.append(value)
    if tuple(int(c) for c in pixels[y, x][:3]) != SYNC:
        return None
    return values


def read_frame(pixels, frame, border_width):
    """Fill in what `pixels` shows into `frame`, problems included."""
    frame.window = find_window(pixels, border_width)

    corners = {
        "top-left": (TOP_LEFT, 0, 0),
        "top-right": (TOP_RIGHT, 1, 0),
        "bottom-left": (BOTTOM_LEFT, 0, 1),
        "bottom-right": (BOTTOM_RIGHT, 1, 1),
    }
    boxes = {name: _bounding_box(_is(pixels, colour)) for name, (colour, _, _) in corners.items()}
    if boxes["top-left"] is None:
        # Neither a window nor a picture: the window is not on screen (it is
        # on another workspace), and there is nothing to check.
        if frame.window is not None:
            frame.problems.append("no-content")
        return
    if frame.window is None:
        # A picture without a border: the window is fullscreen.
        frame.window = (0, 0, pixels.shape[1], pixels.shape[0])

    # The picture starts at its top-left corner, and says how large it is.
    origin = boxes["top-left"][:2]
    sizes = _read_row(pixels, origin, 0, (SIZE_BITS, SIZE_BITS))
    counters = _read_row(pixels, origin, 1, (FRAME_BITS, TAP_BITS))
    if counters is not None:
        frame.counter, frame.taps = counters
    if sizes is None or counters is None:
        frame.problems.append("damaged(code)")
        return
    frame.encoded = tuple(sizes)
    frame.content = (*origin, *frame.encoded)

    if frame.content != frame.window:
        dx = frame.content[0] - frame.window[0]
        dy = frame.content[1] - frame.window[1]
        if frame.encoded == frame.window[2:]:
            frame.problems.append(f"offset({dx:+d},{dy:+d})")
        else:
            # A picture larger than the window is cut off at the window's
            # edges by sway; a smaller one is centred in it.
            frame.problems.append(
                "size({}x{} in a {}x{} window, at {:+d},{:+d})".format(*frame.encoded, *frame.window[2:], dx, dy)
            )
        return

    # The right size in the right place: then all of it has to be there.
    left, top, width, height = frame.content
    damaged = []
    for name, (_, right_side, bottom_side) in corners.items():
        x = left + right_side * (width - CORNER)
        y = top + bottom_side * (height - CORNER)
        if boxes[name] != (x, y, x + CORNER, y + CORNER):
            damaged.append("corner " + name)
    if damaged:
        frame.problems.append("damaged(" + ", ".join(damaged) + ")")


def load_run(run_dir, scale=1.0, only_steps=True):
    """The frames of a run, read. Pre-step frames are startup: skipped by default."""
    run_dir = Path(run_dir)
    # Reading the frames takes a while, and several analyzers want them.
    cache = run_dir / f"probe-frames-{scale:g}.json"
    if cache.exists():
        frames = [Frame(**entry) for entry in json.loads(cache.read_text())]
        for frame in frames:
            for name in ("window", "content", "encoded"):
                value = getattr(frame, name)
                setattr(frame, name, None if value is None else tuple(value))
        return [frame for frame in frames if frame.in_steps or not only_steps]

    frames_dir = run_dir / "frames"
    if not frames_dir.is_dir():
        raise SystemExit("no frames/ in the run; the scenario needs `capture.frames: true`")

    marks = []
    with open(run_dir / "events.jsonl") as events:
        for line in events:
            event = json.loads(line)
            if event.get("type") == "mark":
                marks.append((event["t_ms"], event["text"]))

    border_width = round(BORDER_WIDTH * scale)
    frames = []
    with open(run_dir / "frames.tsv", newline="") as table:
        for row in csv.DictReader(table, delimiter="\t"):
            time_ms = float(row["time_ms"])
            section = "-"
            for at, text in marks:
                if at <= time_ms:
                    section = text
            step = "-" if row["step"] == "-" else f'{row["step_index"]}:{row["step"]}'
            frame = Frame(int(row["index"]), time_ms / 1000, step, section)
            pixels = np.asarray(Image.open(frames_dir / f"{frame.index:06}.png").convert("RGBA"))
            pixels = np.ascontiguousarray(pixels)
            read_frame(pixels, frame, border_width)
            frames.append(frame)
    cache.write_text(json.dumps([asdict(frame) for frame in frames]))
    return [frame for frame in frames if frame.in_steps or not only_steps]


def rect_text(rect):
    return "-" if rect is None else "{2}x{3}+{0}+{1}".format(*rect)


def sections(frames):
    """The frames grouped by section, in order of appearance."""
    grouped = {}
    for frame in frames:
        grouped.setdefault(frame.section, []).append(frame)
    return grouped
