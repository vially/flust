use thiserror::Error;

use crate::ffi::{FlutterBackingStore, FlutterBackingStoreConfig, FlutterPresentViewInfo};

pub trait FlutterCompositorHandler {
    fn present_view(&self, info: FlutterPresentViewInfo) -> Result<(), CompositorPresentError>;

    fn create_backing_store(
        &self,
        config: FlutterBackingStoreConfig,
    ) -> Result<FlutterBackingStore, CompositorCreateBackingStoreError>;

    fn collect_backing_store(
        &self,
        backing_store: FlutterBackingStore,
    ) -> Result<(), CompositorCollectBackingStoreError>;
}

#[derive(Error, Debug)]
pub enum CompositorPresentError {
    #[error("Present failed: {0}")]
    PresentFailed(String),
}

#[derive(Error, Debug)]
pub enum CompositorCreateBackingStoreError {
    #[error("Failed to create backing store: {0}")]
    CreateFailed(String),
}

#[derive(Error, Debug)]
pub enum CompositorCollectBackingStoreError {
    #[error("Failed to collect backing store: {0}")]
    CollectFailed(String),
}
