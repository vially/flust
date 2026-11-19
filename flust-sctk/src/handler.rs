use std::{
    collections::HashMap,
    ffi::{c_void, CStr, CString},
    iter::zip,
    num::NonZeroU32,
    sync::{Arc, Mutex, RwLock, Weak},
    time::Duration,
};

use dpi::PhysicalSize;
use flust_accessibility::Accessibility;
use flust_engine::{
    compositor::{
        CompositorCollectBackingStoreError, CompositorCreateBackingStoreError,
        CompositorPresentError, FlutterCompositorHandler,
    },
    ffi::{
        FlutterBackingStore, FlutterBackingStoreConfig, FlutterBackingStoreDescription,
        FlutterKeyEventDeviceType, FlutterKeyEventType, FlutterLogicalKey,
        FlutterOpenGLBackingStore, FlutterOpenGLBackingStoreFramebuffer, FlutterOpenGLFramebuffer,
        FlutterOpenGLSurface, FlutterOpenGLTargetType, FlutterPhysicalKey, FlutterPresentViewInfo,
    },
    tasks::TaskRunnerHandler,
    FlutterVsyncHandler,
};
use flust_engine_api::FlutterOpenGLHandler;
use flust_engine_sys::FlutterEngineGetCurrentTime;
use flust_glutin::{
    context::{Context, ResourceContext},
    gl,
};
use flust_plugins::{
    accessibility::{AccessibilityHandler, Announcement, Assertiveness},
    keyboard::{KeyboardStateError, KeyboardStateHandler},
    mousecursor::{MouseCursorError, MouseCursorHandler, SystemMouseCursor},
    platform::{AppSwitcherDescription, MimeError, PlatformHandler},
    textinput::TextInputHandler,
};
use smithay_client_toolkit::{
    reexports::{calloop::LoopSignal, protocols::xdg::shell::client::xdg_toplevel::XdgToplevel},
    seat::{
        keyboard::{KeyEvent, Keysym, Modifiers},
        pointer::{CursorIcon, PointerData, PointerDataExt, ThemedPointer},
    },
};
use smithay_clipboard::Clipboard;
use thiserror::Error;
use tracing::{trace, warn};
use wayland_backend::client::ObjectId;
use wayland_client::{protocol::wl_display::WlDisplay, Connection, Proxy};

use crate::keyboard::{SctkKeyEvent, SctkLogicalKey, SctkPhysicalKey};

use crate::{egl::NonZeroU32PhysicalSize, frame_clock::FrameClock, window::SctkFlutterWindowInner};

const WINDOW_FRAMEBUFFER_ID: u32 = 0;

pub(crate) const FRAME_INTERVAL_60_HZ_IN_NANOS: u64 = 1_000_000_000 / 60; // 60Hz per second in nanos

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
        context: Arc<Mutex<Context>>,
        resource_context: Arc<Mutex<ResourceContext>>,
    ) -> Self {
        Self {
            window,
            context,
            resource_context,
            current_frame_size: Default::default(),
        }
    }

    fn load_current_frame_size(&self) -> PhysicalSize<u32> {
        *self.current_frame_size.read().unwrap()
    }
}

// Note: These callbacks are executed on the *render* thread.
impl FlutterOpenGLHandler for SctkOpenGLHandler {
    fn present(&self) -> bool {
        // The engine has drawn the frame into the window's framebuffer.
        let frame_size = self.load_current_frame_size();
        self.window
            .upgrade()
            .unwrap()
            .present_frame(Some(frame_size), || Ok(()))
            .is_ok()
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

#[derive(Clone)]
pub struct SctkCompositorHandler {
    window: Weak<SctkFlutterWindowInner>,
    opengl_compositor: SctkOpenGLCompositor,
}

impl SctkCompositorHandler {
    pub fn new(
        window: Weak<SctkFlutterWindowInner>,
        context: Arc<Mutex<Context>>,
        opengl_target_type: FlutterOpenGLTargetType,
    ) -> Self {
        let opengl_compositor =
            SctkOpenGLCompositor::new(window.clone(), context, opengl_target_type);

        Self {
            window,
            opengl_compositor,
        }
    }
}

impl FlutterCompositorHandler for SctkCompositorHandler {
    fn present_view(&self, info: FlutterPresentViewInfo) -> Result<(), CompositorPresentError> {
        let window = self.window.upgrade().unwrap();

        // TODO: Support compositing layers and platform views.
        debug_assert!(info.layers.len() <= 1);

        // A frame without content has no size.
        let frame_size = info.layers.first().map(|layer| {
            debug_assert!(layer.offset.x == 0.0 && layer.offset.y == 0.0);

            // TODO: Investigate if conversion to `u32` is correct
            PhysicalSize::<u32>::new(
                layer.size.width.round() as u32,
                layer.size.height.round() as u32,
            )
        });

        if !self.opengl_compositor.can_present(&window, frame_size) {
            trace!("frame {:?} was not drawn: not presenting it", frame_size);
            return Ok(());
        }

        window.present_frame(frame_size, || match frame_size {
            Some(_) => self.opengl_compositor.draw_opengl_view(&info),
            None => self.opengl_compositor.clear(),
        })?;

        self.opengl_compositor.frame_presented()
    }

    fn create_backing_store(
        &self,
        config: FlutterBackingStoreConfig,
    ) -> Result<FlutterBackingStore, CompositorCreateBackingStoreError> {
        let opengl_backing_store = self.opengl_compositor.create_opengl_backing_store(config)?;
        let description = FlutterBackingStoreDescription::OpenGL(opengl_backing_store);
        let backing_store = FlutterBackingStore::new(description, config.view_id);

        Ok(backing_store)
    }

    fn collect_backing_store(
        &self,
        mut backing_store: FlutterBackingStore,
    ) -> Result<(), CompositorCollectBackingStoreError> {
        backing_store.drop_raw_user_data();

        let FlutterBackingStoreDescription::OpenGL(opengl_backing_store) =
            backing_store.description
        else {
            return Err(CompositorCollectBackingStoreError::CollectFailed(
                "Only OpenGL backing stores are currently implemented".into(),
            ));
        };

        self.opengl_compositor
            .collect_opengl_backing_store(opengl_backing_store)
    }
}

/// The `draw_opengl_view` and `clear` callbacks are run by
/// `SctkFlutterWindowInner::present_frame`, with the window's context
/// current and its lock held. The window swaps the buffers afterwards.
trait SctkOpenGLCompositorHandler {
    /// Whether there is a frame of this size (`None`: without content) to
    /// present.
    fn can_present(
        &self,
        _window: &SctkFlutterWindowInner,
        _frame_size: Option<PhysicalSize<u32>>,
    ) -> bool {
        true
    }

    /// Puts the frame into the window's framebuffer.
    fn draw_opengl_view(&self, info: &FlutterPresentViewInfo)
        -> Result<(), CompositorPresentError>;

    fn create_opengl_backing_store(
        &self,
        config: FlutterBackingStoreConfig,
    ) -> Result<FlutterOpenGLBackingStore, CompositorCreateBackingStoreError>;

    fn collect_opengl_backing_store(
        &self,
        backing_store: FlutterOpenGLBackingStore,
    ) -> Result<(), CompositorCollectBackingStoreError>;

    /// Puts a frame without content into the window's framebuffer.
    fn clear(&self) -> Result<(), CompositorPresentError>;

    /// Called after a frame was presented or dropped.
    fn frame_presented(&self) -> Result<(), CompositorPresentError>;
}

#[derive(Clone)]
struct SctkOpenGLCompositorHandlerFramebuffer {
    gl: gl::Gl,
    format: u32,
}

impl SctkOpenGLCompositorHandlerFramebuffer {
    pub fn new(context: Arc<Mutex<Context>>) -> Self {
        context.lock().unwrap().make_current();

        let gl = gl::Gl::load_with(|symbol| {
            let proc = CString::new(symbol).unwrap();
            context.lock().unwrap().get_proc_address(proc.as_c_str())
        });

        context.lock().unwrap().make_not_current();

        Self {
            gl,
            format: Context::get_supported_format(),
        }
    }
}

impl SctkOpenGLCompositorHandler for SctkOpenGLCompositorHandlerFramebuffer {
    fn draw_opengl_view(
        &self,
        info: &FlutterPresentViewInfo,
    ) -> Result<(), CompositorPresentError> {
        let layer = info.layers.first().unwrap();
        let source_id = layer
            .content
            .get_opengl_backing_store_framebuffer_name()
            .ok_or(CompositorPresentError::PresentFailed(
                "Unable to retrieve framebuffer name from layer".into(),
            ))?;

        unsafe {
            // Disable the scissor test as it can affect blit operations.
            // Prevents regressions like: https://github.com/flutter/flutter/issues/140828
            // See OpenGL specification version 4.6, section 18.3.1.
            self.gl.Disable(gl::SCISSOR_TEST);

            self.gl.BindFramebuffer(gl::READ_FRAMEBUFFER, source_id);
            self.gl
                .BindFramebuffer(gl::DRAW_FRAMEBUFFER, WINDOW_FRAMEBUFFER_ID);

            let width = layer.size.width.round() as i32;
            let height = layer.size.height.round() as i32;

            // The window's framebuffer has the size of the frame: the window
            // resizes it to the frame it is about to present.
            self.gl.BlitFramebuffer(
                0,                    // srcX0
                0,                    // srcY0
                width,                // srcX1
                height,               // srcY1
                0,                    // dstX0
                0,                    // dstY0
                width,                // dstX1
                height,               // dstY1
                gl::COLOR_BUFFER_BIT, // mask
                gl::NEAREST,          // filter
            );
        }

        Ok(())
    }

    fn create_opengl_backing_store(
        &self,
        config: FlutterBackingStoreConfig,
    ) -> Result<FlutterOpenGLBackingStore, CompositorCreateBackingStoreError> {
        let mut user_data = FlutterOpenGLBackingStoreFramebuffer::new();
        unsafe {
            self.gl.GenTextures(1, &mut user_data.texture_id);
            self.gl.GenFramebuffers(1, &mut user_data.framebuffer_id);

            self.gl
                .BindFramebuffer(gl::FRAMEBUFFER, user_data.framebuffer_id);
            self.gl.BindTexture(gl::TEXTURE_2D, user_data.texture_id);
            self.gl.TexParameteri(
                gl::TEXTURE_2D,
                gl::TEXTURE_MIN_FILTER,
                gl::NEAREST.try_into().unwrap(),
            );
            self.gl.TexParameteri(
                gl::TEXTURE_2D,
                gl::TEXTURE_MAG_FILTER,
                gl::NEAREST.try_into().unwrap(),
            );
            self.gl.TexParameteri(
                gl::TEXTURE_2D,
                gl::TEXTURE_WRAP_S,
                gl::CLAMP_TO_EDGE.try_into().unwrap(),
            );
            self.gl.TexParameteri(
                gl::TEXTURE_2D,
                gl::TEXTURE_WRAP_T,
                gl::CLAMP_TO_EDGE.try_into().unwrap(),
            );
            self.gl.TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA8.try_into().unwrap(),
                config.size.width.round() as i32,
                config.size.height.round() as i32,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            self.gl.BindTexture(gl::TEXTURE_2D, 0);
            self.gl.FramebufferTexture2D(
                gl::FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                gl::TEXTURE_2D,
                user_data.texture_id,
                0,
            );
        };

        let framebuffer = FlutterOpenGLFramebuffer::new(self.format, user_data);

        Ok(FlutterOpenGLBackingStore::Framebuffer(framebuffer))
    }

    fn collect_opengl_backing_store(
        &self,
        backing_store: FlutterOpenGLBackingStore,
    ) -> Result<(), CompositorCollectBackingStoreError> {
        let FlutterOpenGLBackingStore::Framebuffer(mut framebuffer) = backing_store else {
            return Err(CompositorCollectBackingStoreError::CollectFailed(
                "Unexpected OpenGL backing store type received in collect callback for framebuffer type"
                    .into(),
            ));
        };

        unsafe {
            self.gl
                .DeleteFramebuffers(1, &framebuffer.user_data.framebuffer_id);
            self.gl.DeleteTextures(1, &framebuffer.user_data.texture_id);
        }

        framebuffer.drop_raw_user_data();

        Ok(())
    }

    fn clear(&self) -> Result<(), CompositorPresentError> {
        unsafe {
            self.gl
                .BindFramebuffer(gl::DRAW_FRAMEBUFFER, WINDOW_FRAMEBUFFER_ID);
            self.gl.ClearColor(0.0, 0.0, 0.0, 0.0);
            self.gl
                .Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT | gl::STENCIL_BUFFER_BIT);
        };

        Ok(())
    }

    fn frame_presented(&self) -> Result<(), CompositorPresentError> {
        Ok(())
    }
}

#[derive(Clone)]
struct SctkOpenGLCompositorHandlerSurface {
    window: Weak<SctkFlutterWindowInner>,
}

impl SctkOpenGLCompositorHandlerSurface {
    pub fn new(window: Weak<SctkFlutterWindowInner>) -> Self {
        Self { window }
    }
}

impl SctkOpenGLCompositorHandler for SctkOpenGLCompositorHandlerSurface {
    fn can_present(
        &self,
        window: &SctkFlutterWindowInner,
        frame_size: Option<PhysicalSize<u32>>,
    ) -> bool {
        // Only what the engine has drawn into the window's framebuffer: a
        // frame that `make_current_callback` turned down is not in it, and
        // neither is a frame without content.
        let drawn = window.take_surface_frame();
        drawn.is_some() && drawn == frame_size
    }

    fn draw_opengl_view(
        &self,
        _info: &FlutterPresentViewInfo,
    ) -> Result<(), CompositorPresentError> {
        // The engine has drawn the frame into the window's framebuffer.
        Ok(())
    }

    fn create_opengl_backing_store(
        &self,
        config: FlutterBackingStoreConfig,
    ) -> Result<FlutterOpenGLBackingStore, CompositorCreateBackingStoreError> {
        trace!(
            "create opengl backing store: {}x{}",
            config.size.width,
            config.size.height
        );
        let size = PhysicalSize::<u32>::new(
            config.size.width.round() as u32,
            config.size.height.round() as u32,
        )
        .non_zero()
        .ok_or(CompositorCreateBackingStoreError::CreateFailed(
            "Backing store without a size".into(),
        ))?;

        let target = Box::new(SurfaceTarget {
            window: self.window.clone(),
            size,
        });
        let user_data = Box::into_raw(target) as *mut std::ffi::c_void;

        let surface = FlutterOpenGLSurface {
            user_data,
            make_current_callback,
            clear_current_callback,
            destruction_callback,
            format: Context::get_supported_format(),
        };

        Ok(FlutterOpenGLBackingStore::Surface(surface))
    }

    fn collect_opengl_backing_store(
        &self,
        backing_store: FlutterOpenGLBackingStore,
    ) -> Result<(), CompositorCollectBackingStoreError> {
        trace!("collect opengl backing store");

        let FlutterOpenGLBackingStore::Surface(surface) = backing_store else {
            return Err(CompositorCollectBackingStoreError::CollectFailed(
                "Unexpected OpenGL backing store type received in collect callback for surface type"
                    .into(),
            ));
        };

        unsafe { drop(Box::from_raw(surface.user_data as *mut SurfaceTarget)) };

        Ok(())
    }

    fn clear(&self) -> Result<(), CompositorPresentError> {
        Ok(()) // no-op
    }

    fn frame_presented(&self) -> Result<(), CompositorPresentError> {
        Ok(())
    }
}

/// User data of a backing store that is the window's surface itself: what
/// the engine draws into it goes to the window's framebuffer.
struct SurfaceTarget {
    window: Weak<SctkFlutterWindowInner>,
    /// The size of the frames the engine draws into this backing store.
    size: PhysicalSize<NonZeroU32>,
}

/// Called before the engine draws a frame into the window's surface.
/// Returning false cancels the frame.
pub extern "C" fn make_current_callback(
    user_data: *mut c_void,
    _opengl_state_changed: *mut bool,
) -> bool {
    let target = unsafe { &*(user_data as *const SurfaceTarget) };
    target
        .window
        .upgrade()
        .is_some_and(|window| window.begin_surface_frame(target.size))
}

/// Called when the engine is done drawing into the window's surface.
pub extern "C" fn clear_current_callback(
    _user_data: *mut c_void,
    _opengl_state_changed: *mut bool,
) -> bool {
    // The context stays current: making it current again would have Mesa
    // pick the buffer for the next frame before the size of that frame is
    // known. See `SctkFlutterWindowInner::present_frame`.
    true
}

pub extern "C" fn destruction_callback(_user_data: *mut c_void) {
    trace!("destruction_callback");
}

#[derive(Clone)]
enum SctkOpenGLCompositor {
    Framebuffer(Box<SctkOpenGLCompositorHandlerFramebuffer>),
    Surface(SctkOpenGLCompositorHandlerSurface),
}

impl SctkOpenGLCompositor {
    pub fn new(
        window: Weak<SctkFlutterWindowInner>,
        context: Arc<Mutex<Context>>,
        opengl_target_type: FlutterOpenGLTargetType,
    ) -> Self {
        match opengl_target_type {
            FlutterOpenGLTargetType::Framebuffer => Self::Framebuffer(Box::new(
                SctkOpenGLCompositorHandlerFramebuffer::new(context),
            )),
            FlutterOpenGLTargetType::Texture => unimplemented!(
                "`FlutterOpenGLTargetType::Texture` is not currently implemented for SCTK backend"
            ),
            FlutterOpenGLTargetType::Surface => {
                Self::Surface(SctkOpenGLCompositorHandlerSurface::new(window))
            }
        }
    }
}

impl SctkOpenGLCompositorHandler for SctkOpenGLCompositor {
    fn can_present(
        &self,
        window: &SctkFlutterWindowInner,
        frame_size: Option<PhysicalSize<u32>>,
    ) -> bool {
        match self {
            SctkOpenGLCompositor::Framebuffer(handler) => handler.can_present(window, frame_size),
            SctkOpenGLCompositor::Surface(handler) => handler.can_present(window, frame_size),
        }
    }

    fn draw_opengl_view(
        &self,
        info: &FlutterPresentViewInfo,
    ) -> Result<(), CompositorPresentError> {
        match self {
            SctkOpenGLCompositor::Framebuffer(handler) => handler.draw_opengl_view(info),
            SctkOpenGLCompositor::Surface(handler) => handler.draw_opengl_view(info),
        }
    }

    fn create_opengl_backing_store(
        &self,
        config: FlutterBackingStoreConfig,
    ) -> Result<FlutterOpenGLBackingStore, CompositorCreateBackingStoreError> {
        match self {
            SctkOpenGLCompositor::Framebuffer(handler) => {
                handler.create_opengl_backing_store(config)
            }
            SctkOpenGLCompositor::Surface(handler) => handler.create_opengl_backing_store(config),
        }
    }

    fn collect_opengl_backing_store(
        &self,
        backing_store: FlutterOpenGLBackingStore,
    ) -> Result<(), CompositorCollectBackingStoreError> {
        match self {
            SctkOpenGLCompositor::Framebuffer(handler) => {
                handler.collect_opengl_backing_store(backing_store)
            }
            SctkOpenGLCompositor::Surface(handler) => {
                handler.collect_opengl_backing_store(backing_store)
            }
        }
    }

    fn clear(&self) -> Result<(), CompositorPresentError> {
        match self {
            SctkOpenGLCompositor::Framebuffer(handler) => handler.clear(),
            SctkOpenGLCompositor::Surface(handler) => handler.clear(),
        }
    }

    fn frame_presented(&self) -> Result<(), CompositorPresentError> {
        match self {
            SctkOpenGLCompositor::Framebuffer(handler) => handler.frame_presented(),
            SctkOpenGLCompositor::Surface(handler) => handler.frame_presented(),
        }
    }
}

// TODO(multi-view): Add support for multi-view vsync once it is supported
// upstream:
// https://github.com/flutter/flutter/issues/142845#issuecomment-1955345110
pub struct SctkVsyncHandler {
    frame_clock: FrameClock,
}

impl SctkVsyncHandler {
    pub(crate) fn new(frame_clock: FrameClock) -> Self {
        Self { frame_clock }
    }
}

impl FlutterVsyncHandler for SctkVsyncHandler {
    // Note: This callback is executed on an internal engine-managed thread.
    fn request_frame_callback(&self, baton: isize) {
        trace!("[baton: {}] vsync requested", baton);
        self.frame_clock.request_vsync(baton);
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
    clipboard: Clipboard,
}

impl SctkPlatformHandler {
    /// # Safety
    ///
    /// `display` must be a valid `*mut wl_display` pointer, and it must remain
    /// valid for as long as `Clipboard` object is alive.
    pub unsafe fn new(display: WlDisplay, xdg_toplevel: XdgToplevel) -> Self {
        Self {
            implicit_xdg_toplevel: xdg_toplevel,
            clipboard: Clipboard::new(display.id().as_ptr() as *mut _),
        }
    }
}

impl PlatformHandler for SctkPlatformHandler {
    fn set_application_switcher_description(&mut self, description: AppSwitcherDescription) {
        self.implicit_xdg_toplevel.set_title(description.label);
    }

    fn set_clipboard_data(&mut self, text: String) {
        // TODO: Is updating *both* clipboards a reasonable thing to do here?
        self.clipboard.store(text.clone());
        self.clipboard.store_primary(text);
    }

    fn get_clipboard_data(&mut self, _mime: &str) -> Result<String, MimeError> {
        self.clipboard.load().map_err(|_| MimeError {})
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

pub struct SctkAccessibilityHandler {
    accessibility: Accessibility,
}

impl SctkAccessibilityHandler {
    pub fn new(accessibility: Accessibility) -> Self {
        Self { accessibility }
    }
}

impl AccessibilityHandler for SctkAccessibilityHandler {
    fn announce(&mut self, announcement: Announcement) {
        self.accessibility.announce(
            announcement.view_id,
            &announcement.message,
            announcement.assertiveness == Assertiveness::Assertive,
        );
    }
}

#[derive(Default)]
pub struct SctkTextInputHandler {}

impl SctkTextInputHandler {
    pub fn new() -> Self {
        Default::default()
    }
}

impl TextInputHandler for SctkTextInputHandler {
    fn show(&mut self) {}

    fn hide(&mut self) {}
}

#[derive(Error, Debug)]
pub enum SctkPressedStateError {
    #[error("Inconsistent pressed state")]
    InconsistentState,
}

#[derive(Default)]
pub struct SctkKeyboardHandler {
    pressed_state: HashMap<FlutterPhysicalKey, KeyEvent>,
}

impl SctkKeyboardHandler {
    pub(crate) fn new() -> Self {
        Default::default()
    }

    pub(crate) fn press_key(&mut self, event: KeyEvent) -> Result<(), SctkPressedStateError> {
        let physical = SctkPhysicalKey::new(event.raw_code);

        match self.pressed_state.insert(physical.into(), event) {
            Some(_) => Err(SctkPressedStateError::InconsistentState),
            None => Ok(()),
        }
    }

    pub(crate) fn release_key(
        &mut self,
        event: &KeyEvent,
    ) -> Result<Keysym, SctkPressedStateError> {
        let physical = SctkPhysicalKey::new(event.raw_code);

        match self.pressed_state.remove(&physical.into()) {
            Some(event) => Ok(event.keysym),
            None => Err(SctkPressedStateError::InconsistentState),
        }
    }

    pub(crate) fn sync_keyboard_enter_state(
        &mut self,
        raw: &[u32],
        keysyms: &[Keysym],
    ) -> Vec<SctkKeyEvent> {
        let current_time = unsafe { FlutterEngineGetCurrentTime() };
        let time = Duration::from_nanos(current_time).as_millis() as u32;

        let pressed_keys: Vec<_> = zip(raw, keysyms)
            .map(|(&raw_code, &keysym)| KeyEvent {
                raw_code,
                keysym,
                utf8: None,
                time,
            })
            .collect();

        // Extraneous events from `pressed_state` need to be synthesized "up"
        let mut to_be_released: Vec<SctkKeyEvent> = Vec::new();
        self.pressed_state.retain(|_, event| {
            let retain = pressed_keys
                .iter()
                .any(|pressed_key| pressed_key.raw_code == event.raw_code);
            if !retain {
                to_be_released.push(SctkKeyEvent::new(
                    FlutterKeyEventDeviceType::Keyboard,
                    event.clone(),
                    FlutterKeyEventType::Up,
                    Some(event.keysym),
                    Modifiers::default(), // Unused for synthesized events
                    true,
                ));
            }

            retain
        });

        // Missing events from `pressed_state` need to be synthesized "down"
        let to_be_pressed: Vec<_> = pressed_keys
            .iter()
            .filter_map(|event| {
                if self
                    .pressed_state
                    .insert(SctkPhysicalKey::new(event.raw_code).into(), event.clone())
                    .is_some()
                {
                    return None;
                }

                Some(SctkKeyEvent::new(
                    FlutterKeyEventDeviceType::Keyboard,
                    event.clone(),
                    FlutterKeyEventType::Down,
                    None,
                    Modifiers::default(), // Unused for synthesized events
                    true,
                ))
            })
            .collect();

        [to_be_pressed, to_be_released].concat()
    }
}

impl KeyboardStateHandler for SctkKeyboardHandler {
    fn get_keyboard_state(
        &self,
    ) -> Result<HashMap<FlutterPhysicalKey, FlutterLogicalKey>, KeyboardStateError> {
        let state: HashMap<FlutterPhysicalKey, FlutterLogicalKey> = self
            .pressed_state
            .iter()
            .map(|(physical_key, event)| {
                (
                    physical_key.clone(),
                    SctkLogicalKey::new(event.keysym).into(),
                )
            })
            .collect();

        Ok(state)
    }
}

pub type SctkAsyncResult = Result<(), SctkAsyncError>;

#[derive(Error, Debug)]
pub enum SctkAsyncError {
    #[error(transparent)]
    AshpdError(#[from] ashpd::Error),
}
