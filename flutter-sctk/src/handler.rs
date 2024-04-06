use std::{
    ffi::{c_void, CStr},
    num::NonZeroU32,
    sync::{Arc, Mutex, RwLock, Weak},
};

use dpi::PhysicalSize;
use flutter_engine::tasks::TaskRunnerHandler;
use flutter_engine_api::FlutterOpenGLHandler;
use flutter_glutin::context::{Context, ResourceContext};
use flutter_plugins::{
    mousecursor::{MouseCursorError, MouseCursorHandler, SystemMouseCursor},
    platform::{AppSwitcherDescription, MimeError, PlatformHandler},
};
use log::{error, warn};
use smithay_client_toolkit::{
    reexports::{calloop::LoopSignal, protocols::xdg::shell::client::xdg_toplevel::XdgToplevel},
    seat::pointer::{CursorIcon, PointerData, PointerDataExt, ThemedPointer},
};
use wayland_backend::client::ObjectId;
use wayland_client::{Connection, Proxy};

use crate::window::SctkFlutterWindowInner;

#[derive(Clone)]
pub(crate) struct SctkOpenGLHandler {
    window: Weak<SctkFlutterWindowInner>,
    context: Arc<Mutex<Context>>,
    resource_context: Arc<Mutex<ResourceContext>>,
    current_frame_size: Arc<RwLock<PhysicalSize<u32>>>,
}

impl SctkOpenGLHandler {
    pub(crate) fn new(
        window: Weak<SctkFlutterWindowInner>,
        context: Context,
        resource_context: ResourceContext,
    ) -> Self {
        Self {
            window,
            context: Arc::new(Mutex::new(context)),
            resource_context: Arc::new(Mutex::new(resource_context)),
            current_frame_size: Default::default(),
        }
    }

    // Note: This callback is executed on the *platform* thread.
    pub(crate) fn resize(&self, size: PhysicalSize<NonZeroU32>) {
        self.context.lock().unwrap().resize(size);
    }

    fn load_current_frame_size(&self) -> PhysicalSize<u32> {
        *self.current_frame_size.read().unwrap()
    }
}

// Note: These callbacks are executed on the *render* thread.
impl FlutterOpenGLHandler for SctkOpenGLHandler {
    fn present(&self) -> bool {
        let frame_size = self.load_current_frame_size();
        // Check if this frame can be presented. This resizes the surface if a
        // resize is pending and |frame_size| matches the target size.
        if !self
            .window
            .upgrade()
            .unwrap()
            .on_frame_generated(frame_size)
        {
            return false;
        }

        if !self.context.lock().unwrap().present() {
            return false;
        }

        self.window.upgrade().unwrap().on_frame_presented();

        true
    }

    fn make_current(&self) -> bool {
        self.context.lock().unwrap().make_current()
    }

    fn clear_current(&self) -> bool {
        self.context.lock().unwrap().make_not_current()
    }

    fn fbo_with_frame_info_callback(&self, size: PhysicalSize<u32>) -> u32 {
        let mut current_frame_size = self.current_frame_size.write().unwrap();
        *current_frame_size = size;

        0
    }

    fn make_resource_current(&self) -> bool {
        self.resource_context.lock().unwrap().make_current()
    }

    fn gl_proc_resolver(&self, proc: &CStr) -> *mut c_void {
        self.context.lock().unwrap().get_proc_address(proc) as _
    }
}

pub struct SctkPlatformTaskHandler {
    signal: LoopSignal,
}

impl SctkPlatformTaskHandler {
    pub fn new(signal: LoopSignal) -> Self {
        Self { signal }
    }
}

impl TaskRunnerHandler for SctkPlatformTaskHandler {
    fn wake(&self) {
        self.signal.wakeup();
    }
}

// TODO(multi-view): Add support for multi-view once the `flutter/platform`
// plugin supports it.
pub struct SctkPlatformHandler {
    implicit_xdg_toplevel: XdgToplevel,
}

impl SctkPlatformHandler {
    pub fn new(xdg_toplevel: XdgToplevel) -> Self {
        Self {
            implicit_xdg_toplevel: xdg_toplevel,
        }
    }
}

impl PlatformHandler for SctkPlatformHandler {
    fn set_application_switcher_description(&mut self, description: AppSwitcherDescription) {
        self.implicit_xdg_toplevel.set_title(description.label);
    }

    fn set_clipboard_data(&mut self, _text: String) {
        error!(
            "Attempting to set the contents of the clipboard, which hasn't yet been implemented \
             on this platform."
        );
    }

    fn get_clipboard_data(&mut self, _mime: &str) -> Result<String, MimeError> {
        error!(
            "Attempting to get the contents of the clipboard, which hasn't yet been implemented \
             on this platform."
        );
        Ok("".to_string())
    }
}

pub struct SctkMouseCursorHandler {
    conn: Connection,
    themed_pointer: Option<ThemedPointer>,
}

impl SctkMouseCursorHandler {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn,
            themed_pointer: None,
        }
    }

    pub(crate) fn set_themed_pointer(&mut self, themed_pointer: Option<ThemedPointer>) {
        self.themed_pointer = themed_pointer;
    }

    pub(crate) fn remove_themed_pointer_for_seat(&mut self, seat_id: ObjectId) {
        let themed_pointer_belongs_to_seat = self
            .themed_pointer
            .as_ref()
            .and_then(|themed_pointer| {
                themed_pointer
                    .pointer()
                    .data::<PointerData>()
                    .map(|data| data.pointer_data().seat().id() == seat_id)
            })
            .unwrap_or_default();

        if themed_pointer_belongs_to_seat {
            self.themed_pointer = None;
        }
    }
}

impl MouseCursorHandler for SctkMouseCursorHandler {
    fn activate_system_cursor(&mut self, kind: SystemMouseCursor) -> Result<(), MouseCursorError> {
        let Some(themed_pointer) = self.themed_pointer.as_ref() else {
            warn!("[plugin: mousecursor] Unable to update cursor: themed pointer is empty");
            return Err(MouseCursorError);
        };

        let cursor: SctkMouseCursor = kind.into();

        match cursor.icon {
            Some(icon) => themed_pointer
                .set_cursor(&self.conn, icon)
                .or(Err(MouseCursorError)),
            None => themed_pointer.hide_cursor().or(Err(MouseCursorError)),
        }
    }
}

struct SctkMouseCursor {
    icon: Option<CursorIcon>,
}

impl From<SystemMouseCursor> for SctkMouseCursor {
    fn from(kind: SystemMouseCursor) -> Self {
        let icon = match kind {
            SystemMouseCursor::Click => Some(CursorIcon::Pointer),
            SystemMouseCursor::Alias => Some(CursorIcon::Alias),
            SystemMouseCursor::AllScroll => Some(CursorIcon::Default),
            SystemMouseCursor::Basic => Some(CursorIcon::Default),
            SystemMouseCursor::Cell => Some(CursorIcon::Cell),
            SystemMouseCursor::ContextMenu => Some(CursorIcon::ContextMenu),
            SystemMouseCursor::Copy => Some(CursorIcon::Copy),
            SystemMouseCursor::Disappearing => Some(CursorIcon::Default), // fallback
            SystemMouseCursor::Forbidden => Some(CursorIcon::NotAllowed),
            SystemMouseCursor::Grab => Some(CursorIcon::Grab),
            SystemMouseCursor::Grabbing => Some(CursorIcon::Grabbing),
            SystemMouseCursor::Help => Some(CursorIcon::Help),
            SystemMouseCursor::Move => Some(CursorIcon::Move),
            SystemMouseCursor::NoDrop => Some(CursorIcon::NoDrop),
            SystemMouseCursor::None => None,
            SystemMouseCursor::Precise => Some(CursorIcon::Crosshair),
            SystemMouseCursor::Progress => Some(CursorIcon::Progress),
            SystemMouseCursor::ResizeColumn => Some(CursorIcon::ColResize),
            SystemMouseCursor::ResizeDown => Some(CursorIcon::SResize),
            SystemMouseCursor::ResizeDownLeft => Some(CursorIcon::SwResize),
            SystemMouseCursor::ResizeDownRight => Some(CursorIcon::SeResize),
            SystemMouseCursor::ResizeLeft => Some(CursorIcon::WResize),
            SystemMouseCursor::ResizeLeftRight => Some(CursorIcon::EwResize),
            SystemMouseCursor::ResizeRight => Some(CursorIcon::EResize),
            SystemMouseCursor::ResizeRow => Some(CursorIcon::RowResize),
            SystemMouseCursor::ResizeUp => Some(CursorIcon::NResize),
            SystemMouseCursor::ResizeUpDown => Some(CursorIcon::NsResize),
            SystemMouseCursor::ResizeUpLeft => Some(CursorIcon::NwResize),
            SystemMouseCursor::ResizeUpLeftDownRight => Some(CursorIcon::NwseResize),
            SystemMouseCursor::ResizeUpRight => Some(CursorIcon::NeResize),
            SystemMouseCursor::ResizeUpRightDownLeft => Some(CursorIcon::NeswResize),
            SystemMouseCursor::Text => Some(CursorIcon::Text),
            SystemMouseCursor::VerticalText => Some(CursorIcon::VerticalText),
            SystemMouseCursor::Wait => Some(CursorIcon::Wait),
            SystemMouseCursor::ZoomIn => Some(CursorIcon::ZoomIn),
            SystemMouseCursor::ZoomOut => Some(CursorIcon::ZoomOut),
        };

        Self { icon }
    }
}
