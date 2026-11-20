use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    process::ExitCode,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use calloop::futures::{Executor, Scheduler};
use dpi::LogicalSize;
use flust_accessibility::Accessibility;
use flust_engine::{
    builder::FlutterEngineBuilder,
    ffi::{FlutterEngineDisplay, FlutterKeyEventDeviceType, FlutterKeyEventType},
    plugins::{Plugin, PluginRegistrar},
    CreateError, FlutterEngine,
};
use flust_engine::{
    ffi::{FlutterViewId, IMPLICIT_VIEW_ID},
    view::{FlutterViewFocusDirection, FlutterViewFocusState},
};
use flust_engine_sys::FlutterEngineGetCurrentTime;
use flust_plugins::{
    accessibility::AccessibilityPlugin, isolate::IsolatePlugin, keyevent::KeyEventPlugin,
    lifecycle::LifecyclePlugin, localization::LocalizationPlugin, mousecursor::MouseCursorPlugin,
    navigation::NavigationPlugin, platform::PlatformPlugin, system::SystemPlugin,
    textinput::TextInputPlugin,
};
use flust_plugins::{keyboard::KeyboardPlugin, settings::SettingsPlugin};
use flust_runner_api::{ApplicationAttributes, BackendConfigSctk};
use parking_lot::{Mutex, RwLock};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, SurfaceData},
    delegate_compositor, delegate_keyboard, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    globals::GlobalData,
    output::{OutputHandler, OutputState},
    reexports::protocols::xdg::shell::client::{
        xdg_popup::XdgPopup, xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel,
        xdg_wm_base::XdgWmBase,
    },
    reexports::{
        calloop::{
            self,
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler, ThemeSpec},
        Capability, SeatHandler, SeatState,
    },
    shell::xdg::XdgShell,
    shm::{Shm, ShmHandler},
};
use thiserror::Error;
use tracing::{debug, error, trace};
use wayland_backend::client::ObjectId;
use wayland_client::{
    delegate_dispatch,
    globals::{registry_queue_init, BindError, GlobalError},
    protocol::{
        wl_keyboard::WlKeyboard,
        wl_output::{Transform, WlOutput},
        wl_pointer::WlPointer,
        wl_seat::WlSeat,
        wl_surface::WlSurface,
    },
    ConnectError, Connection, Proxy, QueueHandle,
};

use crate::{
    egl::new_wayland_render_context,
    fractional::FractionalScaleGlobals,
    frame_clock::FrameClock,
    handler::{
        SctkAccessibilityHandler, SctkAsyncResult, SctkExitHandler, SctkKeyboardHandler,
        SctkMouseCursorHandler, SctkOpenGLCompositorHandlerFramebuffer, SctkOpenGLHandler,
        SctkPlatformHandler, SctkPlatformTaskHandler, SctkTextInputHandler, SctkVsyncHandler,
        FRAME_INTERVAL_60_HZ_IN_NANOS,
    },
    keyboard::{SctkFlutterStringExt, SctkKeyEvent},
    output::SctkOutput,
    settings::read_and_monitor_desktop_settings,
    toplevel::ToplevelConfigure,
    window::{
        SctkFlutterWindow, SctkFlutterWindowCreateError, SctkRendering, SctkShell, SctkViewState,
        SctkWindowAttributes,
    },
    windowing::{register_application, WindowingPlugin},
    windows::SctkWindows,
};

pub struct SctkApplication {
    event_loop: EventLoop<'static, SctkApplicationState>,
    state: SctkApplicationState,
}

pub struct SctkApplicationState {
    loop_handle: LoopHandle<'static, SctkApplicationState>,
    /// Set, by the exit handler of the platform plugin, to leave the event
    /// loop and exit with this code.
    exit_code: Arc<Mutex<Option<i32>>>,
    registry_state: RegistryState,
    shm_state: Shm,
    output_state: OutputState,
    seat_state: SeatState,
    engine: FlutterEngine,
    /// Until the engine runs, the windows are not polled, so the first
    /// configure waits.
    windows: Rc<SctkWindows>,
    /// Whether each window is activated: the application is resumed while
    /// one of them is.
    active_state: HashMap<FlutterViewId, bool>,
    pointers: HashMap<ObjectId, WlPointer>,
    keyboards: HashMap<ObjectId, WlKeyboard>,
    plugins: Rc<RwLock<PluginRegistrar>>,
    mouse_cursor_handler: Arc<Mutex<SctkMouseCursorHandler>>,
    keyboard_handler: Arc<Mutex<SctkKeyboardHandler>>,
    accessibility: Accessibility,
    frame_clock: FrameClock,
    async_scheduler: Scheduler<SctkAsyncResult>,
    modifiers: Modifiers,
    /// The surface that has the keyboard focus.
    keyboard_focus: Option<ObjectId>,
    /// The keys pressed while the window with the keyboard focus took no
    /// input: their release is not sent either.
    blocked_keys: HashSet<u32>,
}

impl SctkApplication {
    pub fn new(
        attributes: ApplicationAttributes,
        config: BackendConfigSctk,
    ) -> Result<Self, SctkApplicationCreateError> {
        let conn = Connection::connect_to_env()?;
        let (globals, event_queue) = registry_queue_init(&conn)?;
        let qh = event_queue.handle();

        let event_loop: EventLoop<SctkApplicationState> = EventLoop::try_new()?;
        WaylandSource::new(conn.clone(), event_queue).insert(event_loop.handle())?;

        let (async_executor, async_scheduler) = calloop::futures::executor::<SctkAsyncResult>()?;
        event_loop.handle().insert_source(
            async_executor,
            |event, _metadata, _state| match event {
                Ok(_) => {} // no-op
                Err(err) => error!("sctk async error: {:?}", err),
            },
        )?;

        let registry_state = RegistryState::new(&globals);
        let output_state = OutputState::new(&globals, &qh);
        let seat_state = SeatState::new(&globals, &qh);
        let compositor_state = CompositorState::bind(&globals, &qh)?;
        let xdg_shell_state = XdgShell::bind(&globals, &qh)?;
        // Optional: dialogs are only hinted to the compositor with it.
        let xdg_wm_dialog = globals.bind(&qh, 1..=1, GlobalData).ok();
        // Optional: windows are drawn at integer scales without them.
        let fractional_scale = FractionalScaleGlobals::bind(&globals, &qh);
        let shm_state = Shm::bind(&globals, &qh)?;

        let platform_task_handler = Arc::new(SctkPlatformTaskHandler::new(event_loop.get_signal()));
        let frame_clock = FrameClock::new(event_loop.get_signal());
        let vsync_handler = Arc::new(Mutex::new(SctkVsyncHandler::new(frame_clock.clone())));

        let accessibility = Accessibility::new();
        let window_title = attributes.title.clone().unwrap_or_default();

        // One context for all windows: the engine draws every view in it.
        let renderer = attributes.renderer();
        let (render_context, resource_context) = new_wayland_render_context(&conn, renderer)
            .map_err(SctkFlutterWindowCreateError::from)?;
        let render_context = Arc::new(std::sync::Mutex::new(render_context));
        // Before the engine runs: the render context is then not current
        // on any other thread.
        let framebuffer = SctkOpenGLCompositorHandlerFramebuffer::new(&render_context, renderer);
        let opengl_handler = SctkOpenGLHandler::new(render_context.clone(), resource_context);

        let engine = FlutterEngineBuilder::new()
            .with_platform_handler(platform_task_handler)
            .with_opengl_handler(Arc::new(opengl_handler))
            .with_vsync_handler(vsync_handler)
            .with_semantics_handler(Arc::new(accessibility.clone()))
            .with_aot_library_path(attributes.aot_library_path.clone())
            .with_asset_path(attributes.assets_path.clone())
            .with_icu_data_path(attributes.icu_data_path.clone())
            .with_persistent_cache_path(attributes.persistent_cache_path.clone())
            .with_args(attributes.args.clone())
            .with_renderer(attributes.renderer())
            .with_compositor_enabled(true)
            // Dart runs on this thread, as with Flutter's own embedders.
            .with_merged_platform_ui_thread(true)
            .build()?;

        let shell = SctkShell {
            compositor_state,
            xdg_shell_state,
            xdg_wm_dialog,
            fractional_scale,
            frame_clock: frame_clock.clone(),
        };
        let rendering = SctkRendering {
            render_context,
            framebuffer,
            opengl_target_type: config.opengl_target_type,
            renderer,
        };
        let window_attributes = SctkWindowAttributes {
            title: attributes.title.clone(),
            app_id: attributes.app_id.clone(),
            inner_size: attributes.inner_size,
            // The startup window's: windows that the app creates have the
            // limits they are given.
            min_size: Some(LogicalSize::new(256, 256)),
            max_size: None,
            modal_parent: None,
            popup: None,
        };

        // Without a startup window, the implicit view exists in the engine
        // but has nothing to show its frames in.
        let startup_window = match config.startup_window {
            true => {
                let window = SctkFlutterWindow::new(
                    IMPLICIT_VIEW_ID,
                    engine.downgrade(),
                    &qh,
                    &shell,
                    &rendering,
                    window_attributes.clone(),
                )?;
                engine.add_implicit_view(window.create_flutter_view());
                accessibility.add_view(window.view_id(), &window_title);
                Some(window)
            }
            false => None,
        };

        let noop_isolate_cb = || trace!("[isolate-plugin] isolate has been created");
        let platform_handler = unsafe {
            SctkPlatformHandler::new(
                conn.display(),
                startup_window
                    .as_ref()
                    .and_then(|window| window.xdg_toplevel()),
            )
        };
        let platform_handler = Arc::new(Mutex::new(platform_handler));
        let mouse_cursor_handler = Arc::new(Mutex::new(SctkMouseCursorHandler::new(conn.clone())));
        let text_input_handler = Arc::new(Mutex::new(SctkTextInputHandler::new()));
        let keyboard_handler = Arc::new(Mutex::new(SctkKeyboardHandler::new()));

        let accessibility_handler = Arc::new(Mutex::new(SctkAccessibilityHandler::new(
            accessibility.clone(),
        )));

        let mut plugins = PluginRegistrar::new();
        plugins.add_plugin(&engine, AccessibilityPlugin::new(accessibility_handler));
        plugins.add_plugin(&engine, IsolatePlugin::new(noop_isolate_cb));
        plugins.add_plugin(&engine, KeyEventPlugin::new());
        plugins.add_plugin(&engine, TextInputPlugin::new(text_input_handler.clone()));
        plugins.add_plugin(&engine, KeyboardPlugin::new(keyboard_handler.clone()));
        plugins.add_plugin(&engine, LifecyclePlugin::default());
        plugins.add_plugin(&engine, LocalizationPlugin::default());
        plugins.add_plugin(&engine, NavigationPlugin::default());
        let exit_code = Arc::new(Mutex::new(None));
        let exit_handler = SctkExitHandler::new(exit_code.clone(), event_loop.get_signal());
        plugins.add_plugin(
            &engine,
            PlatformPlugin::new(platform_handler)
                .with_exit_handler(Arc::new(Mutex::new(exit_handler))),
        );
        plugins.add_plugin(&engine, SettingsPlugin::default());
        plugins.add_plugin(&engine, WindowingPlugin::new());
        #[cfg(feature = "testing")]
        {
            let (requests, request_source) = calloop::channel::channel();
            event_loop
                .handle()
                .insert_source(request_source, |event, _, state| {
                    if let calloop::channel::Event::Msg(request) = event {
                        crate::testing::WindowRequest::handle(request, state);
                    }
                })
                .map_err(|err| err.error)?;
            plugins.add_plugin(&engine, crate::testing::WindowsTestingPlugin::new(requests));
        }
        plugins.add_plugin(&engine, SystemPlugin::default());
        plugins.add_plugin(
            &engine,
            MouseCursorPlugin::new(mouse_cursor_handler.clone()),
        );

        let windows = Rc::new(SctkWindows::new(
            engine.downgrade(),
            qh.clone(),
            shell,
            rendering,
            window_attributes,
            startup_window,
            accessibility.clone(),
            event_loop.get_signal(),
        ));
        register_application(engine.engine_id(), &windows);

        let state = SctkApplicationState {
            loop_handle: event_loop.handle(),
            exit_code,
            windows,
            pointers: HashMap::new(),
            keyboards: HashMap::new(),
            active_state: HashMap::new(),
            shm_state,
            registry_state,
            output_state,
            seat_state,
            engine,
            plugins: Rc::new(RwLock::new(plugins)),
            mouse_cursor_handler,
            keyboard_handler,
            accessibility,
            frame_clock,
            async_scheduler,
            modifiers: Modifiers::default(),
            keyboard_focus: None,
            blocked_keys: HashSet::new(),
        };

        Ok(Self { event_loop, state })
    }

    /// Runs the application until Dart decides that it exits, and returns
    /// the exit code that Dart asked for.
    pub fn run(mut self) -> Result<ExitCode, SctkApplicationRunError> {
        // The event loop needs to be started *prior* to running the engine (see
        // `FlutterEngineRun` comment in `embedder.h` for additional context).
        // Therefore, use an immediate timer source for starting the engine
        // once the event loop is running.
        //
        // https://github.com/flutter/engine/blob/7c2a56a44b414f2790af277783ec27181337a6d3/shell/platform/embedder/embedder.h#L2313-L2322
        self.state
            .loop_handle
            .insert_source(Timer::immediate(), |_event, _metadata, state| {
                state.engine.run().expect("Failed to run engine");

                state.accessibility.engine_started(&state.engine);
                state.schedule_async_startup_tasks();

                state.windows.engine_started();
                state.notify_display_update();

                TimeoutAction::Drop
            })?;

        while self.state.exit_code.lock().is_none() {
            // Everything that waits for a point in time (engine tasks, the
            // frame clock, a window's frame) is polled here, and the event
            // loop sleeps until the earliest of them. Other threads wake it
            // up when they have something for this one.
            let next_task = self.state.engine.execute_platform_tasks();
            let next_poll = self.state.poll();

            let timeout = [next_task, next_poll]
                .into_iter()
                .flatten()
                .min()
                .map(|deadline| deadline.saturating_duration_since(Instant::now()));

            self.event_loop.dispatch(timeout, &mut self.state)?;
        }

        // Stops the engine's threads before the window they draw to, and
        // everything else they use, is torn down. They would otherwise still
        // be running when the process exits.
        self.state.engine.shutdown();

        let exit_code = self.state.exit_code.lock().unwrap_or_default();
        // Only the low 8 bits reach the parent process, as with `exit`.
        Ok(ExitCode::from(exit_code as u8))
    }

    pub fn add_plugin<P>(&mut self, plugin: P)
    where
        P: Plugin + 'static,
    {
        self.state.add_plugin(plugin);
    }
}

impl SctkApplicationState {
    pub fn add_plugin<P>(&mut self, plugin: P)
    where
        P: Plugin + 'static,
    {
        self.plugins.write().add_plugin(&self.engine, plugin);
    }

    pub fn with_plugin<F, P>(&self, f: F)
    where
        F: FnOnce(&P),
        P: Plugin + 'static,
    {
        self.plugins.read().with_plugin(f)
    }

    pub fn with_plugin_mut<F, P>(&self, f: F)
    where
        F: FnOnce(&mut P),
        P: Plugin + 'static,
    {
        self.plugins.write().with_plugin_mut(f)
    }

    fn find_window_by_surface_id(&self, surface_id: ObjectId) -> Option<SctkFlutterWindow> {
        self.windows
            .find(|window| window.wl_surface_id() == surface_id)
    }

    fn is_engine_running(&self) -> bool {
        self.windows.is_engine_running()
    }

    /// Whether the window with the keyboard focus has a modal dialog open,
    /// and so takes no keys.
    fn is_keyboard_focus_blocked(&self) -> bool {
        self.keyboard_focus
            .clone()
            .and_then(|surface_id| self.find_window_by_surface_id(surface_id))
            .is_some_and(|window| self.windows.is_blocked(window.view_id()))
    }

    /// Remembers the user's input on `seat`, for the grab of a popup it
    /// opens.
    fn user_input(&self, seat: Option<WlSeat>, serial: u32) {
        if let Some(seat) = seat {
            self.windows.set_last_input(seat, serial);
        }
    }

    /// Asks Dart to destroy popups: their windows are dismissed.
    fn dismiss_popups(&self, popups: &[FlutterViewId]) {
        for &view_id in popups {
            debug!("[view {view_id}] popup dismissed");
            self.with_plugin(|windowing: &WindowingPlugin| {
                windowing.window_close_requested(view_id)
            });
        }
    }

    #[cfg(feature = "testing")]
    pub(crate) fn windows(&self) -> &SctkWindows {
        &self.windows
    }

    /// Returns the engine's vsync baton and lets the windows act on their
    /// surface state, as far as they are due. Returns when to call this
    /// again, if nothing else happens before then.
    fn poll(&mut self) -> Option<Instant> {
        if !self.is_engine_running() {
            return None;
        }

        let frame_interval = self.get_frame_interval_in_nanos();
        let now = unsafe { FlutterEngineGetCurrentTime() };
        let (baton, vsync_deadline) = self.frame_clock.poll(now, frame_interval);

        // A new size is sent to the engine between two of its frames: right
        // before it is told to start one, or while it is not waiting to
        // start one at all. At any other time it would have to throw away
        // the frame it is working on.
        let may_request = baton.is_some() || !self.frame_clock.is_vsync_requested();
        let window_deadline = self.poll_windows(may_request);

        if let Some(baton) = baton {
            self.engine.on_vsync(baton, now, now + frame_interval);
        }
        let vsync_deadline = vsync_deadline
            .map(|deadline| Instant::now() + Duration::from_nanos(deadline.saturating_sub(now)));

        [window_deadline, vsync_deadline]
            .into_iter()
            .flatten()
            .min()
    }

    fn poll_windows(&mut self, may_request: bool) -> Option<Instant> {
        self.drop_finished_windows();

        let now = Instant::now();
        self.windows
            .all()
            .iter()
            .filter(|window| window.view_state() == SctkViewState::Added)
            .filter_map(|window| window.poll(now, may_request))
            .min()
    }

    /// Drops the windows whose view the engine is done with, or never
    /// added.
    fn drop_finished_windows(&mut self) {
        for window in self.windows.take_finished() {
            let view_id = window.view_id();
            debug!("[view {view_id}] window destroyed");
            // Its EGL surface goes first, then its role and its surface.
            drop(window);
            if self.active_state.contains_key(&view_id) {
                self.maybe_update_lifecycle_state(view_id, false);
                self.active_state.remove(&view_id);
            }
            self.with_plugin(|windowing: &WindowingPlugin| windowing.window_destroyed(view_id));
            #[cfg(feature = "testing")]
            self.with_plugin(|testing: &crate::testing::WindowsTestingPlugin| {
                testing.window_destroyed(view_id)
            });
        }
    }

    /// The engine has one vsync for all windows: the frame interval of the
    /// fastest output that shows one of them, or 60Hz as long as none is
    /// shown. A window on a slower output presents some of the frames.
    fn get_frame_interval_in_nanos(&self) -> u64 {
        self.windows
            .all()
            .iter()
            .filter_map(|window| self.get_surface_frame_interval_in_nanos(&window.wl_surface()))
            .min()
            .unwrap_or(FRAME_INTERVAL_60_HZ_IN_NANOS)
    }

    fn maybe_update_lifecycle_state(&mut self, view_id: FlutterViewId, is_active: bool) {
        let was_active = self.active_state.values().any(|&active| active);
        self.active_state.insert(view_id, is_active);
        let is_active = self.active_state.values().any(|&active| active);

        if was_active != is_active && self.is_engine_running() {
            self.with_plugin(|lifecycle: &LifecyclePlugin| match is_active {
                true => lifecycle.send_app_is_resumed(),
                false => lifecycle.send_app_is_inactive(),
            })
        }
    }

    fn schedule_async_startup_tasks(&self) {
        self.with_plugin(|settings: &SettingsPlugin| {
            if let Err(err) = self
                .async_scheduler
                .schedule(read_and_monitor_desktop_settings(
                    settings.clone(),
                    self.accessibility.clone(),
                ))
            {
                error!("Failed to schedule engine async jobs: {}", err);
            };
        });
    }

    /// Find the maximum refresh rate from the surface current outputs.
    fn get_surface_refresh_rate_in_mhz(&self, surface: &WlSurface) -> Option<i32> {
        let data = surface.data::<SurfaceData>()?;

        let refresh_rate = data
            .outputs()
            .filter_map(|output| {
                let info = self.output_state.info(&output)?;
                let current_mode = info.modes.iter().find(|mode| mode.current)?;
                Some(current_mode.refresh_rate)
            })
            .max()?;

        Some(refresh_rate)
    }

    fn get_surface_frame_interval_in_nanos(&self, surface: &WlSurface) -> Option<u64> {
        let refresh_rate = self.get_surface_refresh_rate_in_mhz(surface)? as u64;

        // Refresh rate could be zero if an output has no correct refresh rate,
        // such as a virtual output.
        if refresh_rate == 0 {
            return None;
        }

        Some(1_000_000_000_000 / refresh_rate)
    }

    fn notify_display_update(&self) {
        // Ignore display update events if the engine is not running. This
        // method will be called again once the engine is running to ensure the
        // display state is up-to-date on the engine side.
        if !self.is_engine_running() {
            return;
        }

        let output_state = &self.output_state;
        let displays: Vec<FlutterEngineDisplay> = output_state
            .outputs()
            .map(|output| {
                SctkOutput::new(output.id().protocol_id().into(), output_state.info(&output)).into()
            })
            .collect();

        trace!("notifying engine of display update: {:?}", displays);

        self.engine.notify_display_update(
            flust_engine::ffi::FlutterEngineDisplaysUpdateType::Startup,
            displays,
        );
    }

    fn send_key_event(&self, event: SctkKeyEvent) {
        self.engine.send_key_event(event.clone().into());

        // The `flutter/keyevent`'s are considered legacy but they are still
        // required for now [0][1], so the current implementation is mostly
        // using them as a "flush" event for `flutter/keydata` messages.
        //
        // TODO: Remove `KeyEventPlugin` once it is no longer *required* for
        // keyboard handling (planned for Q4 2024 [2]).
        //
        // [0](https://github.com/flutter/flutter/pull/132533)
        // [1](https://github.com/flutter/flutter/issues/136419)
        // [2](https://github.com/flutter/flutter/issues/136419)
        self.with_plugin(|keyevent: &KeyEventPlugin| {
            keyevent.key_action(event.into());
        });
    }

    fn press_key_or_repeat(&mut self, event: SctkKeyEvent) {
        self.send_key_event(event.clone());

        let keysym = event.event.keysym;
        let select = self.modifiers.shift;

        // See OBS project implementation for a list of alternative key names
        // that map to the same logical key:
        // https://github.com/obsproject/obs-browser/blob/b4f724/linux-keyboard-helpers.hpp#L352
        self.with_plugin_mut(|text_input: &mut TextInputPlugin| {
            match keysym {
                Keysym::Return | Keysym::KP_Enter | Keysym::ISO_Enter => {
                    text_input.enter_pressed();
                }
                Keysym::Home | Keysym::KP_Home => {
                    text_input.with_state(|state| state.move_to_beginning(select));
                    text_input.notify_changes();
                }
                Keysym::End | Keysym::KP_End => {
                    text_input.with_state(|state| state.move_to_end(select));
                    text_input.notify_changes();
                }
                Keysym::BackSpace
                | Keysym::Delete
                | Keysym::KP_Delete
                | Keysym::Left
                | Keysym::KP_Left
                | Keysym::Right
                | Keysym::KP_Right
                | Keysym::Up
                | Keysym::KP_Up
                | Keysym::Down
                | Keysym::KP_Down => {
                    // No-op: Already handled inside the framework in
                    // `RenderEditable`.
                }
                Keysym::Escape
                | Keysym::Shift_L
                | Keysym::Shift_R
                | Keysym::Control_L
                | Keysym::Control_R
                | Keysym::Alt_L
                | Keysym::Alt_R
                | Keysym::ISO_Level3_Shift // AltGr on european keyboards
                | Keysym::Super_L
                | Keysym::Super_R
                | Keysym::Meta_L
                | Keysym::Meta_R => {
                    // No-op. A modifier key-down event should *not* be handled
                    // by the fallback code below. Doing so would have
                    // unintended side-effects (e.g.: removing/replacing
                    // selected text).
                }
                _ => {
                    let Some(text) = event.event.utf8 else {
                        return;
                    };

                    if text.is_control_character() {
                        return;
                    }

                    text_input.with_state(|state| {
                        state.add_characters(&text);
                    });
                    text_input.notify_changes();
                }
            }
        });
    }
}

delegate_compositor!(SctkApplicationState);
delegate_output!(SctkApplicationState);
delegate_shm!(SctkApplicationState);

// `XdgShell` answers `xdg_wm_base.ping`. The toplevel's own objects are
// handled in `toplevel.rs`.
delegate_dispatch!(SctkApplicationState: [XdgWmBase: GlobalData] => XdgShell);

delegate_seat!(SctkApplicationState);
delegate_pointer!(SctkApplicationState);
delegate_keyboard!(SctkApplicationState);

delegate_registry!(SctkApplicationState);

impl ProvidesRegistryState for SctkApplicationState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

impl CompositorHandler for SctkApplicationState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        new_scale_factor: i32,
    ) {
        trace!(
            "[{}] scale factor changed: {}",
            surface.id(),
            new_scale_factor
        );

        let Some(window) = self.find_window_by_surface_id(surface.id()) else {
            debug!(
                "[{}] ignoring `scale_factor_changed` event for unknown flutter window",
                surface.id()
            );
            return;
        };

        window.scale_factor_changed(new_scale_factor);
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        new_transform: Transform,
    ) {
        trace!(
            "[{}] transform changed: {}",
            surface.id(),
            u32::from(new_transform),
        );
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        time: u32,
    ) {
        trace!("[{} time: {}] frame callback", surface.id(), time);

        let now = unsafe { FlutterEngineGetCurrentTime() };
        // The baton is returned by `poll`, after the rest of what the
        // compositor sent with this callback.
        let frame_interval = self.get_frame_interval_in_nanos();
        self.frame_clock.frame_callback_done(now, frame_interval);
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wayland_client::protocol::wl_surface::WlSurface,
        output: &wayland_client::protocol::wl_output::WlOutput,
    ) {
        trace!("[{}] entered {}", surface.id(), output.id());

        let Some(window) = self.find_window_by_surface_id(surface.id()) else {
            debug!(
                "[{}] ignoring `surface_enter` event for unknown flutter window",
                surface.id()
            );
            return;
        };

        window.surface_outputs_changed();
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wayland_client::protocol::wl_surface::WlSurface,
        output: &wayland_client::protocol::wl_output::WlOutput,
    ) {
        trace!("[{}] left {}", surface.id(), output.id());

        let Some(window) = self.find_window_by_surface_id(surface.id()) else {
            debug!(
                "[{}] ignoring `surface_leave` event for unknown flutter window",
                surface.id()
            );
            return;
        };

        window.surface_outputs_changed();
    }
}

impl ShmHandler for SctkApplicationState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm_state
    }
}

impl PointerHandler for SctkApplicationState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(window) = self.find_window_by_surface_id(event.surface.id()) else {
                debug!(
                    "[{}] ignoring pointer event for unknown flutter window",
                    event.surface.id()
                );
                continue;
            };

            // Not before the engine has the window's view, nor while it
            // removes it.
            if window.view_state() != SctkViewState::Added {
                continue;
            }
            if let PointerEventKind::Press { serial, .. }
            | PointerEventKind::Release { serial, .. } = event.kind
            {
                let seat = self
                    .seat_state
                    .seats()
                    .find(|seat| self.pointers.get(&seat.id()) == Some(pointer));
                self.user_input(seat, serial);
            }

            // A press outside of the open popups dismisses them, and does
            // nothing else.
            if let PointerEventKind::Press { .. } = event.kind {
                let popups = self.windows.popups_outside(window.view_id());
                if !popups.is_empty() {
                    self.dismiss_popups(&popups);
                    continue;
                }
            }

            let blocked = self.windows.is_blocked(window.view_id());
            window.pointer_event(pointer, event, blocked);
        }
    }
}

impl KeyboardHandler for SctkApplicationState {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        surface: &WlSurface,
        _serial: u32,
        raw: &[u32],
        keysyms: &[Keysym],
    ) {
        // Key events name no view: they go to the view that has focus.
        self.keyboard_focus = Some(surface.id());
        self.send_view_focus_event(surface, FlutterViewFocusState::Focused);

        let synthesized_events = self
            .keyboard_handler
            .lock()
            .sync_keyboard_enter_state(raw, keysyms);

        for event in synthesized_events {
            self.send_key_event(event);
        }
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        surface: &WlSurface,
        _serial: u32,
    ) {
        if self.keyboard_focus == Some(surface.id()) {
            self.keyboard_focus = None;
        }
        // Keys still held when another window gets the focus are pressed
        // in that one, as the keyboard's state is synchronized then.
        self.blocked_keys.clear();
        self.send_view_focus_event(surface, FlutterViewFocusState::Unfocused);
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        keyboard: &WlKeyboard,
        serial: u32,
        event: KeyEvent,
    ) {
        let seat = self
            .seat_state
            .seats()
            .find(|seat| self.keyboards.get(&seat.id()) == Some(keyboard));
        self.user_input(seat, serial);
        trace!(
            "key pressed: {}",
            event.keysym.name().unwrap_or("[unknown]"),
        );

        // A window with a modal dialog open takes no input. The key is not
        // kept in the keyboard's state either: the engine does not know
        // about it.
        if self.is_keyboard_focus_blocked() {
            self.blocked_keys.insert(event.raw_code);
            return;
        }

        if self
            .keyboard_handler
            .lock()
            .press_key(event.clone())
            .is_err()
        {
            error!(
                "A key was pressed which was already found in internal state. Ignoring {:?}",
                event
            );
            return;
        };

        self.press_key_or_repeat(SctkKeyEvent::new(
            FlutterKeyEventDeviceType::Keyboard,
            event,
            FlutterKeyEventType::Down,
            None,
            self.modifiers,
            false,
        ));
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        trace!(
            "key released: {}",
            event.keysym.name().unwrap_or("[unknown]"),
        );

        // The engine did not get the key's press.
        if self.blocked_keys.remove(&event.raw_code) {
            return;
        }

        let Ok(latched_keydown) = self.keyboard_handler.lock().release_key(&event) else {
            error!(
                "A key was released which was not found in internal state. Ignoring {:?}",
                event
            );
            return;
        };

        self.send_key_event(SctkKeyEvent::new(
            FlutterKeyEventDeviceType::Keyboard,
            event,
            FlutterKeyEventType::Up,
            Some(latched_keydown),
            self.modifiers,
            false,
        ));
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl SeatHandler for SctkApplicationState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {
        // not needed for current implementation
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {
        // not needed for current implementation
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            let surface = self.windows.shell.compositor_state.create_surface(qh);
            let themed_pointer = self
                .seat_state
                .get_pointer_with_theme(
                    qh,
                    &seat,
                    self.shm_state.wl_shm(),
                    surface,
                    ThemeSpec::default(),
                )
                .ok();

            let pointer = themed_pointer
                .as_ref()
                .map(|themed_pointer| themed_pointer.pointer().clone());

            if let Some(pointer) = pointer {
                self.pointers.insert(seat.id(), pointer);
            } else {
                error!("Failed to create themed wayland pointer");
                self.pointers.remove(&seat.id());
            }

            self.mouse_cursor_handler
                .lock()
                .set_themed_pointer(themed_pointer);
        }

        if capability == Capability::Keyboard {
            if let Ok(keyboard) = self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                self.loop_handle.clone(),
                Box::new(|state, _keyboard, event| {
                    trace!(
                        "key repeated: {}",
                        event.keysym.name().unwrap_or("[unknown]"),
                    );
                    if state.blocked_keys.contains(&event.raw_code) {
                        return;
                    }

                    state.press_key_or_repeat(SctkKeyEvent::new(
                        FlutterKeyEventDeviceType::Keyboard,
                        event,
                        FlutterKeyEventType::Repeat,
                        None,
                        state.modifiers,
                        false,
                    ));
                }),
            ) {
                self.keyboards.insert(seat.id(), keyboard);
            } else {
                error!("Failed to get keyboard");
                self.keyboards.remove(&seat.id());
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        seat: wayland_client::protocol::wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            self.pointers.remove(&seat.id());

            self.mouse_cursor_handler
                .lock()
                .remove_themed_pointer_for_seat(seat.id());
        }

        if capability == Capability::Keyboard {
            self.keyboards.remove(&seat.id());
        }
    }
}

impl OutputHandler for SctkApplicationState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        trace!("[{}] new output", output.id());

        self.notify_display_update();
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        trace!("[{}] update output", output.id());

        self.notify_display_update();
    }

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        trace!("[{}] output destroyed", output.id());

        self.notify_display_update();
    }
}

impl SctkApplicationState {
    pub(crate) fn toplevel_close_requested(&mut self, xdg_toplevel: &XdgToplevel) {
        let Some(view_id) = self
            .windows
            .find(|window| window.xdg_toplevel_id() == Some(xdg_toplevel.id()))
            .map(|window| window.view_id())
        else {
            debug!(
                "[{}] ignoring close request for unknown window",
                xdg_toplevel.id()
            );
            return;
        };

        // Closing the startup window is closing the application. Dart
        // decides whether that happens: it can cancel.
        if view_id == IMPLICIT_VIEW_ID {
            self.with_plugin(|platform: &PlatformPlugin| platform.request_app_exit());
            return;
        }

        // Dart decides about other windows too: it destroys them, or not.
        self.with_plugin(|windowing: &WindowingPlugin| windowing.window_close_requested(view_id));
        #[cfg(feature = "testing")]
        self.with_plugin(|testing: &crate::testing::WindowsTestingPlugin| {
            testing.window_close_requested(view_id)
        });
    }

    /// Tells the engine that the window of `surface` got or lost the
    /// keyboard focus.
    fn send_view_focus_event(&self, surface: &WlSurface, state: FlutterViewFocusState) {
        let Some(window) = self.find_window_by_surface_id(surface.id()) else {
            return;
        };
        if !self.is_engine_running() || window.view_state() != SctkViewState::Added {
            return;
        }
        let view_id = window.view_id();
        let direction = FlutterViewFocusDirection::Undefined;
        if let Err(err) = self.engine.send_view_focus_event(view_id, state, direction) {
            error!("[view {view_id}] failed to send focus {state:?}: {err}");
        }
    }

    /// The compositor would like the window of `surface_id` drawn at this
    /// scale, in 120ths.
    pub(crate) fn preferred_scale_changed(&mut self, surface_id: &ObjectId, scale: u32) {
        trace!("[{surface_id}] preferred scale: {scale}/120");
        if let Some(window) = self.find_window_by_surface_id(surface_id.clone()) {
            window.preferred_scale_changed(scale);
        }
    }

    /// The compositor placed a popup.
    pub(crate) fn popup_configured(&mut self, xdg_surface: &XdgSurface, serial: u32) {
        let Some(window) = self
            .windows
            .find(|window| window.xdg_surface_id() == xdg_surface.id())
        else {
            return;
        };
        window.popup_configured(serial);
        // Where it is may have changed.
        let view_id = window.view_id();
        self.with_plugin(|windowing: &WindowingPlugin| windowing.window_changed(view_id));
    }

    /// The compositor dismissed a popup: the user clicked into another
    /// application, or it denied the popup's grab. Dart destroys it.
    pub(crate) fn popup_dismissed(&mut self, xdg_popup: &XdgPopup) {
        if let Some(window) = self.windows.find(|window| window.is_xdg_popup(xdg_popup)) {
            self.dismiss_popups(&[window.view_id()]);
        }
    }

    pub(crate) fn toplevel_configured(
        &mut self,
        xdg_surface: &XdgSurface,
        configure: ToplevelConfigure,
        serial: u32,
    ) {
        let Some(window) = self
            .windows
            .find(|window| window.xdg_surface_id() == xdg_surface.id())
        else {
            debug!(
                "[{}] ignoring `configure` event for unknown flutter window",
                xdg_surface.id(),
            );
            return;
        };

        let view_id = window.view_id();
        trace!(
            "[view {}] configure: {}x{}",
            view_id,
            configure.size.map_or(0, |size| size.0.get()),
            configure.size.map_or(0, |size| size.1.get()),
        );

        self.maybe_update_lifecycle_state(view_id, configure.activated);

        // The window acknowledges the configure when it commits a frame for
        // it: see `SurfaceSync`.
        self.accessibility
            .set_view_focused(view_id, configure.activated);
        window.configure(configure, serial);

        // What Dart reads of the window may have changed with it. The startup
        // window is configured before the engine runs: Dart reads it once
        // it does, and the engine takes no messages before then.
        if self.is_engine_running() {
            self.with_plugin(|windowing: &WindowingPlugin| windowing.window_changed(view_id));
        }
    }
}

#[derive(Error, Debug)]
pub enum SctkApplicationCreateError {
    #[error(transparent)]
    CalloopError(#[from] calloop::Error),

    #[error(transparent)]
    CalloopInsertWaylandSourceError(
        #[from] calloop::InsertError<WaylandSource<SctkApplicationState>>,
    ),

    #[error(transparent)]
    CalloopInsertAsyncExecutorError(#[from] calloop::InsertError<Executor<SctkAsyncResult>>),

    #[error(transparent)]
    ConnectError(#[from] ConnectError),

    #[error(transparent)]
    GlobalError(#[from] GlobalError),

    #[error(transparent)]
    BindError(#[from] BindError),

    #[error(transparent)]
    SctkFlutterWindowCreateError(#[from] SctkFlutterWindowCreateError),

    #[error(transparent)]
    EngineCreateError(#[from] CreateError),
}

#[derive(Error, Debug)]
pub enum SctkApplicationRunError {
    #[error(transparent)]
    DispatchError(#[from] calloop::Error),

    #[error(transparent)]
    InsertError(#[from] calloop::InsertError<Timer>),
}
