use flutter_engine::{
    ffi::{FlutterViewId, IMPLICIT_VIEW_ID},
    plugins::Plugin,
    view::FlutterView,
    FlutterEngine,
};
use std::error::Error as StdError;
use thiserror::Error;
use winit::{event_loop::EventLoop, window::WindowAttributes};

use crate::{window::FlutterEvent, FlutterWindow};

pub struct FlutterViewWinit {
    id: FlutterViewId,
    window: FlutterWindow,
}

impl FlutterViewWinit {
    pub fn new_implicit(
        event_loop: &EventLoop<FlutterEvent>,
        engine: FlutterEngine,
        attributes: WindowAttributes,
    ) -> Result<Self, WinitControllerError> {
        let view_id = IMPLICIT_VIEW_ID;
        let window = FlutterWindow::new(view_id, event_loop, engine, attributes)?;

        Ok(Self::new(view_id, window))
    }

    pub fn new(id: FlutterViewId, window: FlutterWindow) -> Self {
        Self { id, window }
    }

    pub(crate) fn window(&self) -> &FlutterWindow {
        &self.window
    }

    pub(crate) fn create_flutter_view(&self) -> FlutterView {
        FlutterView::new_without_compositor(self.id, self.window.create_opengl_handler())
    }

    pub(crate) fn add_plugin<P>(&mut self, plugin: P)
    where
        P: Plugin + 'static,
    {
        self.window.add_plugin(plugin);
    }
}

#[derive(Error, Debug)]
pub enum WinitControllerError {
    #[error(transparent)]
    WinitWindowBuildError(#[from] Box<dyn StdError>),
}
