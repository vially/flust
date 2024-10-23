use std::{fmt::Display, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum FlutterBuildMode {
    Debug(CompilerOptimization),
    Profile,
    Release,
}

impl FlutterBuildMode {
    pub fn iter() -> impl Iterator<Item = Self> {
        [
            Self::Debug(CompilerOptimization::optimized()),
            Self::Debug(CompilerOptimization::unoptimized()),
            Self::Profile,
            Self::Release,
        ]
        .into_iter()
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

impl From<&FlutterBuildMode> for String {
    fn from(build_mode: &FlutterBuildMode) -> Self {
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

impl Display for FlutterBuildMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from(self))
    }
}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum FlutterTargetArch {
    X86_64,
    Aarch64,
}

impl FlutterTargetArch {
    /// Detect the target Flutter architecture from the `cfg!(target_arch)`
    /// attribute.
    ///
    /// ⚠️ Warning ⚠️: This function makes use of `cfg!` attributes which are
    /// evaluated at *build* time so they are not suitable for use in Cargo
    /// build scripts. For Cargo build scripts use `from_cargo_target` instead.
    pub fn from_cfg_target() -> Self {
        if cfg!(target_arch = "x86_64") {
            Self::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Self::Aarch64
        } else {
            panic!("unsupported Flutter target architecture");
        }
    }

    /// Detect the target architecture at *runtime* using the [environment
    /// variables set by Cargo](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates).
    ///
    /// This function is the recommended way of detecting the target Flutter
    /// architecture in Cargo build scripts.
    pub fn from_cargo_target() -> Self {
        match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86_64") => Self::X86_64,
            Ok("aarch64") => Self::Aarch64,
            _ => panic!("unsupported Flutter target architecture"),
        }
    }
}

impl FromStr for FlutterTargetArch {
    type Err = ();

    fn from_str(arch: &str) -> Result<Self, Self::Err> {
        Ok(match arch {
            "x86_64" => Self::X86_64,
            "aarch64" => Self::Aarch64,
            _ => return Err(()),
        })
    }
}

impl From<&FlutterTargetArch> for String {
    fn from(arch: &FlutterTargetArch) -> Self {
        match arch {
            FlutterTargetArch::X86_64 => "x86_64".to_owned(),
            FlutterTargetArch::Aarch64 => "aarch64".to_owned(),
        }
    }
}

impl Display for FlutterTargetArch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from(self))
    }
}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
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

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
pub struct FlutterSDKVersion(String);

impl From<String> for FlutterSDKVersion {
    fn from(version: String) -> Self {
        FlutterSDKVersion(version)
    }
}

impl From<FlutterSDKVersion> for String {
    fn from(version: FlutterSDKVersion) -> Self {
        version.0
    }
}

impl Display for FlutterSDKVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
pub struct FlutterEngineVersion(String);

impl From<String> for FlutterEngineVersion {
    fn from(version: String) -> Self {
        FlutterEngineVersion(version)
    }
}

impl From<FlutterEngineVersion> for String {
    fn from(version: FlutterEngineVersion) -> Self {
        version.0
    }
}

impl Display for FlutterEngineVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum FlutterVersion {
    SDK(FlutterSDKVersion),
    Engine(FlutterEngineVersion),
}

impl From<FlutterSDKVersion> for FlutterVersion {
    fn from(sdk_version: FlutterSDKVersion) -> Self {
        FlutterVersion::SDK(sdk_version)
    }
}

impl From<FlutterEngineVersion> for FlutterVersion {
    fn from(engine_version: FlutterEngineVersion) -> Self {
        FlutterVersion::Engine(engine_version)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FlutterRelease {
    pub sdk_version: FlutterSDKVersion,
    pub engine_version: FlutterEngineVersion,
}
