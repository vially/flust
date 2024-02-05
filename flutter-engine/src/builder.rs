use crate::tasks::TaskRunnerHandler;
use crate::{CreateError, FlutterEngine, FlutterVsyncHandler};
use std::path::PathBuf;
use std::sync::Arc;

pub struct FlutterEngineBuilder {
    pub(crate) platform_handler: Option<Arc<dyn TaskRunnerHandler + Send + Sync>>,
    pub(crate) vsync_handler: Option<Box<dyn FlutterVsyncHandler + Send>>,
    pub(crate) assets: PathBuf,
    pub(crate) icu_data: PathBuf,
    pub(crate) args: Vec<String>,
}

impl FlutterEngineBuilder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            platform_handler: None,
            vsync_handler: None,
            assets: Default::default(),
            icu_data: Default::default(),
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

    pub fn with_vsync_handler<H>(mut self, handler: H) -> Self
    where
        H: FlutterVsyncHandler + Send + 'static,
    {
        self.vsync_handler = Some(Box::new(handler));
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
