use std::process::ExitCode;

use flust::application::Application;

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // The Flutter bundle is loaded from `data/` and `lib/`, next to the binary.
    let app = Application::builder()
        .with_title("Hello Flust")
        .with_app_id("dev.vially.flust.example.hello")
        // `flutter run --dart-entrypoint-args` passes them to the process.
        .with_dart_entrypoint_args(std::env::args().skip(1).collect())
        .build()?;

    Ok(app.run()?)
}
