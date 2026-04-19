use clap::{Parser, Subcommand};

mod install;
mod dev;
mod build;
mod bundle;
mod init;
mod doctor;
mod info;
mod tui;

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
    Dev,
    Build,
    Bundle,
    Init {
        name: Option<String>,

        #[arg(long)]
        template: Option<String>,
    },
    Doctor,
    Info,
}

fn main() -> anyhow::Result<()> {
    validate_platform();

    let cli = Cli::parse();

    match cli.command {
        Commands::Install => install::run(),
        Commands::Dev => dev::run(),
        Commands::Build => build::run(),
        Commands::Bundle => bundle::run(),
        Commands::Init { name, template } => init::run(name, template),
        Commands::Doctor => doctor::run(),
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
