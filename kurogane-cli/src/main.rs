use clap::{Parser, Subcommand};

mod install;
mod dev;
mod build;
mod bundle;
mod init;

#[derive(Parser)]
#[command(name = "kurogane")]
#[command(about = "Kurogane CLI", version)]
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
    Init,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Install => install::run(),
        Commands::Dev => dev::run(),
        Commands::Build => build::run(),
        Commands::Bundle => bundle::run(),
        Commands::Init => init::run(),
    }
}
