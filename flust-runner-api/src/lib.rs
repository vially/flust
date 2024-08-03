use std::path::PathBuf;

use dpi::Size;

#[derive(Debug, Clone, Default)]
pub enum Backend {
    #[default]
    Sctk,
    Winit,
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
