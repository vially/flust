//! Plugin to work with locales.
//! It handles flutter/localization type message.

use icu_locid::Locale;
use std::sync::Weak;
use tracing::{debug, error, info, warn};

use flutter_engine::channel::MethodCall;
use flutter_engine::{
    channel::{MethodCallHandler, MethodChannel},
    codec::JSON_CODEC,
    plugins::Plugin,
    FlutterEngine,
};

pub const PLUGIN_NAME: &str = module_path!();
pub const CHANNEL_NAME: &str = "flutter/localization";

pub struct LocalizationPlugin {
    channel: Weak<MethodChannel>,
}

impl Default for LocalizationPlugin {
    fn default() -> Self {
        Self {
            channel: Weak::new(),
        }
    }
}

impl Plugin for LocalizationPlugin {
    fn plugin_name() -> &'static str {
        PLUGIN_NAME
    }

    fn init(&mut self, engine: &FlutterEngine) {
        self.channel =
            engine.register_channel(MethodChannel::new(CHANNEL_NAME, Handler, &JSON_CODEC));
    }
}

impl LocalizationPlugin {
    pub fn send_locale(&self, locale: String) {
        debug!("Sending locales to flutter");
        if let Some(channel) = self.channel.upgrade() {
            let mut languages = Vec::<String>::new();
            if let Ok(loc) = locale.parse::<Locale>() {
                info!("Available locale: {}", loc);
                if let (Some(region), Some(script)) = (loc.id.region, loc.id.script) {
                    languages.push(loc.id.language.as_str().to_owned());
                    languages.push(region.as_str().to_owned());
                    languages.push(script.as_str().to_owned());
                    languages.push(
                        loc.id
                            .variants
                            .first()
                            .map_or("", |v| v.as_str())
                            .to_owned(),
                    );
                } else {
                    warn!("Failed to unwrap locale region and/or script: {}", locale);
                }
            } else {
                warn!("Failed to parse locale: {}", locale);
            }

            channel.invoke_method("setLocale", languages)
        } else {
            error!("Failed to upgrade channel to send message");
        }
    }
}

struct Handler;

impl MethodCallHandler for Handler {
    fn on_method_call(&mut self, call: MethodCall) {
        debug!(
            "got method call {} with args {:?}",
            call.method(),
            call.raw_args()
        );
        call.not_implemented()
    }
}
