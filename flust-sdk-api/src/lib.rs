use std::str::FromStr;

#[derive(Debug)]
pub enum FlutterBuildMode {
    Debug(CompilerOptimization),
    Profile,
    Release,
}

#[derive(Debug)]
pub struct CompilerOptimization {
    pub optimized: bool,
}

impl CompilerOptimization {
    pub fn optimized() -> Self {
        Self { optimized: true }
    }

    pub fn unoptimized() -> Self {
        Self { optimized: false }
    }
}

impl FlutterBuildMode {
    // TODO: Find a better way of auto-detecting build modes
    pub fn from_cargo_profile() -> Self {
        // Use the Cargo `profile` as a replacement for Flutter build-mode until
        // a better solution is implemented.
        //
        // Docs: https://doc.rust-lang.org/cargo/reference/profiles.html#debug
        match std::env::var("DEBUG").as_deref() {
            Ok("true") => Self::Debug(CompilerOptimization::unoptimized()),
            // TODO: Add support for auto-detecting `profile` mode
            _ => Self::Release,
        }
    }
}

impl FromStr for FlutterBuildMode {
    type Err = ();

    fn from_str(mode: &str) -> Result<Self, Self::Err> {
        match mode {
            "debug" => Ok(FlutterBuildMode::Debug(CompilerOptimization::optimized())),
            "debug_unopt" => Ok(FlutterBuildMode::Debug(CompilerOptimization::unoptimized())),
            "profile" => Ok(FlutterBuildMode::Profile),
            "release" => Ok(FlutterBuildMode::Release),
            _ => Err(()),
        }
    }
}

impl From<FlutterBuildMode> for String {
    fn from(build_mode: FlutterBuildMode) -> Self {
        match build_mode {
            FlutterBuildMode::Debug(CompilerOptimization { optimized }) => match optimized {
                true => "debug".to_owned(),
                false => "debug_unopt".to_owned(),
            },
            FlutterBuildMode::Profile => "profile".to_owned(),
            FlutterBuildMode::Release => "release".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FlutterRelease {
    pub flutter_version: String,
    pub engine_version: String,
}
