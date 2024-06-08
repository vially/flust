use std::sync::{Arc, RwLock, Weak};

use tracing::error;

use crate::channel::method_channel::MethodCallHandler;
use crate::{
    channel::{ChannelImpl, EventHandler, MethodCallHandler, MethodChannel},
    codec::{standard_codec::CODEC, MethodCall, MethodCodec, Value},
    error::MethodCallError,
    FlutterEngine, FlutterEngineWeakRef,
};

pub trait EventHandler {
    fn on_listen(&mut self, args: Value, engine: FlutterEngine) -> Result<Value, MethodCallError>;
    fn on_cancel(&mut self, engine: FlutterEngine) -> Result<Value, MethodCallError>;
}

pub struct EventChannel {
    name: String,
    engine: FlutterEngineWeakRef,
    method_handler: Arc<RwLock<dyn MethodCallHandler + Send + Sync>>,
    plugin_name: Option<&'static str>,
}

struct EventChannelMethodCallHandler {
    event_handler: Weak<RwLock<dyn EventHandler + Send + Sync>>,
}

impl EventChannel {
    pub fn new<N: AsRef<str>>(
        name: N,
        handler: Weak<RwLock<dyn EventHandler + Send + Sync>>,
    ) -> Self {
        Self {
            name: name.as_ref().to_owned(),
            engine: Default::default(),
            method_handler: Arc::new(RwLock::new(EventChannelMethodCallHandler::new(handler))),
            plugin_name: None,
        }
    }

    /// When flutter listen to a stream of events using EventChannel.
    /// This method send back a success event.
    /// It can be call multiple times to simulate stream.
    pub fn send_success_event(&self, data: &Value) {
        let buf = self.codec().encode_success_envelope(data);
        self.send_buffer(&buf);
    }

    /// When flutter listen to a stream of events using EventChannel.
    /// This method send back a error event.
    /// It can be call multiple times to simulate stream.
    pub fn send_error_event(&self, code: &str, message: &str, data: &Value) {
        let buf = self.codec().encode_error_envelope(code, message, data);
        self.send_buffer(&buf);
    }
}

impl ChannelImpl for EventChannel {
    fn name(&self) -> &str {
        self.name.as_str()
    }

    fn engine(&self) -> Option<FlutterEngine> {
        self.engine.upgrade()
    }

    fn init(&mut self, engine: FlutterEngineWeakRef, plugin_name: &'static str) {
        if self.engine.upgrade().is_some() {
            error!("Channel {} was already initialized", self.name);
        }
        self.engine = engine;
        self.plugin_name.replace(plugin_name);
    }

    fn plugin_name(&self) -> &'static str {
        self.plugin_name.unwrap()
    }
}

impl MethodChannel for EventChannel {
    fn method_handler(&self) -> Option<Arc<RwLock<dyn MethodCallHandler + Send + Sync>>> {
        Some(Arc::clone(&self.method_handler))
    }

    fn codec(&self) -> &'static dyn MethodCodec {
        &CODEC
    }
}

impl EventChannelMethodCallHandler {
    pub fn new(handler: Weak<RwLock<dyn EventHandler + Send + Sync>>) -> Self {
        Self {
            event_handler: handler,
        }
    }
}

impl MethodCallHandler for EventChannelMethodCallHandler {
    fn on_method_call(
        &mut self,
        call: MethodCall,
        engine: FlutterEngine,
    ) -> Result<Value, MethodCallError> {
        if let Some(handler) = self.event_handler.upgrade() {
            let mut handler = handler.write().unwrap();
            match call.method.as_str() {
                "listen" => handler.on_listen(call.args, engine),
                "cancel" => handler.on_cancel(engine),
                _ => Err(MethodCallError::NotImplemented),
            }
        } else {
            Err(MethodCallError::ChannelClosed)
        }
    }
}
