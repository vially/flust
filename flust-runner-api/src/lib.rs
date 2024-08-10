use std::path::PathBuf;

use dpi::Size;
use flust_engine::ffi::FlutterOpenGLTargetType;

#[derive(Debug, Clone)]
pub enum Backend {
    Sctk(BackendConfigSctk),
    Winit,
}

impl Default for Backend {
    fn default() -> Self {
        Self::Sctk(Default::default())
    }
}

#[derive(Debug, Clone)]
pub struct BackendConfigSctk {
    pub opengl_target_type: FlutterOpenGLTargetType,
}

impl Default for BackendConfigSctk {
    fn default() -> Self {
        Self {
            opengl_target_type: FlutterOpenGLTargetType::Framebuffer,
        }
    }
}

/// Attributes used when creating an application.
#[derive(Debug, Clone, Default)]
pub struct ApplicationAttributes {
    pub backend: Backend,
    pub inner_size: Option<Size>,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub args: Vec<String>,
    pub aot_library_path: PathBuf,
    pub assets_path: PathBuf,
    pub icu_data_path: PathBuf,
    pub persistent_cache_path: PathBuf,
}
