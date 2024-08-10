use std::{
    ffi::CString,
    mem,
    path::Path,
    ptr, slice,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dpi::{PhysicalPosition, PhysicalSize};
use flust_engine_sys::{
    FlutterBackingStoreType, FlutterEngineDisplayId, FlutterLayerContentType, FlutterSize,
};

pub use flust_engine_sys::FlutterViewId;
use tracing::error;

use crate::{path_to_cstring, FlutterEngine, FlutterEngineError};

// Warning: The implicit view ID value needs to be kept in sync with the
// `kFlutterImplicitViewId` constant on the engine side:
// https://github.com/flutter/engine/blob/9a8a5b6ac7ebb30b4c8d37939f7e397a77067820/shell/platform/embedder/embedder.cc#L107
pub const IMPLICIT_VIEW_ID: FlutterViewId = 0;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlutterPointerPhase {
    Cancel,
    Up,
    Down,
    Move,
    Add,
    Remove,
    Hover,
}

impl From<FlutterPointerPhase> for flust_engine_sys::FlutterPointerPhase {
    fn from(pointer_phase: FlutterPointerPhase) -> Self {
        match pointer_phase {
            FlutterPointerPhase::Cancel => flust_engine_sys::FlutterPointerPhase::kCancel,
            FlutterPointerPhase::Up => flust_engine_sys::FlutterPointerPhase::kUp,
            FlutterPointerPhase::Down => flust_engine_sys::FlutterPointerPhase::kDown,
            FlutterPointerPhase::Move => flust_engine_sys::FlutterPointerPhase::kMove,
            FlutterPointerPhase::Add => flust_engine_sys::FlutterPointerPhase::kAdd,
            FlutterPointerPhase::Remove => flust_engine_sys::FlutterPointerPhase::kRemove,
            FlutterPointerPhase::Hover => flust_engine_sys::FlutterPointerPhase::kHover,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FlutterPointerDeviceKind {
    Mouse,
    Touch,
}

impl From<FlutterPointerDeviceKind> for flust_engine_sys::FlutterPointerDeviceKind {
    fn from(device_kind: FlutterPointerDeviceKind) -> Self {
        match device_kind {
            FlutterPointerDeviceKind::Mouse => {
                flust_engine_sys::FlutterPointerDeviceKind::kFlutterPointerDeviceKindMouse
            }
            FlutterPointerDeviceKind::Touch => {
                flust_engine_sys::FlutterPointerDeviceKind::kFlutterPointerDeviceKindTouch
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlutterPointerSignalKind {
    None,
    Scroll,
}

impl From<FlutterPointerSignalKind> for flust_engine_sys::FlutterPointerSignalKind {
    fn from(pointer_signal_kind: FlutterPointerSignalKind) -> Self {
        match pointer_signal_kind {
            FlutterPointerSignalKind::None => {
                flust_engine_sys::FlutterPointerSignalKind::kFlutterPointerSignalKindNone
            }
            FlutterPointerSignalKind::Scroll => {
                flust_engine_sys::FlutterPointerSignalKind::kFlutterPointerSignalKindScroll
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlutterPointerMouseButtons {
    None = 0,
    Primary = 1,
    Secondary = 2,
    Middle = 4,
    Back = 8,
    Forward = 16,
}

impl From<FlutterPointerMouseButtons> for i64 {
    fn from(btn: FlutterPointerMouseButtons) -> Self {
        btn as i64
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FlutterPointerEvent {
    timestamp: Duration,
    device: i32,
    phase: FlutterPointerPhase,
    x: f64,
    y: f64,
    signal_kind: FlutterPointerSignalKind,
    scroll_delta_x: f64,
    scroll_delta_y: f64,
    device_kind: FlutterPointerDeviceKind,
    buttons: FlutterPointerMouseButtons,
    view_id: FlutterViewId,
}

impl FlutterPointerEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: i32,
        phase: FlutterPointerPhase,
        (x, y): (f64, f64),
        signal_kind: FlutterPointerSignalKind,
        (scroll_delta_x, scroll_delta_y): (f64, f64),
        device_kind: FlutterPointerDeviceKind,
        buttons: FlutterPointerMouseButtons,
        view_id: FlutterViewId,
    ) -> Self {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

        Self {
            timestamp,
            device,
            phase,
            x,
            y,
            signal_kind,
            scroll_delta_x,
            scroll_delta_y,
            device_kind,
            buttons,
            view_id,
        }
    }
}

impl From<FlutterPointerEvent> for flust_engine_sys::FlutterPointerEvent {
    fn from(event: FlutterPointerEvent) -> Self {
        Self {
            struct_size: mem::size_of::<flust_engine_sys::FlutterPointerEvent>(),
            timestamp: event.timestamp.as_micros() as usize,
            phase: event.phase.into(),
            x: event.x,
            y: event.y,
            device: event.device,
            signal_kind: event.signal_kind.into(),
            scroll_delta_x: event.scroll_delta_x,
            scroll_delta_y: event.scroll_delta_y,
            device_kind: event.device_kind.into(),
            buttons: event.buttons.into(),
            pan_x: 0.0,
            pan_y: 0.0,
            scale: 1.0,
            rotation: 0.0,
            view_id: event.view_id,
            #[cfg(all(target_arch = "arm", target_os = "android"))]
            __bindgen_padding_0: 0,
            #[cfg(all(target_arch = "arm", target_os = "android"))]
            __bindgen_padding_1: 0,
        }
    }
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum FlutterKeyEventType {
    Up,
    Down,
    Repeat,
}

impl From<FlutterKeyEventType> for flust_engine_sys::FlutterKeyEventType {
    fn from(value: FlutterKeyEventType) -> Self {
        match value {
            FlutterKeyEventType::Up => {
                flust_engine_sys::FlutterKeyEventType::kFlutterKeyEventTypeUp
            }
            FlutterKeyEventType::Down => {
                flust_engine_sys::FlutterKeyEventType::kFlutterKeyEventTypeDown
            }
            FlutterKeyEventType::Repeat => {
                flust_engine_sys::FlutterKeyEventType::kFlutterKeyEventTypeRepeat
            }
        }
    }
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum FlutterKeyEventDeviceType {
    Keyboard,
    DirectionalPad,
    Gamepad,
    Joystick,
    Hdmi,
}

impl From<FlutterKeyEventDeviceType> for flust_engine_sys::FlutterKeyEventDeviceType {
    fn from(value: FlutterKeyEventDeviceType) -> Self {
        match value {
            FlutterKeyEventDeviceType::Keyboard => flust_engine_sys::FlutterKeyEventDeviceType::kFlutterKeyEventDeviceTypeKeyboard,
            FlutterKeyEventDeviceType::DirectionalPad => flust_engine_sys::FlutterKeyEventDeviceType::kFlutterKeyEventDeviceTypeDirectionalPad,
            FlutterKeyEventDeviceType::Gamepad => flust_engine_sys::FlutterKeyEventDeviceType::kFlutterKeyEventDeviceTypeGamepad,
            FlutterKeyEventDeviceType::Joystick => flust_engine_sys::FlutterKeyEventDeviceType::kFlutterKeyEventDeviceTypeJoystick,
            FlutterKeyEventDeviceType::Hdmi => flust_engine_sys::FlutterKeyEventDeviceType::kFlutterKeyEventDeviceTypeHdmi,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct FlutterPhysicalKey(u64);

impl FlutterPhysicalKey {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct FlutterLogicalKey(u64);

impl FlutterLogicalKey {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// A structure to represent a key event.
///
/// Sending `FlutterKeyEvent` via `FlutterEngineSendKeyEvent` results in a
/// corresponding `FlutterKeyEvent` to be dispatched in the framework. It is
/// embedder's responsibility to ensure the regularity of sent events, since the
/// framework only performs simple one-to-one mapping. The events must conform
/// the following rules:
///
///  * Each key press sequence shall consist of one key down event (`kind` being
///    `kFlutterKeyEventTypeDown`), zero or more repeat events, and one key up
///    event, representing a physical key button being pressed, held, and
///    released.
///  * All events throughout a key press sequence shall have the same `physical`
///    and `logical`. Having different `character`s is allowed.
///
/// A `FlutterKeyEvent` with `physical` 0 and `logical` 0 is an empty event.
/// This is the only case either `physical` or `logical` can be 0. An empty
/// event must be sent if a key message should be converted to no
/// `FlutterKeyEvent`s, for example, when a key down message is received for a
/// key that has already been pressed according to the record. This is to ensure
/// some `FlutterKeyEvent` arrives at the framework before raw key message. See
/// https://github.com/flutter/flutter/issues/87230.
#[derive(Clone, Debug)]
pub struct FlutterKeyEvent {
    /// The timestamp at which the key event was generated. The timestamp should
    /// be specified in microseconds and the clock should be the same as that
    /// used by `FlutterEngineGetCurrentTime`.
    timestamp: Duration,

    /// The event kind.
    kind: FlutterKeyEventType,

    /// The USB HID code for the physical key of the event.
    ///
    /// For the full definition and list of pre-defined physical keys, see
    /// `PhysicalKeyboardKey` from the framework.
    ///
    /// The only case that `physical` might be 0 is when this is an empty event.
    /// See `FlutterKeyEvent` for introduction.
    physical: FlutterPhysicalKey,

    /// The key ID for the logical key of this event.
    ///
    /// For the full definition and a list of pre-defined logical keys, see
    /// `LogicalKeyboardKey` from the framework.
    ///
    /// The only case that `logical` might be 0 is when this is an empty event.
    /// See `FlutterKeyEvent` for introduction.
    logical: FlutterLogicalKey,

    /// Character input from the event. Can be `None`. Ignored for up events.
    character: Option<CString>,

    /// True if this event does not correspond to a native event.
    ///
    /// The embedder is likely to skip events and/or construct new events that
    /// do not correspond to any native events in order to conform the
    /// regularity of events (as documented in `FlutterKeyEvent`). An example is
    /// when a key up is missed due to loss of window focus, on a platform that
    /// provides query to key pressing status, the embedder might realize that
    /// the key has been released at the next key event, and should construct a
    /// synthesized up event immediately before the actual event.
    ///
    /// An event being synthesized means that the `timestamp` might greatly
    /// deviate from the actual time when the event occurs physically.
    synthesized: bool,

    /// The source device for the key event.
    device_type: FlutterKeyEventDeviceType,
}

impl FlutterKeyEvent {
    pub fn new(
        timestamp: Duration,
        kind: FlutterKeyEventType,
        physical: FlutterPhysicalKey,
        logical: FlutterLogicalKey,
        character: Option<CString>,
        synthesized: bool,
        device_type: FlutterKeyEventDeviceType,
    ) -> Self {
        Self {
            timestamp,
            kind,
            physical,
            logical,
            character,
            synthesized,
            device_type,
        }
    }

    // Note: The `From` trait can *not* be used for this conversion because the
    // character's `CString` needs to outlive the conversion.
    pub fn as_ptr(&self) -> flust_engine_sys::FlutterKeyEvent {
        flust_engine_sys::FlutterKeyEvent {
            struct_size: mem::size_of::<flust_engine_sys::FlutterKeyEvent>(),
            timestamp: self.timestamp.as_micros() as f64,
            type_: self.kind.into(),
            physical: self.physical.0,
            logical: self.logical.0,
            character: self
                .character
                .as_ref()
                .map(|character| character.as_ptr())
                .unwrap_or(ptr::null()),
            synthesized: self.synthesized,
            device_type: self.device_type.into(),
        }
    }
}

pub struct FlutterFrameInfo {
    pub size: PhysicalSize<u32>,
}

impl From<flust_engine_sys::FlutterFrameInfo> for FlutterFrameInfo {
    fn from(frame_info: flust_engine_sys::FlutterFrameInfo) -> Self {
        Self {
            size: PhysicalSize::new(frame_info.size.width, frame_info.size.height),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FlutterBackingStoreConfig {
    /// The size of the render target the engine expects to render into.
    pub size: FlutterSize,
    /// The identifier for the view that the engine will use this backing store
    /// to render into.
    pub view_id: FlutterViewId,
}

impl From<flust_engine_sys::FlutterBackingStoreConfig> for FlutterBackingStoreConfig {
    fn from(config: flust_engine_sys::FlutterBackingStoreConfig) -> Self {
        Self {
            size: config.size,
            view_id: config.view_id,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FlutterBackingStoreUserData {
    // The `view_id` field is used for being able to determine the targeted view
    // in the `collect_backing_store_callback`.
    pub view_id: FlutterViewId,
}

impl FlutterBackingStoreUserData {
    fn into_ffi(self, target: &mut flust_engine_sys::FlutterBackingStore) {
        target.user_data = Box::into_raw(Box::new(self)) as _;
    }

    // Creates a *copy* of the data from the raw pointer. This is useful for
    // getting access to the underlying data but without impacting the raw
    // pointer when this copy gets dropped.
    unsafe fn clone_from_raw(raw: *mut Self) -> Self {
        let raw = Box::from_raw(raw);
        let user_data = raw.clone();
        std::mem::forget(raw);
        *user_data
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FlutterBackingStore {
    pub description: FlutterBackingStoreDescription,
    pub user_data: FlutterBackingStoreUserData,

    /// This field is used for collecting the `user_data` memory as part of the
    /// `FlutterCompositor.collect_backing_store_callback`.
    raw_user_data: *mut FlutterBackingStoreUserData,
}

impl FlutterBackingStore {
    pub fn new(description: FlutterBackingStoreDescription, view_id: FlutterViewId) -> Self {
        Self {
            description,
            user_data: FlutterBackingStoreUserData { view_id },
            // Note: `raw_user_data` is initialized as `nullptr` but it will
            // point to an actual `user_data` value after a roundtrip through
            // the embedder API.
            //
            // The `user_data` field is converted into a raw pointer as part of
            // the `FlutterBackingStore::into_ffi` call which is then used to
            // fill both the `user_data` and `raw_user_data` fields when
            // converting from `flust_engine_sys::FlutterBackingStore`.
            raw_user_data: std::ptr::null_mut(),
        }
    }

    pub(crate) fn into_ffi(self, target: &mut flust_engine_sys::FlutterBackingStore) {
        self.user_data.into_ffi(target);
        self.description.into_ffi(target);
    }

    pub fn drop_raw_user_data(&mut self) {
        unsafe {
            drop(Box::from_raw(self.raw_user_data));
        }

        self.raw_user_data = std::ptr::null_mut()
    }
}

impl From<flust_engine_sys::FlutterBackingStore> for FlutterBackingStore {
    fn from(value: flust_engine_sys::FlutterBackingStore) -> Self {
        let raw_user_data = value.user_data as *mut FlutterBackingStoreUserData;
        let user_data = unsafe { FlutterBackingStoreUserData::clone_from_raw(raw_user_data) };

        let description = match value.type_ {
            FlutterBackingStoreType::kFlutterBackingStoreTypeOpenGL => unsafe {
                value.__bindgen_anon_1.open_gl.into()
            },
            FlutterBackingStoreType::kFlutterBackingStoreTypeSoftware => {
                FlutterBackingStoreDescription::Software
            }
            FlutterBackingStoreType::kFlutterBackingStoreTypeSoftware2 => {
                FlutterBackingStoreDescription::Software2
            }
            FlutterBackingStoreType::kFlutterBackingStoreTypeMetal => {
                FlutterBackingStoreDescription::Metal
            }
            FlutterBackingStoreType::kFlutterBackingStoreTypeVulkan => {
                FlutterBackingStoreDescription::Vulkan
            }
        };

        Self {
            description,
            user_data,
            raw_user_data,
        }
    }
}

// TODO: Add support for more backing store types (e.g.: Vulkan, Metal,
// Software, Software2)
#[derive(Copy, Clone, Debug)]
pub enum FlutterBackingStoreDescription {
    OpenGL(FlutterOpenGLBackingStore),
    Software,
    Software2,
    Metal,
    Vulkan,
}

impl FlutterBackingStoreDescription {
    pub(crate) fn into_ffi(self, target: &mut flust_engine_sys::FlutterBackingStore) {
        let FlutterBackingStoreDescription::OpenGL(opengl_target) = self else {
            unimplemented!("Only OpenGL framebuffer backing store is currently implemented");
        };

        target.type_ = self.into();
        unsafe {
            opengl_target.into_ffi(&mut target.__bindgen_anon_1.open_gl);
        };
    }
}

impl From<FlutterBackingStoreDescription> for flust_engine_sys::FlutterBackingStoreType {
    fn from(value: FlutterBackingStoreDescription) -> Self {
        match value {
            FlutterBackingStoreDescription::OpenGL(_) => Self::kFlutterBackingStoreTypeOpenGL,
            FlutterBackingStoreDescription::Software => Self::kFlutterBackingStoreTypeSoftware,
            FlutterBackingStoreDescription::Software2 => Self::kFlutterBackingStoreTypeSoftware2,
            FlutterBackingStoreDescription::Metal => Self::kFlutterBackingStoreTypeMetal,
            FlutterBackingStoreDescription::Vulkan => Self::kFlutterBackingStoreTypeVulkan,
        }
    }
}

impl From<flust_engine_sys::FlutterOpenGLBackingStore> for FlutterBackingStoreDescription {
    fn from(value: flust_engine_sys::FlutterOpenGLBackingStore) -> Self {
        let backing_store = match value.type_ {
            flust_engine_sys::FlutterOpenGLTargetType::kFlutterOpenGLTargetTypeFramebuffer => {
                let framebuffer = unsafe { value.__bindgen_anon_1.framebuffer.into() };
                FlutterOpenGLBackingStore::Framebuffer(framebuffer)
            }
            flust_engine_sys::FlutterOpenGLTargetType::kFlutterOpenGLTargetTypeTexture => {
                FlutterOpenGLBackingStore::Texture
            }
            flust_engine_sys::FlutterOpenGLTargetType::kFlutterOpenGLTargetTypeSurface => {
                let surface = unsafe { value.__bindgen_anon_1.surface.into() };
                FlutterOpenGLBackingStore::Surface(surface)
            }
        };

        Self::OpenGL(backing_store)
    }
}

// TODO: Add support for OpenGL texture target type
#[derive(Copy, Clone, Debug)]
pub enum FlutterOpenGLBackingStore {
    Framebuffer(FlutterOpenGLFramebuffer),
    Texture,
    Surface(FlutterOpenGLSurface),
}

impl FlutterOpenGLBackingStore {
    pub(crate) fn into_ffi(self, target: &mut flust_engine_sys::FlutterOpenGLBackingStore) {
        target.type_ = self.into();

        match self {
            FlutterOpenGLBackingStore::Framebuffer(framebuffer) => unsafe {
                framebuffer.into_ffi(&mut target.__bindgen_anon_1.framebuffer);
            },
            FlutterOpenGLBackingStore::Texture => {
                unimplemented!("OpenGL texture backing store is not currently implemented")
            }
            FlutterOpenGLBackingStore::Surface(surface) => unsafe {
                surface.into_ffi(&mut target.__bindgen_anon_1.surface);
            },
        }
    }
}

impl From<FlutterOpenGLBackingStore> for flust_engine_sys::FlutterOpenGLTargetType {
    fn from(value: FlutterOpenGLBackingStore) -> Self {
        match value {
            FlutterOpenGLBackingStore::Framebuffer(_) => {
                flust_engine_sys::FlutterOpenGLTargetType::kFlutterOpenGLTargetTypeFramebuffer
            }
            FlutterOpenGLBackingStore::Texture => {
                flust_engine_sys::FlutterOpenGLTargetType::kFlutterOpenGLTargetTypeTexture
            }
            FlutterOpenGLBackingStore::Surface(_) => {
                flust_engine_sys::FlutterOpenGLTargetType::kFlutterOpenGLTargetTypeSurface
            }
        }
    }
}

type VoidCallback = unsafe extern "C" fn(user_data: *mut ::std::os::raw::c_void);

type FlutterOpenGLSurfaceCallback = unsafe extern "C" fn(
    user_data: *mut ::std::os::raw::c_void,
    opengl_state_changed: *mut bool,
) -> bool;

#[derive(Copy, Clone, Debug)]
pub struct FlutterOpenGLSurface {
    /// User data to be passed to the make_current, clear_current and
    /// destruction callbacks.
    pub user_data: *mut ::std::os::raw::c_void,

    /// Callback invoked (on an engine-managed thread) that asks the embedder to
    /// make the surface current.
    ///
    /// Should return true if the operation succeeded, false if the surface
    /// could not be made current and rendering should be cancelled.
    ///
    /// The second parameter 'opengl state changed' should be set to true if any
    /// OpenGL API state is different than before this callback was called. In
    /// that case, Flutter will invalidate the internal OpenGL API state cache,
    /// which is a somewhat expensive operation.
    pub make_current_callback: FlutterOpenGLSurfaceCallback,

    /// Callback invoked (on an engine-managed thread) when the current surface
    /// can be cleared.
    ///
    /// Should return true if the operation succeeded, false if an error
    /// ocurred. That error will be logged but otherwise not handled by the
    /// engine.
    ///
    /// The second parameter 'opengl state changed' is the same as with the
    /// [`make_current_callback`].
    ///
    /// The embedder might clear the surface here after it was previously made
    /// current. That's not required however, it's also possible to clear it in
    /// the destruction callback. There's no way to signal OpenGL state changes
    /// in the destruction callback though.
    pub clear_current_callback: FlutterOpenGLSurfaceCallback,

    /// Callback invoked (on an engine-managed thread) that asks the embedder to
    /// collect the surface.
    pub destruction_callback: VoidCallback,

    /// The surface format (example GL_RGBA8).
    pub format: u32,
}

impl FlutterOpenGLSurface {
    pub(crate) fn into_ffi(self, target: &mut flust_engine_sys::FlutterOpenGLSurface) {
        target.struct_size = mem::size_of::<flust_engine_sys::FlutterOpenGLSurface>();
        target.format = self.format;
        target.user_data = self.user_data;
        target.make_current_callback = Some(self.make_current_callback);
        target.clear_current_callback = Some(self.clear_current_callback);
        target.destruction_callback = Some(self.destruction_callback);
    }
}

impl From<flust_engine_sys::FlutterOpenGLSurface> for FlutterOpenGLSurface {
    fn from(surface: flust_engine_sys::FlutterOpenGLSurface) -> Self {
        Self {
            format: surface.format,
            user_data: surface.user_data,
            make_current_callback: surface
                .make_current_callback
                .expect("`FlutterOpenGLSurface.make_current_callback` should not be null"),
            clear_current_callback: surface
                .clear_current_callback
                .expect("`FlutterOpenGLSurface.clear_current_callback` should not be null"),
            destruction_callback: surface
                .destruction_callback
                .expect("`FlutterOpenGLSurface.destruction_callback` should not be null"),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FlutterOpenGLFramebuffer {
    /// The format of the color attachment of the frame-buffer. For example,
    /// GL_RGBA8.
    ///
    /// In case of ambiguity when dealing with Window bound frame-buffers, 0 may
    /// be used.
    ///
    /// @bug      This field is incorrectly named as "target" when it actually
    ///           refers to a format.
    pub target: u32,

    /// The name of the framebuffer.
    pub name: u32,

    /// User data to be returned on the invocation of the destruction callback.
    pub user_data: FlutterOpenGLBackingStoreFramebuffer,

    /// This field is used for collecting the `user_data` memory as part of the
    /// `FlutterCompositor.collect_backing_store_callback`.
    raw_user_data: *mut FlutterOpenGLBackingStoreFramebuffer,
}

impl FlutterOpenGLFramebuffer {
    pub fn new(target: u32, user_data: FlutterOpenGLBackingStoreFramebuffer) -> Self {
        Self {
            name: user_data.framebuffer_id,
            target,
            user_data,
            // Note: `raw_user_data` is initialized as `nullptr` but it will
            // point to an actual `user_data` value after a roundtrip through
            // the embedder API.
            //
            // The `user_data` field is converted into a raw pointer as part of
            // the `FlutterOpenGLFramebuffer::into_ffi` call which is then used
            // to fill both the `user_data` and `raw_user_data` fields when
            // converting from `flust_engine_sys::FlutterOpenGLFramebuffer`.
            raw_user_data: std::ptr::null_mut(),
        }
    }

    pub(crate) fn into_ffi(self, target: &mut flust_engine_sys::FlutterOpenGLFramebuffer) {
        target.name = self.user_data.framebuffer_id;
        target.target = self.target;
        target.user_data = Box::into_raw(Box::new(self.user_data)) as _;
        target.destruction_callback = None;
    }

    pub fn drop_raw_user_data(&mut self) {
        unsafe {
            drop(Box::from_raw(self.raw_user_data));
        }

        self.raw_user_data = std::ptr::null_mut()
    }
}

impl From<flust_engine_sys::FlutterOpenGLFramebuffer> for FlutterOpenGLFramebuffer {
    fn from(value: flust_engine_sys::FlutterOpenGLFramebuffer) -> Self {
        let raw_user_data = value.user_data as *mut FlutterOpenGLBackingStoreFramebuffer;
        let user_data =
            unsafe { FlutterOpenGLBackingStoreFramebuffer::clone_from_raw(raw_user_data) };

        Self {
            target: value.target,
            name: user_data.framebuffer_id,
            raw_user_data,
            user_data,
        }
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct FlutterOpenGLBackingStoreFramebuffer {
    pub framebuffer_id: u32,
    pub texture_id: u32,
}

impl FlutterOpenGLBackingStoreFramebuffer {
    pub fn new() -> Self {
        Default::default()
    }

    // Creates a *copy* of the data from the raw pointer. This is useful for
    // getting access to the underlying data but without impacting the raw
    // pointer when this copy gets dropped.
    unsafe fn clone_from_raw(raw: *mut FlutterOpenGLBackingStoreFramebuffer) -> Self {
        let raw = Box::from_raw(raw);
        let framebuffer = raw.clone();
        std::mem::forget(raw);
        *framebuffer
    }
}

pub struct FlutterPresentViewInfo {
    pub view_id: FlutterViewId,
    pub layers: Vec<FlutterLayer>,
}

impl FlutterPresentViewInfo {
    pub fn new(view_id: FlutterViewId, layers: Vec<FlutterLayer>) -> Self {
        Self { view_id, layers }
    }
}

pub struct FlutterLayer {
    pub content: FlutterLayerContent,

    /// The offset of this layer (in physical pixels) relative to the top left
    /// of the root surface used by the engine.
    pub offset: PhysicalPosition<f64>,

    /// The size of the layer (in physical pixels).
    pub size: PhysicalSize<f64>,

    /// Extra information for the backing store that the embedder may use during
    /// presentation.
    pub backing_store_present_info: FlutterBackingStorePresentInfo,
}

impl From<flust_engine_sys::FlutterLayer> for FlutterLayer {
    fn from(layer: flust_engine_sys::FlutterLayer) -> Self {
        Self {
            content: match layer.type_ {
                FlutterLayerContentType::kFlutterLayerContentTypeBackingStore => {
                    let backing_store = unsafe { (*layer.__bindgen_anon_1.backing_store).into() };
                    FlutterLayerContent::BackingStore(backing_store)
                }
                FlutterLayerContentType::kFlutterLayerContentTypePlatformView => {
                    FlutterLayerContent::PlatformView
                }
            },
            offset: PhysicalPosition::new(layer.offset.x, layer.offset.y),
            size: PhysicalSize::new(layer.size.width, layer.size.height),
            backing_store_present_info: unsafe { (*layer.backing_store_present_info).into() },
        }
    }
}

// TODO: Add support for platform view layer content
pub enum FlutterLayerContent {
    /// Indicates that the contents of this layer are rendered by Flutter into a
    /// backing store.
    BackingStore(FlutterBackingStore),

    /// Indicates that the contents of this layer are determined by the
    /// embedder.
    PlatformView,
}

impl FlutterLayerContent {
    pub fn get_opengl_backing_store_framebuffer_name(&self) -> Option<u32> {
        let FlutterLayerContent::BackingStore(backing_store) = self else {
            return None;
        };

        let FlutterBackingStoreDescription::OpenGL(FlutterOpenGLBackingStore::Framebuffer(
            framebuffer,
        )) = backing_store.description
        else {
            return None;
        };

        Some(framebuffer.name)
    }
}

/// Contains additional information about the backing store provided during
/// presentation to the embedder.
pub struct FlutterBackingStorePresentInfo {
    /// The area of the backing store that contains Flutter contents. Pixels
    /// outside of this area are transparent and the embedder may choose not to
    /// render them. Coordinates are in physical pixels.
    pub paint_region: FlutterRegion,
}

impl From<flust_engine_sys::FlutterBackingStorePresentInfo> for FlutterBackingStorePresentInfo {
    fn from(present_info: flust_engine_sys::FlutterBackingStorePresentInfo) -> Self {
        Self {
            paint_region: unsafe { *present_info.paint_region }.into(),
        }
    }
}

/// A region represented by a collection of non-overlapping rectangles.
pub struct FlutterRegion {
    /// The rectangles that make up the region.
    pub rects: Vec<flust_engine_sys::FlutterRect>,
}

impl From<flust_engine_sys::FlutterRegion> for FlutterRegion {
    fn from(region: flust_engine_sys::FlutterRegion) -> Self {
        let rects: Vec<flust_engine_sys::FlutterRect> =
            unsafe { slice::from_raw_parts(region.rects, region.rects_count).to_vec() };

        Self { rects }
    }
}

/// The update type parameter that is passed to `FlutterEngineNotifyDisplayUpdate`.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum FlutterEngineDisplaysUpdateType {
    /// `FlutterEngineDisplay`s that were active during start-up. A display is
    /// considered active if:
    /// 1. The frame buffer hardware is connected.
    /// 2. The display is drawable, e.g. it isn't being mirrored from another
    ///    connected display or sleeping.
    Startup,
    Count,
}

impl From<FlutterEngineDisplaysUpdateType> for flust_engine_sys::FlutterEngineDisplaysUpdateType {
    fn from(value: FlutterEngineDisplaysUpdateType) -> Self {
        match value {
            FlutterEngineDisplaysUpdateType::Startup => flust_engine_sys::FlutterEngineDisplaysUpdateType::kFlutterEngineDisplaysUpdateTypeStartup,
            FlutterEngineDisplaysUpdateType::Count => flust_engine_sys::FlutterEngineDisplaysUpdateType::kFlutterEngineDisplaysUpdateTypeCount,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct FlutterEngineDisplay {
    pub display_id: FlutterEngineDisplayId,

    /// This is set to true if the embedder only has one display. In cases where
    /// this is set to true, the value of display_id is ignored. In cases where
    /// this is not set to true, it is expected that a valid display_id be
    /// provided.
    pub single_display: bool,

    /// This represents the refresh period in frames per second. This value may
    /// be zero if the device is not running or unavailable or unknown.
    pub refresh_rate: f64,

    /// The size of the display, in physical pixels.
    pub size: PhysicalSize<usize>,

    /// The pixel ratio of the display, which is used to convert physical pixels
    /// to logical pixels.
    pub device_pixel_ratio: f64,
}

impl From<FlutterEngineDisplay> for flust_engine_sys::FlutterEngineDisplay {
    fn from(display: FlutterEngineDisplay) -> Self {
        Self {
            struct_size: mem::size_of::<Self>(),
            display_id: display.display_id,
            single_display: display.single_display,
            refresh_rate: display.refresh_rate,
            width: display.size.width,
            height: display.size.height,
            device_pixel_ratio: display.device_pixel_ratio,
        }
    }
}

pub(crate) type FlutterEngineResult = Result<(), FlutterEngineError>;

pub(crate) trait FlutterEngineResultExt {
    fn from_ffi(result: flust_engine_sys::FlutterEngineResult) -> Self;
}

impl FlutterEngineResultExt for FlutterEngineResult {
    fn from_ffi(result: flust_engine_sys::FlutterEngineResult) -> Self {
        match result {
            flust_engine_sys::FlutterEngineResult::kSuccess => Ok(()),
            flust_engine_sys::FlutterEngineResult::kInvalidLibraryVersion => {
                Err(FlutterEngineError::InvalidLibraryVersion)
            }
            flust_engine_sys::FlutterEngineResult::kInvalidArguments => {
                Err(FlutterEngineError::InvalidArguments)
            }
            flust_engine_sys::FlutterEngineResult::kInternalInconsistency => {
                Err(FlutterEngineError::InternalInconsistency)
            }
        }
    }
}

pub(crate) struct FlutterEngineAOTData {
    pub(crate) data: flust_engine_sys::FlutterEngineAOTData,
}

impl FlutterEngineAOTData {
    pub(crate) fn new(aot_library_path: &Path) -> Result<Self, FlutterEngineError> {
        let data: flust_engine_sys::FlutterEngineAOTData = ptr::null_mut();

        if FlutterEngine::runs_aot_compiled_dart_code() {
            Self::create_aot_data(aot_library_path, &data)?;
        }

        Ok(Self { data })
    }

    fn create_aot_data(
        aot_library_path: &Path,
        data_out: &flust_engine_sys::FlutterEngineAOTData,
    ) -> Result<(), FlutterEngineError> {
        let elf_path = path_to_cstring(aot_library_path).into_raw();
        let source = &flust_engine_sys::FlutterEngineAOTDataSource {
            type_: flust_engine_sys::FlutterEngineAOTDataSourceType::kFlutterEngineAOTDataSourceTypeElfPath,
            __bindgen_anon_1: flust_engine_sys::FlutterEngineAOTDataSource__bindgen_ty_1 { elf_path }
        } as *const flust_engine_sys::FlutterEngineAOTDataSource;

        let result = unsafe {
            flust_engine_sys::FlutterEngineCreateAOTData(
                source,
                data_out as *const flust_engine_sys::FlutterEngineAOTData
                    as *mut flust_engine_sys::FlutterEngineAOTData,
            )
        };
        FlutterEngineResult::from_ffi(result)
    }

    fn collect_aot_data(
        data: flust_engine_sys::FlutterEngineAOTData,
    ) -> Result<(), FlutterEngineError> {
        let result = unsafe { flust_engine_sys::FlutterEngineCollectAOTData(data) };
        FlutterEngineResult::from_ffi(result)
    }
}

impl Drop for FlutterEngineAOTData {
    fn drop(&mut self) {
        if !self.data.is_null() {
            if let Err(err) = Self::collect_aot_data(self.data) {
                error!("Failed to collect AOT data: {:?}", err);
            };

            self.data = ptr::null_mut();
        }
    }
}
