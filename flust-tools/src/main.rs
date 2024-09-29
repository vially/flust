use clap::{Parser, Subcommand};
use flust_tools::{EngineLibraryCache, Error, Flutter};
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
}

#[derive(Subcommand)]
enum EngineLibraryCommands {
    /// List Flutter engine library versions
    List {
        /// Show additional information about library versions
        #[arg(short, long)]
        long: bool,
    },
}

fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    let Some(command) = &cli.command else {
        return Ok(());
    };

    match command {
        Command::Doctor {} => {
            let flutter = Flutter::auto_detect()?;
            let version = flutter.version()?;
            let engine_version = flutter.engine_version()?;

            println!("Flutter {}", version);
            println!("Engine • revision {}", &engine_version[..9]);

            Ok(())
        }
        Command::EngineLibrary { command } => match command {
            Some(command) => match command {
                EngineLibraryCommands::List { long } => {
                    let flutter = Flutter::auto_detect()?;
                    let current_version = flutter.version()?;

                    let mut builder = tabled::builder::Builder::default();

                    let versions = EngineLibraryCache::find_installed_versions()?;
                    for version in versions {
                        let current = match version == current_version {
                            true => "*",
                            false => " ",
                        };

                        let build_modes =
                            EngineLibraryCache::find_build_modes_for_installed_version(
                                version.clone(),
                            )?;

                        if *long {
                            for (build_mode, path) in build_modes {
                                builder.push_record([
                                    current,
                                    &version,
                                    &build_mode.to_string(),
                                    &path.display().to_string(),
                                ]);
                            }
                        } else {
                            let mut build_modes: Vec<_> = build_modes
                                .keys()
                                .map(|build_mode| build_mode.to_string())
                                .collect();
                            build_modes.sort();

                            builder.push_record([current, &version, &build_modes.join(" • ")]);
                        }
                    }

                    let mut table = builder.build();
                    table.with(Style::blank());
                    println!("{}", table);

                    Ok(())
                }
            },
            None => Ok(()),
        },
    }
}
