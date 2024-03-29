use std::time::SystemTimeError;

use dpi::LogicalPosition;
use flutter_engine::ffi::FlutterPointerEvent;
use flutter_engine::ffi::{
    FlutterPointerDeviceKind, FlutterPointerMouseButtons, FlutterPointerPhase,
    FlutterPointerSignalKind,
};
use smithay_client_toolkit::seat::pointer::{
    PointerEvent, PointerEventKind, BTN_BACK, BTN_EXTRA, BTN_FORWARD, BTN_LEFT, BTN_RIGHT, BTN_SIDE,
};
use thiserror::Error;

#[derive(Copy, Clone, Debug)]
pub(crate) struct Pointer {
    pub(crate) device: i32,
    pub(crate) pressed: u32,
}

impl Pointer {
    pub(crate) fn new(device: i32) -> Self {
        Self { device, pressed: 0 }
    }

    pub(crate) fn increment_pressed(&mut self) {
        self.pressed += 1;
    }

    pub(crate) fn decrement_pressed(&mut self) {
        self.pressed -= 1;
    }
}

#[derive(Error, Debug)]
pub enum PointerConversionError {
    #[error("Invalid pointer conversion")]
    Invalid,

    #[error(transparent)]
    SystemTimeError(#[from] SystemTimeError),
}

#[derive(Debug, Clone)]
pub(crate) struct SctkPointerEvent(PointerEvent, Pointer, f64);

impl SctkPointerEvent {
    pub(crate) fn new(event: PointerEvent, pointer: Pointer, scale_factor: f64) -> Self {
        Self(event, pointer, scale_factor)
    }
}

impl TryFrom<SctkPointerEvent> for FlutterPointerEvent {
    type Error = PointerConversionError;

    fn try_from(
        SctkPointerEvent(event, pointer, scale_factor): SctkPointerEvent,
    ) -> Result<Self, Self::Error> {
        use PointerEventKind::*;

        // Convert pointer coordinates from logical to physical pixels
        let physical_position =
            LogicalPosition::<f64>::from(event.position).to_physical::<f64>(scale_factor);
        let (x, y) = (physical_position.x, physical_position.y);

        match event.kind {
            Enter { .. } => Ok(FlutterPointerEvent::new(
                pointer.device,
                FlutterPointerPhase::Add,
                (x, y),
                FlutterPointerSignalKind::None,
                (0.0, 0.0),
                FlutterPointerDeviceKind::Mouse,
                FlutterPointerMouseButtons::None,
            )),
            Leave { .. } => Ok(FlutterPointerEvent::new(
                pointer.device,
                FlutterPointerPhase::Remove,
                (x, y),
                FlutterPointerSignalKind::None,
                (0.0, 0.0),
                FlutterPointerDeviceKind::Mouse,
                FlutterPointerMouseButtons::None,
            )),
            Motion { .. } => Ok(FlutterPointerEvent::new(
                pointer.device,
                if pointer.pressed > 0 {
                    FlutterPointerPhase::Move
                } else {
                    FlutterPointerPhase::Hover
                },
                (x, y),
                FlutterPointerSignalKind::None,
                (0.0, 0.0),
                FlutterPointerDeviceKind::Mouse,
                FlutterPointerMouseButtons::None,
            )),
            Press { button, .. } => Ok(FlutterPointerEvent::new(
                pointer.device,
                FlutterPointerPhase::Down,
                (x, y),
                FlutterPointerSignalKind::None,
                (0.0, 0.0),
                FlutterPointerDeviceKind::Mouse,
                pointer_mouse_buttons_from_wayland(button),
            )),
            Release { button, .. } => Ok(FlutterPointerEvent::new(
                pointer.device,
                FlutterPointerPhase::Up,
                (x, y),
                FlutterPointerSignalKind::None,
                (0.0, 0.0),
                FlutterPointerDeviceKind::Mouse,
                pointer_mouse_buttons_from_wayland(button),
            )),
            Axis {
                horizontal,
                vertical,
                ..
            } => Ok(FlutterPointerEvent::new(
                pointer.device,
                if pointer.pressed > 0 {
                    FlutterPointerPhase::Move
                } else {
                    FlutterPointerPhase::Hover
                },
                (x, y),
                FlutterPointerSignalKind::Scroll,
                (horizontal.discrete as f64, vertical.discrete as f64),
                FlutterPointerDeviceKind::Mouse,
                // TODO: Are these values correct?
                FlutterPointerMouseButtons::None,
            )),
        }
    }
}

fn pointer_mouse_buttons_from_wayland(button: u32) -> FlutterPointerMouseButtons {
    match button {
        BTN_LEFT => FlutterPointerMouseButtons::Primary,
        BTN_RIGHT => FlutterPointerMouseButtons::Secondary,
        BTN_BACK | BTN_SIDE => FlutterPointerMouseButtons::Back,
        BTN_FORWARD | BTN_EXTRA => FlutterPointerMouseButtons::Forward,
        _ => FlutterPointerMouseButtons::None,
    }
}
