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
