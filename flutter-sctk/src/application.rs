use std::{collections::HashMap, fmt::Debug};

use flutter_runner_api::ApplicationAttributes;
use log::{error, trace};
use smithay_client_toolkit::{
    compositor::CompositorHandler,
    delegate_compositor, delegate_output, delegate_pointer, delegate_registry, delegate_seat,
    delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::xdg::window::{Window, WindowConfigure, WindowHandler},
};
use thiserror::Error;
use wayland_backend::client::ObjectId;
use wayland_client::{
    globals::{registry_queue_init, GlobalError},
    protocol::{
        wl_output::{Transform, WlOutput},
        wl_pointer::WlPointer,
        wl_seat::WlSeat,
        wl_surface::WlSurface,
    },
    ConnectError, Connection, DispatchError, EventQueue, Proxy, QueueHandle,
};

pub struct SctkApplication {
    event_queue: EventQueue<SctkApplicationState>,
    state: SctkApplicationState,
}

struct SctkApplicationState {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    pointers: HashMap<ObjectId, WlPointer>,
    exit: bool,
}

impl SctkApplication {
    pub fn new(_attributes: ApplicationAttributes) -> Result<Self, SctkApplicationCreateError> {
        let conn = Connection::connect_to_env()?;
        let (globals, event_queue) = registry_queue_init(&conn)?;
        let qh = event_queue.handle();

        let registry_state = RegistryState::new(&globals);
        let output_state = OutputState::new(&globals, &qh);
        let seat_state = SeatState::new(&globals, &qh);

        let state = SctkApplicationState {
            pointers: HashMap::new(),
            registry_state,
            output_state,
            seat_state,
            exit: false,
        };

        Ok(Self { event_queue, state })
    }

    pub fn run(mut self) -> Result<(), SctkApplicationRunError> {
        while !self.state.exit {
            self.event_queue.blocking_dispatch(&mut self.state)?;
        }

        Ok(())
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
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        _events: &[PointerEvent],
    ) {
        // not implemented yet
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
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        trace!(
            "[{}] configure: {}x{}",
            window.xdg_toplevel().id(),
            configure.new_size.0.map_or(0, |v| v.get()),
            configure.new_size.1.map_or(0, |v| v.get()),
        );
    }
}

#[derive(Error, Debug)]
pub enum SctkApplicationCreateError {
    #[error(transparent)]
    ConnectError(#[from] ConnectError),

    #[error(transparent)]
    GlobalError(#[from] GlobalError),
}

#[derive(Error, Debug)]
pub enum SctkApplicationRunError {
    #[error(transparent)]
    DispatchError(#[from] DispatchError),
}
