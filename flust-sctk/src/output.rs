use dpi::PhysicalSize;
use flust_engine::ffi::FlutterEngineDisplay;
use flutter_engine_sys::FlutterEngineDisplayId;
use smithay_client_toolkit::output::OutputInfo;

#[derive(Debug, Clone)]
pub(crate) struct SctkOutput {
    pub(crate) display_id: FlutterEngineDisplayId,
    pub(crate) refresh_rate: f64,
    pub(crate) size: PhysicalSize<usize>,
    pub(crate) device_pixel_ratio: f64,
}

impl SctkOutput {
    pub(crate) fn new(display_id: FlutterEngineDisplayId, info: Option<OutputInfo>) -> Self {
        let Some(info) = info.as_ref() else {
            return Self {
                display_id,
                refresh_rate: 0.0,
                size: PhysicalSize::new(0, 0),
                device_pixel_ratio: 1.0,
            };
        };

        let device_pixel_ratio = info.scale_factor as f64;

        let current_mode = info.modes.iter().find(|mode| mode.current);

        let refresh_rate = current_mode
            .map(|mode| mode.refresh_rate as f64 / 1000.0)
            .unwrap_or(0.0);

        let size = current_mode
            .and_then(|mode| {
                let (width, height) = mode.dimensions;

                Some(PhysicalSize::new(
                    width.try_into().ok()?,
                    height.try_into().ok()?,
                ))
            })
            .unwrap_or_default();

        Self {
            display_id,
            refresh_rate,
            size,
            device_pixel_ratio,
        }
    }
}

impl From<SctkOutput> for FlutterEngineDisplay {
    fn from(output: SctkOutput) -> Self {
        Self {
            display_id: output.display_id,
            single_display: false,
            refresh_rate: output.refresh_rate,
            size: output.size,
            device_pixel_ratio: output.device_pixel_ratio,
        }
    }
}
