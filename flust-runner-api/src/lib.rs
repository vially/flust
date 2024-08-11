use std::path::PathBuf;

use dpi::Size;
pub use flust_engine::ffi::FlutterOpenGLTargetType;

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

impl From<BackendConfigSctk> for Backend {
    fn from(config: BackendConfigSctk) -> Self {
        Self::Sctk(config)
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

impl From<FlutterOpenGLTargetType> for BackendConfigSctk {
    fn from(opengl_target_type: FlutterOpenGLTargetType) -> Self {
        Self { opengl_target_type }
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
