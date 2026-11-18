# End-to-end tests

These tests run a Flust app the way a user would: in a real compositor,
driven with a pointer, with every frame the compositor displays recorded
and checked. They exist so rendering changes (resize handling in
particular) can be checked without a person watching a window, including
by a coding agent.

They are [waytape](https://github.com/vially/waytape) scenarios. waytape
starts a private headless sway, so nothing appears on, or connects to,
your own desktop session.

Most of them were written for
[the resize issue](https://forge.lab.vially.dev/vially/flust/issues/1).
They describe what a Flutter embedder should do. Flust failed all of them
before that issue was worked on and passes all of them now: see
[Results](#results).

## Directory structure

- `app`: The probe app, a Flutter app with a Flust runner, made to be read
  back from a recording. Most scenarios drive it.
- `scenarios`: One YAML file per test. Each starts an app, drives it and
  names the analyzers to run over the result.
- `analyzers`: Scripts that check a finished run. A scenario refers to
  them from its `analyze` entry.
- `build-app.sh`, `build-reference.sh`, `run.py`: Build the probe app, and
  run all of its scenarios.

Runs are written to `.waytape/runs/` in the directory `waytape` is run
from. They are output, not references: nothing in there is compared
against on the next run.

## Requirements

`waytape` on `PATH` (and what it needs: `sway`, `swaybg`, `ffmpeg`;
`waytape doctor` checks), Python 3 with `numpy` and `Pillow` for the
analyzers, and the Flutter SDK and a Rust toolchain to build the app.

## Running

waytape does not build the app. Build it first, and again after changing
Flust:

```sh
tests/e2e/build-app.sh
```

Then, from the repository root:

```sh
tests/e2e/run.py                      # every probe scenario, one line each
tests/e2e/run.py drag-edges steps     # some of them
tests/e2e/run.py --target surface     # all of them, with the other render target
tests/e2e/run.py --repeat 5 storm     # five times, to catch what is rare

waytape run tests/e2e/scenarios/drag-edges.yaml       # one, in full
waytape check tests/e2e/scenarios/*.yaml              # validate, don't run
```

`run.py` keeps each run in `.waytape/runs/<scenario>`. `waytape run`
prints the run directory, which is `.waytape/runs/latest` afterwards. Read
`summary.txt` first: it has the end of each analyzer's output, which is
its verdict. The contact sheets are in `sheets/`, the frames in `frames/`,
and `waytape frames <run> --range 37-48 --zoom` writes a closer look at a
range of them.

Flust can have the engine render into a framebuffer that it then copies
to the window (`FlutterOpenGLTargetType::Framebuffer`, the default here),
or straight into the window's surface (`Surface`). The two take different
paths through a resize, so run both: `--target surface` does the second,
into `.waytape/runs/<scenario>-surface`. With `waytape run`, that is
`--env PROBE_TARGET=surface`.

To see what the app did in a run, add `--env
RUST_LOG=info,flust_sctk=trace` to a `waytape run`: `app.log` then has
Flust's decisions between the lines of the protocol trace.

### The reference

The same Dart app can be built with Flutter's own Linux (GTK) embedder and
put through the same scenarios:

```sh
tests/e2e/build-reference.sh
tests/e2e/run.py --reference          # runs in .waytape/runs/<scenario>-reference
```

That is what shows that the checks can be passed at all, and that a
failure is the embedder's and not the scenario's or the analyzer's. Run it
after changing a scenario or an analyzer.

## The probe app

`app/lib/main.dart` draws, on whole physical pixels:

- a 16x16 square in each corner: red top-left, blue top-right, yellow
  bottom-left, cyan bottom-right;
- a row of 8x8 cells, black for 0 and white for 1 between two orange
  ones, that spells out the width and height (13 bits each) the frame was
  laid out for;
- a second row with the number of frames painted so far (16 bits) and the
  number of taps received (8 bits);
- the same in words, and a bar that moves with the frame number, for a
  person looking at the recording.

It needs a window at least 264 pixels wide. It prints `probe: size WxH`
when the size it paints at changes, `probe: tap N at X,Y` for every tap,
and `probe: alive frames=N` twice a second.

With `PROBE_ANIMATE=1` it paints a frame on every vsync. Without, only
when something changes, so that every frame during a resize is one the
resize asked for.

`analyzers/probe.py` reads all of that back from a frame of the recording.
The constants there mirror the ones in `main.dart`.

## Scenarios

For the probe app:

| Scenario | What it covers |
|---|---|
| `startup` | A window that is not being resized: the baseline |
| `drag-edges` | Interactive resizes from all four edges and four corners, out and back |
| `drag-speeds` | The same drag slow, fast, as a flick, and back and forth |
| `drag-animated` | Interactive resizes while the app paints a frame on every vsync |
| `steps` | One-shot resizes, a single configure each: larger, smaller, odd sizes |
| `storm` | Resizes faster than the app can draw |
| `states` | Fullscreen and back, tiled and back |
| `input` | Taps land where the pointer is after resizes |
| `hidden` | The window is hidden, resized while hidden, and shown again |
| `scale` | Startup, taps and resizes on an output with scale 2 |
| `scale-change` | The output's scale changes while the app runs, to 2 and back |
| `close` | Closing the window in the middle of a resize |

For the template app (`flust-app-template`, built with `cargo build`
there), with `window_fill.py` only: `idle`, `resize-drag`, `resize-steps`.

## What fails a run

A failed step, or an analyzer that exits with a non-zero status. Each
analyzer's docstring says exactly what it checks and which options it
takes.

The scenarios give the output a magenta background and the window an 8px
green border, which sway draws at the size it has given the window. The
app's surface belongs exactly inside that border.

| Analyzer | Fails when |
|---|---|
| `probe_frames.py` | In any displayed frame, the app's picture is not at the window's position and size, or not all of it is there. Reports `size`, `offset`, `damaged`, `no-content` per frame in `probe-frames.tsv`. `--scale` and `--scale-from` tell it the output's scale |
| `probe_cadence.py` | While animating: the picture stands still for more than 100 ms, more than 5% of the frames the app painted were never displayed, or frames are displayed out of order |
| `protocol.py` | The app answers a configure with a commit that has no buffer of the configured size, commits a buffer that does not fit the buffer scale, commits buffers faster than the output refreshes, or gets a protocol error. Reads the `WAYLAND_DEBUG` trace in `app.log` |
| `app_log.py` | The app logs a warning, an error or a panic |
| `window_fill.py` | Any app: magenta inside the border (the buffer is smaller than the window) or content outside it |

Only frames with damage are recorded, so a gap in timestamps means
nothing changed on screen. Frames from before the first step are startup
and are not checked by the `probe_*` analyzers.

## Results

On 2026-11-18, with Flutter 3.47.9, everything in debug mode. "Before" is
Flust at `8b3fe3b`, before the resize handling was reworked. "Frames" is
displayed frames whose picture did not match the window, of those checked.
"Protocol" is the number of violations.

| Scenario | Before: frames | Before: protocol | Now: frames | Now: protocol | Reference |
|---|---|---|---|---|---|
| `startup` | 0 of 1 | 1 | 0 of 1 | 0 | passes |
| `drag-edges` | 857 of 940 | 881 | 0 of 990 | 0 | passes |
| `drag-speeds` | 492 of 543 | 507 | 0 of 558 | 0 | passes |
| `drag-animated` | 361 of 679 | 384 | 0 of 735 | 0 | frames and protocol pass |
| `steps` | 8 of 32 | 9 | 0 of 16 | 0 | passes |
| `storm` | 13 of 29 | 20 | 0 of 28 | 0 | passes |
| `states` | 6 of 23 | 7 | 0 of 12 | 0 | passes |
| `input` | 58 of 72 | 63 | 0 of 66 | 0 | passes |
| `hidden` | 1 of 251 | 3 | 0 of 246 | 0 | passes |
| `scale` | 94 of 110 | 98 | 0 of 125 | 0 | passes |
| `scale-change` | | | 2 of 10 (4 allowed) | 0 | not run |
| `close` | 1 of 63 | 2 | 0 of 61 | 0 | passes |

All twelve pass, with both render targets, and the app logs no warnings or
errors in any of them (before: in at least six). The animation in `drag-animated`:

| | Before | Now | Reference |
|---|---|---|---|
| Painted frames never displayed | 84% | 4 to 6% | 26% |
| Longest the picture stood still | 273 ms | 33 to 50 ms | 199 ms |

The "now" numbers are of one run of the suite, and those of the animation
of the ten runs made while it was worked on. During a drag, a configure is
answered with a buffer of its size after 16 ms (median) and 26 ms at worst.

What the suite found on the way, beyond what the issue describes:

- **Sway waits for one particular configure.** It holds a resize back
  until a commit acknowledges exactly the configure it sent for it, and
  shows the window's old content for up to 200 ms otherwise. xdg-shell
  lets a client acknowledge only the last of several configures, but then
  the end of every drag, where sway sends three for the same size, stalls
  for those 200 ms. That is the 199 ms of the reference, which
  acknowledges what it has received when it commits. Flust acknowledges
  each configure with a commit of its own.
- **Sway sends a frame callback right after every commit it does not
  show**, which is every commit while it holds a resize back. Taken as a
  vsync, that made Flust draw several hundred frames a second.
- **Mesa picks the buffer for a frame when the context is made current**,
  if it was not current before, and not only when the frame is first
  drawn to. The window's size has to be set before that, or it takes
  effect a frame late.

What is left:

- The frames `drag-animated` still loses are frames of the old size that
  Flust commits while sway holds a resize back and shows the old content.
  That is up to one per step of the drag. `--max-dropped` is 10 there for
  that reason; the default of 5 is what an app that is not being resized
  should stay under.
- In `scale-change`, the frame in which the output's scale changes shows
  the app's old buffer at the new scale. Only the compositor could avoid
  that.

## Writing a scenario

Copy an existing one: the `app` and `session` headers are the same in all
of them, and the analyzers rely on the `session` colours and border width.
`waytape schema` prints the format, and the waytape README describes every
step.

Sizes and points are the app's surface, without the border, in logical
pixels. The window starts at 624x464, which is 640x480 with its border.

A `mark` step starts a section: the `probe_*` analyzers report per
section. Leave the pointer out of the recording (`capture.cursor: false`):
it would cover the corner it drags. Do not assert the window's size after
a `drag_resize` out and back: the window ends a few pixels off, with the
reference as well.

## Limitations

- Sway is the only compositor covered. It applies resizes transactionally
  (it holds the new window size back until the client commits for it, or a
  timeout passes). Other compositors may behave differently, so a clean
  run here does not prove a resize is clean everywhere.
- The output is 60Hz. Fractional scales are not covered.
- waytape places the pointer by the scale the session started with, so
  `scale-change` has no taps or drags after the change.
- Times are when waytape received a frame. They are good for spotting
  stalls of several frames, not for measuring latency.
- `window_fill.py` does not detect content that is offset, stretched or
  stale; the `probe_*` analyzers do, for the probe app only.
