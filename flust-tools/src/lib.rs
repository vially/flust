use curl::easy::Easy;
use indicatif::{style::TemplateError, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::fs::{read_to_string, File};
use std::io::{BufRead, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::string::ToString;
use std::sync::Arc;
use strum::EnumIter;
use strum::IntoEnumIterator;
use tracing::warn;
use zip::result::ZipError;
use zip::ZipArchive;

#[derive(Debug)]
pub enum Error {
    FlutterNotFound,
    FlutterVersionNotFound,
    DownloadNotFound,
    DartNotFound,
    Io(std::io::Error),
    Which(which::Error),
    Curl(curl::Error),
    Zip(zip::result::ZipError),
    Indicatif(indicatif::style::TemplateError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::FlutterNotFound => write!(f, "Couldn't find flutter sdk"),
            Error::FlutterVersionNotFound => write!(f, "Unable to determine Flutter SDK version"),
            Error::DownloadNotFound => write!(
                f,
                r#"We couldn't find the requested engine version 'missing'.
This means that your flutter version is too old or to new.

To update flutter run `flutter upgrade`. If the problem persists the engine
build has not completed yet. This means you need to manually supply the flutter
engine version through one of the following methods:

```bash
export FLUTTER_ENGINE_VERSION = "..."
```

`Cargo.toml`
```toml
[package.metadata.flutter]
engine_version = "..."
```

You'll find the available builds on our github releases page [0].

- [0] https://github.com/flutter-rs/engine-builds/releases"#,
            ),
            Error::DartNotFound => write!(f, "Could't find dart"),
            Error::Which(error) => error.fmt(f),
            Error::Io(error) => error.fmt(f),
            Error::Curl(error) => error.fmt(f),
            Error::Zip(error) => error.fmt(f),
            Error::Indicatif(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error)
    }
}

impl From<which::Error> for Error {
    fn from(error: which::Error) -> Self {
        Error::Which(error)
    }
}

impl From<curl::Error> for Error {
    fn from(error: curl::Error) -> Self {
        Error::Curl(error)
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(error: ZipError) -> Self {
        Error::Zip(error)
    }
}

impl From<indicatif::style::TemplateError> for Error {
    fn from(error: TemplateError) -> Self {
        Error::Indicatif(error)
    }
}

pub struct Flutter {
    root_path: PathBuf,
}

impl Flutter {
    pub fn new_from_path(path: PathBuf) -> Result<Self, Error> {
        if !path.exists() {
            return Err(Error::FlutterNotFound);
        }
        Ok(Self { root_path: path })
    }

    pub fn auto_detect() -> Result<Self, Error> {
        let root = if let Ok(root) = std::env::var("FLUTTER_ROOT") {
            PathBuf::from(root)
        } else {
            let flutter = which::which("flutter").or(Err(Error::FlutterNotFound))?;
            let flutter = std::fs::canonicalize(flutter)?;
            flutter
                .parent()
                .ok_or(Error::FlutterNotFound)?
                .parent()
                .ok_or(Error::FlutterNotFound)?
                .to_owned()
        };
        Self::new_from_path(root)
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn version_path(&self) -> PathBuf {
        self.root_path.join("version")
    }

    pub fn flutter_bin_path(&self) -> PathBuf {
        self.root_path.join("bin").join("flutter")
    }

    pub fn engine_version_path(&self) -> PathBuf {
        self.root_path
            .join("bin")
            .join("internal")
            .join("engine.version")
    }

    fn read_version_from_path(path: PathBuf) -> Result<String, Error> {
        let version = read_to_string(path).map(|v| v.trim().to_owned())?;
        Ok(version)
    }

    // This method returns the equivalent of `flutter --version | head -1 | awk '{ print $2 }'`
    fn read_version_from_flutter_output(&self) -> Result<String, Error> {
        let first_output_line = Command::new(self.flutter_bin_path())
            .args(["--no-version-check", "--version"])
            .output()?
            .stdout
            .lines()
            .next()
            .ok_or(Error::FlutterVersionNotFound)??;

        let version = first_output_line
            .split_ascii_whitespace()
            .nth(1)
            .ok_or(Error::FlutterVersionNotFound)?;

        Ok(version.into())
    }

    pub fn engine_version(&self) -> Result<String, Error> {
        Self::read_version_from_path(self.engine_version_path())
    }

    pub fn version(&self) -> Result<String, Error> {
        match Self::read_version_from_path(self.version_path()) {
            Ok(version) => Ok(version),
            Err(err) => match err {
                // `$FLUTTER_SDK_ROOT/version` does not always exist. If that's
                // the case, read the version from `flutter --version` output.
                Error::Io(err) if err.kind() == ErrorKind::NotFound => {
                    self.read_version_from_flutter_output()
                }
                _ => Err(err),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, EnumIter, strum::Display)]
pub enum Build {
    #[strum(serialize = "debug")]
    Debug,
    #[strum(serialize = "profile")]
    Profile,
    #[strum(serialize = "release")]
    Release,
}

impl Build {
    pub fn build(&self) -> &str {
        match self {
            Self::Debug => "debug_unopt",
            Self::Release => "release",
            Self::Profile => "profile",
        }
    }
}

pub struct EngineLibraryCache {}

impl EngineLibraryCache {
    pub fn find_installed_versions() -> Result<Vec<String>, Error> {
        let cache_dir = Self::engine_cache_dir().join("by-flutter-version");
        let entries = std::fs::read_dir(cache_dir)?;

        let mut paths: Vec<String> = Vec::new();

        for entry in entries {
            match entry {
                Ok(entry) => {
                    if !entry.file_type()?.is_dir() {
                        warn!(
                            "Invalid file type found in Engine library cache directory: {:?}",
                            entry.file_name(),
                        );
                        continue;
                    }

                    let file_name = entry.file_name();
                    match file_name.to_str() {
                        Some(version) => paths.push(version.into()),
                        None => {
                            warn!(
                                "Invalid file name found in Engine library cache directory: {:?}",
                                file_name
                            );
                        }
                    };
                }
                Err(err) => {
                    warn!(
                        "Invalid entry found in Engine library cache directory: {}",
                        err
                    );
                }
            }
        }

        paths.sort();
        paths.reverse();

        Ok(paths)
    }

    pub fn find_build_modes_for_installed_version<P: AsRef<Path>>(
        version: P,
    ) -> Result<HashMap<Build, PathBuf>, Error> {
        let mut build_modes: HashMap<Build, PathBuf> = HashMap::new();
        for build_mode in Build::iter() {
            if let Ok(path) = Self::find_canonical_path_for_installed_version(&version, build_mode)
            {
                build_modes.insert(build_mode, path);
            }
        }

        Ok(build_modes)
    }

    pub fn find_canonical_path_for_installed_version<P: AsRef<Path>>(
        version: P,
        build: Build,
    ) -> Result<PathBuf, Error> {
        let path = Self::engine_cache_dir()
            .join("by-flutter-version")
            .join(version)
            .join(build.to_string())
            .join("libflutter_engine.so");

        Ok(std::fs::canonicalize(path)?)
    }

    pub fn engine_cache_dir() -> PathBuf {
        dirs::cache_dir()
            .expect("Cannot get cache dir")
            .join("flutter-engine-lib")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Engine {
    version: String,
    target: String,
    build: Build,
}

impl Engine {
    pub fn new(version: String, target: String, build: Build) -> Self {
        Self {
            version,
            target,
            build,
        }
    }

    pub fn download_url(&self) -> String {
        let build = self.build.build();
        let platform = match self.target.as_str() {
            "x86_64-unknown-linux-gnu" => format!("linux_x64-host_{}", build),
            "armv7-linux-androideabi" => format!("linux_x64-android_{}", build),
            "aarch64-linux-android" => format!("linux_x64-android_{}_arm64", build),
            "i686-linux-android" => format!("linux_x64-android_{}_x64", build),
            "x86_64-linux-android" => format!("linux_x64-android_{}_x86", build),
            "x86_64-apple-darwin" => format!("macosx_x64-host_{}", build),
            "armv7-apple-ios" => format!("macosx_x64-ios_{}_arm", build),
            "aarch64-apple-ios" => format!("macosx_x64-ios_{}", build),
            "x86_64-pc-windows-msvc" => format!("windows_x64-host_{}", build),
            _ => panic!("unsupported platform"),
        };
        format!(
            "https://github.com/flutter-rs/engine-builds/releases/download/f-{0}/{1}.zip",
            &self.version, platform
        )
    }

    pub fn library_name(&self) -> &'static str {
        match self.target.as_str() {
            "x86_64-unknown-linux-gnu" => "libflutter_engine.so",
            "armv7-linux-androideabi" => "libflutter_engine.so",
            "aarch64-linux-android" => "libflutter_engine.so",
            "i686-linux-android" => "libflutter_engine.so",
            "x86_64-linux-android" => "libflutter_engine.so",
            "x86_64-apple-darwin" => "libflutter_engine.dylib",
            "armv7-apple-ios" => "libflutter_engine.dylib",
            "aarch64-apple-ios" => "libflutter_engine.dylib",
            "x86_64-pc-windows-msvc" => "flutter_engine.dll",
            _ => panic!("unsupported platform"),
        }
    }

    pub fn engine_dir(&self) -> PathBuf {
        dirs::cache_dir()
            .expect("Cannot get cache dir")
            .join("flust-engine")
            .join(&self.version)
            .join(&self.target)
            .join(self.build.build())
    }

    pub fn library_path(&self) -> PathBuf {
        self.engine_dir().join(self.library_name())
    }

    pub fn download(&self) -> Result<(), Error> {
        let url = self.download_url();
        let path = self.library_path();
        let dir = path.parent().unwrap().to_owned();

        if path.exists() {
            return Ok(());
        }

        std::fs::create_dir_all(&dir)?;

        let download_file = dir.join("engine.zip");
        download(&url, &download_file)?;
        unzip(&download_file, &dir)?;

        Ok(())
    }

    pub fn dart(&self) -> Result<PathBuf, Error> {
        let host_engine_dir = self.engine_dir();
        ["dart", "dart.exe"]
            .iter()
            .map(|bin| host_engine_dir.join(bin))
            .find(|path| path.exists())
            .ok_or(Error::DartNotFound)
    }
}

fn download(url: &str, target: &Path) -> Result<(), Error> {
    println!("Starting download from {}", url);
    let mut file = File::create(target)?;
    let mut last_done = 0.0;

    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")?
        .progress_chars("#>-"));

    let pb = Arc::new(pb);

    let mut easy = Easy::new();
    easy.fail_on_error(true)?;
    easy.url(url)?;
    easy.follow_location(true)?;
    easy.progress(true)?;
    let pb2 = pb.clone();
    easy.progress_function(move |total, done, _, _| {
        if done > last_done {
            last_done = done;

            pb2.set_length(total as u64);
            pb2.set_position(done as u64);
        }
        true
    })?;
    easy.write_function(move |data| Ok(file.write(data).unwrap()))?;

    easy.perform().map_err(|_| Error::DownloadNotFound)?;

    pb.finish_with_message("Downloaded");

    println!("Download finished");
    Ok(())
}

fn unzip(archive: &Path, dir: &Path) -> Result<(), Error> {
    println!("Extracting {:?}...", archive.file_name().unwrap());

    let file = File::open(archive)?;
    let mut archive = ZipArchive::new(file)?;

    let pb = ProgressBar::new(archive.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {wide_msg}")?
            .progress_chars("#>-"),
    );

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = dir.join(file.mangled_name());

        pb.inc(1);

        if file.name().ends_with('/') {
            pb.set_message(format!("File {} extracted to \"{}\"", i, outpath.display()));
            std::fs::create_dir_all(&outpath)?;
        } else {
            pb.set_message(format!(
                "File {} extracted to \"{}\" ({} bytes)",
                i,
                outpath.display(),
                file.size()
            ));
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(p)?;
                }
            }
            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile).unwrap();

            #[cfg(unix)]
            {
                use std::fs::Permissions;
                use std::os::unix::fs::PermissionsExt;

                if let Some(mode) = file.unix_mode() {
                    std::fs::set_permissions(&outpath, Permissions::from_mode(mode))?;
                }
            }
        }
    }

    pb.finish_with_message("Extracted");

    Ok(())
}
