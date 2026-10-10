//! Kurogane command-line entry point.
//!
//! Defines the CLI surface and dispatches each subcommand to its
//! corresponding command implementations.

use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

mod install;
mod launch;
mod run;
mod sandbox;
mod bundle;
mod distribution;
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
mod template_store;
mod starters;
mod tui;

#[derive(Parser)]
#[command(name = "kurogane")]
#[command(
    about = "Kurogane: GPU-accelerated runtime for building high-performance desktop apps",
    version
)]
struct Cli {
    /// Never prompt; a true `CI` environment variable does the same.
    #[arg(long, global = true)]
    ci: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install the Chromium runtime the project uses.
    ///
    /// Outside a project, installs the version this CLI was built with.
    Install,
    /// Run the application, installing its Chromium runtime when missing.
    Dev,
    /// Run the application with Cargo, installing its Chromium runtime when
    /// missing.
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
    /// Build the application and package it for distribution.
    Bundle {
        /// Build with Cargo's dev profile instead of release.
        #[arg(long)]
        debug: bool,
        /// Package format: dir or appimage on Linux, dir or nsis on Windows,
        /// app on macOS.
        #[arg(long, default_value = crate::bundle::DEFAULT_FORMAT)]
        format: String,
        /// Sign the bundle's Windows binaries ([signing.windows]) or macOS app
        /// ([signing.macos]). Linux bundles are not signed.
        #[arg(long)]
        sign: bool,
    },
    /// Create a project from a starter or template.
    New {
        /// Official starter: minimal, react, svelte or vue.
        starter: Option<String>,

        /// Project name.
        #[arg(long)]
        name: Option<String>,

        /// Starter language.
        #[arg(long)]
        language: Option<String>,

        /// Use a template: a local path, git URL or cargo-generate shorthand
        /// such as gh:owner/repository.
        #[arg(long, conflicts_with = "starter")]
        template: Option<String>,

        /// Read placeholder values from this TOML file.
        #[arg(long, value_name = "FILE")]
        values: Option<PathBuf>,

        /// Allow the template's hooks to run commands without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Add Kurogane to an existing frontend project.
    Init {
        /// Frontend assets directory.
        #[arg(long)]
        assets: Option<PathBuf>,

        /// Dev server URL.
        #[arg(long)]
        dev_url: Option<String>,

        /// Allow the template's hooks to run commands without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Remove the project's dist/ and Kurogane's caches.
    Clean {
        /// `all` also removes tanso's shared Chromium installation, build tools
        /// and every application profile.
        #[arg(value_parser = ["all"])]
        target: Option<String>,

        /// Accept the confirmation without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Run Kurogane's showcase application.
    Showcase {
        /// Allow the template's hooks to run commands without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Check the Chromium runtime, toolchain and project.
    Doctor {
        /// Print the full report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List application profiles and versions.
    List {
        /// Only profiles or only versions; both by default.
        #[arg(value_enum)]
        target: Option<list::Target>,
    },
    /// Show the CLI, environment and project configuration.
    Info,
    /// Manage the kurogane CLI itself.
    #[command(name = "self", subcommand)]
    Self_(SelfCommand),
}

#[derive(Subcommand)]
enum SelfCommand {
    /// Remove an installer-managed Kurogane installation.
    ///
    /// Removes the CLI, PATH setup, Kurogane's caches and tanso's shared
    /// Chromium runtimes, which other tanso projects use too. Application
    /// profiles and unmanaged installations are preserved.
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

/// Returns `run`'s arguments for Cargo as they were `given`.
///
/// Clap takes a `--` that comes first as its own separator, while Cargo
/// needs it to tell the application's arguments from its own.
fn cargo_args(parsed: Vec<OsString>, given: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let separator_first = given
        .into_iter()
        .skip(1)
        .skip_while(|arg| arg != "run")
        .nth(1)
        .is_some_and(|arg| arg == "--");

    if separator_first {
        std::iter::once(OsString::from("--"))
            .chain(parsed)
            .collect()
    } else {
        parsed
    }
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
        Commands::Dev => run::run("Kurogane Dev", Vec::new()),
        Commands::Run { cargo_args: parsed } => {
            run::run("Kurogane Run", cargo_args(parsed, std::env::args_os()))
        }
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
            values,
            yes,
        } => new::run(starter, name, language, template, values, consent(yes)),
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

    /// Returns the arguments `kurogane <given>` hands to `cargo run`.
    fn run_args(given: &[&str]) -> Vec<String> {
        let given: Vec<OsString> = std::iter::once("kurogane")
            .chain(given.iter().copied())
            .map(OsString::from)
            .collect();
        let Commands::Run { cargo_args: parsed } = Cli::try_parse_from(&given).unwrap().command
        else {
            panic!("not a run");
        };

        cargo_args(parsed, given)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn run_hands_cargo_its_arguments_as_given() {
        assert_eq!(run_args(&["run", "--", "--bench=x"]), ["--", "--bench=x"]);
        assert_eq!(
            run_args(&["run", "--release", "--", "--bench=x"]),
            ["--release", "--", "--bench=x"]
        );
        assert_eq!(run_args(&["run", "--", "--", "x"]), ["--", "--", "x"]);
        assert_eq!(run_args(&["--ci", "run", "--", "x"]), ["--", "x"]);
        assert!(run_args(&["run"]).is_empty());
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
