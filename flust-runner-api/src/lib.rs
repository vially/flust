use std::path::PathBuf;

use dpi::Size;
pub use flust_engine::ffi::FlutterOpenGLTargetType;
pub use flust_engine::renderer::Renderer;

#[derive(Debug, Clone)]
pub enum Backend {
    Sctk(BackendConfigSctk),
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
    /// Whether the application starts with a window, the implicit view,
    /// which is what an app that calls `runApp` draws into. On by default.
    ///
    /// Without it, the app creates its windows itself, from Dart, and only
    /// then can it have more than one: Flutter does not combine a window
    /// the embedder creates with windows created from Dart.
    pub startup_window: bool,
}

impl BackendConfigSctk {
    /// See [`Self::startup_window`].
    pub fn with_startup_window(mut self, startup_window: bool) -> Self {
        self.startup_window = startup_window;
        self
    }
}

impl Default for BackendConfigSctk {
    fn default() -> Self {
        Self::from(FlutterOpenGLTargetType::Framebuffer)
    }
}

impl From<FlutterOpenGLTargetType> for BackendConfigSctk {
    fn from(opengl_target_type: FlutterOpenGLTargetType) -> Self {
        Self {
            opengl_target_type,
            startup_window: true,
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
    /// The renderer to use, unless the engine switches select one with
    /// `--enable-impeller`: see [`ApplicationAttributes::renderer`].
    pub renderer: Renderer,
    pub aot_library_path: PathBuf,
    pub assets_path: PathBuf,
    pub icu_data_path: PathBuf,
    pub persistent_cache_path: PathBuf,
}

impl ApplicationAttributes {
    /// The renderer the engine will draw with: the one its switches select
    /// with `--enable-impeller` (`args`, or the `flutter` tool's from the
    /// environment), or `renderer` if they don't.
    pub fn renderer(&self) -> Renderer {
        Renderer::resolve(
            self.renderer,
            &flust_engine::effective_engine_switches(&self.args),
        )
    }
}
