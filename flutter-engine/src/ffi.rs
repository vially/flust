use std::{
    mem,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dpi::PhysicalSize;

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

impl From<FlutterPointerPhase> for flutter_engine_sys::FlutterPointerPhase {
    fn from(pointer_phase: FlutterPointerPhase) -> Self {
        match pointer_phase {
            FlutterPointerPhase::Cancel => flutter_engine_sys::FlutterPointerPhase::kCancel,
            FlutterPointerPhase::Up => flutter_engine_sys::FlutterPointerPhase::kUp,
            FlutterPointerPhase::Down => flutter_engine_sys::FlutterPointerPhase::kDown,
            FlutterPointerPhase::Move => flutter_engine_sys::FlutterPointerPhase::kMove,
            FlutterPointerPhase::Add => flutter_engine_sys::FlutterPointerPhase::kAdd,
            FlutterPointerPhase::Remove => flutter_engine_sys::FlutterPointerPhase::kRemove,
            FlutterPointerPhase::Hover => flutter_engine_sys::FlutterPointerPhase::kHover,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FlutterPointerDeviceKind {
    Mouse,
    Touch,
}

impl From<FlutterPointerDeviceKind> for flutter_engine_sys::FlutterPointerDeviceKind {
    fn from(device_kind: FlutterPointerDeviceKind) -> Self {
        match device_kind {
            FlutterPointerDeviceKind::Mouse => {
                flutter_engine_sys::FlutterPointerDeviceKind::kFlutterPointerDeviceKindMouse
            }
            FlutterPointerDeviceKind::Touch => {
                flutter_engine_sys::FlutterPointerDeviceKind::kFlutterPointerDeviceKindTouch
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlutterPointerSignalKind {
    None,
    Scroll,
}

impl From<FlutterPointerSignalKind> for flutter_engine_sys::FlutterPointerSignalKind {
    fn from(pointer_signal_kind: FlutterPointerSignalKind) -> Self {
        match pointer_signal_kind {
            FlutterPointerSignalKind::None => {
                flutter_engine_sys::FlutterPointerSignalKind::kFlutterPointerSignalKindNone
            }
            FlutterPointerSignalKind::Scroll => {
                flutter_engine_sys::FlutterPointerSignalKind::kFlutterPointerSignalKindScroll
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
}

impl FlutterPointerEvent {
    pub fn new(
        device: i32,
        phase: FlutterPointerPhase,
        (x, y): (f64, f64),
        signal_kind: FlutterPointerSignalKind,
        (scroll_delta_x, scroll_delta_y): (f64, f64),
        device_kind: FlutterPointerDeviceKind,
        buttons: FlutterPointerMouseButtons,
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
        }
    }
}

impl From<FlutterPointerEvent> for flutter_engine_sys::FlutterPointerEvent {
    fn from(event: FlutterPointerEvent) -> Self {
        Self {
            struct_size: mem::size_of::<flutter_engine_sys::FlutterPointerEvent>(),
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
            #[cfg(all(target_arch = "arm", target_os = "android"))]
            __bindgen_padding_0: 0,
            #[cfg(all(target_arch = "arm", target_os = "android"))]
            __bindgen_padding_1: 0,
        }
    }
}

pub struct FlutterFrameInfo {
    pub size: PhysicalSize<u32>,
}

impl From<flutter_engine_sys::FlutterFrameInfo> for FlutterFrameInfo {
    fn from(frame_info: flutter_engine_sys::FlutterFrameInfo) -> Self {
        Self {
            size: PhysicalSize::new(frame_info.size.width, frame_info.size.height),
        }
    }
}
