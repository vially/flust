use flust::{application::Application, Backend, BackendConfigSctk, FlutterOpenGLTargetType};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // `PROBE_TARGET=surface` renders straight into the window surface
    // instead of into a framebuffer that is then copied to it.
    let target = match std::env::var("PROBE_TARGET").as_deref() {
        Ok("surface") => FlutterOpenGLTargetType::Surface,
        _ => FlutterOpenGLTargetType::Framebuffer,
    };

    let app = Application::builder()
        .with_backend(Backend::Sctk(BackendConfigSctk::from(target)))
        .with_title("Flust E2E Probe")
        .with_app_id("dev.vially.flust.e2e-probe")
        .build()?;

    Ok(app.run()?)
}
