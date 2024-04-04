use std::ffi::{c_void, CStr};

use dpi::PhysicalSize;

pub trait FlutterOpenGLHandler {
    fn present(&self) -> bool;

    fn make_current(&self) -> bool;

    fn clear_current(&self) -> bool;

    fn fbo_with_frame_info_callback(&self, size: PhysicalSize<u32>) -> u32;

    fn make_resource_current(&self) -> bool;

    fn gl_proc_resolver(&self, proc: &CStr) -> *mut c_void;
}
