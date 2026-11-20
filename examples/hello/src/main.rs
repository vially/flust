use std::process::ExitCode;

use flust::{Backend, BackendConfigSctk, FlutterOpenGLTargetType, application::Application};

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config = BackendConfigSctk::from(FlutterOpenGLTargetType::Framebuffer);

    // The Flutter bundle is loaded from `data/` and `lib/`, next to the binary.
    let app = Application::builder()
        .with_backend(Backend::Sctk(config))
        .with_title("Hello Flust")
        .with_app_id("dev.vially.flust.example.hello")
        .build()?;

    Ok(app.run()?)
}
