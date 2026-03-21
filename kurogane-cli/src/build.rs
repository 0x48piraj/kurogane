use anyhow::Result;
use std::process::Command;

pub fn run() -> Result<()> {
    println!("Building release app...");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .status()?;

    if !status.success() {
        anyhow::bail!("Build failed.");
    }

    println!("Build complete.");

    Ok(())
}
