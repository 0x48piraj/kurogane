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
        #[arg(long, default_value = "dir")]
        format: String,
        /// Sign bundle binaries.
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

/// Whether `--ci` was asked for, by flag or by environment.
///
/// The flag takes precedence, `CI` enables non-interactive execution
/// unless its value is empty, `0`, or `false`. `CI` is parsed manually
/// because Clap's `env` bool parser rejects values such as `CI=1`.
fn ci_requested(flag: bool) -> bool {
    flag || std::env::var_os("CI").is_some_and(|value| {
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
    let unattended = is_unattended(ci_requested(cli.ci));
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

    /// `CI` is read directly, so these cases are asserted against the values
    /// providers actually set rather than clap's bool grammar.
    ///
    /// Access is serialized because environment variables are process-global.
    fn with_ci<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let original = std::env::var_os("CI");
        // SAFETY: CI access is serialized by LOCK
        unsafe {
            match value {
                Some(v) => std::env::set_var("CI", v),
                None => std::env::remove_var("CI"),
            }
        }
        let result = f();
        // SAFETY: CI access is serialized by LOCK
        unsafe {
            match original {
                Some(v) => std::env::set_var("CI", v),
                None => std::env::remove_var("CI"),
            }
        }
        result
    }

    #[test]
    fn the_flag_alone_is_enough() {
        assert!(with_ci(None, || ci_requested(true)));
    }

    #[test]
    fn unset_ci_stays_interactive() {
        assert!(!with_ci(None, || ci_requested(false)));
    }

    #[test]
    fn common_truthy_ci_values_are_detected() {
        for value in ["true", "1", "yes", "TRUE"] {
            assert!(
                with_ci(Some(value), || ci_requested(false)),
                "CI={value} should be non-interactive"
            );
        }
    }

    #[test]
    fn explicitly_falsey_ci_values_stay_interactive() {
        for value in ["", "0", "false", "FALSE"] {
            assert!(
                !with_ci(Some(value), || ci_requested(false)),
                "CI={value} should remain interactive"
            );
        }
    }
}
