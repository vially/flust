//! What Flutter's windowing API calls into: the functions that Dart calls
//! to create, change and destroy windows, and the `flust/windowing` channel,
//! which gives Dart their addresses and tells it what happens to windows.
//! Dart's side is the `flust_windowing` package.
//!
//! Creating a window is synchronous in Flutter's API: the window's view has
//! to exist when the call returns. Dart runs on the platform thread, so the
//! functions are called on it, by Dart, while the engine runs a task or
//! handles an event. They find the application by the id of its engine.
//!
//! The `flust/windowing` method channel takes:
//!
//! - `initialize`, answered with a map of the functions' names to their
//!   addresses;
//!
//! and calls into Dart, with `{"viewId": int}`:
//!
//! - `onWindowChanged` when a window's size or state changed;
//! - `onWindowCloseRequested` when the compositor asks for a window to
//!   close, or a popup is dismissed (the window stays until Dart destroys
//!   it);
//! - `onWindowDestroyed` when a window is gone.

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{c_char, CStr},
    rc::{Rc, Weak},
};

use dpi::LogicalSize;
use flust_engine::{
    channel::{MethodCall, MethodCallHandler, MethodChannel},
    codec::{Value, STANDARD_CODEC},
    ffi::FlutterViewId,
    plugins::Plugin,
    FlutterEngine,
};
use tracing::{debug, error, warn};

use crate::{
    popup::{PlacementAnchor, PopupPlacement},
    window::{SctkFlutterWindow, SctkWindowAttributes, SizeConstraints},
    windows::{SctkCreateWindowError, SctkWindows},
};

const CHANNEL_NAME: &str = "flust/windowing";

thread_local! {
    /// The windows of each application on this thread, by the id of its
    /// engine.
    static APPLICATIONS: RefCell<HashMap<i64, Weak<SctkWindows>>> = RefCell::default();
}

/// Lets Dart find the windows of the application whose engine has
/// `engine_id`. Must be called on the platform thread.
pub(crate) fn register_application(engine_id: i64, windows: &Rc<SctkWindows>) {
    APPLICATIONS.with_borrow_mut(|applications| {
        applications.retain(|_, windows| windows.strong_count() > 0);
        applications.insert(engine_id, Rc::downgrade(windows));
    });
}

fn windows(engine_id: i64) -> Option<Rc<SctkWindows>> {
    let windows = APPLICATIONS.with_borrow(|applications| applications.get(&engine_id)?.upgrade());
    if windows.is_none() {
        error!("windowing: no application with engine {engine_id} on this thread");
    }
    windows
}

fn window(engine_id: i64, view_id: FlutterViewId) -> Option<SctkFlutterWindow> {
    let window = windows(engine_id)?.get(view_id);
    if window.is_none() {
        warn!("windowing: no window with view id {view_id}");
    }
    window
}

/// A size limit from Dart, in logical pixels. A dimension that is not
/// finite, or not positive, is not limited: xdg-shell takes zero for that.
fn size_limit(width: f64, height: f64) -> Option<LogicalSize<u32>> {
    let limit = |value: f64| match value.is_finite() && value > 0.0 {
        true => value.round() as u32,
        false => 0,
    };
    let size = LogicalSize::new(limit(width), limit(height));
    (size.width > 0 || size.height > 0).then_some(size)
}

/// What `create_window` returns instead of a view id when it fails.
#[repr(i64)]
#[derive(Clone, Copy, Debug)]
enum CreateWindowFailure {
    /// The application has a startup window.
    StartupWindow = -1,
    /// The application renders straight into its startup window.
    UnsupportedTargetType = -2,
    EngineNotRunning = -3,
    /// Anything else: the log says what.
    Other = -4,
    /// The parent asked for is not a window, or is being destroyed.
    UnknownParent = -5,
}

impl From<&SctkCreateWindowError> for CreateWindowFailure {
    fn from(err: &SctkCreateWindowError) -> Self {
        match err {
            SctkCreateWindowError::StartupWindow => Self::StartupWindow,
            SctkCreateWindowError::UnsupportedTargetType(_) => Self::UnsupportedTargetType,
            SctkCreateWindowError::EngineNotRunning => Self::EngineNotRunning,
            SctkCreateWindowError::UnknownParent(_) => Self::UnknownParent,
            SctkCreateWindowError::Window(_) | SctkCreateWindowError::View(_) => Self::Other,
        }
    }
}

/// Creates a window and returns the id of its view, which the engine has by
/// then, or a negative `CreateWindowFailure`.
///
/// Sizes are in logical pixels. A size or a limit that is not finite is not
/// set. `title` is a UTF-8 string, or null. With a `modal_parent` that is
/// not negative, the window is a modal dialog of the window with that view
/// id; a dialog without a parent is a regular window on Wayland.
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn create_window(
    engine_id: i64,
    width: f64,
    height: f64,
    min_width: f64,
    min_height: f64,
    max_width: f64,
    max_height: f64,
    title: *const c_char,
    modal_parent: FlutterViewId,
) -> i64 {
    let Some(windows) = windows(engine_id) else {
        return CreateWindowFailure::Other as i64;
    };
    let title = (!title.is_null()).then(|| {
        unsafe { CStr::from_ptr(title) }
            .to_string_lossy()
            .into_owned()
    });
    let inner_size = (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
        .then(|| LogicalSize::new(width, height).into());

    let attributes = SctkWindowAttributes {
        title,
        inner_size,
        min_size: size_limit(min_width, min_height),
        max_size: size_limit(max_width, max_height),
        ..Default::default()
    };
    let modal_parent = (modal_parent >= 0).then_some(modal_parent);
    match windows.create_window(attributes, modal_parent) {
        Ok(window) => window.view_id(),
        Err(err) => {
            error!("windowing: failed to create a window: {err}");
            CreateWindowFailure::from(&err) as i64
        }
    }
}

/// Flutter's `WindowPositionerConstraintAdjustment`, as flags.
const ADJUST_FLIP_X: u32 = 1 << 0;
const ADJUST_FLIP_Y: u32 = 1 << 1;
const ADJUST_SLIDE_X: u32 = 1 << 2;
const ADJUST_SLIDE_Y: u32 = 1 << 3;
const ADJUST_RESIZE_X: u32 = 1 << 4;
const ADJUST_RESIZE_Y: u32 = 1 << 5;

/// A placement from Dart: the anchor rectangle and the offset in logical
/// pixels of the parent, the anchors as indices of Flutter's
/// `WindowPositionerAnchor`, and the adjustments as `ADJUST_*` flags.
#[allow(clippy::too_many_arguments)]
fn placement(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    parent_anchor: u32,
    child_anchor: u32,
    offset_x: f64,
    offset_y: f64,
    adjustment: u32,
) -> Option<PopupPlacement> {
    let int = |value: f64| match value.is_finite() {
        true => value.round() as i32,
        false => 0,
    };
    let adjust = |flag: u32| adjustment & flag != 0;
    Some(PopupPlacement {
        anchor_rect: (int(x), int(y), int(width), int(height)),
        parent_anchor: PlacementAnchor::from_index(parent_anchor)?,
        child_anchor: PlacementAnchor::from_index(child_anchor)?,
        offset: (int(offset_x), int(offset_y)),
        flip_x: adjust(ADJUST_FLIP_X),
        flip_y: adjust(ADJUST_FLIP_Y),
        slide_x: adjust(ADJUST_SLIDE_X),
        slide_y: adjust(ADJUST_SLIDE_Y),
        resize_x: adjust(ADJUST_RESIZE_X),
        resize_y: adjust(ADJUST_RESIZE_Y),
    })
}

/// Constraints from Dart, in logical pixels. A maximum that is not finite
/// is not limited.
fn constraints(
    min_width: f64,
    min_height: f64,
    max_width: f64,
    max_height: f64,
) -> SizeConstraints {
    let min = |value: f64| match value.is_finite() {
        true => value.max(0.0),
        false => 0.0,
    };
    SizeConstraints {
        min: LogicalSize::new(min(min_width), min(min_height)),
        max: LogicalSize::new(max_width, max_height),
    }
}

/// Creates a popup of the window with view id `parent`, or a tooltip, and
/// returns the id of its view, or a negative `CreateWindowFailure`. It is
/// placed as `placement` says (see there), and sized to its content within
/// the constraints, in logical pixels.
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn create_popup(
    engine_id: i64,
    parent: FlutterViewId,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    parent_anchor: u32,
    child_anchor: u32,
    offset_x: f64,
    offset_y: f64,
    adjustment: u32,
    min_width: f64,
    min_height: f64,
    max_width: f64,
    max_height: f64,
    tooltip: bool,
) -> i64 {
    let Some(windows) = windows(engine_id) else {
        return CreateWindowFailure::Other as i64;
    };
    let Some(placement) = placement(
        x,
        y,
        width,
        height,
        parent_anchor,
        child_anchor,
        offset_x,
        offset_y,
        adjustment,
    ) else {
        error!("windowing: invalid anchors {parent_anchor} and {child_anchor}");
        return CreateWindowFailure::Other as i64;
    };
    let constraints = constraints(min_width, min_height, max_width, max_height);
    match windows.create_popup(parent, placement, constraints, tooltip) {
        Ok(window) => window.view_id(),
        Err(err) => {
            error!("windowing: failed to create a popup: {err}");
            CreateWindowFailure::from(&err) as i64
        }
    }
}

/// Places a popup again: see `create_popup`.
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn update_position(
    engine_id: i64,
    view_id: FlutterViewId,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    parent_anchor: u32,
    child_anchor: u32,
    offset_x: f64,
    offset_y: f64,
    adjustment: u32,
) {
    let placement = placement(
        x,
        y,
        width,
        height,
        parent_anchor,
        child_anchor,
        offset_x,
        offset_y,
        adjustment,
    );
    match (window(engine_id, view_id), placement) {
        (Some(window), Some(placement)) => window.set_placement(placement),
        (_, None) => error!("windowing: invalid anchors {parent_anchor} and {child_anchor}"),
        (None, _) => {}
    }
}

/// Where the compositor put a popup, relative to its parent, in logical
/// pixels.
unsafe extern "C" fn get_offset_x(engine_id: i64, view_id: FlutterViewId) -> f64 {
    window(engine_id, view_id)
        .and_then(|window| window.popup_position())
        .map_or(0.0, |(x, _)| x.into())
}

unsafe extern "C" fn get_offset_y(engine_id: i64, view_id: FlutterViewId) -> f64 {
    window(engine_id, view_id)
        .and_then(|window| window.popup_position())
        .map_or(0.0, |(_, y)| y.into())
}

/// The sizes the content of a popup may take, in logical pixels.
unsafe extern "C" fn set_content_constraints(
    engine_id: i64,
    view_id: FlutterViewId,
    min_width: f64,
    min_height: f64,
    max_width: f64,
    max_height: f64,
) {
    if let Some(window) = window(engine_id, view_id) {
        window.set_content_constraints(constraints(min_width, min_height, max_width, max_height));
    }
}

unsafe extern "C" fn destroy_window(engine_id: i64, view_id: FlutterViewId) {
    let Some(windows) = windows(engine_id) else {
        return;
    };
    if let Err(err) = windows.destroy_window(view_id) {
        error!("windowing: failed to destroy window {view_id}: {err}");
    }
}

/// The width of the window's content, in logical pixels.
unsafe extern "C" fn get_content_width(engine_id: i64, view_id: FlutterViewId) -> f64 {
    window(engine_id, view_id).map_or(0.0, |window| window.content_size().width)
}

/// The height of the window's content, in logical pixels.
unsafe extern "C" fn get_content_height(engine_id: i64, view_id: FlutterViewId) -> f64 {
    window(engine_id, view_id).map_or(0.0, |window| window.content_size().height)
}

const STATE_ACTIVATED: u32 = 1 << 0;
const STATE_MAXIMIZED: u32 = 1 << 1;
const STATE_FULLSCREEN: u32 = 1 << 2;

/// The window's state, as `STATE_*` flags.
unsafe extern "C" fn get_state(engine_id: i64, view_id: FlutterViewId) -> u32 {
    let Some(window) = window(engine_id, view_id) else {
        return 0;
    };
    let state = window.toplevel_state();
    [
        (state.activated, STATE_ACTIVATED),
        (state.maximized, STATE_MAXIMIZED),
        (state.fullscreen, STATE_FULLSCREEN),
    ]
    .into_iter()
    .filter(|(set, _)| *set)
    .fold(0, |flags, (_, flag)| flags | flag)
}

unsafe extern "C" fn set_title(engine_id: i64, view_id: FlutterViewId, title: *const c_char) {
    if title.is_null() {
        return;
    }
    let title = unsafe { CStr::from_ptr(title) }
        .to_string_lossy()
        .into_owned();
    if let Some(window) = window(engine_id, view_id) {
        window.set_title(title);
    }
}

unsafe extern "C" fn set_size(engine_id: i64, view_id: FlutterViewId, width: f64, height: f64) {
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        warn!("windowing: ignoring size {width}x{height} for window {view_id}");
        return;
    }
    if let Some(window) = window(engine_id, view_id) {
        window.set_size(LogicalSize::new(width, height));
    }
}

unsafe extern "C" fn set_constraints(
    engine_id: i64,
    view_id: FlutterViewId,
    min_width: f64,
    min_height: f64,
    max_width: f64,
    max_height: f64,
) {
    if let Some(window) = window(engine_id, view_id) {
        window.set_size_limits(
            size_limit(min_width, min_height),
            size_limit(max_width, max_height),
        );
    }
}

/// Wayland only lets a window be activated with a token from the window
/// that has the focus (`xdg_activation_v1`), which is not supported yet.
unsafe extern "C" fn activate(_engine_id: i64, view_id: FlutterViewId) {
    debug!("windowing: ignoring activation of window {view_id}: not supported on Wayland yet");
}

unsafe extern "C" fn set_maximized(engine_id: i64, view_id: FlutterViewId, maximized: bool) {
    if let Some(window) = window(engine_id, view_id) {
        window.set_maximized(maximized);
    }
}

unsafe extern "C" fn set_minimized(engine_id: i64, view_id: FlutterViewId, minimized: bool) {
    if !minimized {
        debug!("windowing: ignoring un-minimizing window {view_id}: Wayland has no way to");
        return;
    }
    if let Some(window) = window(engine_id, view_id) {
        window.set_minimized();
    }
}

unsafe extern "C" fn set_fullscreen(engine_id: i64, view_id: FlutterViewId, fullscreen: bool) {
    if let Some(window) = window(engine_id, view_id) {
        window.set_fullscreen(fullscreen);
    }
}

/// The functions Dart calls, by name. Their signatures are repeated in the
/// `flust_windowing` package.
fn functions() -> HashMap<String, Value> {
    type Engine = i64;
    type View = FlutterViewId;
    let functions: [(&str, usize); 17] = [
        (
            "createWindow",
            create_window
                as unsafe extern "C" fn(
                    Engine,
                    f64,
                    f64,
                    f64,
                    f64,
                    f64,
                    f64,
                    *const c_char,
                    View,
                ) -> i64 as usize,
        ),
        (
            "createPopup",
            create_popup
                as unsafe extern "C" fn(
                    Engine,
                    View,
                    f64,
                    f64,
                    f64,
                    f64,
                    u32,
                    u32,
                    f64,
                    f64,
                    u32,
                    f64,
                    f64,
                    f64,
                    f64,
                    bool,
                ) -> i64 as usize,
        ),
        (
            "updatePosition",
            update_position
                as unsafe extern "C" fn(Engine, View, f64, f64, f64, f64, u32, u32, f64, f64, u32)
                as usize,
        ),
        (
            "getOffsetX",
            get_offset_x as unsafe extern "C" fn(Engine, View) -> f64 as usize,
        ),
        (
            "getOffsetY",
            get_offset_y as unsafe extern "C" fn(Engine, View) -> f64 as usize,
        ),
        (
            "setContentConstraints",
            set_content_constraints as unsafe extern "C" fn(Engine, View, f64, f64, f64, f64)
                as usize,
        ),
        (
            "destroyWindow",
            destroy_window as unsafe extern "C" fn(Engine, View) as usize,
        ),
        (
            "getContentWidth",
            get_content_width as unsafe extern "C" fn(Engine, View) -> f64 as usize,
        ),
        (
            "getContentHeight",
            get_content_height as unsafe extern "C" fn(Engine, View) -> f64 as usize,
        ),
        (
            "getState",
            get_state as unsafe extern "C" fn(Engine, View) -> u32 as usize,
        ),
        (
            "setTitle",
            set_title as unsafe extern "C" fn(Engine, View, *const c_char) as usize,
        ),
        (
            "setSize",
            set_size as unsafe extern "C" fn(Engine, View, f64, f64) as usize,
        ),
        (
            "setConstraints",
            set_constraints as unsafe extern "C" fn(Engine, View, f64, f64, f64, f64) as usize,
        ),
        (
            "activate",
            activate as unsafe extern "C" fn(Engine, View) as usize,
        ),
        (
            "setMaximized",
            set_maximized as unsafe extern "C" fn(Engine, View, bool) as usize,
        ),
        (
            "setMinimized",
            set_minimized as unsafe extern "C" fn(Engine, View, bool) as usize,
        ),
        (
            "setFullscreen",
            set_fullscreen as unsafe extern "C" fn(Engine, View, bool) as usize,
        ),
    ];

    functions
        .into_iter()
        .map(|(name, address)| (name.to_owned(), Value::I64(address as i64)))
        .collect()
}

pub(crate) struct WindowingPlugin {
    channel: std::sync::Weak<MethodChannel>,
}

impl WindowingPlugin {
    pub(crate) fn new() -> Self {
        Self {
            channel: std::sync::Weak::new(),
        }
    }

    pub(crate) fn window_changed(&self, view_id: FlutterViewId) {
        self.notify("onWindowChanged", view_id);
    }

    pub(crate) fn window_close_requested(&self, view_id: FlutterViewId) {
        self.notify("onWindowCloseRequested", view_id);
    }

    pub(crate) fn window_destroyed(&self, view_id: FlutterViewId) {
        self.notify("onWindowDestroyed", view_id);
    }

    fn notify(&self, method: &str, view_id: FlutterViewId) {
        if let Some(channel) = self.channel.upgrade() {
            channel.invoke_method(method, HashMap::from([("viewId", view_id)]));
        }
    }
}

impl Plugin for WindowingPlugin {
    fn plugin_name() -> &'static str {
        module_path!()
    }

    fn init(&mut self, engine: &FlutterEngine) {
        self.channel =
            engine.register_channel(MethodChannel::new(CHANNEL_NAME, Handler, &STANDARD_CODEC));
    }
}

struct Handler;

impl MethodCallHandler for Handler {
    fn on_method_call(&mut self, call: MethodCall) {
        match call.method().as_str() {
            "initialize" => call.success(functions()),
            _ => call.not_implemented(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_limits_leave_out_what_is_not_limited() {
        assert_eq!(size_limit(0.0, 0.0), None);
        assert_eq!(size_limit(f64::INFINITY, f64::INFINITY), None);
        assert_eq!(size_limit(f64::NAN, -1.0), None);
        assert_eq!(
            size_limit(f64::INFINITY, 300.0),
            Some(LogicalSize::new(0, 300))
        );
        assert_eq!(size_limit(400.0, 0.0), Some(LogicalSize::new(400, 0)));
        assert_eq!(size_limit(399.6, 300.2), Some(LogicalSize::new(400, 300)));
    }
}
