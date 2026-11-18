# End-to-end tests

These tests run the template app (`flust-app-template`) the way a user
would: in a real compositor, driven with a pointer, with every frame the
compositor displays recorded and checked. They exist so rendering changes
(resize handling in particular) can be checked without a person watching
a window, including by a coding agent.

They are [waytape](https://github.com/vially/waytape) scenarios. waytape
starts a private headless sway, so nothing appears on, or connects to,
your own desktop session.

## Directory structure

- `scenarios`: One YAML file per test. Each starts the app, drives it and
  names the analyzers to run over the result.
- `analyzers`: Scripts that check a finished run. A scenario refers to
  them from its `analyze` entry.

Runs are written to `.waytape/runs/` in the directory `waytape` is run
from. They are output, not references: nothing in there is compared
against on the next run.

## Requirements

`waytape` on `PATH` (and what it needs: `sway`, `swaybg`, `ffmpeg`;
`waytape doctor` checks), Python 3 with `numpy` and `Pillow` for the
analyzers, and a built template app with its Flutter bundle next to it
(`flust-app-template/target/debug/{data,lib}`, see `docs/building.md`).

waytape does not build the app. Run `cargo build` in `flust-app-template`
first.

## Running

```sh
waytape run tests/e2e/scenarios/resize-drag.yaml
waytape run tests/e2e/scenarios/resize-steps.yaml --wayland-debug
waytape run tests/e2e/scenarios/idle.yaml --app /path/to/another/build
waytape check tests/e2e/scenarios/*.yaml     # validate without running
```

The run directory is printed, and is `.waytape/runs/latest` afterwards.
Read `summary.txt` first. The contact sheets are in `sheets/`, the frames
in `frames/`, and `waytape frames <run> --range 37-48 --zoom` writes a
closer look at a range of them.

To check a change against a known good run, keep that run and pass it as
`--baseline <run>`: the screenshots are compared by name.

## Scenarios

| Scenario | What it covers |
|---|---|
| `idle` | Startup and steady-state rendering |
| `resize-drag` | Interactive resizes, dragging the border from different edges |
| `resize-steps` | One-shot resizes, a single configure each |

## What fails a run

A failed step, or an analyzer that exits with a non-zero status.

`analyzers/window_fill.py` checks every recorded frame. The scenarios
give the output a magenta background and the window an 8px green border,
which sway draws at the size it has given the window. The app's surface
belongs exactly inside that border, so in any frame:

- **magenta inside the border** (`uncovered_px`): the app's buffer is
  smaller than the window, i.e. the app has not caught up with a resize.
  Sway centres an undersized surface.
- **anything else outside the border** (`overflow_px`): the buffer is
  larger than the window. The pointer cursor also counts here, so up to
  32x32 pixels are ignored.

It prints the flagged frames, writes all of them to `analysis.tsv` in the
run directory, and exits with 1 if there are any. `--max-flagged N` in
the scenario's `analyze` entry accepts up to N.

Content that is offset, stretched or stale relative to its neighbours is
not detected. It shows up when comparing consecutive tiles on a contact
sheet.

Only frames with damage are recorded, so a gap in timestamps means
nothing changed on screen. `--every-frame` records every output refresh.

## Writing a scenario

Copy an existing one: the `app` and `session` headers are the same in all
of them, and `window_fill.py` relies on the `session` colours and border
width. `waytape schema` prints the format, and the waytape README
describes every step.

Sizes and points are the app's surface, without the border. The window
starts at 624x464, which is 640x480 with its border.

## Limitations

- Sway is the only compositor covered. It applies resizes transactionally
  (it holds the new window size back until the client commits a matching
  buffer, or a timeout passes). Other compositors may behave differently,
  so a clean run here does not prove a resize is clean everywhere.
- The output is 60Hz at scale 1, with no fractional scaling.
