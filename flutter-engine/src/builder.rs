use parking_lot::Mutex;

use std::path::PathBuf;
use std::sync::Arc;

use crate::tasks::TaskRunnerHandler;
use crate::{CreateError, FlutterEngine, FlutterVsyncHandler};

pub struct FlutterEngineBuilder {
    pub(crate) platform_handler: Option<Arc<dyn TaskRunnerHandler + Send + Sync>>,
    pub(crate) vsync_handler: Option<Arc<Mutex<dyn FlutterVsyncHandler + Send>>>,
    pub(crate) compositor_enabled: bool,
    pub(crate) assets: PathBuf,
    pub(crate) icu_data: PathBuf,
    pub(crate) persistent_cache: PathBuf,
    pub(crate) args: Vec<String>,
}

impl FlutterEngineBuilder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            platform_handler: None,
            vsync_handler: None,
            compositor_enabled: false,
            assets: Default::default(),
            icu_data: Default::default(),
            persistent_cache: Default::default(),
            args: vec![],
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

    pub fn with_compositor_enabled(mut self, enabled: bool) -> Self {
        self.compositor_enabled = enabled;
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

    pub fn build(self) -> Result<FlutterEngine, CreateError> {
        FlutterEngine::new(self)
    }
}
