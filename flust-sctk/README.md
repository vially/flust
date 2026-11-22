# flust-sctk

The Wayland backend of Flust, built on [Smithay's client toolkit](https://github.com/Smithay/client-toolkit). It creates the window, passes input to the engine, and presents the frames the engine draws.

## Rendering

The engine draws with OpenGL into a framebuffer (a backing store) that Flust then copies to the window. It can draw with either of Flutter's renderers: Impeller, the default, or Skia.

There is one EGL context for the whole application, which the engine draws in, current without a surface. Each window has only an EGL surface: the context is made current on it to copy a frame to the window and present it, and current without a surface again right after. This is how the engine can draw several windows: it draws every view in the one context it is given, and does not say which view it is drawing. With `FlutterOpenGLTargetType::Surface`, the engine draws straight into the window's surface instead, which the context is made current on for that.

The work on Impeller is tracked in https://forge.lab.vially.dev/vially/flust/issues/8.

### Choosing the renderer

Impeller is the default, as in Flutter's own desktop embedders since Flutter 3.47. To draw with Skia instead:

```rust
use flust::{application::Application, Renderer};

let app = Application::builder()
    .with_renderer(Renderer::Skia)
    .build()?;
```

Or, without rebuilding the app, with the engine switch `--enable-impeller` among the engine arguments (`ApplicationBuilder::with_arg`): `--enable-impeller` or `--enable-impeller=true` for Impeller, `--enable-impeller=false` for Skia. The switch takes precedence over `with_renderer`. The app logs `rendering with Impeller` or `rendering with Skia` when it starts, and the engine logs `Using the Impeller rendering backend (OpenGLES)` when it uses Impeller.

### What changes with Impeller

Impeller draws through its OpenGL ES backend, and needs more from the framebuffers it draws into than Skia does. With Impeller, Flust:

- **Asks for an OpenGL ES context first.** Impeller works on a desktop OpenGL context too, but only antialiases what it draws offscreen (layers, backdrop filters) on OpenGL ES 3.
- **Gives backing stores depth and stencil.** Impeller fills paths with the stencil buffer and clips with the depth buffer. Without them a star is filled as its bounding box, and clips other than rectangles do nothing.
- **Multisamples backing stores (4x).** That is how Impeller antialiases. The copy to the window resolves the samples. This needs OpenGL (ES) 3; with OpenGL ES 2, backing stores get depth and stencil only, and edges are not antialiased.

The app logs which kind of backing store it uses: `backing store framebuffers: Multisampled`, `TextureWithDepthStencil`, or `Texture` (Skia).

With Skia, nothing of this changes.

### Limitations

- **`FlutterOpenGLTargetType::Surface` does not work with Impeller.** The engine can not draw into the window's surface directly with Impeller (https://github.com/flutter/flutter/issues/151670). Creating the window fails with `UnsupportedTargetType`. Use the default, `FlutterOpenGLTargetType::Framebuffer`, or Skia.
- **There is no fallback.** If the engine can not start with Impeller, the app does not start. It does not switch to Skia by itself.
- **External textures** have to be `GL_RGBA8` with Impeller. Give a `TextureFrame` its size with `with_size`; without one, the size the texture is drawn at is used.
- **Vulkan** is not supported, with either renderer.
- **Partial repaint** is not supported: every frame is drawn and presented whole (https://forge.lab.vially.dev/vially/flust/issues/18).
- Upstream rates Impeller through the embedder API with OpenGL as experimental. Flutter's own Linux embedder renders with Impeller through the same path by default, but Flust has only been tried with Mesa's `radeonsi` driver so far.

### Testing

The end-to-end tests run with either renderer: `tests/e2e/run.py` for Impeller, `tests/e2e/run.py --skia` for Skia. The `shapes` scenario checks that paths, clips and antialiasing come out right. See [`tests/e2e/README.md`](../tests/e2e/README.md).

## Scale

A window is drawn at the scale of the outputs it is on, as the compositor
says.
- **Fractional scales:** where the compositor has `wp_fractional_scale_manager_v1` and `wp_viewporter`, a window takes its scale from `wp_fractional_scale_v1.preferred_scale`, in 120ths.
  - **Buffer:** at a fractional scale such as 1.25 or 1.5, the buffer is the window's logical size times the scale, rounded half away from zero, with buffer scale 1.
  - **Viewport:** its destination is the logical size.
  - **Engine:** gets the fractional scale as the device pixel ratio.
- **Integer scales:** a whole scale (1, 2) is drawn with `wl_surface.set_buffer_scale`, as without those globals, where the scale is always a whole number.

A new scale reaches the compositor in the same commit as the first buffer drawn for it, like a new size: see `SurfaceGeometry` in `src/sync.rs`.

### Testing

The `scale-fractional` scenario runs on an output at scale 1.5. The `scale-change-fractional` scenario changes the scale from 1 to 1.25, then 2, then back to 1 while the app runs. See [`tests/e2e/README.md`](../tests/e2e/README.md).

## Windows

By default, Flust creates one window when the app starts, for Flutter's implicit view. An app that wants more windows creates all of them itself, from Dart, with Flutter's windowing API (`WindowController`, the `Window` widget, `WindowManager`), and is started without a window of its own:

```rust
use flust::application::Application;

let app = Application::builder()
    .with_startup_window(false)
    .build()?;
```

Flutter does not combine a window the embedder creates with windows created from Dart, and neither does Flust: in an app with a startup window, creating a window fails.

The windowing API is experimental, on Flutter's master channel only, and needs the windowing feature (`flutter config --enable-windowing`, or `FLUTTER_WINDOWING=true` in the environment of the build). Flutter's own windowing for Linux calls GTK, so a Flust app installs Flust's with the `flust_windowing` package ([`packages/flust_windowing`](../packages/flust_windowing)) before it creates a window:

```dart
Future<void> main() async {
  await FlustWindowing.ensureInitialized();
  runWidget(...);
}
```

Creating a window is synchronous in the API: the window's view exists when the call returns. Dart runs on the platform thread, so Dart calls Flust's functions directly, on that thread, while Flust is in a call into the engine (`src/windowing.rs`). Dart gets their addresses over the `flust/windowing` channel when it installs the package, and hears about close requests and changes of a window's state over the same channel.

A dialog (`DialogWindowController`) with a parent is modal: it gets the parent with `xdg_toplevel.set_parent`, and is marked modal with `xdg_dialog_v1` where the compositor offers it. Keeping input from the parent is up to the app on Wayland, so Flust does it whether the compositor offers xdg-dialog or not: while a window has a modal dialog open, its presses, scrolling and keys are dropped. A dialog is destroyed with its parent, before it, as on Flutter's other platforms. A dialog without a parent is a regular window on Wayland.

Popups and tooltips (`PopupWindowController`, `TooltipWindowController`) are `xdg_popup`s:
- **Placement:** the compositor places them against a rectangle of their parent, as Flutter's `WindowPositioner` says. It was modelled on `xdg_positioner`, so the anchors, offset, and flip, slide and resize adjustments map onto it one to one.
- **Moving:** `updatePosition` moves them with `xdg_popup.reposition` (xdg-shell version 3), which also places them again whenever they change size. GTK can only place a popup once.
- **Size:** they are sized to their content. The engine lays the view out within its constraints, and when a frame comes at a size the popup doesn't have, Flust gives the popup that size first, then shows the next frame of it.

A popup takes input:
- **Grab:** when it's opened by the user's input (a press or a key), it takes an explicit grab, and with it the keyboard focus.
- **Dismissed:** a click outside of it dismisses and destroys it. That's either the compositor (`popup_done`) or Flust itself, for a click into another of the app's windows, which the grab still lets through. That click goes no further.
- **One menu at a time:** opening a popup dismisses the open popups that aren't its parent's.

A tooltip takes no input: its input region is empty.

Both are destroyed with their parent, the topmost first, as xdg-shell requires.

The work is tracked in https://forge.lab.vially.dev/vially/flust/issues/7.

### Limitations

- **No satellite windows**: they throw `UnimplementedError`, as on Flutter's own Linux embedder.
- **Regular windows and dialogs are not sized to their content** (`shrinkWrap`): they get the app's default size, as with Flutter's own Linux embedder. Popups and tooltips are.
- **A popup is not resized to fit.** A compositor that shrinks a popup to fit on the output (the positioner's resize adjustment) gets the size of its content anyway.
- **A popup wants the user's input to open.** Its grab takes the serial of the user's last press or key, and a compositor may refuse a grab with a serial that is not recent: the popup is dismissed right away then.
- **Compositors may dismiss tooltips.** sway dismisses every popup of a window that loses the focus, tooltips too, and Flust destroys them then.
- **Activating a window does nothing.** Wayland only lets a window be activated with a token from the window that has the focus (`xdg_activation_v1`), which Flust does not ask for yet.
- **Minimizing can not be undone, and is not reported.** Wayland has no way for an app to bring a minimized window back, and compositors do not say whether a window is minimized.
- **The compositor decides about sizes.** A size asked for with `setSize` does not change a maximized, fullscreen or tiled window, and the size limits are hints.
- **Windows created from Dart can not be drawn into straight**: they need the default `FlutterOpenGLTargetType::Framebuffer`.
- **Ahead-of-time builds need the feature passed by hand.** `flutter run` and `flutter build bundle` compile the app with the windowing feature (as the `FLUTTER_ENABLED_FEATURE_FLAGS` Dart define), but `flutter assemble`, which builds an AOT `libapp.so`, only compiles with the defines it is given: add `-dDartDefines=$(printf FLUTTER_ENABLED_FEATURE_FLAGS=windowing | base64)`. Without it, the app throws an `UnsupportedError` when it creates a window. Flutter publishes no AOT engine library of a master commit for embedders either.
