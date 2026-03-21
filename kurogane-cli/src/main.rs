use clap::{Parser, Subcommand};

mod install;
mod dev;
mod build;
mod bundle;
mod init;

#[derive(Parser)]
#[command(name = "kurogane")]
#[command(about = "Kurogane: Chromium runtime for building high-performance apps", version)]
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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Install => install::run(),
        Commands::Dev => dev::run(),
        Commands::Build => build::run(),
        Commands::Bundle => bundle::run(),
        Commands::Init { name, template } => init::run(name, template),
    }
}
