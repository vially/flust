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
They describe what a Flutter embedder should do, so **Flust fails them
today**: see [Results](#results).

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

waytape run tests/e2e/scenarios/drag-edges.yaml       # one, in full
waytape check tests/e2e/scenarios/*.yaml              # validate, don't run
```

`run.py` keeps each run in `.waytape/runs/<scenario>`. `waytape run`
prints the run directory, which is `.waytape/runs/latest` afterwards. Read
`summary.txt` first: it has the end of each analyzer's output, which is
its verdict. The contact sheets are in `sheets/`, the frames in `frames/`,
and `waytape frames <run> --range 37-48 --zoom` writes a closer look at a
range of them.

To render straight into the window surface instead of a framebuffer
(`FlutterOpenGLTargetType::Surface`), add `--env PROBE_TARGET=surface` to
a `waytape run`.

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
| `probe_frames.py` | In any displayed frame, the app's picture is not at the window's position and size, or not all of it is there. Reports `size`, `offset`, `damaged`, `no-content` per frame in `probe-frames.tsv` |
| `probe_cadence.py` | While animating: the picture stands still for more than 100 ms, more than 5% of the frames the app painted were never displayed, or frames are displayed out of order |
| `protocol.py` | The app answers a configure with a commit that has no buffer of the configured size, commits a buffer that does not fit the buffer scale, commits buffers faster than the output refreshes, or gets a protocol error. Reads the `WAYLAND_DEBUG` trace in `app.log` |
| `app_log.py` | The app logs a warning, an error or a panic |
| `window_fill.py` | Any app: magenta inside the border (the buffer is smaller than the window) or content outside it |

Only frames with damage are recorded, so a gap in timestamps means
nothing changed on screen. Frames from before the first step are startup
and are not checked by the `probe_*` analyzers.

## Results

On 2026-11-18, Flust at `8b3fe3b` and Flutter 3.47.9, both in debug mode.
"Frames" is displayed frames whose picture did not match the window, of
those checked. "Protocol" is the number of violations.

| Scenario | Flust: frames | Flust: protocol | Flust: also | Reference |
|---|---|---|---|---|
| `startup` | 0 of 1 | 1 | | passes |
| `drag-edges` | 857 of 940 | 881 | 20 warnings | passes |
| `drag-speeds` | 492 of 543 | 507 | 11 warnings | passes |
| `drag-animated` | 361 of 679 | 384 | 84% of frames dropped, 273 ms stall | frames and protocol pass; 26% dropped, 199 ms stall |
| `steps` | 8 of 32 | 9 | | passes |
| `storm` | 13 of 29 | 20 | 1 warning | passes |
| `states` | 6 of 23 | 7 | | passes |
| `input` | 58 of 72 | 63 | 4 warnings; every tap lands | passes |
| `hidden` | 1 of 251 | 3 | | passes |
| `scale` | 94 of 110 | 98 | 1 warning | passes |
| `close` | 1 of 63 | 2 | 1 error at exit; exits with 0 | passes |

What that shows about Flust, in the terms of the issue:

- **Configures are answered with stale content** (finding 2). Nearly every
  configure is acknowledged and then committed with no new buffer, by the
  commit that requests the frame callback. Sway takes that as the answer,
  applies the new window size, and displays the old buffer in it: the
  `size` frames. During a drag the picture is one size behind its window
  in almost every frame. The reference commits a buffer of the configured
  size for every configure it answers, and sway never has to show a
  mismatch.
- **Frames are thrown away** (finding 1). In `drag-animated` the app
  paints, and commits buffers for, 500 to 1000 frames in some seconds of
  a drag, on a 60 Hz output: `protocol.py` reports the pacing, and
  `probe_cadence.py` the 84% that were never displayed. In `drag-edges`,
  where the app paints only what a resize asks for, the picture stands
  still for up to 114 ms at a time during a drag (the reference: 33 ms).
- **A hidden window stops** (finding 4). In `hidden` no buffer is
  committed while the window is on another workspace, and the resize sent
  meanwhile is applied 2 s later, one frame after the window is back.
- **The scale is committed without a buffer for it** (finding 2):
  `scale`, at startup.
- `flust_sctk::window` logs "A frame of the wrong size might have been
  presented after a resize was started" in every drag scenario (finding
  3).

What it does not show:

- Frames that are shifted vertically or copied into a buffer of another
  size (finding 3) would be reported as `damaged`. None were, in these
  runs on sway.
- The reference fails `probe_cadence.py` in `drag-animated` too: it halves
  its frame rate while the window grows. The limits are what an embedder
  should manage, and nothing has been seen to pass them yet. They are
  `--max-stall-ms` and `--max-dropped` in the scenario's `analyze` entry.

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
- The output is 60Hz. Fractional scales and scale changes while the app
  runs are not covered.
- Times are when waytape received a frame. They are good for spotting
  stalls of several frames, not for measuring latency.
- `window_fill.py` does not detect content that is offset, stretched or
  stale; the `probe_*` analyzers do, for the probe app only.
