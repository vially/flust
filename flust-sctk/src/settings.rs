//! The settings of the desktop that the framework adapts to, from the
//! settings portal.

use ashpd::{
    desktop::settings::Settings,
    zvariant::{OwnedValue, Value},
};
use flust_accessibility::Accessibility;
use flust_engine::semantics::{FlutterAccessibilityFeature, FlutterAccessibilityFeatures};
use flust_plugins::settings::{PlatformBrightness, SettingsPlugin};
use futures_lite::StreamExt;
use tracing::debug;

use crate::handler::SctkAsyncResult;

/// Settings that every desktop has, if it has them at all.
const APPEARANCE: &str = "org.freedesktop.appearance";
/// GNOME's settings, which other desktops provide as well for the
/// applications that only look there.
const GNOME_INTERFACE: &str = "org.gnome.desktop.interface";

/// The settings as the portal has them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DesktopSettings {
    /// `1` for dark, `2` for light, `0` for no preference.
    color_scheme: u32,
    /// `1` for more contrast.
    contrast: u32,
    /// `1` for less motion.
    reduced_motion: u32,
    enable_animations: bool,
    text_scaling_factor: f64,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            color_scheme: 0,
            contrast: 0,
            reduced_motion: 0,
            enable_animations: true,
            text_scaling_factor: 1.0,
        }
    }
}

impl DesktopSettings {
    /// Takes in the value of a setting. Returns whether it is one of the
    /// settings in here and has changed.
    pub(crate) fn set(&mut self, namespace: &str, key: &str, value: &Value<'_>) -> bool {
        // Some portals wrap the value in another variant.
        if let Value::Value(inner) = value {
            return self.set(namespace, key, inner);
        }

        let before = self.clone();
        match (namespace, key, value) {
            (APPEARANCE, "color-scheme", Value::U32(value)) => self.color_scheme = *value,
            (APPEARANCE, "contrast", Value::U32(value)) => self.contrast = *value,
            (APPEARANCE, "reduced-motion", Value::U32(value)) => self.reduced_motion = *value,
            (GNOME_INTERFACE, "enable-animations", Value::Bool(value)) => {
                self.enable_animations = *value
            }
            (GNOME_INTERFACE, "text-scaling-factor", Value::F64(value)) => {
                // A factor that is not a size makes no sense, and no text
                // shows up at all with it.
                if value.is_finite() && *value > 0.0 {
                    self.text_scaling_factor = *value
                }
            }
            _ => {}
        }
        *self != before
    }

    pub(crate) fn platform_brightness(&self) -> PlatformBrightness {
        match self.color_scheme {
            1 => PlatformBrightness::Dark,
            // Light is also what there is without a preference.
            _ => PlatformBrightness::Light,
        }
    }

    pub(crate) fn text_scale_factor(&self) -> f64 {
        self.text_scaling_factor
    }

    pub(crate) fn accessibility_features(&self) -> FlutterAccessibilityFeatures {
        let mut features = FlutterAccessibilityFeatures::empty();
        if self.contrast == 1 {
            features.insert(FlutterAccessibilityFeature::HighContrast);
        }
        if self.reduced_motion == 1 {
            features.insert(FlutterAccessibilityFeature::ReduceMotion);
        }
        // Switching animations off is how desktops asked for less motion
        // before there was a setting for it.
        if self.reduced_motion == 1 || !self.enable_animations {
            features.insert(FlutterAccessibilityFeature::DisableAnimations);
        }
        features
    }

    fn send(&self, plugin: &SettingsPlugin, accessibility: &Accessibility) {
        debug!("desktop settings: {self:?}");
        plugin
            .start_message()
            .set_platform_brightness(self.platform_brightness())
            .set_use_24_hour_format(true)
            .set_text_scale_factor(self.text_scale_factor())
            .send();
        accessibility.set_platform_features(self.accessibility_features());
    }
}

/// Passes the settings of the desktop on to the framework, now and whenever
/// they change. Runs until the portal goes away.
///
/// Has to run on the platform thread.
// Note: zbus is runtime-agnostic and should work out of the box with
// different Rust async runtimes. However, in order to achieve that, zbus
// spawns a thread per connection to handle various internal tasks.
//
// https://docs.rs/zbus/5.19.0/zbus/#compatibility-with-async-runtimes
pub(crate) async fn read_and_monitor_desktop_settings(
    plugin: SettingsPlugin,
    accessibility: Accessibility,
) -> SctkAsyncResult {
    let portal = Settings::new().await?;

    // Listening before reading, so that a change in between is not missed.
    let mut changes = portal.receive_setting_changed().await?;

    let mut settings = DesktopSettings::default();
    for (namespace, values) in portal.read_all(&[APPEARANCE, GNOME_INTERFACE]).await? {
        for (key, value) in values {
            settings.set(&namespace, &key, &value);
        }
    }
    settings.send(&plugin, &accessibility);

    while let Some(change) = changes.next().await {
        let value: &OwnedValue = change.value();
        if settings.set(change.namespace(), change.key(), value) {
            settings.send(&plugin, &accessibility);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(settings: &DesktopSettings) -> Vec<FlutterAccessibilityFeature> {
        settings.accessibility_features().iter().collect()
    }

    #[test]
    fn defaults() {
        let settings = DesktopSettings::default();

        assert!(matches!(
            settings.platform_brightness(),
            PlatformBrightness::Light
        ));
        assert_eq!(settings.text_scale_factor(), 1.0);
        assert!(settings.accessibility_features().is_empty());
    }

    #[test]
    fn color_scheme() {
        let mut settings = DesktopSettings::default();

        assert!(settings.set(APPEARANCE, "color-scheme", &Value::U32(1)));
        assert!(matches!(
            settings.platform_brightness(),
            PlatformBrightness::Dark
        ));

        assert!(settings.set(APPEARANCE, "color-scheme", &Value::U32(2)));
        assert!(matches!(
            settings.platform_brightness(),
            PlatformBrightness::Light
        ));
    }

    #[test]
    fn a_setting_that_stays_the_same_has_not_changed() {
        let mut settings = DesktopSettings::default();

        assert!(settings.set(APPEARANCE, "contrast", &Value::U32(1)));
        assert!(!settings.set(APPEARANCE, "contrast", &Value::U32(1)));
    }

    #[test]
    fn other_settings_are_ignored() {
        let mut settings = DesktopSettings::default();

        assert!(!settings.set(GNOME_INTERFACE, "cursor-size", &Value::I32(48)));
        assert!(!settings.set("org.example", "contrast", &Value::U32(1)));
        // Not the type the setting has.
        assert!(!settings.set(APPEARANCE, "contrast", &Value::Bool(true)));
        assert_eq!(settings, DesktopSettings::default());
    }

    #[test]
    fn a_value_in_a_variant() {
        let mut settings = DesktopSettings::default();
        let wrapped = Value::Value(Box::new(Value::U32(1)));

        assert!(settings.set(APPEARANCE, "contrast", &wrapped));
        assert_eq!(
            features(&settings),
            [FlutterAccessibilityFeature::HighContrast]
        );
    }

    #[test]
    fn high_contrast() {
        let mut settings = DesktopSettings::default();

        settings.set(APPEARANCE, "contrast", &Value::U32(1));
        assert_eq!(
            features(&settings),
            [FlutterAccessibilityFeature::HighContrast]
        );

        settings.set(APPEARANCE, "contrast", &Value::U32(0));
        assert!(features(&settings).is_empty());
    }

    #[test]
    fn reduced_motion() {
        let mut settings = DesktopSettings::default();

        settings.set(APPEARANCE, "reduced-motion", &Value::U32(1));

        assert_eq!(
            features(&settings),
            [
                FlutterAccessibilityFeature::DisableAnimations,
                FlutterAccessibilityFeature::ReduceMotion,
            ]
        );
    }

    #[test]
    fn animations_switched_off() {
        let mut settings = DesktopSettings::default();

        settings.set(GNOME_INTERFACE, "enable-animations", &Value::Bool(false));

        assert_eq!(
            features(&settings),
            [FlutterAccessibilityFeature::DisableAnimations]
        );
    }

    #[test]
    fn text_scale_factor() {
        let mut settings = DesktopSettings::default();

        assert!(settings.set(GNOME_INTERFACE, "text-scaling-factor", &Value::F64(1.5)));
        assert_eq!(settings.text_scale_factor(), 1.5);

        assert!(!settings.set(GNOME_INTERFACE, "text-scaling-factor", &Value::F64(0.0)));
        assert!(!settings.set(
            GNOME_INTERFACE,
            "text-scaling-factor",
            &Value::F64(f64::NAN)
        ));
        assert_eq!(settings.text_scale_factor(), 1.5);
    }
}
