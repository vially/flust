use std::{fmt::Display, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use flust_build::{EngineVersionManager, Error, FlutterReleaseExt, FlutterSDK};
use flust_sdk_api::{CompilerOptimization, FlutterBuildMode, FlutterRelease, FlutterSDKVersion};
use supports_hyperlinks::supports_hyperlinks;
use tabled::settings::Style;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show information about the installed Flutter engine library versions.
    Doctor {},

    /// Manage Flutter engine library versions
    EngineLibrary {
        #[command(subcommand)]
        command: Option<EngineLibraryCommands>,
    },

    /// Custom device management commands
    CustomDevice {
        #[command(subcommand)]
        command: CustomDeviceCommands,
    },
}

#[derive(Subcommand)]
enum EngineLibraryCommands {
    /// List Flutter engine library versions
    List {
        /// Show additional information about library versions
        #[arg(short, long)]
        long: bool,
    },

    /// Install a Flutter engine library version
    Install {
        /// The Flutter engine library version to install
        version: Option<FlutterSDKVersion>,
    },

    /// Uninstall a Flutter engine library version
    Uninstall {
        /// The Flutter engine library version to uninstall
        version: Option<FlutterSDKVersion>,
    },
}

#[derive(Subcommand)]
enum CustomDeviceCommands {
    /// Copy `flutter_assets`, `lib` and `icudtl.dat` files to the cargo build
    /// directory. This command should be configured as the `postBuild` command
    /// in the custom-device JSON.
    PostBuild {
        /// Flutter build mode. This typically corresponds to the `${buildMode}`
        /// string interpolated variable set by the Flutter tool.
        #[arg(long)]
        build_mode: BuildMode,

        /// Path to the output directory of the `flutter build bundle` command.
        /// This typically corresponds to the `${localPath}` string interpolated
        /// variable set by the Flutter tool.
        #[arg(long)]
        bundle_output_path: PathBuf,

        /// Flutter engine revision. This typically corresponds to the
        /// `${engineRevision}` string interpolated variable set by the Flutter
        /// tool.
        #[arg(long)]
        engine_revision: String,

        /// Path to the `icudtl.dat` file that should be copied to the cargo
        /// build directory. This typically corresponds to the `${icuDataPath}`
        /// string interpolated variable set by the Flutter tool.
        #[arg(long)]
        icu_data_path: PathBuf,
    },

    /// Build and run the application using `cargo run`. This command should be
    /// configured as the `runDebug` command in the custom-device JSON.
    Run {
        /// Flutter build mode.
        #[arg(long)]
        build_mode: BuildMode,

        /// Flutter engine revision.
        #[arg(long)]
        engine_revision: String,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum BuildMode {
    Debug,
    Profile,
    Release,
}

impl From<BuildMode> for FlutterBuildMode {
    fn from(build_mode: BuildMode) -> Self {
        match build_mode {
            BuildMode::Debug => FlutterBuildMode::Debug(CompilerOptimization::unoptimized()),
            BuildMode::Profile => FlutterBuildMode::Profile,
            BuildMode::Release => FlutterBuildMode::Release,
        }
    }
}

fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    let Some(command) = &cli.command else {
        return Ok(());
    };

    match command {
        Command::Doctor {} => {
            let flutter = FlutterSDK::auto_detect()?.release()?;

            println!("Flutter {}", flutter.sdk_version);
            println!(
                "Engine • revision {}",
                &String::from(flutter.engine_version)[..9]
            );

            Ok(())
        }
        Command::EngineLibrary { command } => match command {
            Some(command) => match command {
                EngineLibraryCommands::List { long } => {
                    let current_version = FlutterSDK::auto_detect()?.sdk_version()?;

                    let sdk_versions = EngineVersionManager::find_installed_flutter_versions()?;
                    if sdk_versions.is_empty() {
                        println!("No Flutter engine library versions have been installed");
                        return Ok(());
                    }

                    let mut builder = tabled::builder::Builder::default();
                    for sdk_version in sdk_versions {
                        let current = match sdk_version == current_version {
                            true => "*",
                            false => " ",
                        };

                        let build_modes =
                            EngineVersionManager::find_build_modes_for_installed_flutter_version(
                                &sdk_version,
                            )?;

                        if *long {
                            for (build_mode, path) in build_modes {
                                builder.push_record([
                                    current,
                                    &sdk_version.to_string(),
                                    &build_mode.to_string(),
                                    &path.display().to_string(),
                                ]);
                            }
                        } else {
                            let mut build_modes: Vec<_> = build_modes
                                .into_iter()
                                .map(|(build_mode, path)| {
                                    Link::new(
                                        build_mode.to_string(),
                                        format!("file://{}", path.display()),
                                    )
                                })
                                .collect();
                            build_modes.sort_by_key(|link| link.text.clone());

                            builder.push_record([
                                current,
                                &sdk_version.to_string(),
                                &build_modes
                                    .iter()
                                    .map(|link| format!("{}", link))
                                    .collect::<Vec<_>>()
                                    .join(" • "),
                            ]);
                        }
                    }

                    let mut table = builder.build();
                    table.with(Style::blank());
                    println!("{}", table);

                    Ok(())
                }
                EngineLibraryCommands::Install { version } => {
                    let release = FlutterRelease::for_sdk_version(version.as_ref())?;
                    match EngineVersionManager::install_version(&release) {
                        Ok(()) => {
                            println!(
                                "Installed engine library for Flutter {} ({})",
                                release.sdk_version, release.engine_version
                            );
                            Ok(())
                        }
                        Err(Error::FlutterVersionAlreadyInstalled) => {
                            println!(
                                "Engine library for Flutter {} ({}) is already installed",
                                release.sdk_version, release.engine_version
                            );
                            Ok(())
                        }
                        Err(err) => {
                            println!("Failed to install Flutter engine library version: {}", err);
                            Err(err)
                        }
                    }
                }
                EngineLibraryCommands::Uninstall { version } => {
                    let release = FlutterRelease::for_sdk_version(version.as_ref())?;
                    match EngineVersionManager::uninstall_version(&release) {
                        Ok(()) => {
                            println!(
                                "Uninstalled engine library for Flutter {} ({})",
                                release.sdk_version, release.engine_version
                            );
                            Ok(())
                        }
                        Err(Error::FlutterVersionNotFound) => {
                            println!(
                                "No existing engine library has been found for Flutter {} ({})",
                                release.sdk_version, release.engine_version
                            );
                            Ok(())
                        }
                        Err(err) => {
                            println!(
                                "Failed to uninstall Flutter engine library version: {}",
                                err
                            );
                            Err(err)
                        }
                    }
                }
            },
            None => Ok(()),
        },
        Command::CustomDevice { command } => match command {
            CustomDeviceCommands::PostBuild {
                build_mode,
                bundle_output_path,
                engine_revision,
                icu_data_path,
            } => flust_build::CustomDeviceCommands::post_build(
                (*build_mode).into(),
                engine_revision,
                bundle_output_path,
                icu_data_path,
            ),
            CustomDeviceCommands::Run {
                build_mode,
                engine_revision,
            } => flust_build::CustomDeviceCommands::run((*build_mode).into(), engine_revision),
        },
    }
}

/// A clickable link component.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Link {
    pub id: String,
    pub text: String,
    pub url: String,
}

impl Link {
    /// Create a new link with a name and target url.
    pub fn new(text: String, url: String) -> Self {
        Self {
            text,
            url,
            id: "".into(),
        }
    }

    /// Create a new link with a name, a target url and an id.
    pub fn with_id(text: String, url: String, id: String) -> Self {
        Self { text, url, id }
    }
}

impl Display for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !supports_hyperlinks() {
            return write!(f, "{}", self.text);
        }

        if !self.id.is_empty() {
            write!(
                f,
                "\u{1b}]8;id={};{}\u{1b}\\{}\u{1b}]8;;\u{1b}\\",
                self.id, self.url, self.text
            )
        } else {
            write!(
                f,
                "\u{1b}]8;;{}\u{1b}\\{}\u{1b}]8;;\u{1b}\\",
                self.url, self.text
            )
        }
    }
}
