use std::{collections::HashMap, fmt::Debug, sync::Arc};

use flutter_engine::{builder::FlutterEngineBuilder, CreateError, FlutterEngine};
use flutter_runner_api::ApplicationAttributes;
use log::{error, trace, warn};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_pointer, delegate_registry, delegate_seat,
    delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            self,
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle, LoopSignal,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::xdg::{
        window::{Window, WindowConfigure, WindowHandler},
        XdgShell,
    },
};
use thiserror::Error;
use wayland_backend::client::ObjectId;
use wayland_client::{
    globals::{registry_queue_init, BindError, GlobalError},
    protocol::{
        wl_output::{Transform, WlOutput},
        wl_pointer::WlPointer,
        wl_seat::WlSeat,
        wl_surface::WlSurface,
    },
    ConnectError, Connection, Proxy, QueueHandle,
};

use crate::{
    handler::SctkPlatformTaskHandler,
    window::{SctkFlutterWindow, SctkFlutterWindowCreateError},
};

pub struct SctkApplication {
    event_loop: EventLoop<'static, SctkApplicationState>,
    state: SctkApplicationState,
}

pub struct SctkApplicationState {
    loop_handle: LoopHandle<'static, SctkApplicationState>,
    loop_signal: LoopSignal,
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    engine: FlutterEngine,
    windows: HashMap<ObjectId, SctkFlutterWindow>,
    pointers: HashMap<ObjectId, WlPointer>,
}

impl SctkApplication {
    pub fn new(attributes: ApplicationAttributes) -> Result<Self, SctkApplicationCreateError> {
        let conn = Connection::connect_to_env()?;
        let (globals, event_queue) = registry_queue_init(&conn)?;
        let qh = event_queue.handle();

        let event_loop: EventLoop<SctkApplicationState> = EventLoop::try_new()?;
        WaylandSource::new(conn.clone(), event_queue).insert(event_loop.handle())?;

        let registry_state = RegistryState::new(&globals);
        let output_state = OutputState::new(&globals, &qh);
        let seat_state = SeatState::new(&globals, &qh);
        let compositor_state = CompositorState::bind(&globals, &qh)?;
        let xdg_shell_state = XdgShell::bind(&globals, &qh)?;

        let platform_task_handler = Arc::new(SctkPlatformTaskHandler::new(event_loop.get_signal()));

        let engine = FlutterEngineBuilder::new()
            .with_platform_handler(platform_task_handler)
            .with_asset_path(attributes.assets_path.clone())
            .with_icu_data_path(attributes.icu_data_path.clone())
            .with_args(attributes.args.clone())
            .build()?;

        let implicit_window =
            SctkFlutterWindow::new(&qh, &compositor_state, &xdg_shell_state, attributes)?;

        engine.add_view(implicit_window.create_flutter_view());

        let state = SctkApplicationState {
            loop_handle: event_loop.handle(),
            loop_signal: event_loop.get_signal(),
            windows: HashMap::from([(implicit_window.xdg_toplevel_id(), implicit_window)]),
            pointers: HashMap::new(),
            registry_state,
            output_state,
            seat_state,
            engine,
        };

        Ok(Self { event_loop, state })
    }

    pub fn run(mut self) -> Result<(), SctkApplicationRunError> {
        // The event loop needs to be started *prior* to running the engine (see
        // `FlutterEngineRun` comment in `embedder.h` for additional context).
        // Therefore, use an immediate timer source for starting the engine
        // once the event loop is running.
        //
        // https://github.com/flutter/engine/blob/7c2a56a44b414f2790af277783ec27181337a6d3/shell/platform/embedder/embedder.h#L2313-L2322
        let _ =
            self.state
                .loop_handle
                .insert_source(Timer::immediate(), |_event, _metadata, state| {
                    state.engine.run().expect("Failed to run engine");
                    TimeoutAction::Drop
                });

        self.event_loop.run(None, &mut self.state, |state| {
            let next_task_timer = state
                .engine
                .execute_platform_tasks()
                .map(Timer::from_deadline);

            insert_timer_source(&state.loop_handle, next_task_timer);
        })?;

        Ok(())
    }
}

impl SctkApplicationState {
    fn find_window_by_surface_id_mut(
        &mut self,
        surface_id: ObjectId,
    ) -> Option<&mut SctkFlutterWindow> {
        self.windows.iter_mut().find_map(|(_key, val)| {
            if val.wl_surface_id() == surface_id {
                Some(val)
            } else {
                None
            }
        })
    }
}

delegate_compositor!(SctkApplicationState);
delegate_output!(SctkApplicationState);

delegate_xdg_shell!(SctkApplicationState);
delegate_xdg_window!(SctkApplicationState);

delegate_seat!(SctkApplicationState);
delegate_pointer!(SctkApplicationState);

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
        conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        new_scale_factor: i32,
    ) {
        trace!(
            "[{}] scale factor changed: {}",
            surface.id(),
            new_scale_factor
        );

        let Some(window) = self.find_window_by_surface_id_mut(surface.id()) else {
            warn!(
                "[{}] ignoring `scale_factor_changed` event for unknown flutter window",
                surface.id()
            );
            return;
        };

        window.scale_factor_changed(conn, surface, new_scale_factor);
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
        trace!("[{}] frame callback: {}", surface.id(), time,);
    }
}

impl PointerHandler for SctkApplicationState {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _qh: &QueueHandle<Self>,
        pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(window) = self.find_window_by_surface_id_mut(event.surface.id()) else {
                warn!(
                    "[{}] ignoring pointer event for unknown flutter window",
                    event.surface.id()
                );
                continue;
            };

            window.pointer_event(conn, pointer, event);
        }
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
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => {
                    self.pointers.insert(seat.id(), pointer);
                }
                _ => error!("Failed to create wayland pointer"),
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
        }
    }
}

impl OutputHandler for SctkApplicationState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {
        // not needed for current implementation
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {
        // not needed for current implementation
    }

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {
        // not needed for current implementation
    }
}

impl WindowHandler for SctkApplicationState {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.loop_signal.stop();
    }

    fn configure(
        &mut self,
        conn: &Connection,
        _qh: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        serial: u32,
    ) {
        let xdg_toplevel_id = window.xdg_toplevel().id();
        trace!(
            "[{}] configure: {}x{}",
            xdg_toplevel_id,
            configure.new_size.0.map_or(0, |v| v.get()),
            configure.new_size.1.map_or(0, |v| v.get()),
        );

        let Some(window) = self.windows.get_mut(&xdg_toplevel_id) else {
            warn!(
                "[{}] ignoring `configure` event for unknown flutter window",
                xdg_toplevel_id,
            );
            return;
        };

        window.configure(conn, configure, serial);
    }
}

#[derive(Error, Debug)]
pub enum SctkApplicationCreateError {
    #[error(transparent)]
    CalloopError(#[from] calloop::Error),

    #[error(transparent)]
    CalloopInsertError(#[from] calloop::InsertError<WaylandSource<SctkApplicationState>>),

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
}

fn insert_timer_source<Data>(handle: &LoopHandle<'static, Data>, timer: Option<Timer>) {
    let Some(timer) = timer else {
        return;
    };

    handle
        .insert_source(timer, |_, _, _| TimeoutAction::Drop)
        .expect("Unable to insert timer source");
}
