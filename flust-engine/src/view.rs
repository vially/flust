use std::{collections::HashMap, sync::Arc};

use flust_engine_api::FlutterOpenGLHandler;

use crate::{
    compositor::FlutterCompositorHandler,
    ffi::{FlutterViewId, IMPLICIT_VIEW_ID},
};

/// The view capable of acting as a rendering target and input source for the Flutter engine.
pub struct FlutterView {
    id: FlutterViewId,
    opengl_handler: Arc<dyn FlutterOpenGLHandler>,
    compositor_handler: Option<Arc<dyn FlutterCompositorHandler>>,
}

impl FlutterView {
    pub fn new_without_compositor(
        id: FlutterViewId,
        opengl_handler: impl FlutterOpenGLHandler + 'static,
    ) -> Self {
        Self {
            id,
            opengl_handler: Arc::new(opengl_handler),
            compositor_handler: None,
        }
    }

    pub fn new_with_compositor(
        id: FlutterViewId,
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
    views: HashMap<FlutterViewId, FlutterView>,
}

impl ViewRegistry {
    pub fn add_view(&mut self, view: FlutterView) {
        self.views.insert(view.id, view);
    }

    pub fn remove_view(&mut self, view_id: FlutterViewId) {
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

    pub fn compositor_handler_for_view(
        &self,
        view_id: FlutterViewId,
    ) -> Option<Arc<dyn FlutterCompositorHandler>> {
        self.views
            .get(&view_id)
            .and_then(|view| view.compositor_handler.as_ref().cloned())
    }
}
