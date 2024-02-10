use std::error::Error as StdError;
use std::path::PathBuf;

use dpi::Size;
use flutter_engine::{FlutterEngine, RunError};
use flutter_winit::{EventLoopError, PhysicalSize};
use thiserror::Error;

#[cfg(feature = "flutter-winit")]
use flutter_winit::{FlutterWindow, WindowBuilder, WindowBuilderExtWayland};

pub enum Application {
    #[cfg(feature = "flutter-winit")]
    Winit(WinitApplication),
}

#[cfg(feature = "flutter-winit")]
pub struct WinitApplication {
    window: Option<FlutterWindow>,
    engine: FlutterEngine,
}

impl WinitApplication {
    pub fn new(
        attributes: ApplicationAttributes,
    ) -> Result<WinitApplication, ApplicationBuildError> {
        let builder = WindowBuilder::new();
        let builder = attributes
            .title
            .map_or(builder.clone(), |title| builder.with_title(title));

        let builder = attributes
            .app_id
            .map_or(builder.clone(), |app_id| builder.with_name(app_id, ""));

        let builder = attributes.inner_size.map_or(builder.clone(), |size| {
            builder.with_inner_size(PhysicalSize::new(
                size.to_physical::<u32>(1.0).width,
                size.to_physical::<u32>(1.0).height,
            ))
        });

        let (window, engine) = FlutterWindow::new(
            builder,
            attributes.assets_path,
            attributes.icu_data_path,
            attributes.args,
        )?;

        Ok(WinitApplication {
            window: Some(window),
            engine,
        })
    }

    pub fn run(&mut self) -> Result<(), ApplicationRunError> {
        let Some(window) = self.window.take() else {
            return Err(RunError::InternalInconsistency.into());
        };

        self.engine.run()?;
        window.run()?;

        Ok(())
    }
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
            Self::Winit(app) => app.run(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub enum Backend {
    #[cfg_attr(feature = "flutter-winit", default)]
    Winit,
}

/// Attributes used when creating an application.
#[derive(Debug, Clone, Default)]
pub struct ApplicationAttributes {
    pub(crate) backend: Backend,
    pub(crate) inner_size: Option<Size>,
    pub(crate) title: Option<String>,
    pub(crate) app_id: Option<String>,
    pub(crate) args: Vec<String>,
    pub(crate) assets_path: PathBuf,
    pub(crate) icu_data_path: PathBuf,
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
    WinitWindowBuildError(#[from] Box<dyn StdError>),
}

#[derive(Error, Debug)]
pub enum ApplicationRunError {
    #[cfg_attr(feature = "flutter-winit", error(transparent))]
    WinitStartEngineError(#[from] RunError),

    #[cfg_attr(feature = "flutter-winit", error(transparent))]
    WinitEventLoopError(#[from] EventLoopError),
}
