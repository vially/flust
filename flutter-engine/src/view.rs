use std::{collections::HashMap, sync::Arc};

use flutter_engine_api::FlutterOpenGLHandler;

use crate::compositor::FlutterCompositorHandler;

pub const IMPLICIT_VIEW_ID: u32 = 1;

/// The view capable of acting as a rendering target and input source for the Flutter engine.
pub struct FlutterView {
    id: u32,
    opengl_handler: Arc<dyn FlutterOpenGLHandler>,
    compositor_handler: Option<Arc<dyn FlutterCompositorHandler>>,
}

impl FlutterView {
    pub fn new_without_compositor(
        id: u32,
        opengl_handler: impl FlutterOpenGLHandler + 'static,
    ) -> Self {
        Self {
            id,
            opengl_handler: Arc::new(opengl_handler),
            compositor_handler: None,
        }
    }

    pub fn new_with_compositor(
        id: u32,
        opengl_handler: impl FlutterOpenGLHandler + 'static,
        compositor_handler: impl FlutterCompositorHandler + 'static,
    ) -> Self {
        Self {
            id,
            opengl_handler: Arc::new(opengl_handler),
            compositor_handler: Some(Arc::new(compositor_handler)),
        }
    }
}

#[derive(Default)]
pub struct ViewRegistry {
    views: HashMap<u32, FlutterView>,
}

impl ViewRegistry {
    pub fn add_view(&mut self, view: FlutterView) {
        self.views.insert(view.id, view);
    }

    pub fn remove_view(&mut self, view_id: u32) {
        self.views.remove(&view_id);
    }

    pub fn implicit_view(&self) -> Option<&FlutterView> {
        self.views.get(&IMPLICIT_VIEW_ID)
    }

    pub fn implicit_view_opengl_handler(&self) -> Option<Arc<dyn FlutterOpenGLHandler>> {
        self.views
            .get(&IMPLICIT_VIEW_ID)
            .map(|view| view.opengl_handler.clone())
    }

    pub fn implicit_view_compositor_handler(&self) -> Option<Arc<dyn FlutterCompositorHandler>> {
        self.views
            .get(&IMPLICIT_VIEW_ID)
            .and_then(|view| view.compositor_handler.as_ref().cloned())
    }
}
