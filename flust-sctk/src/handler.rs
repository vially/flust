use std::{
    collections::HashMap,
    ffi::{c_void, CStr, CString},
    iter::zip,
    num::NonZeroU32,
    sync::{
        atomic::{AtomicBool, AtomicIsize, Ordering},
        Arc, Mutex, RwLock, Weak,
    },
    time::Duration,
};

use ashpd::desktop::settings::{ColorScheme, Settings};
use dpi::PhysicalSize;
use flust_engine::{
    compositor::{
        CompositorCollectBackingStoreError, CompositorCreateBackingStoreError,
        CompositorPresentError, FlutterCompositorHandler,
    },
    ffi::{
        FlutterBackingStore, FlutterBackingStoreConfig, FlutterBackingStoreDescription,
        FlutterKeyEventDeviceType, FlutterKeyEventType, FlutterLogicalKey,
        FlutterOpenGLBackingStore, FlutterOpenGLBackingStoreFramebuffer, FlutterOpenGLFramebuffer,
        FlutterOpenGLTargetType, FlutterPhysicalKey, FlutterPresentViewInfo,
    },
    tasks::TaskRunnerHandler,
    FlutterEngineWeakRef, FlutterVsyncHandler,
};
use flust_engine_api::FlutterOpenGLHandler;
use flust_engine_sys::FlutterEngineGetCurrentTime;
use flust_glutin::{
    context::{Context, ResourceContext},
    gl,
};
use flust_plugins::{
    keyboard::{KeyboardStateError, KeyboardStateHandler},
    mousecursor::{MouseCursorError, MouseCursorHandler, SystemMouseCursor},
    platform::{AppSwitcherDescription, MimeError, PlatformHandler},
    settings::{PlatformBrightness, SettingsPlugin},
    textinput::TextInputHandler,
};
use futures_lite::StreamExt;
use smithay_client_toolkit::{
    reexports::{calloop::LoopSignal, protocols::xdg::shell::client::xdg_toplevel::XdgToplevel},
    seat::{
        keyboard::{KeyEvent, Keysym, Modifiers},
        pointer::{CursorIcon, PointerData, PointerDataExt, ThemedPointer},
    },
};
use smithay_clipboard::Clipboard;
use thiserror::Error;
use tracing::{error, trace, warn};
use wayland_backend::client::ObjectId;
use wayland_client::{
    protocol::{wl_display::WlDisplay, wl_surface::WlSurface},
    Connection, Proxy, QueueHandle,
};

use crate::{
    application::SctkApplicationState,
    keyboard::{SctkKeyEvent, SctkLogicalKey, SctkPhysicalKey},
};

use crate::window::SctkFlutterWindowInner;

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
        let opengl_compositor = SctkOpenGLCompositor::new(context, opengl_target_type);

        Self {
            window,
            opengl_compositor,
        }
    }

    fn clear(&self) -> Result<(), CompositorPresentError> {
        let window = self.window.upgrade().unwrap();

        if !window.on_empty_frame_generated() {
            return Err(CompositorPresentError::PresentFailed(
                "Empty frame generated callback failed".into(),
            ));
        }

        self.opengl_compositor.clear()?;

        window.on_frame_presented();
        Ok(())
    }
}

impl FlutterCompositorHandler for SctkCompositorHandler {
    fn present_view(&self, info: FlutterPresentViewInfo) -> Result<(), CompositorPresentError> {
        if info.layers.is_empty() {
            return self.clear();
        }

        // TODO: Support compositing layers and platform views.
        debug_assert_eq!(info.layers.len(), 1);
        let layer = info.layers.first().unwrap();
        debug_assert!(layer.offset.x == 0.0 && layer.offset.y == 0.0);

        // TODO: Investigate if conversion to `u32` is correct
        let frame_size = PhysicalSize::<u32>::new(
            layer.size.width.round() as u32,
            layer.size.height.round() as u32,
        );

        let window = self.window.upgrade().unwrap();

        if !window.on_frame_generated(frame_size) {
            return Err(CompositorPresentError::PresentFailed(
                "Frame generated callback failed".into(),
            ));
        }

        self.opengl_compositor.present_opengl_view(info)?;

        window.on_frame_presented();
        Ok(())
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

trait SctkOpenGLCompositorHandler {
    fn present_opengl_view(
        &self,
        info: FlutterPresentViewInfo,
    ) -> Result<(), CompositorPresentError>;

    fn create_opengl_backing_store(
        &self,
        config: FlutterBackingStoreConfig,
    ) -> Result<FlutterOpenGLBackingStore, CompositorCreateBackingStoreError>;

    fn collect_opengl_backing_store(
        &self,
        backing_store: FlutterOpenGLBackingStore,
    ) -> Result<(), CompositorCollectBackingStoreError>;

    fn clear(&self) -> Result<(), CompositorPresentError>;
}

#[derive(Clone)]
struct SctkOpenGLCompositorHandlerFramebuffer {
    context: Arc<Mutex<Context>>,
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
            context,
            gl,
            // TODO: Use similar logic for detecting supported formats as the
            // Windows embedder:
            // https://github.com/flutter/engine/blob/a6acfa4/shell/platform/windows/compositor_opengl.cc#L23-L34
            format: gl::RGBA8,
        }
    }
}

impl SctkOpenGLCompositorHandler for SctkOpenGLCompositorHandlerFramebuffer {
    fn present_opengl_view(
        &self,
        info: FlutterPresentViewInfo,
    ) -> Result<(), CompositorPresentError> {
        let layer = info.layers.first().unwrap();
        let source_id = layer
            .content
            .get_opengl_backing_store_framebuffer_name()
            .ok_or(CompositorPresentError::PresentFailed(
                "Unable to retrieve framebuffer name from layer".into(),
            ))?;

        if !self.context.lock().unwrap().make_current() {
            return Err(CompositorPresentError::PresentFailed(
                "Unable to make context current".into(),
            ));
        }

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

        if !self.context.lock().unwrap().present() {
            return Err(CompositorPresentError::PresentFailed(
                "Present failed".into(),
            ));
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
        if !self.context.lock().unwrap().make_current() {
            return Err(CompositorPresentError::PresentFailed(
                "Unable to make context current".into(),
            ));
        }

        unsafe {
            self.gl.ClearColor(0.0, 0.0, 0.0, 0.0);
            self.gl
                .Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT | gl::STENCIL_BUFFER_BIT);
        };

        if !self.context.lock().unwrap().present() {
            return Err(CompositorPresentError::PresentFailed(
                "Present failed".into(),
            ));
        }

        Ok(())
    }
}

#[derive(Clone)]
enum SctkOpenGLCompositor {
    Framebuffer(SctkOpenGLCompositorHandlerFramebuffer),
}

impl SctkOpenGLCompositor {
    pub fn new(context: Arc<Mutex<Context>>, opengl_target_type: FlutterOpenGLTargetType) -> Self {
        match opengl_target_type {
            FlutterOpenGLTargetType::Framebuffer => {
                Self::Framebuffer(SctkOpenGLCompositorHandlerFramebuffer::new(context))
            }
            FlutterOpenGLTargetType::Texture => unimplemented!(
                "`FlutterOpenGLTargetType::Texture` is not currently implemented for SCTK backend"
            ),
            FlutterOpenGLTargetType::Surface => unimplemented!(
                "`FlutterOpenGLTargetType::Surface` is not currently implemented for SCTK backend"
            ),
        }
    }
}

impl SctkOpenGLCompositorHandler for SctkOpenGLCompositor {
    fn present_opengl_view(
        &self,
        info: FlutterPresentViewInfo,
    ) -> Result<(), CompositorPresentError> {
        match self {
            SctkOpenGLCompositor::Framebuffer(handler) => handler.present_opengl_view(info),
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
        }
    }

    fn clear(&self) -> Result<(), CompositorPresentError> {
        match self {
            SctkOpenGLCompositor::Framebuffer(handler) => handler.clear(),
        }
    }
}

// TODO(multi-view): Add support for multi-view vsync once it is supported
// upstream:
// https://github.com/flutter/flutter/issues/142845#issuecomment-1955345110
pub struct SctkVsyncHandler {
    qh: QueueHandle<SctkApplicationState>,
    engine: FlutterEngineWeakRef,
    implicit_window_surface: Option<WlSurface>,
    pending_baton: AtomicIsize,
    can_schedule_frames: AtomicBool,
}

impl SctkVsyncHandler {
    pub(crate) fn new(qh: QueueHandle<SctkApplicationState>) -> Self {
        Self {
            qh,
            engine: Default::default(),
            implicit_window_surface: Default::default(),
            pending_baton: Default::default(),
            can_schedule_frames: Default::default(),
        }
    }

    pub(crate) fn init(&mut self, engine: FlutterEngineWeakRef, surface: WlSurface) {
        if self.engine.upgrade().is_some() {
            error!("Vsync handler engine was already initialized");
        }
        self.engine = engine;

        if self.implicit_window_surface.is_some() {
            error!("Vsync handler surface was already initialized");
        }
        self.implicit_window_surface = Some(surface)
    }

    pub(crate) fn load_pending_baton(&mut self) -> isize {
        self.pending_baton.load(Ordering::Relaxed)
    }

    pub(crate) fn notify_present(&self) {
        self.can_schedule_frames.store(true, Ordering::Relaxed);
    }
}

impl FlutterVsyncHandler for SctkVsyncHandler {
    // Note: This callback is executed on an internal engine-managed thread.
    fn request_frame_callback(&self, baton: isize) {
        trace!("[baton: {}] requesting frame callback", baton);

        self.pending_baton.store(baton, Ordering::Relaxed);

        let Some(engine) = self.engine.upgrade() else {
            error!("Engine upgrade failed while requesting frame callback");
            return;
        };

        // Note: Frame callbacks do not fire for unmapped surfaces on Wayland.
        // Therefore, pass back the `baton` to `FlutterEngineOnVsync` directly
        // until the surface is mapped (e.g.: until the first `present()`).
        let can_schedule_frames = self.can_schedule_frames.load(Ordering::Relaxed);
        if !can_schedule_frames {
            engine.run_on_platform_thread(move |engine| {
                // Once the surface is mapped, the `wl_output`'s refresh rate
                // will be used for determining the frame interval. But until
                // then, 60hz seems like a reasonable default.
                let (frame_start_time_nanos, frame_target_time_nanos) =
                    get_flutter_frame_time_nanos(FRAME_INTERVAL_60_HZ_IN_NANOS);
                engine.on_vsync(baton, frame_start_time_nanos, frame_target_time_nanos);
            });
            return;
        }

        let Some(surface) = self.implicit_window_surface.clone() else {
            error!("Missing window surface while requesting frame callback");
            return;
        };

        let qh = self.qh.clone();

        engine.run_on_platform_thread(move |_engine| {
            surface.frame(&qh, surface.clone());
            surface.commit();
        });
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

pub(crate) fn get_flutter_frame_time_nanos(frame_interval: u64) -> (u64, u64) {
    let current_time = unsafe { FlutterEngineGetCurrentTime() };
    let frame_start_time_nanos = current_time;
    let frame_target_time_nanos = frame_start_time_nanos + frame_interval;

    (frame_start_time_nanos, frame_target_time_nanos)
}

pub type SctkAsyncResult = Result<(), SctkAsyncError>;

#[derive(Error, Debug)]
pub enum SctkAsyncError {
    #[error(transparent)]
    AshpdError(#[from] ashpd::Error),
}

struct SctkColorScheme(ColorScheme);

impl From<SctkColorScheme> for PlatformBrightness {
    fn from(color_scheme: SctkColorScheme) -> Self {
        match color_scheme.0 {
            ColorScheme::PreferDark => PlatformBrightness::Dark,
            ColorScheme::PreferLight => PlatformBrightness::Light,
            ColorScheme::NoPreference => PlatformBrightness::Light, // fallback
        }
    }
}

pub(crate) struct SctkSettingsHandler {}

impl SctkSettingsHandler {
    // Note: zbus is runtime-agnostic and should work out of the box with
    // different Rust async runtimes. However, in order to achieve that, zbus
    // spawns a thread per connection to handle various internal tasks.
    //
    // https://docs.rs/zbus/4.2.2/zbus/#compatibility-with-async-runtimes
    pub(crate) async fn read_and_monitor_color_scheme_changes(
        plugin: SettingsPlugin,
    ) -> SctkAsyncResult {
        let settings = Settings::new().await?;

        let value_change_stream = settings.receive_color_scheme_changed().await?;
        let read_current_value_stream =
            futures_lite::stream::once_future(Box::pin(settings.color_scheme()))
                .filter_map(|t| t.ok());

        // TODO: Investigate if this code is prone to race conditions when the
        // color scheme is changed just *after* the current value is read but
        // *before* the change stream is fully initialized.
        //
        // However, even *if* a race condition is possible, the likelihood of it
        // happening in practice is pretty low considering that the
        // `color-scheme` setting is rarely changed on a typical device.
        let mut stream = read_current_value_stream
            .or(value_change_stream)
            .map(|color_scheme| PlatformBrightness::from(SctkColorScheme(color_scheme)));

        while let Some(platform_brightness) = stream.next().await {
            plugin
                .start_message()
                .set_platform_brightness(platform_brightness)
                .set_use_24_hour_format(true)
                .set_text_scale_factor(1.0)
                .send();
        }

        Ok(())
    }
}
