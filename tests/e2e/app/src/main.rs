use std::process::ExitCode;

use flust::{
    application::Application, Backend, BackendConfigSctk, FlutterOpenGLTargetType, Renderer,
};

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // `PROBE_TARGET=surface` renders straight into the window surface
    // instead of into a framebuffer that is then copied to it. Only Skia can
    // do that, so it also switches to Skia.
    let (target, renderer) = match std::env::var("PROBE_TARGET").as_deref() {
        Ok("surface") => (FlutterOpenGLTargetType::Surface, Renderer::Skia),
        _ => (FlutterOpenGLTargetType::Framebuffer, Renderer::default()),
    };

    // `PROBE_WINDOWS=1`: the app opens its windows itself.
    let startup_window = std::env::var("PROBE_WINDOWS").as_deref() != Ok("1");
    let config = BackendConfigSctk::from(target).with_startup_window(startup_window);

    let app = Application::builder()
        .with_backend(Backend::Sctk(config))
        .with_title("Flust E2E Probe")
        .with_app_id("dev.vially.flust.e2e-probe")
        .with_renderer(renderer)
        .with_args(engine_args())
        .build()?;

    Ok(app.run()?)
}

/// `PROBE_ENGINE_ARGS` is a list of switches for the Flutter engine, separated
/// by spaces: `--enable-impeller=false` to render with Skia instead of
/// Impeller.
fn engine_args() -> Vec<String> {
    std::env::var("PROBE_ENGINE_ARGS")
        .map(|args| args.split_whitespace().map(String::from).collect())
        .unwrap_or_default()
}
