use std::process::ExitCode;

use flust::application::Application;

/// An app that opens its windows itself, with Flutter's windowing API:
/// Flust starts it without a window.
fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let app = Application::builder()
        .with_title("Flust E2E Windowing")
        .with_app_id("dev.vially.flust.e2e-windowing")
        .with_startup_window(false)
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
