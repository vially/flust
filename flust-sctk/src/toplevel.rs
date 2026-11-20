//! An `xdg_toplevel` window that leaves `xdg_surface.ack_configure` to its
//! owner.
//!
//! SCTK's `Window` acknowledges every configure as soon as it arrives, before
//! its handler runs. An acknowledgement takes effect at the next
//! `wl_surface.commit`, and that commit is expected to carry content drawn
//! for the acknowledged configure. Flutter draws on another thread, some
//! frames later, so the acknowledgement has to wait for that frame: see
//! `SurfaceSync`.

use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use smithay_client_toolkit::{
    globals::{GlobalData, ProvidesBoundGlobal},
    reexports::protocols::xdg::{
        decoration::zv1::client::{
            zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
            zxdg_toplevel_decoration_v1::{self, ZxdgToplevelDecorationV1},
        },
        shell::client::{
            xdg_surface::{self, XdgSurface},
            xdg_toplevel::{self, XdgToplevel},
        },
    },
    shell::xdg::{XdgShell, XdgShellSurface},
};
use wayland_client::{protocol::wl_surface::WlSurface, Connection, Dispatch, QueueHandle};

use crate::application::SctkApplicationState;

/// What the compositor asked for in one configure sequence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ToplevelConfigure {
    /// The size of the window in surface coordinates. `None` leaves it to
    /// the client.
    pub(crate) size: Option<(NonZeroU32, NonZeroU32)>,
    pub(crate) activated: bool,
    pub(crate) maximized: bool,
    pub(crate) fullscreen: bool,
}

/// User data of the window's `xdg_surface`, `xdg_toplevel` and decoration
/// objects: the state of the configure sequence that is being received.
#[derive(Clone, Debug, Default)]
pub(crate) struct ToplevelData(Arc<Mutex<ToplevelConfigure>>);

pub(crate) struct Toplevel {
    // Field order is destruction order: the decoration goes before the
    // toplevel, and the toplevel before its `xdg_surface` (xdg-decoration and
    // xdg-shell both require it).
    _decoration: Option<Decoration>,
    xdg_toplevel: OwnedXdgToplevel,
    xdg_surface: XdgShellSurface,
}

impl Toplevel {
    /// Gives `surface` the toplevel role. Takes ownership of the surface.
    ///
    /// Server-side decorations are used when the compositor offers them. No
    /// client-side decorations are drawn otherwise.
    pub(crate) fn new(
        xdg_shell: &XdgShell,
        surface: WlSurface,
        qh: &QueueHandle<SctkApplicationState>,
    ) -> Self {
        let data = ToplevelData::default();

        // `XdgShell` always holds the `xdg_wm_base` global, so this can not
        // fail.
        let xdg_surface = XdgShellSurface::new(xdg_shell, qh, surface, data.clone())
            .expect("xdg_wm_base is bound");
        let xdg_toplevel = xdg_surface.xdg_surface().get_toplevel(qh, data.clone());

        let decoration_manager =
            <XdgShell as ProvidesBoundGlobal<ZxdgDecorationManagerV1, 1>>::bound_global(xdg_shell)
                .ok();
        let decoration = decoration_manager
            .map(|manager| Decoration(manager.get_toplevel_decoration(&xdg_toplevel, qh, data)));

        Self {
            _decoration: decoration,
            xdg_toplevel: OwnedXdgToplevel(xdg_toplevel),
            xdg_surface,
        }
    }

    pub(crate) fn wl_surface(&self) -> &WlSurface {
        self.xdg_surface.wl_surface()
    }

    pub(crate) fn xdg_surface(&self) -> &XdgSurface {
        self.xdg_surface.xdg_surface()
    }

    pub(crate) fn xdg_toplevel(&self) -> &XdgToplevel {
        &self.xdg_toplevel.0
    }
}

struct OwnedXdgToplevel(XdgToplevel);

impl Drop for OwnedXdgToplevel {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

struct Decoration(ZxdgToplevelDecorationV1);

impl Drop for Decoration {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

impl Dispatch<XdgToplevel, ToplevelData> for SctkApplicationState {
    fn event(
        state: &mut Self,
        xdg_toplevel: &XdgToplevel,
        event: xdg_toplevel::Event,
        data: &ToplevelData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                // Negative sizes are invalid, and zero leaves the size to the
                // client.
                let width = u32::try_from(width).ok().and_then(NonZeroU32::new);
                let height = u32::try_from(height).ok().and_then(NonZeroU32::new);

                // The states are an array of native-endian `u32`.
                let states: Vec<u32> = states
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|state| u32::from_ne_bytes(*state))
                    .collect();
                let has = |state: xdg_toplevel::State| states.contains(&u32::from(state));

                let mut configure = data.0.lock().unwrap();
                configure.size = width.zip(height);
                configure.activated = has(xdg_toplevel::State::Activated);
                configure.maximized = has(xdg_toplevel::State::Maximized);
                configure.fullscreen = has(xdg_toplevel::State::Fullscreen);
            }
            xdg_toplevel::Event::Close => state.toplevel_close_requested(xdg_toplevel),
            // The bounds and the window manager's capabilities are not used.
            _ => {}
        }
    }
}

impl Dispatch<XdgSurface, ToplevelData> for SctkApplicationState {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        data: &ToplevelData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            let configure = *data.0.lock().unwrap();
            state.toplevel_configured(xdg_surface, configure, serial);
        }
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, ToplevelData> for SctkApplicationState {
    fn event(
        _state: &mut Self,
        _decoration: &ZxdgToplevelDecorationV1,
        _event: zxdg_toplevel_decoration_v1::Event,
        _data: &ToplevelData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The only event is the decoration mode. Nothing is drawn differently
        // for either mode.
    }
}

impl Dispatch<ZxdgDecorationManagerV1, GlobalData> for SctkApplicationState {
    fn event(
        _state: &mut Self,
        _manager: &ZxdgDecorationManagerV1,
        _event: <ZxdgDecorationManagerV1 as wayland_client::Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        unreachable!("zxdg_decoration_manager_v1 has no events")
    }
}
