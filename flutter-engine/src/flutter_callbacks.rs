use crate::tasks::{TaskRunner, TaskRunnerInner};
use crate::FlutterEngineInner;
use log::trace;
use parking_lot::Mutex;
use std::ffi::{c_char, c_uint, c_void, CStr};

pub extern "C" fn present(user_data: *mut c_void) -> bool {
    trace!("present");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine
            .implicit_view_opengl_handler()
            .unwrap()
            .swap_buffers()
    }
}

pub extern "C" fn make_current(user_data: *mut c_void) -> bool {
    trace!("make_current");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine
            .implicit_view_opengl_handler()
            .unwrap()
            .make_current()
    }
}

pub extern "C" fn clear_current(user_data: *mut c_void) -> bool {
    trace!("clear_current");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine
            .implicit_view_opengl_handler()
            .unwrap()
            .clear_current()
    }
}

pub extern "C" fn fbo_callback(user_data: *mut c_void) -> c_uint {
    trace!("fbo_callback");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine
            .implicit_view_opengl_handler()
            .unwrap()
            .fbo_callback()
    }
}

pub extern "C" fn make_resource_current(user_data: *mut c_void) -> bool {
    trace!("make_resource_current");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine
            .implicit_view_opengl_handler()
            .unwrap()
            .make_resource_current()
    }
}

pub extern "C" fn gl_proc_resolver(user_data: *mut c_void, proc: *const c_char) -> *mut c_void {
    trace!("gl_proc_resolver");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        let proc = CStr::from_ptr(proc);
        engine
            .implicit_view_opengl_handler()
            .unwrap()
            .gl_proc_resolver(proc)
    }
}

pub extern "C" fn vsync_callback(user_data: *mut c_void, baton: isize) {
    trace!("vsync_callback");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        // `vsync_callback` will only be called when `vsync_handler` is not empty,
        // so using `unwrap()` should be safe in here.
        engine
            .vsync_handler
            .as_ref()
            .unwrap()
            .request_frame_callback(baton);
    }
}

pub extern "C" fn platform_message_callback(
    platform_message: *const flutter_engine_sys::FlutterPlatformMessage,
    user_data: *mut c_void,
) {
    trace!("platform_message_callback");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine
            .channel_registry
            .read()
            .handle((*platform_message).into());
    }
}

pub extern "C" fn root_isolate_create_callback(_user_data: *mut c_void) {
    trace!("root_isolate_create_callback");
    // // This callback is executed on the main thread
    // unsafe {
    //     let user_data = &mut *(user_data as *mut DesktopUserData);
    //     if let DesktopUserData::WindowState(window_state) = user_data {
    //         window_state.set_isolate_created();
    //     }
    // }
}

pub extern "C" fn runs_task_on_current_thread(user_data: *mut c_void) -> bool {
    trace!("runs_task_on_current_thread");
    unsafe {
        let inner = &*(user_data as *const Mutex<TaskRunnerInner>);
        inner.lock().runs_task_on_current_thread()
    }
}

pub extern "C" fn post_task(
    task: flutter_engine_sys::FlutterTask,
    target_time_nanos: u64,
    user_data: *mut c_void,
) {
    trace!("post_task");
    unsafe {
        let inner = &*(user_data as *const Mutex<TaskRunnerInner>);
        let mut inner = inner.lock();
        TaskRunner::post_task(&mut inner, task, target_time_nanos);
    }
}

pub extern "C" fn gl_external_texture_frame(
    user_data: *mut c_void,
    texture_id: i64,
    width: usize,
    height: usize,
    texture: *mut flutter_engine_sys::FlutterOpenGLTexture,
) -> bool {
    trace!("gl_external_texture_frame");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        if let Some(frame) = engine
            .texture_registry
            .get_texture_frame(texture_id, (width, height))
        {
            frame.into_ffi(&mut *texture);
            return true;
        }
        false
    }
}
