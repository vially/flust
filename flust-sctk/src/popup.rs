//! An `xdg_popup` window: placed by the compositor relative to a rectangle
//! of its parent, with an `xdg_positioner`. Like `Toplevel`, it leaves
//! `xdg_surface.ack_configure` to its owner.

use std::sync::{Arc, Mutex};

use smithay_client_toolkit::{
    compositor::{CompositorState, Region},
    reexports::protocols::xdg::shell::client::{
        xdg_popup::{self, XdgPopup},
        xdg_positioner::{self, Anchor, ConstraintAdjustment, Gravity, XdgPositioner},
        xdg_surface::{self, XdgSurface},
        xdg_wm_base::XdgWmBase,
    },
    shell::xdg::{XdgShell, XdgShellSurface},
};
use tracing::warn;
use wayland_client::{
    protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    Connection, Dispatch, Proxy, QueueHandle,
};

use crate::application::SctkApplicationState;

/// Where a popup goes, in the logical coordinates of its parent's surface.
/// Mirrors Flutter's `WindowPositioner`, which was modelled on
/// `xdg_positioner`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PopupPlacement {
    /// The rectangle the popup is placed against: x, y, width, height.
    pub(crate) anchor_rect: (i32, i32, i32, i32),
    /// The point of the anchor rectangle the popup is placed at.
    pub(crate) parent_anchor: PlacementAnchor,
    /// The point of the popup that goes there.
    pub(crate) child_anchor: PlacementAnchor,
    pub(crate) offset: (i32, i32),
    /// What the compositor may do when the popup does not fit: Flutter's
    /// `WindowPositionerConstraintAdjustment`.
    pub(crate) flip_x: bool,
    pub(crate) flip_y: bool,
    pub(crate) slide_x: bool,
    pub(crate) slide_y: bool,
    pub(crate) resize_x: bool,
    pub(crate) resize_y: bool,
}

/// A point of a rectangle: Flutter's `WindowPositionerAnchor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlacementAnchor {
    Center,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    BottomLeft,
    TopRight,
    BottomRight,
}

impl PlacementAnchor {
    /// From the index of Flutter's `WindowPositionerAnchor`.
    pub(crate) fn from_index(index: u32) -> Option<Self> {
        Some(match index {
            0 => Self::Center,
            1 => Self::Top,
            2 => Self::Bottom,
            3 => Self::Left,
            4 => Self::Right,
            5 => Self::TopLeft,
            6 => Self::BottomLeft,
            7 => Self::TopRight,
            8 => Self::BottomRight,
            _ => return None,
        })
    }

    fn anchor(self) -> Anchor {
        match self {
            Self::Center => Anchor::None,
            Self::Top => Anchor::Top,
            Self::Bottom => Anchor::Bottom,
            Self::Left => Anchor::Left,
            Self::Right => Anchor::Right,
            Self::TopLeft => Anchor::TopLeft,
            Self::BottomLeft => Anchor::BottomLeft,
            Self::TopRight => Anchor::TopRight,
            Self::BottomRight => Anchor::BottomRight,
        }
    }

    /// The gravity that puts this point of the popup on the anchor: the
    /// popup extends away from it.
    fn gravity(self) -> Gravity {
        match self {
            Self::Center => Gravity::None,
            Self::Top => Gravity::Bottom,
            Self::Bottom => Gravity::Top,
            Self::Left => Gravity::Right,
            Self::Right => Gravity::Left,
            Self::TopLeft => Gravity::BottomRight,
            Self::BottomLeft => Gravity::TopRight,
            Self::TopRight => Gravity::BottomLeft,
            Self::BottomRight => Gravity::TopLeft,
        }
    }
}

impl PopupPlacement {
    fn constraint_adjustment(&self) -> ConstraintAdjustment {
        [
            (self.flip_x, ConstraintAdjustment::FlipX),
            (self.flip_y, ConstraintAdjustment::FlipY),
            (self.slide_x, ConstraintAdjustment::SlideX),
            (self.slide_y, ConstraintAdjustment::SlideY),
            (self.resize_x, ConstraintAdjustment::ResizeX),
            (self.resize_y, ConstraintAdjustment::ResizeY),
        ]
        .into_iter()
        .filter(|(set, _)| *set)
        .fold(ConstraintAdjustment::None, |all, (_, adjustment)| {
            all | adjustment
        })
    }

    /// A positioner for a popup of `size` (logical) placed like this.
    fn positioner(
        &self,
        xdg_wm_base: &XdgWmBase,
        size: (i32, i32),
        qh: &QueueHandle<SctkApplicationState>,
    ) -> OwnedPositioner {
        let positioner = xdg_wm_base.create_positioner(qh, ());
        // Both have to be at least 1x1.
        positioner.set_size(size.0.max(1), size.1.max(1));
        let (x, y, width, height) = self.anchor_rect;
        positioner.set_anchor_rect(x, y, width.max(1), height.max(1));
        positioner.set_anchor(self.parent_anchor.anchor());
        positioner.set_gravity(self.child_anchor.gravity());
        positioner.set_offset(self.offset.0, self.offset.1);
        positioner.set_constraint_adjustment(self.constraint_adjustment());
        // Placed again when the parent changes (version 3).
        if positioner.version() >= 3 {
            positioner.set_reactive();
        }
        OwnedPositioner(positioner)
    }
}

/// Destroyed when dropped: it is only needed for the request it is used in.
struct OwnedPositioner(XdgPositioner);

impl Drop for OwnedPositioner {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

/// Where the compositor put the popup, relative to its parent: from its
/// last configure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PopupConfigure {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

/// User data of the popup's `xdg_surface` and `xdg_popup`.
#[derive(Clone, Debug, Default)]
pub(crate) struct PopupData(Arc<Mutex<PopupConfigure>>);

/// How a popup takes input.
#[derive(Clone, Debug)]
pub(crate) enum PopupInput {
    /// It takes an explicit grab with the serial of the user's input that
    /// opened it, and the keyboard focus with it. The compositor dismisses
    /// it when the user clicks into another application.
    Grab(WlSeat, u32),
    /// It takes input like any surface, without a grab.
    Normal,
    /// It takes no input at all: a tooltip.
    None,
}

impl PopupInput {
    pub(crate) fn kind(&self) -> PopupKind {
        match self {
            Self::Grab(..) => PopupKind::Grabbing,
            Self::Normal => PopupKind::Normal,
            Self::None => PopupKind::Tooltip,
        }
    }
}

/// How a popup takes input: see `PopupInput`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PopupKind {
    Grabbing,
    Normal,
    Tooltip,
}

pub(crate) struct Popup {
    // Field order is destruction order: the popup before its `xdg_surface`.
    xdg_popup: Mutex<Option<XdgPopup>>,
    xdg_surface: XdgShellSurface,
    xdg_wm_base: XdgWmBase,
    qh: QueueHandle<SctkApplicationState>,
    data: PopupData,
    placement: Mutex<PopupPlacement>,
    /// The token of the last `xdg_popup.reposition`.
    reposition_token: Mutex<u32>,
}

impl Popup {
    /// Gives `surface` the popup role, as a child of `parent`, `size`
    /// large (logical). Takes ownership of the surface.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        xdg_shell: &XdgShell,
        compositor: &CompositorState,
        surface: WlSurface,
        parent: &XdgSurface,
        placement: PopupPlacement,
        size: (i32, i32),
        input: PopupInput,
        qh: &QueueHandle<SctkApplicationState>,
    ) -> Option<Self> {
        let data = PopupData::default();
        let xdg_wm_base = xdg_shell.xdg_wm_base().clone();
        let positioner = placement.positioner(&xdg_wm_base, size, qh);
        let xdg_surface = XdgShellSurface::new(xdg_shell, qh, surface, data.clone()).ok()?;
        let xdg_popup =
            xdg_surface
                .xdg_surface()
                .get_popup(Some(parent), &positioner.0, qh, data.clone());

        match input {
            // Before the initial commit, as the protocol requires.
            PopupInput::Grab(seat, serial) => xdg_popup.grab(&seat, serial),
            PopupInput::Normal => {}
            PopupInput::None => match Region::new(compositor) {
                // An empty input region: input goes through it.
                Ok(region) => xdg_surface
                    .wl_surface()
                    .set_input_region(Some(region.wl_region())),
                Err(err) => warn!("tooltip takes input: no region: {err}"),
            },
        }

        Some(Self {
            xdg_popup: Mutex::new(Some(xdg_popup)),
            xdg_surface,
            xdg_wm_base,
            qh: qh.clone(),
            data,
            placement: Mutex::new(placement),
            reposition_token: Mutex::new(0),
        })
    }

    pub(crate) fn wl_surface(&self) -> &WlSurface {
        self.xdg_surface.wl_surface()
    }

    pub(crate) fn xdg_surface(&self) -> &XdgSurface {
        self.xdg_surface.xdg_surface()
    }

    /// Where the compositor put the popup, relative to its parent.
    pub(crate) fn position(&self) -> PopupConfigure {
        *self.data.0.lock().unwrap()
    }

    pub(crate) fn is_xdg_popup(&self, xdg_popup: &XdgPopup) -> bool {
        self.xdg_popup.lock().unwrap().as_ref() == Some(xdg_popup)
    }

    /// Places the popup again, `size` large (logical), and with a new
    /// placement if there is one. The compositor configures it for that.
    /// Needs `xdg_popup` version 3.
    pub(crate) fn reposition(&self, size: (i32, i32), placement: Option<PopupPlacement>) {
        let mut current = self.placement.lock().unwrap();
        if let Some(placement) = placement {
            *current = placement;
        }
        let xdg_popup = self.xdg_popup.lock().unwrap();
        let Some(xdg_popup) = xdg_popup.as_ref() else {
            return;
        };
        if xdg_popup.version() < 3 {
            warn!("the compositor can not place a popup again: xdg_popup is older than version 3");
            return;
        }
        let positioner = current.positioner(&self.xdg_wm_base, size, &self.qh);
        let mut token = self.reposition_token.lock().unwrap();
        *token += 1;
        xdg_popup.reposition(&positioner.0, *token);
    }

    /// Unmaps the popup. Nothing may be committed to its surface after
    /// that. A popup with an explicit grab has to be the topmost one.
    pub(crate) fn dismiss(&self) {
        if let Some(xdg_popup) = self.xdg_popup.lock().unwrap().take() {
            xdg_popup.destroy();
        }
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        self.dismiss();
    }
}

impl Dispatch<XdgPopup, PopupData> for SctkApplicationState {
    fn event(
        state: &mut Self,
        xdg_popup: &XdgPopup,
        event: xdg_popup::Event,
        data: &PopupData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            // The size is not used: the popup has the size of its content.
            xdg_popup::Event::Configure { x, y, .. } => {
                *data.0.lock().unwrap() = PopupConfigure { x, y };
            }
            xdg_popup::Event::PopupDone => state.popup_dismissed(xdg_popup),
            // Followed by a configure.
            xdg_popup::Event::Repositioned { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<XdgSurface, PopupData> for SctkApplicationState {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        _data: &PopupData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            state.popup_configured(xdg_surface, serial);
        }
    }
}

impl Dispatch<XdgPositioner, ()> for SctkApplicationState {
    fn event(
        _state: &mut Self,
        _positioner: &XdgPositioner,
        _event: xdg_positioner::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        unreachable!("xdg_positioner has no events")
    }
}
