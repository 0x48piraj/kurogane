//! Kurogane command-line entry point.
//!
//! This module defines the CLI surface and dispatches subcommands
//! to the corresponding command implementations.

use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

mod install;
mod dev;
mod launch;
mod run;
mod sandbox;
mod build;
mod bundle;
mod config;
mod signing;
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

#[cfg(target_os = "macos")]
mod app_bundle;

#[cfg(target_os = "macos")]
mod dmg;

#[cfg(target_os = "macos")]
mod macos_settings;

#[cfg(target_os = "macos")]
mod plist;

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
    /// Run the Kurogane development workflow.
    Dev,
    /// Run the application with Cargo.
    ///
    /// Unlike `dev`, this command passes arguments directly to Cargo.
    #[command(disable_help_flag = true)]
    Run {
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
        #[arg(long, default_value = crate::bundle::DEFAULT_FORMAT)]
        format: String,
        /// Sign the bundle: a Windows bundle's binaries ([signing.windows]) or a
        /// macOS app ([signing.macos]); a Linux bundle has nothing to sign.
        #[arg(long)]
        sign: bool,
    },
    New {
        /// Official starter name.
        starter: Option<String>,

        /// Project name.
        #[arg(long)]
        name: Option<String>,

        /// Starter language.
        #[arg(long)]
        language: Option<String>,

        /// Use an arbitrary template source.
        #[arg(long)]
        template: Option<String>,

        /// Accept template hooks without prompting.
        #[arg(long)]
        yes: bool,
    },
    Init {
        /// Frontend assets directory.
        #[arg(long)]
        assets: Option<PathBuf>,

        /// Dev server URL.
        #[arg(long)]
        dev_url: Option<String>,

        /// Accept template hooks without prompting.
        #[arg(long)]
        yes: bool,
    },
    Clean {
        #[arg(value_parser = ["all"])]
        target: Option<String>,

        /// Accept the confirmation without prompting.
        #[arg(long)]
        yes: bool,
    },
    Showcase {
        /// Accept template hooks without prompting.
        #[arg(long)]
        yes: bool,
    },
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

/// Whether `--ci` was asked for, by flag or by the `CI` variable's value.
///
/// The flag takes precedence, `CI` enables non-interactive execution
/// unless its value is empty, `0`, or `false`. `CI` is parsed manually
/// because Clap's `env` bool parser rejects values such as `CI=1`.
fn ci_requested(flag: bool, ci: Option<&std::ffi::OsStr>) -> bool {
    flag || ci.is_some_and(|value| {
        let value = value.to_string_lossy();
        let value = value.trim();

        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

/// Whether the CLI must run without prompting.
fn is_unattended(ci: bool) -> bool {
    use std::io::IsTerminal;
    ci || !std::io::stdin().is_terminal()
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Keep prompting and consent separate.
    // `--ci` means "do not ask"; `--yes` means "approve"
    let unattended = is_unattended(ci_requested(cli.ci, std::env::var_os("CI").as_deref()));
    let consent = |yes: bool| template::Consent {
        hooks: yes,
        non_interactive: unattended,
    };

    match cli.command {
        Commands::Install => install::run(),
        Commands::Dev => dev::run(),
        Commands::Run { cargo_args } => run::run(cargo_args),
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
        } => new::run(starter, name, language, template, consent(yes)),
        Commands::Init {
            assets,
            dev_url,
            yes,
        } => init::run(assets, dev_url, consent(yes)),
        Commands::Clean { target, yes } => clean::run(target, yes, unattended),
        Commands::Showcase { yes } => showcase::run(consent(yes)),
        Commands::Doctor { json } => doctor::run(json),
        Commands::List { target } => list::run(target),
        Commands::Info => info::run(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CI`'s value as providers set it, rather than clap's bool grammar.
    fn ci(value: Option<&str>) -> Option<&std::ffi::OsStr> {
        value.map(std::ffi::OsStr::new)
    }

    #[test]
    fn the_flag_alone_is_enough() {
        assert!(ci_requested(true, ci(None)));
    }

    #[test]
    fn unset_ci_stays_interactive() {
        assert!(!ci_requested(false, ci(None)));
    }

    #[test]
    fn common_truthy_ci_values_are_detected() {
        for value in ["true", "1", "yes", "TRUE"] {
            assert!(
                ci_requested(false, ci(Some(value))),
                "CI={value} should be non-interactive"
            );
        }
    }

    #[test]
    fn explicitly_falsey_ci_values_stay_interactive() {
        for value in ["", "0", "false", "FALSE"] {
            assert!(
                !ci_requested(false, ci(Some(value))),
                "CI={value} should remain interactive"
            );
        }
    }
}
