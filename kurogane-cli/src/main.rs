//! Kurogane command-line entry point.
//!
//! Defines the CLI surface and dispatches each subcommand to its
//! corresponding command implementations.

use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

mod install;
mod dev;
mod launch;
mod run;
mod sandbox;
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
mod receipt;
mod uninstall;

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
    Bundle {
        #[arg(long)]
        debug: bool,
        #[arg(long, default_value = crate::bundle::DEFAULT_FORMAT)]
        format: String,
        /// Sign the bundle's Windows binaries ([signing.windows]) or macOS app
        /// ([signing.macos]). Linux bundles are not signed.
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
    /// Manage the kurogane CLI itself.
    #[command(name = "self", subcommand)]
    Self_(SelfCommand),
}

#[derive(Subcommand)]
enum SelfCommand {
    /// Remove an installer-managed Kurogane installation.
    ///
    /// Removes the CLI, PATH setup and Kurogane's installed runtimes and
    /// caches. Application profiles and unmanaged installations are preserved.
    Uninstall {
        /// Accept the confirmation without prompting.
        #[arg(long)]
        yes: bool,

        /// Keep the installed Chromium runtimes and caches.
        #[arg(long)]
        keep_data: bool,
    },
}

/// Returns whether non-interactive mode was requested via `--ci` or `CI`.
///
/// Parsed here rather than by Clap so values such as `CI=1` are accepted.
fn ci_requested(flag: bool, ci: Option<&std::ffi::OsStr>) -> bool {
    flag || ci.is_some_and(|value| {
        let value = value.to_string_lossy();
        let value = value.trim();

        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

/// Returns whether the CLI must run without prompting.
fn is_unattended(ci: bool) -> bool {
    use std::io::IsTerminal;
    ci || !std::io::stdin().is_terminal()
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Keep prompting and consent separate
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
        Commands::Self_(SelfCommand::Uninstall { yes, keep_data }) => {
            uninstall::run(yes, keep_data, unattended)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a `CI` value as providers set it, not as Clap's bool parser reads it.
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
