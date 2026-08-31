//! Kurogane command-line entry point.
//!
//! This module defines the CLI surface and dispatches subcommands
//! to the corresponding command implementations.

use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

mod install;
mod dev;
mod build;
mod bundle;
mod new;
mod init;
mod showcase;
mod clean;
mod doctor;
mod list;
mod info;

#[cfg(target_os = "linux")]
mod appimage;

#[cfg(target_os = "windows")]
mod nsis;

mod collector;
mod cache;
mod template;
mod starters;
mod tui;

mod platform;

#[derive(Parser)]
#[command(name = "kurogane")]
#[command(
    about = "Kurogane: GPU-accelerated runtime for building high-performance desktop apps",
    version
)]
struct Cli {
    #[arg(long, global = true)]
    ci: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Install,
    Dev {
        #[arg(
            num_args = 0..,
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_parser = clap::value_parser!(OsString)
        )]
        cargo_args: Vec<OsString>,
    },
    Build,
    Bundle {
        #[arg(long)]
        debug: bool,
        #[arg(long, default_value = "dir")]
        format: String,
        /// Sign bundle binaries
        #[arg(long)]
        sign: bool,
    },
    New {
        /// Official starter name
        starter: Option<String>,

        /// Project name (required when non-interactive)
        #[arg(long)]
        name: Option<String>,

        /// Starter language (required when non-interactive)
        #[arg(long)]
        language: Option<String>,

        /// Use an arbitrary template source
        #[arg(long)]
        template: Option<String>,

        /// Accept template hooks without prompting
        #[arg(long)]
        yes: bool,
    },
    Init {
        /// Frontend assets directory
        #[arg(long)]
        assets: Option<PathBuf>,

        /// Dev server URL
        #[arg(long)]
        dev_url: Option<String>,

        /// Accept template hooks without prompting
        #[arg(long)]
        yes: bool,
    },
    Clean {
        #[arg(value_parser = ["all"])]
        target: Option<String>,
    },
    Showcase,
    Doctor {
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(value_parser = ["profiles", "version"])]
        target: Option<String>,
    },
    Info,
}

/// Enables non-interactive execution.
///
/// The flag takes precedence, `CI` enables non-interactive execution
/// unless its value is empty, `0`, or `false`. `CI` is parsed manually
/// because Clap's `env` bool parser rejects values such as `CI=1`.
fn non_interactive(flag: bool) -> bool {
    flag || std::env::var_os("CI").is_some_and(|value| {
        let value = value.to_string_lossy();
        let value = value.trim();

        !value.is_empty()
            && value != "0"
            && !value.eq_ignore_ascii_case("false")
    })
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let ci = non_interactive(cli.ci);

    match cli.command {
        Commands::Install => install::run(),
        Commands::Dev { cargo_args } => dev::run(cargo_args),
        Commands::Build => build::run(),
        Commands::Bundle {
            debug,
            format,
            sign,
        } => {
            let format = bundle::PackageFormat::from_str(&format)?;
            bundle::run(debug, format, sign)
        }
        Commands::New {
            starter,
            name,
            language,
            template,
            yes,
        } => new::run(starter, name, language, template, yes || ci, ci),
        Commands::Init {
            assets,
            dev_url,
            yes,
        } => init::run(assets, dev_url, yes),
        Commands::Clean { target } => clean::run(target),
        Commands::Showcase => showcase::run(),
        Commands::Doctor { json } => doctor::run(json),
        Commands::List { target } => list::run(target),
        Commands::Info => info::run(),
    }
}
