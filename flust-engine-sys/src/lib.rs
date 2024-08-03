#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(clippy::unreadable_literal)]

include!(concat!(env!("OUT_DIR"), "/flust-engine-sys.rs"));

#[cfg(test)]
mod tests {
    #[allow(unused)]
    use super::*;
    use libloading::Library;

    #[cfg(target_os = "linux")]
    const LIB: &str = "libflutter_engine.so";
    #[cfg(target_os = "macos")]
    const LIB: &str = "libflutter_engine.dylib";
    #[cfg(target_os = "windows")]
    const LIB: &str = "flutter_engine.lib";

    #[test]
    fn link() {
        unsafe {
            let lib = Library::new(LIB).unwrap();
            lib.get::<*const ()>(b"FlutterEngineRun\0").unwrap();
        }
    }
}
