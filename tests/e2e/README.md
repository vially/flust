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
They describe what a Flutter embedder should do, and Flust is expected to
pass all of them. The issue has the results from before and after its
resize handling was reworked.

## Directory structure

- `app`: The probe app, a Flutter app with a Flust runner, made to be read
  back from a recording. Most scenarios drive it.
- `windowing-app`: An app that opens its windows itself, with Flutter's
  windowing API. The `windowing` scenarios drive it.
- `scenarios`: One YAML file per test. Each starts an app, drives it and
  names the analyzers to run over the result.
- `analyzers`: Scripts that check a finished run. A scenario refers to
  them from its `analyze` entry.
- `tools`: Scripts a scenario runs while the app is up, from an `exec`
  step.
- `build-app.sh`, `build-reference.sh`, `run.py`: Build the probe app, and
  run all of its scenarios.

Runs are written to `.waytape/runs/` in the directory `waytape` is run
from. They are output, not references: nothing in there is compared
against on the next run.

## Requirements

`waytape` on `PATH` (and what it needs: `sway`, `swaybg`, `ffmpeg`;
`waytape doctor` checks), Python 3 with `numpy` and `Pillow` for the
analyzers, and a Rust toolchain and Flutter with the flust tool extension
to build the apps: `build-app.sh` runs `flutter build flust` (see
[its setup](../../packages/flust_flutter_tools/README.md#setup)).

The `semantics` scenario also needs AT-SPI (`at-spi2-core`, with its
`Atspi` bindings for Python through PyGObject), `dbus-run-session`,
`dbus-daemon` and `gdbus`.

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
tests/e2e/run.py --skia               # all of them, rendered with Skia
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
`--env PROBE_TARGET=surface`. Only Skia can render into the window's
surface, so the probe app renders with Skia then.

Otherwise the engine renders with Impeller, Flust's default. `--skia` has
it render with Skia, into `.waytape/runs/<scenario>-skia`. With `waytape
run`, that is `--env PROBE_ENGINE_ARGS=--enable-impeller=false`:
`PROBE_ENGINE_ARGS` is a list of switches for the engine, separated by
spaces.

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

With `PROBE_SEMANTICS=1` it shows a heading, a button, a count, a
checkbox and a text field instead, for the `semantics` scenario. It prints
`probe: semantics count N` when the button is activated, `probe: semantics
checked true|false` when the checkbox is, and `probe: semantics text
"TEXT" selection BASE,EXTENT` when the text of the text field or its
selection changes, and `probe: semantics accessible navigation
true|false` when the framework is told that an assistive technology is in
use or no longer is. Activating the button also announces `Counted to N`
to assistive technologies.

With `PROBE_SHAPES=1` it draws shapes instead, for the `shapes` scenario:
two stars that cross themselves (one filled with the non-zero rule, one
with the even-odd rule), a square clipped to a rounded rectangle and one
clipped to a circle, and two black squares rotated by 30 degrees, the
second in a layer, so that it is drawn offscreen and then composited. It
prints `probe: shapes` when it paints them. `analyzers/shapes.py` checks
pixels of the stars and clips, and that the edges of the squares are
antialiased: the other scenarios only look at rectangles, and pass on a
renderer that fills a star as its bounding box.

With `PROBE_EXIT=1` it cancels every request to exit (a close of its
window), and prints `probe: exit requested, cancelling` when it does. A
tap then has it exit by itself with exit code 3, after printing `probe:
exiting with code 3`. That is for the `exit` scenario.

With `PROBE_WINDOWS=1` it starts without a window and opens two itself,
`probe 1` (624x464) and `probe 2` (400x300), through Flust's test hook for
windows (the `testing` feature, `flust-sctk/src/testing.rs`). Each shows a
probe of its own, whose lines start with its name instead of `probe`
(`probe 1: size WxH ...`), and prints the keys it receives (`probe 1: key
A`). A window that the compositor asks to close is destroyed, with `probe
N: close requested` and `probe N: window destroyed`, and the app exits
when it has none left, after `probe: no window left, exiting`. That is for
the `windows` and `windows-semantics` scenarios, which need the
framebuffer target: windows created while the app runs can not be drawn
into straight.

With `PROBE_WINDOWS=startup` it is the usual probe, which also tries to
open a window, and prints `probe: window refused: CODE: MESSAGE` when it
is refused, for the `windows-startup` scenario.

## The windowing app

`windowing-app/lib/main.dart` uses Flutter's windowing API the way
upstream's `examples/multiple_windows` does, through Flust's
`flust_windowing` package (`packages/flust_windowing`): it starts without
a window, and opens a main window (`main`, 480x360) of its own. A tap in
it opens another window (`window N`, 320x240). Every window prints its
state when it changes:

    window 1: size 320x240 activated true maximized false fullscreen false

and takes keys: `n` opens another window, `d` opens a modal dialog of
the window (`dialog N`, 300x200), `e` a dialog without a parent, `p` a
popup of the window (`popup N`, 160x100) below its rectangle (20, 20, 100,
30), flipped above it or slid sideways if it does not fit, `o` a tooltip
(`tooltip N`, 120x40) placed the same way, `j` moves the rectangle of the
window's tooltips 50 to the right, `m`
maximizes or restores the window, `f` makes it fullscreen or restores it
(not dialogs), `t` renames it to `<title> renamed`, `s` asks for the size
400x300, `c` limits its size to between 300x250 and 500x400, and `w`
destroys it. A dialog prints only its size and whether it is activated, a
popup its offset from its parent and its size (`popup 1: offset 20,50 size
160x100`), and a tooltip its size. A
tap in any window prints `<title>: tap`. A window that the compositor
asks to close is destroyed, with `<title>: destroyed`. When the main
window is, the app exits, after `main: destroyed, exiting`.

The windowing API is only on Flutter's master channel, which the flust
tool extension runs on too. Build the app with:

```sh
tests/e2e/build-app.sh windowing
```

It turns the windowing feature on for the build (`FLUTTER_WINDOWING=true`).
`run.py` skips the `windowing` scenarios while the app is not built.

## Accessibility

The `semantics` scenario reads the app through AT-SPI, the accessibility
API of Linux desktops, like a screen reader does. `tools/atspi_tree.py`
prints the tree of an application, checks that it has certain nodes, or
activates one, or changes the text of a text field or its selection:

```sh
tests/e2e/tools/atspi_tree.py flust-e2e-probe
tests/e2e/tools/atspi_tree.py flust-e2e-probe --expect "button:Increment:enabled"
tests/e2e/tools/atspi_tree.py flust-e2e-probe --click Increment
tests/e2e/tools/atspi_tree.py flust-e2e-probe --set-text Message "Hi there"
tests/e2e/tools/atspi_tree.py flust-e2e-probe --click Increment --expect-announcement "Counted to 1"
```

Applications only put their tree on the accessibility bus once
accessibility is switched on, which is a setting of the session bus
(`org.a11y.Status.IsEnabled`). The scenario must not do that to the
desktop it runs on, so `run.py` gives it a session of its own. Always
run it through `run.py`. What that does:

```sh
runtime_dir="$(mktemp -d)"
XDG_RUNTIME_DIR="$runtime_dir" GSETTINGS_BACKEND=memory \
    ATSPI_DBUS_IMPLEMENTATION=dbus-daemon \
    dbus-run-session -- waytape run tests/e2e/scenarios/semantics.yaml
rm -r "$runtime_dir"
```

A session bus of its own (`dbus-run-session`) is not enough. Each of the
variables is there for something that got out without it:

- `XDG_RUNTIME_DIR`: the accessibility bus has its socket in the runtime
  directory (`at-spi/bus_0`), under the same name as the one of the
  desktop. Without a directory of its own, the scenario's bus takes the
  place of the desktop's and removes the socket when it goes: screen
  readers can not connect anymore (`Couldn't connect to accessibility
  bus`) until `systemctl --user restart at-spi-dbus-bus.service`.
- `GSETTINGS_BACKEND=memory`: the accessibility bus saves that setting to
  the desktop settings of the user (`toolkit-accessibility` in dconf),
  which are in a file and not on the bus. Without this, the scenario
  switches accessibility on for the desktop after all, from the next
  login on.
- `ATSPI_DBUS_IMPLEMENTATION=dbus-daemon`: the accessibility bus is
  started with `dbus-broker` by default, which needs systemd's user
  session and does not find it on a private bus.

The accessibility bus also switches accessibility on while something
listens for its events, as a screen reader does, and off when the last
one that does is gone. `atspi_tree.py --expect-announcement` listens, so
accessibility is off once it exits. The scenario ends with that.

Flutter's GTK embedder is not put through this scenario: it waits for
what Flust logs.

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
| `scale-fractional` | Startup, taps and resizes on an output with scale 1.5: the app draws with a viewport |
| `scale-change-fractional` | The output's scale changes while the app runs, to 1.25, to 2 and back to 1: between a viewport and the buffer scale |
| `close` | Closing the window in the middle of a resize |
| `exit` | The app cancels the close of its window, then exits by itself with an exit code |
| `windows` | Two windows opened by the app: resized on their own, taps and keys reach the right one, one closed while the other draws on |
| `windows-startup` | An app with a startup window is refused another one |
| `windows-semantics` | Each of two windows is an accessible window of its own |
| `semantics` | What a screen reader sees of the app, and that what it asks for reaches the app |
| `shapes` | Paths, clips and antialiasing: what the renderer draws |

For the windowing app, with the framebuffer target only:

| Scenario | What it covers |
|---|---|
| `windowing` | Windows opened with Flutter's windowing API: one closed of several, one destroyed from Dart, and the app exits from the main window's delegate |
| `windowing-state` | A window's title, size, size limits and fullscreen state, changed from Dart and read back |
| `windowing-dialog` | A modal dialog keeps taps and keys from its parent until it is closed, and goes with its parent; a dialog without a parent keeps nothing from input |
| `windowing-popup` | Popups at their anchor and at the size of their content, nested, dismissed by a click outside, flipped and slid at the output's corner; a tooltip that takes no input and follows its moved anchor |

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
- The output is 60Hz.
- At a fractional scale, sway rounds a window's edges to pixels on its
  own, so `probe_frames.py` lets the picture be one pixel off its window
  there. At integer scales it has to match exactly.
- waytape places the pointer by the scale the session started with, so
  `scale-change` and `scale-change-fractional` have no taps or drags
  after the change.
- Times are when waytape received a frame. They are good for spotting
  stalls of several frames, not for measuring latency.
- `window_fill.py` does not detect content that is offset, stretched or
  stale; the `probe_*` analyzers do, for the probe app only.
