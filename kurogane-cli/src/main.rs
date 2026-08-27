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

#[derive(Parser)]
#[command(name = "kurogane")]
#[command(
    about = "Kurogane: GPU-accelerated runtime for building high-performance desktop apps",
    version
)]
struct Cli {
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

fn main() -> anyhow::Result<()> {
    validate_platform();

    let cli = Cli::parse();

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
            template,
            yes,
        } => new::run(starter, template, yes),
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

/// macOS is currently unsupported due to missing platform-specific runtime support.
/// Fail fast to avoid undefined behavior.
#[cold]
fn validate_platform() {
    #[cfg(target_os = "macos")]
    {
        tui::error("macOS is not supported");
        tui::info("Support is planned but not implemented yet");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod test_helpers {
    use std::path::{Path, PathBuf};

    pub(crate) fn create_cef_fixture(dir: &Path) -> PathBuf {
        let cef = dir.join("cef");
        std::fs::create_dir_all(&cef).unwrap();
        if cfg!(target_os = "windows") {
            std::fs::write(cef.join("libcef.dll"), "cef").unwrap();
            std::fs::write(cef.join("chrome_elf.dll"), "elf").unwrap();
        } else {
            std::fs::write(cef.join("libcef.so"), "cef").unwrap();
            std::fs::write(cef.join("chrome-sandbox"), "sandbox").unwrap();
        }
        std::fs::write(cef.join("icudtl.dat"), "icu").unwrap();
        std::fs::write(cef.join("v8_context_snapshot.bin"), "v8").unwrap();
        std::fs::create_dir_all(cef.join("locales")).unwrap();
        std::fs::write(cef.join("locales").join("en-US.pak"), "pak").unwrap();
        cef
    }
}
