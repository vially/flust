use bindgen::EnumVariation;
use flust_build::{EngineLibraryBuild, EngineVersionManager, FlutterSDK};
use flust_sdk_api::{FlutterBuildMode, FlutterTargetArch};
use std::path::{Path, PathBuf};
use thiserror::Error;

const FLUTTER_SDK_MISSING_NO_REBUILD_WARNING: &str = "Flutter SDK path could not be determined. \
Linking to the Flutter engine library might not work as expected.";

fn main() -> Result<(), BuildError> {
    BindingsBuilder::generate("flust-engine-sys.rs")?;

    // TODO: Investigate if it is possible to set the `rpath` on the *app binary*
    // using an instruction like: `cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN`
    CargoInstruction::rustc_link_lib("flutter_engine");
    CargoInstruction::rerun_if_changed("embedder.h");
    CargoInstruction::rerun_if_changed("src/lib.rs");
    CargoInstruction::rerun_if_env_changed("FLUST_BUILD_MODE");

    let engine_build_context = EngineLibraryBuildContext::from_cargo_build_env();
    engine_build_context.print_extra_cargo_instructions();
    engine_build_context.symlink_to_target_dir()?;

    Ok(())
}

enum EngineLibraryLocation {
    Path(PathBuf),
    Build(EngineLibraryBuild),
}

struct EngineLibraryBuildContext {
    engine_library: EngineLibraryLocation,
    flutter: Option<FlutterSDK>,
    library_name: &'static str,
}

impl EngineLibraryBuildContext {
    fn from_cargo_build_env() -> EngineLibraryBuildContext {
        let flutter = FlutterSDK::auto_detect().ok();

        let library_name = match std::env::var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
            "linux" => "libflutter_engine.so",
            "macos" => "libflutter_engine.dylib",
            "windows" => "flutter_engine.lib",
            _ => panic!("Unsupported target OS"),
        };

        let engine_library_location = match std::env::var("FLUTTER_ENGINE_LIB_PATH").ok() {
            Some(library_path) => EngineLibraryLocation::Path(PathBuf::from(library_path)),
            None => {
                let release = flutter
                    .as_ref()
                    .and_then(|flutter| flutter.release().ok())
                    .expect("Failed to determine Flutter release");
                let build_mode = FlutterBuildMode::from_env();
                let target = FlutterTargetArch::from_cargo_target();

                EngineLibraryLocation::Build(EngineLibraryBuild::new(
                    release,
                    target,
                    build_mode,
                    library_name,
                ))
            }
        };

        EngineLibraryBuildContext {
            flutter,
            engine_library: engine_library_location,
            library_name,
        }
    }

    fn library_path(&self) -> PathBuf {
        match &self.engine_library {
            EngineLibraryLocation::Path(library_path) => library_path.join(self.library_name),
            EngineLibraryLocation::Build(engine_build) => {
                engine_build.library_path_by_sdk_version()
            }
        }
    }

    fn library_path_dir(&self) -> PathBuf {
        self.library_path().parent().unwrap().to_path_buf()
    }

    fn target_build_dir(&self) -> PathBuf {
        // `OUT_DIR` points to `./target/debug/build/<pkg>/out` so we need to go
        // up 3 levels to get to the target directory.
        PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR environment variable not set"))
            .ancestors()
            .nth(3)
            .expect("Unable to determine target build directory from OUT_DIR")
            .into()
    }

    fn target_engine_library_path(&self) -> PathBuf {
        self.target_build_dir().join(self.library_name)
    }

    fn print_extra_cargo_instructions(&self) {
        CargoInstruction::rustc_link_search(&self.library_path_dir().display().to_string());

        let Some(flutter) = self.flutter.as_ref() else {
            CargoInstruction::warning(FLUTTER_SDK_MISSING_NO_REBUILD_WARNING);
            return;
        };

        let engine_version_path = flutter.engine_version_path().display().to_string();
        CargoInstruction::rerun_if_changed(&engine_version_path);
    }

    fn symlink_to_target_dir(&self) -> Result<(), BuildError> {
        if let EngineLibraryLocation::Build(engine_build) = &self.engine_library {
            EngineVersionManager::ensure_library_version_is_installed(engine_build)?;
        }

        let engine_library_path = self.library_path();

        let target_engine_library_path = self.target_engine_library_path();
        if target_engine_library_path.exists() {
            std::fs::remove_file(&target_engine_library_path)?;
        }

        // Note: Symlinking the engine library to the target directory ensures
        // `cargo run`/`cargo test` will be able to find the engine library
        // without having to set the `LD_LIBRARY_PATH` environment variable.
        //
        // Docs: https://doc.rust-lang.org/cargo/reference/environment-variables.html#dynamic-library-paths
        std::os::unix::fs::symlink(&engine_library_path, self.target_engine_library_path())?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum BuildError {
    #[error(transparent)]
    Bindgen(#[from] bindgen::BindgenError),

    #[error(transparent)]
    IO(#[from] std::io::Error),

    #[error(transparent)]
    Flutter(#[from] flust_build::Error),
}

struct CargoInstruction {}

impl CargoInstruction {
    fn rustc_link_lib(lib: &str) {
        println!("cargo::rustc-link-lib={lib}");
    }

    fn rerun_if_changed(path: &str) {
        println!("cargo::rerun-if-changed={path}");
    }

    fn rerun_if_env_changed(name: &str) {
        println!("cargo::rerun-if-env-changed={name}");
    }

    fn rustc_link_search(path: &str) {
        println!("cargo::rustc-link-search={path}");
    }

    fn warning(msg: &str) {
        println!("cargo::warning={msg}");
    }
}

struct BindingsBuilder {}

impl BindingsBuilder {
    fn generate<P: AsRef<Path>>(filename: P) -> Result<(), BuildError> {
        let bindings = bindgen::Builder::default()
            .header("embedder.h")
            .default_enum_style(EnumVariation::Rust {
                non_exhaustive: false,
            })
            .clang_args(Self::clang_args())
            .generate()?;

        let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        bindings.write_to_file(out_path.join(filename))?;

        Ok(())
    }

    fn clang_args() -> Vec<String> {
        let target = std::env::var("TARGET").expect("TARGET is not set");
        let mut args: Vec<String> = Vec::new();

        // This adds the sysroot specific to the android NDK
        if target.contains("android") {
            let ndk_home = std::env::var("NDK_HOME").expect("NDK_HOME is not set");
            let sysroot = PathBuf::from(ndk_home).join("sysroot");
            args.push("--sysroot".into());
            args.push(sysroot.to_str().unwrap().to_string());
        }

        // This adds the sysroot specific to the apple SDK for clang.
        if let Some(sdk_path) = Self::sdk_path(&target) {
            args.push("-isysroot".into());
            args.push(sdk_path);
        }

        args.push(format!("--target={}", target));

        args
    }

    fn sdk_path(target: &str) -> Option<String> {
        use std::process::Command;

        let sdk = if target.contains("apple-darwin") {
            "macosx"
        } else if target == "x86_64-apple-ios" || target == "i386-apple-ios" {
            "iphonesimulator"
        } else if target == "aarch64-apple-ios" || target == "armv7-apple-ios" {
            "iphoneos"
        } else {
            return None;
        };

        let output = Command::new("xcrun")
            .args(["--sdk", sdk, "--show-sdk-path"])
            .output()
            .expect("xcrun command failed")
            .stdout;
        let prefix_str = std::str::from_utf8(&output).expect("invalid output from `xcrun`");
        Some(prefix_str.trim_end().to_string())
    }
}
