use curl::easy::Easy;
use indicatif::{style::TemplateError, ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{read_to_string, File};
use std::io::{BufRead, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::string::ToString;
use std::sync::Arc;
use strum::EnumIter;
use strum::IntoEnumIterator;
use tempfile;
use tracing::warn;

#[derive(Debug)]
pub enum Error {
    FlutterNotFound,
    FlutterVersionNotFound,
    FlutterVersionAlreadyInstalled,
    DownloadNotFound,
    DartNotFound,
    Io(std::io::Error),
    Which(which::Error),
    Curl(curl::Error),
    Reqwest(reqwest::Error),
    Indicatif(indicatif::style::TemplateError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::FlutterNotFound => write!(f, "Couldn't find flutter sdk"),
            Error::FlutterVersionNotFound => write!(f, "Unable to determine Flutter SDK version"),
            Error::FlutterVersionAlreadyInstalled => write!(f, "Flutter version already installed"),
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
            Error::Reqwest(error) => error.fmt(f),
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

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Error::Reqwest(error)
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

struct FlutterRelease {
    flutter_version: String,
    engine_version: String,
}

impl FlutterRelease {
    fn current_version() -> Result<Self, Error> {
        let flutter = Flutter::auto_detect()?;
        Ok(Self {
            flutter_version: flutter.version()?,
            engine_version: flutter.engine_version()?,
        })
    }

    fn for_flutter_version(flutter_version: Option<&str>) -> Result<Self, Error> {
        let Some(flutter_version) = flutter_version else {
            return Self::current_version();
        };

        let engine_version = match VersionMappingCache::find_engine_version(flutter_version) {
            Some(engine_version) => engine_version,
            None => Self::read_engine_version_from_github_tag(flutter_version)?,
        };

        Ok(Self {
            flutter_version: flutter_version.to_owned(),
            engine_version,
        })
    }

    fn read_engine_version_from_github_tag(flutter_version: &str) -> Result<String, Error> {
        let url = format!(
            "https://raw.githubusercontent.com/flutter/flutter/refs/tags/{}/bin/internal/engine.version",
            flutter_version
        );

        Ok(reqwest::blocking::get(url)?.text()?.trim().to_owned())
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

    pub fn is_version_installed<P: AsRef<Path>>(flutter_version: P) -> Result<bool, Error> {
        let path = Self::engine_cache_dir()
            .join("by-flutter-version")
            .join(flutter_version);

        Ok(std::fs::exists(path)?)
    }

    pub fn install_version(flutter_version: Option<&str>) -> Result<(), Error> {
        let release = FlutterRelease::for_flutter_version(flutter_version)?;

        if EngineLibraryCache::is_version_installed(&release.flutter_version)? {
            return Err(Error::FlutterVersionAlreadyInstalled);
        }

        for build_mode in Build::iter() {
            let library_path = Engine::new(
                &release.flutter_version,
                &release.engine_version,
                "x86_64-unknown-linux-gnu",
                build_mode,
            )
            .download()?;

            let library_dirs = vec![
                Self::engine_cache_dir()
                    .join("by-flutter-version")
                    .join(&release.flutter_version)
                    .join(build_mode.to_string()),
                Self::engine_cache_dir()
                    .join("by-engine-version")
                    .join(&release.engine_version)
                    .join(build_mode.to_string()),
            ];
            for library_dir in library_dirs {
                if !library_dir.exists() {
                    std::fs::create_dir_all(&library_dir)?;
                }
                std::os::unix::fs::symlink(
                    &library_path,
                    library_dir.join("libflutter_engine.so"),
                )?;
            }
        }

        VersionMappingCache::insert(&release.flutter_version, &release.engine_version)?;

        Ok(())
    }

    pub fn uninstall_version(flutter_version: Option<&str>) -> Result<(), Error> {
        let release = FlutterRelease::for_flutter_version(flutter_version)?;

        if !EngineLibraryCache::is_version_installed(&release.flutter_version)? {
            return Err(Error::FlutterVersionNotFound);
        }

        let library_dirs = vec![
            Self::engine_cache_dir()
                .join("by-flutter-version")
                .join(&release.flutter_version),
            Self::engine_cache_dir()
                .join("by-engine-version")
                .join(&release.engine_version),
        ];
        for library_dir in library_dirs {
            if library_dir.exists() {
                std::fs::remove_dir_all(library_dir)?;
            }
        }

        let build_modes = vec!["debug", "debug_unstripped", "profile", "release"];
        for build_mode in build_modes {
            let library_name = format!(
                "libflutter_engine_{}-{}.so",
                build_mode, &release.flutter_version
            );
            let library_path = Self::engine_cache_dir().join(library_name);
            if library_path.exists() {
                std::fs::remove_file(library_path)?;
            }
        }

        VersionMappingCache::remove(&release.flutter_version, &release.engine_version)?;

        Ok(())
    }

    pub fn engine_cache_dir() -> PathBuf {
        dirs::cache_dir()
            .expect("Cannot get cache dir")
            .join("flutter-engine-lib")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Engine {
    flutter_version: String,
    engine_version: String,
    target: String,
    build: Build,
}

impl Engine {
    pub fn new(
        flutter_version: impl Into<String>,
        engine_version: impl Into<String>,
        target: impl Into<String>,
        build: Build,
    ) -> Self {
        Self {
            flutter_version: flutter_version.into(),
            engine_version: engine_version.into(),
            target: target.into(),
            build,
        }
    }

    pub fn download_url(&self) -> String {
        let build = self.build.build();
        let platform = match self.target.as_str() {
            "x86_64-unknown-linux-gnu" => format!("engine-x64-generic-{}", build),
            _ => panic!("unsupported platform"),
        };
        format!(
            "https://github.com/ardera/flutter-ci/releases/download/engine%2F{}/{}.tar.xz",
            &self.engine_version, platform
        )
    }

    pub fn library_name(&self) -> String {
        match self.target.as_str() {
            "x86_64-unknown-linux-gnu" => format!(
                "libflutter_engine_{}-{}.so",
                self.build, &self.flutter_version
            ),
            _ => panic!("unsupported platform"),
        }
    }

    pub fn library_path(&self) -> PathBuf {
        EngineLibraryCache::engine_cache_dir().join(self.library_name())
    }

    pub fn download(&self) -> Result<PathBuf, Error> {
        let url = self.download_url();
        let path = self.library_path();
        let dir = path.parent().unwrap().to_owned();

        if path.exists() {
            return Ok(path);
        }

        std::fs::create_dir_all(&dir)?;

        let tempdir = tempfile::tempdir()?;
        let download_file = tempdir.path().join("engine.tar.xz");
        download(&url, &download_file)?;
        unarchive(&download_file, &tempdir.path())?;

        if tempdir.path().join("libflutter_engine.so").exists() {
            std::fs::copy(tempdir.path().join("libflutter_engine.so"), &path)?;
        }

        Ok(path)
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

fn unarchive(archive_path: &Path, target_dir: &Path) -> Result<(), Error> {
    println!("Extracting {:?}...", archive_path.file_name().unwrap());

    let decoder = xz2::read::XzDecoder::new(File::open(archive_path)?);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(target_dir)?;

    Ok(())
}

#[derive(Serialize, Deserialize)]
struct VersionMappingCache {
    by_flutter_version: HashMap<String, String>,
    by_engine_version: HashMap<String, String>,
}

impl VersionMappingCache {
    fn from_json_file() -> Result<Self, Error> {
        let mapping_file = File::open(Self::get_file_path())?;
        Ok(serde_json::from_reader(mapping_file)
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?)
    }

    fn write_json_file(&self) -> Result<(), Error> {
        let file_path = Self::get_file_path();
        std::fs::create_dir_all(file_path.parent().unwrap())?;

        serde_json::to_writer(File::create(file_path)?, self)
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;

        Ok(())
    }

    fn get_file_path() -> PathBuf {
        EngineLibraryCache::engine_cache_dir().join("version_mapping.json")
    }

    fn find_engine_version(flutter_version: &str) -> Option<String> {
        Self::from_json_file()
            .ok()?
            .by_flutter_version
            .get(flutter_version)
            .cloned()
    }

    fn remove(flutter_version: &str, engine_version: &str) -> Result<(), Error> {
        let mut mapping = Self::from_json_file()?;
        mapping.by_flutter_version.remove(flutter_version);
        mapping.by_engine_version.remove(engine_version);
        mapping.write_json_file()?;
        Ok(())
    }

    fn insert(flutter_version: &str, engine_version: &str) -> Result<(), Error> {
        let mut mapping = Self::from_json_file()?;
        mapping
            .by_flutter_version
            .insert(flutter_version.to_owned(), engine_version.to_owned());
        mapping
            .by_engine_version
            .insert(engine_version.to_owned(), flutter_version.to_owned());
        mapping.write_json_file()?;
        Ok(())
    }
}
