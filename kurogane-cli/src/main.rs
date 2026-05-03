use clap::{Parser, Subcommand};
use std::ffi::OsString;

mod install;
mod dev;
mod build;
mod bundle;
mod init;
mod showcase;
mod clean;
mod doctor;
mod info;

mod templates;
mod tui;
mod collector;

#[derive(Parser)]
#[command(name = "kurogane")]
#[command(about = "Kurogane: GPU-accelerated runtime for building high-performance desktop apps", version)]
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
    },
    Init {
        name: Option<String>,

        #[arg(long)]
        template: Option<String>,
    },
    Clean,
    Showcase,
    Doctor {
        #[arg(long)]
        json: bool,
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
        Commands::Bundle { debug } => bundle::run(debug),
        Commands::Init { name, template } => init::run(name, template),
        Commands::Clean => clean::run(),
        Commands::Showcase => showcase::run(),
        Commands::Doctor { json } => doctor::run(json),
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
