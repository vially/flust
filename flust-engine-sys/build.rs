use bindgen::EnumVariation;
use flust_tools::Flutter;
use std::path::{Path, PathBuf};
use thiserror::Error;

const FLUTTER_SDK_MISSING_NO_REBUILD_WARNING: &str = "Flutter SDK path could not be determined. \
The flust-engine-sys crate might not get rebuilt when the Flutter version changes.";

fn main() -> Result<(), BuildError> {
    Cargo::print_instructions()?;

    BindingsBuilder::generate("flust-engine-sys.rs")?;

    Ok(())
}

#[derive(Error, Debug)]
pub enum BuildError {
    #[error(transparent)]
    Bindgen(#[from] bindgen::BindgenError),

    #[error(transparent)]
    IO(#[from] std::io::Error),

    #[error(transparent)]
    Flutter(#[from] flust_tools::Error),
}

struct Cargo {}

impl Cargo {
    fn print_instructions() -> Result<(), BuildError> {
        let flutter = Flutter::auto_detect().ok();
        let engine_version_path = flutter.as_ref().and_then(|flutter| {
            flutter
                .engine_version_path()
                .into_os_string()
                .into_string()
                .ok()
        });

        println!("cargo::rustc-link-lib=flutter_engine");
        println!("cargo::rerun-if-changed=embedder.h");
        println!("cargo::rerun-if-changed=src/lib.rs");

        if let Some(engine_version_path) = engine_version_path {
            println!("cargo::rerun-if-changed={engine_version_path}");
        } else {
            println!("cargo::warning={FLUTTER_SDK_MISSING_NO_REBUILD_WARNING}");
        }

        let link_search_path = Self::auto_detect_link_search_path(&flutter);
        if let Some(link_search_path) = link_search_path {
            println!("cargo::rustc-link-search={link_search_path}");
        }

        Ok(())
    }

    fn auto_detect_link_search_path(flutter: &Option<Flutter>) -> Option<String> {
        if let Ok(flutter_engine_search_path) = std::env::var("FLUTTER_ENGINE_LIB_PATH") {
            return Some(flutter_engine_search_path);
        }

        let engine_version = flutter.as_ref()?.engine_version().ok()?;

        // TODO: Remove hard-coded "debug" Flutter build-mode (once a reasonable
        // strategy of auto-detection has been found).
        dirs::cache_dir()?
            .join("flutter-engine-lib")
            .join("by-engine-version")
            .join(engine_version)
            .join("debug")
            .into_os_string()
            .into_string()
            .ok()
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
