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
    pub fn id(&self) -> FlutterViewId {
        self.id
    }

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

/// The views whose handlers the engine's callbacks go to. A view is in it
/// from before the engine is told about it until after the engine is done
/// with it.
#[derive(Default)]
pub struct ViewRegistry {
    views: HashMap<FlutterViewId, FlutterView>,
}

impl ViewRegistry {
    /// Returns the view back if there is one with its id already.
    pub fn add_view(&mut self, view: FlutterView) -> Result<(), FlutterView> {
        if self.views.contains_key(&view.id) {
            return Err(view);
        }
        self.views.insert(view.id, view);
        Ok(())
    }

    pub fn remove_view(&mut self, view_id: FlutterViewId) -> Option<FlutterView> {
        self.views.remove(&view_id)
    }

    pub fn contains(&self, view_id: FlutterViewId) -> bool {
        self.views.contains_key(&view_id)
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

#[cfg(test)]
mod tests {
    use std::ffi::{c_void, CStr};

    use dpi::PhysicalSize;

    use super::*;

    struct NoOpenGL;

    impl FlutterOpenGLHandler for NoOpenGL {
        fn present(&self) -> bool {
            false
        }

        fn make_current(&self) -> bool {
            false
        }

        fn clear_current(&self) -> bool {
            false
        }

        fn fbo_with_frame_info_callback(&self, _size: PhysicalSize<u32>) -> u32 {
            0
        }

        fn make_resource_current(&self) -> bool {
            false
        }

        fn gl_proc_resolver(&self, _proc: &CStr) -> *mut c_void {
            std::ptr::null_mut()
        }
    }

    fn view(id: FlutterViewId) -> FlutterView {
        FlutterView::new_without_compositor(id, NoOpenGL)
    }

    #[test]
    fn views_are_added_and_removed() {
        let mut registry = ViewRegistry::default();
        assert!(registry.add_view(view(IMPLICIT_VIEW_ID)).is_ok());
        assert!(registry.add_view(view(1)).is_ok());
        assert!(registry.contains(1));
        assert!(registry.implicit_view().is_some());

        assert_eq!(registry.remove_view(1).map(|view| view.id()), Some(1));
        assert!(!registry.contains(1));
        assert!(registry.contains(IMPLICIT_VIEW_ID));
    }

    #[test]
    fn a_view_is_added_only_once() {
        let mut registry = ViewRegistry::default();
        assert!(registry.add_view(view(1)).is_ok());
        let duplicate = registry.add_view(view(1));
        assert_eq!(duplicate.err().map(|view| view.id()), Some(1));
        assert!(registry.contains(1));
    }

    #[test]
    fn unknown_views_have_no_handlers() {
        let mut registry = ViewRegistry::default();
        assert!(registry.remove_view(1).is_none());
        assert!(registry.implicit_view_opengl_handler().is_none());
        assert!(registry.compositor_handler_for_view(1).is_none());
    }
}
