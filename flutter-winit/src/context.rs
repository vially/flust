use glutin::{
    api::egl,
    context::PossiblyCurrentContext,
    display::Display,
    prelude::{GlDisplay, NotCurrentGlContext, PossiblyCurrentGlContext},
    surface::{GlSurface, Surface, WindowSurface},
};
use std::{
    ffi::{c_void, CStr},
    num::NonZeroU32,
};
use winit::dpi::PhysicalSize;
use winit::window::Window;

pub struct Context {
    window: Window,
    display: Display,
    surface: Surface<WindowSurface>,
    context: Option<PossiblyCurrentContext>,
}

impl Context {
    pub fn new(
        window: Window,
        display: Display,
        surface: Surface<WindowSurface>,
        context: PossiblyCurrentContext,
    ) -> Self {
        Self {
            window,
            display,
            surface,
            context: Some(context),
        }
    }

    pub fn make_current(&mut self) -> bool {
        if let Some(ctx) = self.context.take() {
            let result = ctx.make_current(&self.surface).is_ok();
            self.context = Some(ctx);
            return result;
        }
        false
    }

    pub fn make_not_current(&mut self) -> bool {
        if let Some(ctx) = self.context.take() {
            if let Ok(ctx) = ctx.make_not_current() {
                self.context = Some(ctx.treat_as_possibly_current());
                return true;
            }
        }
        false
    }

    pub fn get_proc_address(&mut self, proc: &CStr) -> *const c_void {
        self.display.get_proc_address(proc)
    }

    pub fn resize(&mut self, size: PhysicalSize<NonZeroU32>) {
        if let Some(ctx) = self.context.take() {
            self.surface.resize(&ctx, size.width, size.height);
            self.context = Some(ctx);
        }
    }

    pub fn present(&mut self) -> bool {
        if let Some(ctx) = self.context.take() {
            let result = self.surface.swap_buffers(&ctx).is_ok();
            self.context = Some(ctx);
            return result;
        }
        false
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn size(&self) -> PhysicalSize<u32> {
        self.window().inner_size()
    }

    pub fn hidpi_factor(&self) -> f64 {
        self.window().scale_factor()
    }
}

// `Context` is only `Send` as long as it's used correctly by the engine (e.g.:
// `make_current`/`make_not_current` are *always* called in the correct order
// and on the correct thread). Therefore, just mark it as `Send` until a better
// solution is found.
//
// TODO: Find a solution that better leverages Rust's type system
unsafe impl Send for Context {}

pub struct ResourceContext {
    context: Option<egl::context::PossiblyCurrentContext>,
}

impl ResourceContext {
    pub fn new(context: egl::context::PossiblyCurrentContext) -> Self {
        Self {
            context: Some(context),
        }
    }

    pub fn make_current(&mut self) -> bool {
        if let Some(ctx) = self.context.take() {
            let result = ctx.make_current_surfaceless().is_ok();
            self.context = Some(ctx);
            return result;
        }
        false
    }
}

unsafe impl Send for ResourceContext {}
