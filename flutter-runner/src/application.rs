use std::path::PathBuf;

use dpi::Size;
use flutter_runner_api::{ApplicationAttributes, Backend};
use thiserror::Error;

#[cfg(feature = "flutter-winit")]
use flutter_winit::{WinitApplication, WinitApplicationBuildError, WinitApplicationRunError};

pub enum Application {
    #[cfg(feature = "flutter-winit")]
    Winit(WinitApplication),
}

impl Application {
    pub fn builder() -> ApplicationBuilder {
        ApplicationBuilder::default()
    }

    pub fn new(attributes: ApplicationAttributes) -> Result<Application, ApplicationBuildError> {
        match attributes.backend {
            #[cfg(feature = "flutter-winit")]
            Backend::Winit => Ok(Application::Winit(WinitApplication::new(attributes)?)),
        }
    }

    pub fn run(&mut self) -> Result<(), ApplicationRunError> {
        match self {
            #[cfg(feature = "flutter-winit")]
            Self::Winit(app) => Ok(app.run()?),
        }
    }
}

/// Configure application before creation.
///
/// You can access this from [`Application::builder`].
#[derive(Clone, Default)]
pub struct ApplicationBuilder {
    /// The attributes to use to create the application.
    pub(crate) attributes: ApplicationAttributes,
}

impl ApplicationBuilder {
    /// Builds the application.
    pub fn build(self) -> Result<Application, ApplicationBuildError> {
        let application = Application::new(self.attributes)?;
        Ok(application)
    }

    pub fn with_backend(mut self, backend: Backend) -> Self {
        self.attributes.backend = backend;
        self
    }

    pub fn with_inner_size<S: Into<Size>>(mut self, size: S) -> Self {
        self.attributes.inner_size = Some(size.into());
        self
    }

    pub fn with_title<T: Into<String>>(mut self, title: T) -> Self {
        self.attributes.title = Some(title.into());
        self
    }

    pub fn with_app_id<T: Into<String>>(mut self, app_id: T) -> Self {
        self.attributes.app_id = Some(app_id.into());
        self
    }

    pub fn with_arg(mut self, arg: String) -> Self {
        self.attributes.args.push(arg);
        self
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        for arg in args.into_iter() {
            self.attributes.args.push(arg);
        }
        self
    }

    pub fn with_assets_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.attributes.assets_path = path.into();
        self
    }

    pub fn with_icu_data_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.attributes.icu_data_path = path.into();
        self
    }
}

#[derive(Error, Debug)]
pub enum ApplicationBuildError {
    #[cfg_attr(feature = "flutter-winit", error(transparent))]
    WinitApplicationBuildError(#[from] WinitApplicationBuildError),
}

#[derive(Error, Debug)]
pub enum ApplicationRunError {
    #[cfg_attr(feature = "flutter-winit", error(transparent))]
    WinitApplicationRunError(#[from] WinitApplicationRunError),
}
