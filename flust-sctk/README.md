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
- The `flust-winit` backend always draws with Skia, unless `--enable-impeller` is passed.
- Upstream rates Impeller through the embedder API with OpenGL as experimental. Flutter's own Linux embedder renders with Impeller through the same path by default, but Flust has only been tried with Mesa's `radeonsi` driver so far.

### Testing

The end-to-end tests run with either renderer: `tests/e2e/run.py` for Impeller, `tests/e2e/run.py --skia` for Skia. The `shapes` scenario checks that paths, clips and antialiasing come out right. See [`tests/e2e/README.md`](../tests/e2e/README.md).

## Windows

By default, Flust creates one window when the app starts, for Flutter's implicit view. An app that wants more windows creates all of them itself, from Dart, with Flutter's windowing API (`WindowController`, the `Window` widget, `WindowManager`), and is started without a window of its own:

```rust
use flust::{application::Application, Backend, BackendConfigSctk};

let config = BackendConfigSctk::default().with_startup_window(false);
let app = Application::builder()
    .with_backend(Backend::Sctk(config))
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

The work is tracked in https://forge.lab.vially.dev/vially/flust/issues/7.

### Limitations

- **Only regular windows** so far: dialogs, tooltips, popups and satellite windows throw `UnimplementedError`.
- **Windows are not sized to their content** (`WindowController.shrinkWrap`): they get the app's default size, as with Flutter's own Linux embedder.
- **Activating a window does nothing.** Wayland only lets a window be activated with a token from the window that has the focus (`xdg_activation_v1`), which Flust does not ask for yet.
- **Minimizing can not be undone, and is not reported.** Wayland has no way for an app to bring a minimized window back, and compositors do not say whether a window is minimized.
- **The compositor decides about sizes.** A size asked for with `setSize` does not change a maximized, fullscreen or tiled window, and the size limits are hints.
- **Windows created from Dart can not be drawn into straight**: they need the default `FlutterOpenGLTargetType::Framebuffer`.
- **Ahead-of-time builds need the feature passed by hand.** `flutter run` and `flutter build bundle` compile the app with the windowing feature (as the `FLUTTER_ENABLED_FEATURE_FLAGS` Dart define), but `flutter assemble`, which builds an AOT `libapp.so`, only compiles with the defines it is given: add `-dDartDefines=$(printf FLUTTER_ENABLED_FEATURE_FLAGS=windowing | base64)`. Without it, the app throws an `UnsupportedError` when it creates a window. Flutter publishes no AOT engine library of a master commit for embedders either.
