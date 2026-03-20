use anyhow::{Context, Result};
use serde::Deserialize;
use download_cef::{CefIndex, DEFAULT_TARGET};
use std::time::Duration;
use std::path::{Path, PathBuf};

fn default_install_dir() -> PathBuf {
    dirs::home_dir().expect("no home dir").join(".local/share/cef")
}

fn find_workspace_lock() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;

    loop {
        let candidate = dir.join("Cargo.lock");
        if candidate.exists() {
            return Ok(candidate);
        }

        if !dir.pop() {
            anyhow::bail!("Could not find Cargo.lock in this project");
        }
    }
}

#[derive(Deserialize)]
struct Lockfile {
    package: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
}

fn required_cef_version(lock_path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(lock_path)?;
    let parsed: Lockfile = toml::from_str(&content)?;

    let pkg = parsed
        .package
        .into_iter()
        .find(|p| p.name == "cef-dll-sys")
        .context("cef-dll-sys not found in Cargo.lock (did you build once?)")?;

    Ok(pkg.version.split('+').nth(1)
        .context("invalid cef-dll-sys version format")?
        .to_string())
}

fn print_env_instructions(root: &Path) {
    println!("\nInitializing runtime (one-time setup)\n");

    #[cfg(target_os="windows")]
    {
        println!("PowerShell:");
        println!(r#"$env:CEF_PATH="{}""#, root.display());
        println!(r#"$env:PATH="$env:PATH;$env:CEF_PATH""#);
    }

    #[cfg(target_os="linux")]
    {
        println!(r#"export CEF_PATH="{}""#, root.display());
        println!(r#"export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:$CEF_PATH""#);
        println!("Run once:");
        println!(" sudo chown root:root {}/cef_linux_x86_64/chrome-sandbox", root.display());
        println!(" sudo chmod 4755 {}/cef_linux_x86_64/chrome-sandbox", root.display());
    }

    #[cfg(target_os="macos")]
    {
        println!(r#"export CEF_PATH="{}""#, root.display());
        println!(r#"export DYLD_FALLBACK_LIBRARY_PATH="$DYLD_FALLBACK_LIBRARY_PATH:$CEF_PATH:$CEF_PATH/Chromium Embedded Framework.framework/Libraries""#);
    }

    println!("\nRestart your terminal after running these commands.");
    println!("Then run: cargo run --example demo\n");
}

fn main() -> Result<()> {
    println!("Kurogane CEF installer");

    let lock = find_workspace_lock()?;
    println!("Using lockfile: {}", lock.display());

    let cef_version = required_cef_version(&lock)?;
    println!("Required CEF version: {}", cef_version);

    let install_dir = default_install_dir(); // ~/.local/share/cef
    let parent = install_dir.parent().unwrap(); // ~/.local/share
    std::fs::create_dir_all(parent)?;

    let index = CefIndex::download()?;
    let platform = index.platform(DEFAULT_TARGET)?;
    let version = platform.version(&cef_version)?;

    println!("Downloading matching CEF build...");

    let archive = version.download_archive_with_retry(
        parent,
        true,
        Duration::from_secs(15),
        3,
    )?;

    println!("Extracting...");
    let extracted = download_cef::extract_target_archive(
        DEFAULT_TARGET,
        &archive,
        parent,
        true,
    )?;

    // Write archive.json
    version.minimal()?.write_archive_json(&extracted)?;

    // Replace existing install (if any)
    if install_dir.exists() {
        println!("Removing old install...");
        std::fs::remove_dir_all(&install_dir)?;
    }

    println!("Installing to {}", install_dir.display());
    std::fs::rename(&extracted, &install_dir)?;

    let _ = std::fs::remove_file(&archive);

    println!("\n[+] CEF installed at {}", install_dir.display());
    print_env_instructions(&install_dir);

    Ok(())
}
