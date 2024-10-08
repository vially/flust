use std::{
    ffi::{c_void, CStr},
    sync::{Arc, Mutex},
};

use dpi::PhysicalSize;
use flust_engine_api::FlutterOpenGLHandler;

use crate::context::{Context, ResourceContext};

pub struct GlutinOpenGLHandler {
    context: Arc<Mutex<Context>>,
    resource_context: Arc<Mutex<ResourceContext>>,
}

impl GlutinOpenGLHandler {
    pub fn new(
        context: Arc<Mutex<Context>>,
        resource_context: Arc<Mutex<ResourceContext>>,
    ) -> Self {
        Self {
            context,
            resource_context,
        }
    }
}

impl FlutterOpenGLHandler for GlutinOpenGLHandler {
    fn present(&self) -> bool {
        self.context.lock().unwrap().present()
    }

    fn make_current(&self) -> bool {
        self.context.lock().unwrap().make_current()
    }

    fn clear_current(&self) -> bool {
        self.context.lock().unwrap().make_not_current()
    }

    fn fbo_with_frame_info_callback(&self, _size: PhysicalSize<u32>) -> u32 {
        0
    }

    fn make_resource_current(&self) -> bool {
        self.resource_context.lock().unwrap().make_current()
    }

    fn gl_proc_resolver(&self, proc: &CStr) -> *mut c_void {
        self.context.lock().unwrap().get_proc_address(proc) as _
    }
}
