use crate::window::{CreateError, FlutterWindow, WindowArgs};
use tracing::error;
use std::path::PathBuf;

mod handler;
pub mod window;

pub fn init() -> Result<FlutterDesktop, glfw::InitError> {
    glfw::init(Some(glfw::Callback {
        f: glfw_error_callback,
        data: (),
    }))
    .map(|glfw| FlutterDesktop { glfw })
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn glfw_error_callback(error: glfw::Error, description: String, _: &()) {
    error!("GLFW error ({}): {}", error, description);
}

pub struct FlutterDesktop {
    glfw: glfw::Glfw,
}

impl FlutterDesktop {
    pub fn create_window(
        &mut self,
        window_args: &WindowArgs,
        assets_path: PathBuf,
        arguments: Vec<String>,
    ) -> Result<FlutterWindow, CreateError> {
        FlutterWindow::create(&mut self.glfw, window_args, assets_path, arguments)
    }

    pub fn glfw(&self) -> glfw::Glfw {
        self.glfw.clone()
    }
}
