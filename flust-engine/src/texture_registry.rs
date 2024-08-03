use crate::FlutterEngine;
use flust_engine_sys::FlutterOpenGLTexture;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tracing::trace;

pub(crate) struct TextureRegistry {
    last_id: AtomicI64,
    frames: Arc<Mutex<HashMap<TextureId, TextureFrame>>>,
}

impl TextureRegistry {
    pub fn new() -> Self {
        Self {
            last_id: AtomicI64::new(1),
            frames: Arc::new(Default::default()),
        }
    }

    pub fn create_texture(&self, engine: FlutterEngine) -> Texture {
        let texture_id = self.last_id.fetch_add(1, Ordering::Relaxed);

        engine.run_on_platform_thread(move |engine| {
            trace!("texture {}: register", texture_id);
            unsafe {
                flust_engine_sys::FlutterEngineRegisterExternalTexture(
                    engine.engine_ptr(),
                    texture_id,
                );
            }
        });

        Texture {
            engine,
            texture_id,
            frames: self.frames.clone(),
        }
    }

    pub fn get_texture_frame(
        &self,
        texture_id: TextureId,
        _size: (usize, usize),
    ) -> Option<TextureFrame> {
        self.frames.lock().remove(&texture_id)
    }
}

pub type TextureId = i64;

pub struct Texture {
    engine: FlutterEngine,
    texture_id: TextureId,
    frames: Arc<Mutex<HashMap<TextureId, TextureFrame>>>,
}

impl Texture {
    pub fn id(&self) -> TextureId {
        self.texture_id
    }

    pub fn post_frame(&self, frame: TextureFrame) {
        post_frame_internal(&self.engine, self.texture_id, &self.frames, frame);
    }
}

fn post_frame_internal(
    engine: &FlutterEngine,
    texture_id: TextureId,
    frames: &Arc<Mutex<HashMap<TextureId, TextureFrame>>>,
    frame: TextureFrame,
) {
    if let Some(old_frame) = frames.lock().insert(texture_id, frame) {
        engine.run_on_render_thread(move |_| {
            (old_frame.destruction_callback)();
        });
    }

    engine.run_on_platform_thread(move |engine| {
        trace!("texture {}: marking frame available", texture_id);
        unsafe {
            flust_engine_sys::FlutterEngineMarkExternalTextureFrameAvailable(
                engine.engine_ptr(),
                texture_id,
            );
        }
    });
}

impl Drop for Texture {
    fn drop(&mut self) {
        let texture_id = self.texture_id;
        self.engine.run_on_platform_thread(move |engine| {
            trace!("texture {}: unregister", texture_id);
            unsafe {
                flust_engine_sys::FlutterEngineUnregisterExternalTexture(
                    engine.engine_ptr(),
                    texture_id,
                );
            }
        });
    }
}

type DestructorType = Box<dyn FnOnce() + Send>;

pub struct TextureFrame {
    pub target: u32,
    pub name: u32,
    pub format: u32,
    pub destruction_callback: Box<DestructorType>,
}

impl TextureFrame {
    pub fn new<F>(target: u32, name: u32, format: u32, destruction_callback: F) -> TextureFrame
    where
        F: FnOnce() + 'static + Send,
    {
        Self {
            target,
            name,
            format,
            destruction_callback: Box::new(Box::new(destruction_callback)),
        }
    }

    pub(crate) fn into_ffi(self, target: &mut FlutterOpenGLTexture) {
        target.target = self.target;
        target.name = self.name;
        target.format = self.format;
        target.destruction_callback = Some(texture_destruction_callback);
        target.user_data = Box::into_raw(self.destruction_callback) as _;
    }
}

unsafe extern "C" fn texture_destruction_callback(user_data: *mut c_void) {
    trace!("texture_destruction_callback");
    let user_data = user_data as *mut DestructorType;
    let user_data = Box::from_raw(user_data);
    user_data();
}
