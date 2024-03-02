use std::ffi::{c_void, CStr};

pub trait FlutterOpenGLHandler {
    fn swap_buffers(&self) -> bool;

    fn make_current(&self) -> bool;

    fn clear_current(&self) -> bool;

    fn fbo_callback(&self) -> u32;

    fn make_resource_current(&self) -> bool;

    fn gl_proc_resolver(&self, proc: &CStr) -> *mut c_void;
}
