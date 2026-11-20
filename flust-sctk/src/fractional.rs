//! Fractional scales: `wp_fractional_scale_v1` says which scale the
//! compositor would like a surface drawn at, in 120ths, and a
//! `wp_viewport` shows a buffer of any size at the surface's logical size.
//! Both globals are needed; without them, windows are drawn at integer
//! scales only.

use smithay_client_toolkit::{
    globals::GlobalData,
    reexports::protocols::wp::{
        fractional_scale::v1::client::{
            wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
            wp_fractional_scale_v1::{self, WpFractionalScaleV1},
        },
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
};
use wayland_backend::client::ObjectId;
use wayland_client::{
    globals::GlobalList, protocol::wl_surface::WlSurface, Connection, Dispatch, Proxy,
    QueueHandle,
};

use crate::application::SctkApplicationState;

/// The globals for fractional scales.
#[derive(Debug)]
pub(crate) struct FractionalScaleGlobals {
    manager: WpFractionalScaleManagerV1,
    viewporter: WpViewporter,
}

impl FractionalScaleGlobals {
    /// Binds both globals, if the compositor has both.
    pub(crate) fn bind(
        globals: &GlobalList,
        qh: &QueueHandle<SctkApplicationState>,
    ) -> Option<Self> {
        Some(Self {
            manager: globals.bind(qh, 1..=1, GlobalData).ok()?,
            viewporter: globals.bind(qh, 1..=1, GlobalData).ok()?,
        })
    }

    /// Lets `surface` be drawn at fractional scales: its scale comes from
    /// the compositor's `preferred_scale` from now on.
    pub(crate) fn for_surface(
        &self,
        surface: &WlSurface,
        qh: &QueueHandle<SctkApplicationState>,
    ) -> FractionalScaling {
        FractionalScaling {
            fractional_scale: self
                .manager
                .get_fractional_scale(surface, qh, surface.id()),
            viewport: self.viewporter.get_viewport(surface, qh, ()),
        }
    }
}

/// A surface's objects for fractional scales. Destroyed when dropped.
#[derive(Debug)]
pub(crate) struct FractionalScaling {
    fractional_scale: WpFractionalScaleV1,
    viewport: WpViewport,
}

impl FractionalScaling {
    /// Shows the surface's buffer at `size`, in logical pixels, or at the
    /// size the buffer and its scale give it (`None`). Takes effect at the
    /// next commit.
    pub(crate) fn set_destination(&self, size: Option<(u32, u32)>) {
        let clamp = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
        match size {
            Some((width, height)) => self.viewport.set_destination(clamp(width), clamp(height)),
            None => self.viewport.set_destination(-1, -1),
        }
    }
}

impl Drop for FractionalScaling {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.fractional_scale.destroy();
    }
}

impl Dispatch<WpFractionalScaleV1, ObjectId> for SctkApplicationState {
    fn event(
        state: &mut Self,
        _fractional_scale: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface_id: &ObjectId,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.preferred_scale_changed(surface_id, scale);
        }
    }
}

impl Dispatch<WpFractionalScaleManagerV1, GlobalData> for SctkApplicationState {
    fn event(
        _state: &mut Self,
        _manager: &WpFractionalScaleManagerV1,
        _event: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        unreachable!("wp_fractional_scale_manager_v1 has no events")
    }
}

impl Dispatch<WpViewporter, GlobalData> for SctkApplicationState {
    fn event(
        _state: &mut Self,
        _viewporter: &WpViewporter,
        _event: <WpViewporter as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        unreachable!("wp_viewporter has no events")
    }
}

impl Dispatch<WpViewport, ()> for SctkApplicationState {
    fn event(
        _state: &mut Self,
        _viewport: &WpViewport,
        _event: <WpViewport as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        unreachable!("wp_viewport has no events")
    }
}
