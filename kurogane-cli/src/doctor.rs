use anyhow::Result;
use std::path::PathBuf;

pub fn run() -> Result<()> {
    println!("Kurogane Doctor\n");

    // Check CEF installation
    let cef_path = dirs::home_dir()
        .map(|h| h.join(".local/share/cef"))
        .unwrap_or(PathBuf::from("~/.local/share/cef"));

    if cef_path.exists() {
        println!("[+] CEF installed at {}", cef_path.display());
    } else {
        println!("[-] CEF not found at {}", cef_path.display());
        println!("    Run: kurogane install");
    }

    // Check CEF_PATH env
    match std::env::var("CEF_PATH") {
        Ok(v) => println!("[+] CEF_PATH is set ({})", v),
        Err(_) => println!("[!] CEF_PATH not set (fallback will be used)"),
    }

    // Check project structure
    if std::path::Path::new("content").exists() {
        println!("[+] content/ directory found");
    } else {
        println!("[!] content/ directory missing");
    }

    // Check Cargo.toml
    if std::path::Path::new("Cargo.toml").exists() {
        println!("[+] Cargo.toml found");
    } else {
        println!("[-] Not inside a Rust project");
    }

    println!("\nDoctor check complete.\n");

    Ok(())
}
