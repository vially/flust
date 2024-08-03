use bindgen::EnumVariation;
use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-link-lib=flutter_engine");

    // Tell cargo to look for shared libraries in the specified directory (needed for `cargo test`)
    if let Ok(flutter_engine_search_path) = std::env::var("FLUTTER_ENGINE_LIB_PATH") {
        println!("cargo::rustc-link-search={flutter_engine_search_path}");
    }

    let target = std::env::var("TARGET").unwrap();
    let mut clang_args: Vec<String> = Vec::new();

    // This adds the sysroot specific to the android NDK
    if target.contains("android") {
        let ndk_home = std::env::var("NDK_HOME").expect("NDK_HOME is set");
        let sysroot = PathBuf::from(ndk_home).join("sysroot");
        clang_args.push("--sysroot".into());
        clang_args.push(sysroot.to_str().unwrap().to_string());
    }

    // This adds the sysroot specific to the apple SDK for clang.
    if let Some(sdk_path) = sdk_path(&target) {
        clang_args.push("-isysroot".into());
        clang_args.push(sdk_path);
    }

    clang_args.push(format!("--target={}", target));

    let bindings = bindgen::Builder::default()
        .header("embedder.h")
        .default_enum_style(EnumVariation::Rust {
            non_exhaustive: false,
        })
        .clang_args(&clang_args)
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("flust-engine-sys.rs"))
        .expect("Couldn't write bindings!");
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
