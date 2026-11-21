use parking_lot::Mutex;

use std::path::PathBuf;
use std::sync::Arc;

use crate::renderer::Renderer;
use crate::semantics::FlutterSemanticsHandler;
use crate::tasks::TaskRunnerHandler;
use crate::view::FlutterViewFocusHandler;
use crate::{CreateError, FlutterEngine, FlutterVsyncHandler};
use flust_engine_api::FlutterOpenGLHandler;

pub struct FlutterEngineBuilder {
    pub(crate) platform_handler: Option<Arc<dyn TaskRunnerHandler + Send + Sync>>,
    pub(crate) vsync_handler: Option<Arc<Mutex<dyn FlutterVsyncHandler + Send>>>,
    pub(crate) semantics_handler: Option<Arc<dyn FlutterSemanticsHandler + Send + Sync>>,
    pub(crate) view_focus_handler: Option<Arc<dyn FlutterViewFocusHandler + Send + Sync>>,
    pub(crate) opengl_handler: Option<Arc<dyn FlutterOpenGLHandler + Send + Sync>>,
    pub(crate) compositor_enabled: bool,
    pub(crate) merged_platform_ui_thread: bool,
    pub(crate) aot_library: PathBuf,
    pub(crate) assets: PathBuf,
    pub(crate) icu_data: PathBuf,
    pub(crate) persistent_cache: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) dart_entrypoint_args: Vec<String>,
    pub(crate) renderer: Renderer,
}

impl FlutterEngineBuilder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            platform_handler: None,
            vsync_handler: None,
            semantics_handler: None,
            view_focus_handler: None,
            opengl_handler: None,
            compositor_enabled: false,
            merged_platform_ui_thread: false,
            aot_library: Default::default(),
            assets: Default::default(),
            icu_data: Default::default(),
            persistent_cache: Default::default(),
            args: vec![],
            dart_entrypoint_args: vec![],
            renderer: Renderer::default(),
        }
    }

    pub fn with_platform_handler(
        mut self,
        handler: Arc<dyn TaskRunnerHandler + Send + Sync>,
    ) -> Self {
        self.platform_handler = Some(handler);
        self
    }

    pub fn with_vsync_handler(
        mut self,
        handler: Arc<Mutex<dyn FlutterVsyncHandler + Send>>,
    ) -> Self {
        self.vsync_handler = Some(handler);
        self
    }

    /// Without a handler, the engine is not asked for semantics updates.
    pub fn with_semantics_handler(
        mut self,
        handler: Arc<dyn FlutterSemanticsHandler + Send + Sync>,
    ) -> Self {
        self.semantics_handler = Some(handler);
        self
    }

    /// The OpenGL callbacks of the engine (`make_current` and the others),
    /// for all views. Without it, they go to the implicit view's handler.
    ///
    /// With a compositor, the engine draws every view in the one context
    /// that `make_current` makes current, and does not say which view it
    /// is drawing: a handler of the implicit view can only serve one.
    pub fn with_opengl_handler(
        mut self,
        handler: Arc<dyn FlutterOpenGLHandler + Send + Sync>,
    ) -> Self {
        self.opengl_handler = Some(handler);
        self
    }

    /// Without a handler, the framework's requests to move the platform's
    /// focus between views are ignored.
    pub fn with_view_focus_handler(
        mut self,
        handler: Arc<dyn FlutterViewFocusHandler + Send + Sync>,
    ) -> Self {
        self.view_focus_handler = Some(handler);
        self
    }

    /// The renderer to use, unless the engine arguments select one with
    /// `--enable-impeller`: see [`Renderer::resolve`].
    pub fn with_renderer(mut self, renderer: Renderer) -> Self {
        self.renderer = renderer;
        self
    }

    pub fn with_compositor_enabled(mut self, enabled: bool) -> Self {
        self.compositor_enabled = enabled;
        self
    }

    /// Runs Dart on the platform thread, instead of on a UI thread of the
    /// engine's own.
    ///
    /// The engine then runs Dart from within some of the calls made to it
    /// on the platform thread (window metrics, pointer events, platform
    /// messages, among others), before they return. Callbacks that Dart
    /// triggers synchronously, such as the vsync callback, are then called
    /// on the platform thread too, from within those calls: no lock that
    /// such a callback takes may be held while making them. Messages from
    /// Dart, and the responses to messages sent to it, are still delivered
    /// as platform tasks.
    ///
    /// Flutter's own desktop embedders do this by default, and its
    /// windowing API depends on it: a window is created synchronously, from
    /// Dart.
    pub fn with_merged_platform_ui_thread(mut self, merged: bool) -> Self {
        self.merged_platform_ui_thread = merged;
        self
    }

    pub fn with_aot_library_path(mut self, path: PathBuf) -> Self {
        self.aot_library = path;
        self
    }

    pub fn with_asset_path(mut self, path: PathBuf) -> Self {
        self.assets = path;
        self
    }

    pub fn with_icu_data_path(mut self, path: PathBuf) -> Self {
        self.icu_data = path;
        self
    }

    pub fn with_persistent_cache_path(mut self, path: PathBuf) -> Self {
        self.persistent_cache = path;
        self
    }

    pub fn with_arg(mut self, arg: String) -> Self {
        self.args.push(arg);
        self
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        for arg in args.into_iter() {
            self.args.push(arg);
        }
        self
    }

    /// Arguments for the Dart entrypoint: the `args` of
    /// `main(List<String> args)`. Engine switches go to [`Self::with_args`]
    /// instead.
    pub fn with_dart_entrypoint_args(mut self, args: Vec<String>) -> Self {
        self.dart_entrypoint_args = args;
        self
    }

    pub fn build(self) -> Result<FlutterEngine, CreateError> {
        FlutterEngine::new(self)
    }
}
