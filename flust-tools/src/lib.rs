use curl::easy::Easy;
use flust_sdk_api::{
    FlutterBuildMode, FlutterEngineVersion, FlutterRelease, FlutterSDKVersion, FlutterTargetArch,
    FlutterVersion,
};
use indicatif::{style::TemplateError, ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{read_to_string, File};
use std::io::{BufRead, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
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

pub struct FlutterSDK {
    root_path: PathBuf,
}

impl FlutterSDK {
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

    fn version_path(&self) -> PathBuf {
        self.root_path.join("version")
    }

    fn flutter_bin_path(&self) -> PathBuf {
        self.root_path.join("bin").join("flutter")
    }

    pub fn engine_version_path(&self) -> PathBuf {
        self.root_path
            .join("bin")
            .join("internal")
            .join("engine.version")
    }

    // This method returns the equivalent of `flutter --version | head -1 | awk '{ print $2 }'`
    fn read_sdk_version_from_flutter_output(&self) -> Result<FlutterSDKVersion, Error> {
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
            .ok_or(Error::FlutterVersionNotFound)?
            .to_owned();

        Ok(version.into())
    }

    pub fn release(&self) -> Result<FlutterRelease, Error> {
        Ok(FlutterRelease {
            sdk_version: self.sdk_version()?,
            engine_version: self.engine_version()?,
        })
    }

    pub fn engine_version(&self) -> Result<FlutterEngineVersion, Error> {
        Ok(read_trimmed_string(self.engine_version_path())?.into())
    }

    pub fn sdk_version(&self) -> Result<FlutterSDKVersion, Error> {
        match read_trimmed_string(self.version_path()) {
            Ok(version) => Ok(version.into()),
            Err(err) => match err {
                // `$FLUTTER_SDK_ROOT/version` does not always exist. If that's
                // the case, read the version from `flutter --version` output.
                Error::Io(err) if err.kind() == ErrorKind::NotFound => {
                    self.read_sdk_version_from_flutter_output()
                }
                _ => Err(err),
            },
        }
    }
}

pub trait FlutterReleaseExt {
    fn for_current_sdk_version() -> Result<FlutterRelease, Error>;
    fn for_sdk_version(sdk_version: Option<&FlutterSDKVersion>) -> Result<FlutterRelease, Error>;
}

impl FlutterReleaseExt for FlutterRelease {
    fn for_current_sdk_version() -> Result<Self, Error> {
        Ok(FlutterSDK::auto_detect()?.release()?)
    }

    fn for_sdk_version(sdk_version: Option<&FlutterSDKVersion>) -> Result<Self, Error> {
        let Some(sdk_version) = sdk_version else {
            return Self::for_current_sdk_version();
        };

        let engine_version = match VersionMappingCache::find_engine_version(sdk_version) {
            Some(engine_version) => engine_version,
            None => read_flutter_engine_version_from_github_tag(sdk_version)?,
        };

        Ok(Self {
            sdk_version: sdk_version.clone(),
            engine_version,
        })
    }
}

pub trait EngineLibraryCachePathExt {
    fn cache_path(&self) -> PathBuf;
}

impl EngineLibraryCachePathExt for FlutterSDKVersion {
    fn cache_path(&self) -> PathBuf {
        EngineVersionManager::engine_cache_dir()
            .join("by-sdk-version")
            .join(self.to_string())
    }
}

impl EngineLibraryCachePathExt for FlutterEngineVersion {
    fn cache_path(&self) -> PathBuf {
        EngineVersionManager::engine_cache_dir()
            .join("by-engine-version")
            .join(self.to_string())
    }
}

impl EngineLibraryCachePathExt for FlutterVersion {
    fn cache_path(&self) -> PathBuf {
        match self {
            FlutterVersion::SDK(sdk_version) => sdk_version.cache_path(),
            FlutterVersion::Engine(engine_version) => engine_version.cache_path(),
        }
    }
}

pub struct EngineVersionManager {}

impl EngineVersionManager {
    pub fn find_installed_flutter_versions() -> Result<Vec<FlutterSDKVersion>, Error> {
        let cache_dir = Self::engine_cache_dir().join("by-sdk-version");
        let entries = std::fs::read_dir(cache_dir)?;

        let mut flutter_versions: Vec<FlutterSDKVersion> = Vec::new();

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
                        Some(version) => flutter_versions.push(version.to_owned().into()),
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

        flutter_versions.sort();
        flutter_versions.reverse();

        Ok(flutter_versions)
    }

    pub fn find_build_modes_for_installed_flutter_version(
        sdk_version: &FlutterSDKVersion,
    ) -> Result<HashMap<FlutterBuildMode, PathBuf>, Error> {
        let mut build_modes: HashMap<FlutterBuildMode, PathBuf> = HashMap::new();
        for build_mode in FlutterBuildMode::iter() {
            if let Ok(path) =
                Self::find_canonical_path_for_installed_flutter_version(sdk_version, &build_mode)
            {
                build_modes.insert(build_mode, path);
            }
        }

        Ok(build_modes)
    }

    pub fn find_canonical_path_for_installed_flutter_version(
        sdk_version: &FlutterSDKVersion,
        build_mode: &FlutterBuildMode,
    ) -> Result<PathBuf, Error> {
        let path = sdk_version
            .cache_path()
            .join(build_mode.to_string())
            .join("libflutter_engine.so");

        Ok(std::fs::canonicalize(path)?)
    }

    pub fn is_flutter_version_installed(version: &FlutterVersion) -> Result<bool, Error> {
        Ok(std::fs::exists(version.cache_path())?)
    }

    pub fn install_version(release: &FlutterRelease) -> Result<(), Error> {
        if EngineVersionManager::is_flutter_version_installed(
            &release.sdk_version.to_owned().into(),
        )? {
            return Err(Error::FlutterVersionAlreadyInstalled);
        }

        for build_mode in FlutterBuildMode::iter() {
            Self::install_version_build(release, &build_mode)?;
        }

        VersionMappingCache::insert(release)?;

        Ok(())
    }

    pub fn install_version_build(
        release: &FlutterRelease,
        build_mode: &FlutterBuildMode,
    ) -> Result<(), Error> {
        let engine_build = EngineLibraryBuild::new(
            release.clone(),
            FlutterTargetArch::new(),
            build_mode.clone(),
        );

        let library_path_by_sdk_version = engine_build.library_path_by_sdk_version();
        if !library_path_by_sdk_version.exists() {
            let target_build_dir = library_path_by_sdk_version.parent().unwrap().to_owned();
            if !target_build_dir.exists() {
                std::fs::create_dir_all(&target_build_dir)?;
            }

            engine_build.download_to(&library_path_by_sdk_version)?;
        }

        let by_engine_version_cache_dir = release.engine_version.cache_path();
        if !by_engine_version_cache_dir.exists() {
            let by_engine_version_parent_dir =
                by_engine_version_cache_dir.parent().unwrap().to_owned();
            if !by_engine_version_parent_dir.exists() {
                std::fs::create_dir_all(&by_engine_version_parent_dir)?;
            }

            let by_sdk_version_cache_dir = Path::new("..")
                .join("by-sdk-version")
                .join(release.sdk_version.to_string());

            std::os::unix::fs::symlink(&by_sdk_version_cache_dir, by_engine_version_cache_dir)?;
        }

        Ok(())
    }

    pub fn uninstall_version(release: &FlutterRelease) -> Result<(), Error> {
        if !EngineVersionManager::is_flutter_version_installed(
            &release.sdk_version.to_owned().into(),
        )? {
            return Err(Error::FlutterVersionNotFound);
        }

        let library_dirs = vec![
            release.engine_version.cache_path(),
            release.sdk_version.cache_path(),
        ];
        for library_dir in library_dirs {
            if library_dir.exists() {
                std::fs::remove_dir_all(library_dir)?;
            }
        }

        VersionMappingCache::remove(release)?;

        Ok(())
    }

    pub fn engine_cache_dir() -> PathBuf {
        dirs::cache_dir()
            .expect("Cannot get cache dir")
            .join("flutter-engine-lib")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineLibraryBuild {
    release: FlutterRelease,
    target: FlutterTargetArch,
    build_mode: FlutterBuildMode,
}

impl EngineLibraryBuild {
    pub fn new(
        release: FlutterRelease,
        target: FlutterTargetArch,
        build_mode: FlutterBuildMode,
    ) -> Self {
        Self {
            release,
            target,
            build_mode,
        }
    }

    pub fn download_url(&self) -> String {
        let platform = match self.target {
            FlutterTargetArch::X86_64 => "x64",
            FlutterTargetArch::Aarch64 => "aarch64",
        };
        format!(
            "https://github.com/ardera/flutter-ci/releases/download/engine%2F{}/engine-{}-generic-{}.tar.xz",
            &self.release.engine_version, platform, self.build_mode
        )
    }

    pub fn library_path_by_sdk_version(&self) -> PathBuf {
        self.release
            .sdk_version
            .cache_path()
            .join(self.build_mode.to_string())
            .join("libflutter_engine.so")
    }

    pub fn library_path_by_engine_version(&self) -> PathBuf {
        self.release
            .engine_version
            .cache_path()
            .join(self.build_mode.to_string())
            .join("libflutter_engine.so")
    }

    pub fn download_to<P: AsRef<Path>>(&self, target_library_path: P) -> Result<(), Error> {
        let tempdir = tempfile::tempdir()?;
        let download_file = tempdir.path().join("engine.tar.xz");
        let url = self.download_url();

        download(&url, &download_file)?;
        unarchive(&download_file, &tempdir.path())?;

        let temp_library_path = tempdir.path().join("libflutter_engine.so");
        if !temp_library_path.exists() {
            return Err(Error::DownloadNotFound);
        }
        std::fs::copy(temp_library_path, target_library_path)?;

        Ok(())
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

fn read_flutter_engine_version_from_github_tag(
    sdk_version: &FlutterSDKVersion,
) -> Result<FlutterEngineVersion, Error> {
    let url = format!(
        "https://raw.githubusercontent.com/flutter/flutter/refs/tags/{}/bin/internal/engine.version",
        sdk_version
    );

    Ok(reqwest::blocking::get(url)?
        .text()?
        .trim()
        .to_owned()
        .into())
}

fn read_trimmed_string(path: PathBuf) -> Result<String, Error> {
    Ok(read_to_string(path).map(|v| v.trim().to_owned())?)
}

#[derive(Serialize, Deserialize)]
struct VersionMappingCache {
    by_sdk_version: HashMap<String, String>,
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
        EngineVersionManager::engine_cache_dir().join("version_mapping.json")
    }

    fn find_engine_version(sdk_version: &FlutterSDKVersion) -> Option<FlutterEngineVersion> {
        Self::from_json_file()
            .ok()?
            .by_sdk_version
            .get(&sdk_version.to_string())
            .cloned()
            .map(|engine_version| engine_version.into())
    }

    fn remove(release: &FlutterRelease) -> Result<(), Error> {
        let mut mapping = Self::from_json_file()?;
        mapping
            .by_sdk_version
            .remove(&release.sdk_version.to_string());
        mapping
            .by_engine_version
            .remove(&release.engine_version.to_string());
        mapping.write_json_file()?;
        Ok(())
    }

    fn insert(release: &FlutterRelease) -> Result<(), Error> {
        let mut mapping = Self::from_json_file()?;
        mapping.by_sdk_version.insert(
            release.sdk_version.to_string(),
            release.engine_version.to_string(),
        );
        mapping.by_engine_version.insert(
            release.engine_version.to_string(),
            release.sdk_version.to_string(),
        );
        mapping.write_json_file()?;
        Ok(())
    }
}
