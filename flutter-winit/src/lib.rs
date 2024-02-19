#![deny(warnings)]

mod application;
mod egl;
mod handler;
mod keyboard;
mod pointer;
mod view;
mod window;

pub use application::{WinitApplication, WinitApplicationBuildError, WinitApplicationRunError};
pub use handler::WinitPlatformTaskHandler;
pub use window::FlutterWindow;
pub use winit::event_loop::{ControlFlow, EventLoop, EventLoopBuilder};
pub use winit::{
    dpi::*, error::EventLoopError, platform::wayland::WindowBuilderExtWayland,
    window::WindowBuilder,
};

pub use view::*;

#[cfg(test)]
mod tests {
    #[test]
    fn test_link() {
        println!("Linking worked");
    }
}
