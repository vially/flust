use std::fmt::Display;

use clap::{Parser, Subcommand};
use flust_tools::{EngineLibraryCache, Error, Flutter};
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
                    let current_version = Flutter::auto_detect()?.version()?;

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
                                &version,
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
            },
            None => Ok(()),
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
