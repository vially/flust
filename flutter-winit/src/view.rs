use flutter_engine::{
    view::{FlutterView, IMPLICIT_VIEW_ID},
    FlutterEngine,
};
use std::error::Error as StdError;
use thiserror::Error;
use winit::{event_loop::EventLoop, window::WindowBuilder};

use crate::{window::FlutterEvent, FlutterWindow};

pub struct FlutterViewWinit {
    id: u32,
    window: FlutterWindow,
}

impl FlutterViewWinit {
    pub fn new_implicit(
        event_loop: &EventLoop<FlutterEvent>,
        engine: FlutterEngine,
        builder: WindowBuilder,
    ) -> Result<Self, WinitControllerError> {
        let window = FlutterWindow::new(event_loop, engine, builder)?;

        Ok(Self::new(IMPLICIT_VIEW_ID, window))
    }

    pub fn new(id: u32, window: FlutterWindow) -> Self {
        Self { id, window }
    }

    pub(crate) fn window(&self) -> &FlutterWindow {
        &self.window
    }

    pub(crate) fn create_flutter_view(&self) -> FlutterView {
        FlutterView::new_without_compositor(self.id, self.window.create_opengl_handler())
    }
}

#[derive(Error, Debug)]
pub enum WinitControllerError {
    #[error(transparent)]
    WinitWindowBuildError(#[from] Box<dyn StdError>),
}
