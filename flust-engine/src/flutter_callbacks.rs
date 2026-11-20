use crate::ffi::{
    FlutterBackingStore, FlutterBackingStoreConfig, FlutterFrameInfo, FlutterLayer,
    FlutterPresentViewInfo,
};
use crate::semantics::FlutterSemanticsUpdate;
use crate::tasks::{TaskRunner, TaskRunnerInner};
use crate::view::FlutterViewFocusChangeRequest;
use crate::{FlutterEngineInner, ViewBaton};
use core::slice;
use parking_lot::Mutex;
use std::ffi::{c_char, c_uint, c_void, CStr};
use tracing::{debug, trace, warn};

pub extern "C" fn present(user_data: *mut c_void) -> bool {
    trace!("present");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine.opengl_handler().unwrap().present()
    }
}

pub extern "C" fn make_current(user_data: *mut c_void) -> bool {
    trace!("make_current");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine.opengl_handler().unwrap().make_current()
    }
}

pub extern "C" fn clear_current(user_data: *mut c_void) -> bool {
    trace!("clear_current");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine.opengl_handler().unwrap().clear_current()
    }
}

pub extern "C" fn fbo_with_frame_info_callback(
    user_data: *mut c_void,
    frame_info: *const flust_engine_sys::FlutterFrameInfo,
) -> c_uint {
    trace!("fbo_with_frame_info_callback");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        let frame_info = FlutterFrameInfo::from(*frame_info);
        engine
            .opengl_handler()
            .unwrap()
            .fbo_with_frame_info_callback(frame_info.size)
    }
}

pub extern "C" fn make_resource_current(user_data: *mut c_void) -> bool {
    trace!("make_resource_current");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        engine.opengl_handler().unwrap().make_resource_current()
    }
}

pub extern "C" fn gl_proc_resolver(user_data: *mut c_void, proc: *const c_char) -> *mut c_void {
    trace!("gl_proc_resolver");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        let proc = CStr::from_ptr(proc);
        engine.opengl_handler().unwrap().gl_proc_resolver(proc)
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
            .lock()
            .request_frame_callback(baton);
    }
}

pub extern "C" fn compositor_backing_store_create_callback(
    config: *const flust_engine_sys::FlutterBackingStoreConfig,
    backing_store_out: *mut flust_engine_sys::FlutterBackingStore,
    user_data: *mut c_void,
) -> bool {
    trace!("compositor_backing_store_create_callback");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        let config = FlutterBackingStoreConfig::from(*config);
        let Some(handler) = engine.compositor_handler_for_view(config.view_id) else {
            debug!("no backing store for unknown view {}", config.view_id);
            return false;
        };
        if let Ok(backing_store) = handler.create_backing_store(config) {
            backing_store.into_ffi(&mut *backing_store_out);
            return true;
        };
        false
    }
}

pub extern "C" fn compositor_backing_store_collect_callback(
    backing_store: *const flust_engine_sys::FlutterBackingStore,
    user_data: *mut c_void,
) -> bool {
    trace!("compositor_backing_store_collect_callback");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        let backing_store = FlutterBackingStore::from(*backing_store);
        let view_id = backing_store.user_data.view_id;
        let Some(handler) = engine.compositor_handler_for_view(view_id) else {
            // A view's backing stores are collected before the engine is
            // done with it, and it is in the registry until then.
            warn!("backing store of unknown view {view_id} not collected");
            return true;
        };
        handler.collect_backing_store(backing_store).is_ok()
    }
}

pub extern "C" fn compositor_present_view_callback(
    info: *const flust_engine_sys::FlutterPresentViewInfo,
) -> bool {
    trace!("compositor_present_view_callback");
    unsafe {
        let info = *info;
        let engine = &*(info.user_data as *const FlutterEngineInner);

        let layers: Vec<FlutterLayer> = slice::from_raw_parts(*info.layers, info.layers_count)
            .iter()
            .map(|layer| (*layer).into())
            .collect();

        let info = FlutterPresentViewInfo::new(info.view_id, layers);

        // A view without a window (the implicit view, in an app that starts
        // without one) has nothing to show the frame in.
        let Some(handler) = engine.compositor_handler_for_view(info.view_id) else {
            debug!("not presenting unknown view {}", info.view_id);
            return true;
        };
        handler.present_view(info).is_ok()
    }
}

pub extern "C" fn platform_message_callback(
    platform_message: *const flust_engine_sys::FlutterPlatformMessage,
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

pub extern "C" fn update_semantics_callback(
    update: *const flust_engine_sys::FlutterSemanticsUpdate2,
    user_data: *mut c_void,
) {
    trace!("update_semantics_callback");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        // This callback is only registered when there is a handler.
        let Some(handler) = engine.semantics_handler.as_ref() else {
            return;
        };
        handler.update_semantics(FlutterSemanticsUpdate::from_ffi(update));
    }
}

/// Called from a task on the platform thread.
pub extern "C" fn view_focus_change_request_callback(
    request: *const flust_engine_sys::FlutterViewFocusChangeRequest,
    user_data: *mut c_void,
) {
    trace!("view_focus_change_request_callback");
    unsafe {
        let engine = &*(user_data as *const FlutterEngineInner);
        let request = &*request;
        let request = FlutterViewFocusChangeRequest {
            view_id: request.view_id,
            state: request.state.into(),
            direction: request.direction.into(),
        };
        match engine.view_focus_handler.as_ref() {
            Some(handler) => handler.request_view_focus_change(request),
            None => debug!("ignoring view focus change request: {request:?}"),
        }
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
    task: flust_engine_sys::FlutterTask,
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
    texture: *mut flust_engine_sys::FlutterOpenGLTexture,
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

/// The engine has tried to add a view: see `FlutterEngine::add_view`. Called
/// on a thread of the engine's.
pub extern "C" fn add_view_callback(result: *const flust_engine_sys::FlutterAddViewResult) {
    let (added, baton) = unsafe {
        let result = &*result;
        (
            result.added,
            Box::from_raw(result.user_data as *mut ViewBaton),
        )
    };
    trace!(
        "add_view_callback: view {}: added: {}",
        baton.view_id,
        added
    );

    let Some(engine) = baton.engine.upgrade() else {
        return;
    };
    engine.run_on_platform_thread(move |engine| {
        if !added {
            let view = engine
                .inner
                .view_registry
                .write()
                .remove_view(baton.view_id);
            // Not while the registry is locked.
            drop(view);
        }
        (baton.callback)(added);
    });
}

/// The engine has tried to remove a view: see `FlutterEngine::remove_view`.
/// Called on the raster thread, once the engine is done with the view.
pub extern "C" fn remove_view_callback(result: *const flust_engine_sys::FlutterRemoveViewResult) {
    let (removed, baton) = unsafe {
        let result = &*result;
        (
            result.removed,
            Box::from_raw(result.user_data as *mut ViewBaton),
        )
    };
    trace!(
        "remove_view_callback: view {}: removed: {}",
        baton.view_id,
        removed
    );

    let Some(engine) = baton.engine.upgrade() else {
        return;
    };
    // The view is dropped on the platform thread, where it was created.
    engine.run_on_platform_thread(move |engine| {
        if removed {
            let view = engine
                .inner
                .view_registry
                .write()
                .remove_view(baton.view_id);
            // Not while the registry is locked.
            drop(view);
        }
        (baton.callback)(removed);
    });
}
