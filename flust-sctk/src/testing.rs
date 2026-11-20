//! A way for the end-to-end tests to open and close windows from Dart, until
//! Flutter's windowing API can (https://forge.lab.vially.dev/vially/flust/issues/15).
//! Only built with the `testing` feature.
//!
//! The `flust/testing/windows` method channel takes:
//!
//! - `createWindow` with `{"title": String?, "width": int?, "height": int?}`
//!   (the size in logical pixels), answered with the new window's view id;
//! - `destroyWindow` with `{"viewId": int}`.
//!
//! and calls into Dart:
//!
//! - `onWindowCloseRequested` with `{"viewId": int}` when the compositor asks
//!   for a window to close (the window stays until Dart destroys it);
//! - `onWindowDestroyed` with `{"viewId": int}` when a window is gone.

use std::{collections::HashMap, sync::Weak};

use dpi::{LogicalSize, Size};
use flust_engine::{
    channel::{MethodCall, MethodCallHandler, MethodChannel},
    codec::{Value, JSON_CODEC},
    ffi::FlutterViewId,
    plugins::Plugin,
    FlutterEngine,
};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use tracing::warn;

use crate::{application::SctkApplicationState, window::SctkWindowAttributes};

const CHANNEL_NAME: &str = "flust/testing/windows";

/// A request from Dart, handled on the event loop, where the application's
/// state is at hand.
pub(crate) enum WindowRequest {
    Create(MethodCall, SctkWindowAttributes),
    Destroy(MethodCall, FlutterViewId),
}

impl WindowRequest {
    pub(crate) fn handle(self, state: &mut SctkApplicationState) {
        match self {
            WindowRequest::Create(call, attributes) => {
                match state.windows().create_window(attributes, None) {
                    Ok(window) => call.success(window.view_id()),
                    Err(err) => call.error("create-failed", err.to_string(), Value::Null),
                }
            }
            WindowRequest::Destroy(call, view_id) => {
                match state.windows().destroy_window(view_id) {
                    Ok(()) => call.success_empty(),
                    Err(err) => call.error("destroy-failed", err.to_string(), Value::Null),
                }
            }
        }
    }
}

pub(crate) struct WindowsTestingPlugin {
    channel: Weak<MethodChannel>,
    requests: Sender<WindowRequest>,
}

impl WindowsTestingPlugin {
    pub(crate) fn new(requests: Sender<WindowRequest>) -> Self {
        Self {
            channel: Weak::new(),
            requests,
        }
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

impl Plugin for WindowsTestingPlugin {
    fn plugin_name() -> &'static str {
        module_path!()
    }

    fn init(&mut self, engine: &FlutterEngine) {
        self.channel = engine.register_channel(MethodChannel::new(
            CHANNEL_NAME,
            Handler {
                requests: self.requests.clone(),
            },
            &JSON_CODEC,
        ));
    }
}

struct Handler {
    requests: Sender<WindowRequest>,
}

impl MethodCallHandler for Handler {
    fn on_method_call(&mut self, call: MethodCall) {
        let args = match call.raw_args() {
            Value::Map(args) => args.clone(),
            _ => Default::default(),
        };
        let int = |key: &str| match args.get(key) {
            Some(Value::I64(value)) => Some(*value),
            Some(Value::I32(value)) => Some(i64::from(*value)),
            _ => None,
        };

        let request = match call.method().as_str() {
            "createWindow" => {
                let title = match args.get("title") {
                    Some(Value::String(title)) => Some(title.clone()),
                    _ => None,
                };
                let inner_size = int("width").zip(int("height")).map(|(width, height)| {
                    Size::from(LogicalSize::new(width as f64, height as f64))
                });
                let attributes = SctkWindowAttributes {
                    title,
                    app_id: None,
                    inner_size,
                    ..Default::default()
                };
                WindowRequest::Create(call, attributes)
            }
            "destroyWindow" => match int("viewId") {
                Some(view_id) => WindowRequest::Destroy(call, view_id),
                None => return call.error("bad-arguments", "viewId is missing", Value::Null),
            },
            _ => return call.not_implemented(),
        };

        if let Err(err) = self.requests.send(request) {
            warn!("window request dropped: {err}");
        }
    }
}
