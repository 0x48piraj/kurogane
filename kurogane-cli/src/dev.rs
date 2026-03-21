use anyhow::Result;
use std::process::Command;

pub fn run() -> Result<()> {
    println!("Starting dev mode...");

    let cef = dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/cef");

    if !cef.exists() {
        println!("CEF not found. Installing...");
        crate::install::run()?;
    }

    // Pass env to build step
    let mut build = Command::new("cargo");
    build.arg("build");
    build.env("CEF_PATH", &cef);
    build.status()?;

    // Run exe
    let mut run = Command::new("cargo");
    run.arg("run");
    run.env("CEF_PATH", &cef);

    // Windows needs PATH too
    let mut path = std::env::var("PATH").unwrap_or_default();
    path = format!("{};{}", cef.display(), path);
    run.env("PATH", path);

    run.status()?;

    Ok(())
}
