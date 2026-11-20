use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};

use flust_accessibility::Accessibility;
use flust_engine::{
    ffi::{FlutterOpenGLTargetType, FlutterViewId, IMPLICIT_VIEW_ID},
    FlutterEngineWeakRef, ViewError,
};
use smithay_client_toolkit::reexports::calloop::LoopSignal;
use thiserror::Error;
use tracing::debug;
use wayland_client::{protocol::wl_seat::WlSeat, QueueHandle};

use crate::{
    application::SctkApplicationState,
    popup::{PopupInput, PopupKind, PopupPlacement},
    window::{
        SctkFlutterWindow, SctkFlutterWindowCreateError, SctkModalParent, SctkPopupAttributes,
        SctkRendering, SctkShell, SctkViewState, SctkWindowAttributes, SizeConstraints,
    },
};

/// The application's windows, and what they are created with.
///
/// Shared with what Dart calls to create and change windows, which runs
/// while the event loop holds the application's state: Dart runs in calls
/// into the engine, on the platform thread. So no borrow of the windows is
/// held across a call into the engine: they are cloned out first.
pub(crate) struct SctkWindows {
    windows: RefCell<HashMap<FlutterViewId, SctkFlutterWindow>>,
    engine: FlutterEngineWeakRef,
    qh: QueueHandle<SctkApplicationState>,
    /// What windows are created with on the Wayland side.
    pub(crate) shell: SctkShell,
    rendering: SctkRendering,
    /// What windows are created with unless asked for something else: the
    /// application's title, app id and size.
    window_attributes: SctkWindowAttributes,
    /// Whether the application has a startup window. Windows can only be
    /// created while it runs if it does not.
    has_startup_window: bool,
    /// Window metrics can not be sent before the engine is running: that
    /// results in a `Viewport metrics were invalid` embedder error.
    is_engine_running: Cell<bool>,
    accessibility: Accessibility,
    loop_signal: LoopSignal,
    /// The seat and serial of the user's last press or key.
    last_input: RefCell<Option<(WlSeat, u32)>>,
}

impl SctkWindows {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        engine: FlutterEngineWeakRef,
        qh: QueueHandle<SctkApplicationState>,
        shell: SctkShell,
        rendering: SctkRendering,
        window_attributes: SctkWindowAttributes,
        startup_window: Option<SctkFlutterWindow>,
        accessibility: Accessibility,
        loop_signal: LoopSignal,
    ) -> Self {
        let has_startup_window = startup_window.is_some();
        let windows = startup_window
            .into_iter()
            .map(|window| (window.view_id(), window))
            .collect();

        Self {
            windows: RefCell::new(windows),
            engine,
            qh,
            shell,
            rendering,
            window_attributes,
            has_startup_window,
            is_engine_running: Cell::new(false),
            accessibility,
            loop_signal,
            last_input: RefCell::default(),
        }
    }

    pub(crate) fn is_engine_running(&self) -> bool {
        self.is_engine_running.get()
    }

    pub(crate) fn engine_started(&self) {
        self.is_engine_running.set(true);
    }

    pub(crate) fn get(&self, view_id: FlutterViewId) -> Option<SctkFlutterWindow> {
        self.windows.borrow().get(&view_id).cloned()
    }

    /// The first window that `matches`.
    pub(crate) fn find(
        &self,
        matches: impl Fn(&SctkFlutterWindow) -> bool,
    ) -> Option<SctkFlutterWindow> {
        self.windows
            .borrow()
            .values()
            .find(|window| matches(window))
            .cloned()
    }

    pub(crate) fn all(&self) -> Vec<SctkFlutterWindow> {
        self.windows.borrow().values().cloned().collect()
    }

    /// Whether the window of `view_id` has a modal dialog open, and so
    /// takes no input.
    pub(crate) fn is_blocked(&self, view_id: FlutterViewId) -> bool {
        self.windows.borrow().values().any(|window| {
            window.modal_parent() == Some(view_id)
                && matches!(
                    window.view_state(),
                    SctkViewState::Adding | SctkViewState::Added
                )
        })
    }

    /// Takes out the windows whose view the engine is done with, or never
    /// added. Dropping them destroys them.
    pub(crate) fn take_finished(&self) -> Vec<SctkFlutterWindow> {
        let mut windows = self.windows.borrow_mut();
        let finished: Vec<FlutterViewId> = windows
            .iter()
            .filter(|(_, window)| {
                matches!(
                    window.view_state(),
                    SctkViewState::Removed | SctkViewState::Failed
                )
            })
            .map(|(&view_id, _)| view_id)
            .collect();

        finished
            .into_iter()
            .filter_map(|view_id| windows.remove(&view_id))
            .collect()
    }

    /// Creates a window, with a view of its own, while the engine runs. It
    /// is shown once the engine has drawn its first frame.
    ///
    /// With a `modal_parent`, the window is a modal dialog of that window,
    /// which takes no input until the dialog is destroyed.
    ///
    /// The engine has the view when this returns, as Dart expects: the
    /// engine adds it right away when called on the thread that runs Dart.
    pub(crate) fn create_window(
        &self,
        attributes: SctkWindowAttributes,
        modal_parent: Option<FlutterViewId>,
    ) -> Result<SctkFlutterWindow, SctkCreateWindowError> {
        let modal_parent = match modal_parent {
            Some(view_id) => {
                // A dialog of a popup is a dialog of the window the popup
                // belongs to: only a toplevel can be a parent.
                let parent = self
                    .toplevel_of(view_id)
                    .ok_or(SctkCreateWindowError::UnknownParent(view_id))?;
                Some(SctkModalParent {
                    view_id,
                    xdg_toplevel: parent
                        .xdg_toplevel()
                        .ok_or(SctkCreateWindowError::UnknownParent(view_id))?,
                })
            }
            None => None,
        };

        let attributes = SctkWindowAttributes {
            title: attributes.title.or(self.window_attributes.title.clone()),
            app_id: attributes.app_id.or(self.window_attributes.app_id.clone()),
            inner_size: attributes.inner_size.or(self.window_attributes.inner_size),
            modal_parent,
            popup: None,
            ..attributes
        };
        self.add_window(attributes)
    }

    /// Creates a popup of `parent`, or a tooltip: a window placed against a
    /// rectangle of its parent, and sized to its content, within
    /// `constraints`. See `create_window`.
    ///
    /// A popup takes an explicit grab, with the user's last input, when it
    /// can: when its parent is a toplevel or a popup with a grab. It takes
    /// the keyboard focus then, and the compositor dismisses it when the
    /// user clicks into another application.
    pub(crate) fn create_popup(
        &self,
        parent: FlutterViewId,
        placement: PopupPlacement,
        constraints: SizeConstraints,
        tooltip: bool,
    ) -> Result<SctkFlutterWindow, SctkCreateWindowError> {
        let parent_window = self
            .get(parent)
            .filter(|window| window.view_state() == SctkViewState::Added)
            .ok_or(SctkCreateWindowError::UnknownParent(parent))?;
        let parent_kind = parent_window.popup_parent().map(|(_, kind)| kind);
        // Only a toplevel or a popup with a grab can have a popup with a
        // grab, and a tooltip has no popups.
        let input = match (tooltip, parent_kind) {
            (true, _) => PopupInput::None,
            (false, Some(PopupKind::Tooltip)) => {
                return Err(SctkCreateWindowError::UnknownParent(parent))
            }
            (false, None | Some(PopupKind::Grabbing)) => match self.last_input.borrow().clone() {
                Some((seat, serial)) => PopupInput::Grab(seat, serial),
                None => PopupInput::Normal,
            },
            (false, Some(PopupKind::Normal)) => PopupInput::Normal,
        };

        let attributes = SctkWindowAttributes {
            popup: Some(SctkPopupAttributes {
                parent_view_id: parent,
                parent_xdg_surface: parent_window.xdg_surface(),
                placement,
                constraints,
                input,
            }),
            ..Default::default()
        };
        self.add_window(attributes)
    }

    /// The window of `view_id`, or the toplevel its popup belongs to.
    fn toplevel_of(&self, view_id: FlutterViewId) -> Option<SctkFlutterWindow> {
        let window = self
            .get(view_id)
            .filter(|window| window.view_state() == SctkViewState::Added)?;
        match window.popup_parent() {
            Some((parent, _)) => self.toplevel_of(parent),
            None => Some(window),
        }
    }

    /// Whether `ancestor` is `view_id`, or a window that `view_id` is a
    /// popup of, directly or not.
    fn is_popup_ancestor(&self, ancestor: FlutterViewId, view_id: FlutterViewId) -> bool {
        let mut current = Some(view_id);
        while let Some(view_id) = current {
            if view_id == ancestor {
                return true;
            }
            current = self
                .get(view_id)
                .and_then(|window| window.popup_parent())
                .map(|(parent, _)| parent);
        }
        false
    }

    /// The popups that a press into the window of `view_id` dismisses: the
    /// ones that take input and are not that window or below it. Under a
    /// grab, the compositor sends the press to the app's own windows, and
    /// leaves dismissing popups to it.
    pub(crate) fn popups_outside(&self, view_id: FlutterViewId) -> Vec<FlutterViewId> {
        self.all()
            .into_iter()
            .filter(|window| {
                matches!(
                    window.popup_parent(),
                    Some((_, PopupKind::Grabbing | PopupKind::Normal))
                ) && matches!(
                    window.view_state(),
                    SctkViewState::Adding | SctkViewState::Added
                )
            })
            .map(|window| window.view_id())
            .filter(|&popup| !self.is_popup_ancestor(popup, view_id))
            .collect()
    }

    /// The seat and serial of the user's last press or key, for a popup's
    /// grab.
    pub(crate) fn set_last_input(&self, seat: WlSeat, serial: u32) {
        *self.last_input.borrow_mut() = Some((seat, serial));
    }

    /// Creates a window as `attributes` say: see `create_window`.
    fn add_window(
        &self,
        attributes: SctkWindowAttributes,
    ) -> Result<SctkFlutterWindow, SctkCreateWindowError> {
        if self.has_startup_window {
            return Err(SctkCreateWindowError::StartupWindow);
        }
        // Only the startup window can be drawn into straight.
        if !matches!(
            self.rendering.opengl_target_type,
            FlutterOpenGLTargetType::Framebuffer
        ) {
            return Err(SctkCreateWindowError::UnsupportedTargetType(
                self.rendering.opengl_target_type,
            ));
        }
        let engine = match self.engine.upgrade() {
            Some(engine) if self.is_engine_running() => engine,
            _ => return Err(SctkCreateWindowError::EngineNotRunning),
        };

        let title = attributes.title.clone().unwrap_or_default();
        let view_id = engine.allocate_view_id();
        let window = SctkFlutterWindow::new(
            view_id,
            self.engine.clone(),
            &self.qh,
            &self.shell,
            &self.rendering,
            attributes,
        )?;

        // Before the view is added: Dart runs while it is, and may look
        // for the window.
        self.windows.borrow_mut().insert(view_id, window.clone());

        let view_state = window.shared_view_state();
        let signal = self.loop_signal.clone();
        let added = engine.add_view(
            window.create_flutter_view(),
            window.initial_metrics(),
            move |added| {
                debug!("[view {view_id}] added: {added}");
                *view_state.lock() = match added {
                    true => SctkViewState::Added,
                    false => SctkViewState::Failed,
                };
                // The window's first configure may be waiting to be sent.
                signal.wakeup();
            },
        );
        if let Err(err) = added {
            self.windows.borrow_mut().remove(&view_id);
            return Err(err.into());
        }

        self.accessibility.add_view(view_id, &title);
        debug!("[view {view_id}] window created");
        Ok(window)
    }

    /// Destroys a window that was created with `create_window`. The window
    /// stays until the engine is done with its view.
    pub(crate) fn destroy_window(
        &self,
        view_id: FlutterViewId,
    ) -> Result<(), SctkDestroyWindowError> {
        if view_id == IMPLICIT_VIEW_ID {
            return Err(SctkDestroyWindowError::StartupWindow);
        }
        let window = self
            .get(view_id)
            .ok_or(SctkDestroyWindowError::UnknownWindow(view_id))?;
        if window.view_state() != SctkViewState::Added {
            return Ok(());
        }
        let Some(engine) = self.engine.upgrade() else {
            return Ok(());
        };

        window.set_view_state(SctkViewState::Removing);
        // A popup goes right away: popups with a grab have to be destroyed
        // in order, the topmost first, and a new one may be opened next.
        window.dismiss();
        self.accessibility.remove_view(view_id);

        let view_state = window.shared_view_state();
        let signal = self.loop_signal.clone();
        engine.remove_view(view_id, move |removed| {
            debug!("[view {view_id}] removed: {removed}");
            // Whether the engine had it or not, nothing draws to it now.
            *view_state.lock() = SctkViewState::Removed;
            signal.wakeup();
        })?;
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum SctkCreateWindowError {
    #[error("Windows can only be created by an application that starts without a window")]
    StartupWindow,

    #[error("Windows can not be created with the {0:?} OpenGL target type")]
    UnsupportedTargetType(FlutterOpenGLTargetType),

    #[error("Windows can only be created while the engine runs")]
    EngineNotRunning,

    #[error("There is no window with view id {0} that can be the parent")]
    UnknownParent(FlutterViewId),

    #[error(transparent)]
    Window(#[from] SctkFlutterWindowCreateError),

    #[error(transparent)]
    View(#[from] ViewError),
}

#[derive(Error, Debug)]
pub enum SctkDestroyWindowError {
    #[error("The startup window can not be destroyed")]
    StartupWindow,

    #[error("There is no window with view id {0}")]
    UnknownWindow(FlutterViewId),

    #[error(transparent)]
    View(#[from] ViewError),
}
