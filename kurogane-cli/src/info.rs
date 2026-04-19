use anyhow::Result;

use crate::tui;

pub fn run() -> Result<()> {
    tui::section("Kurogane Info");

    tui::info("Runtime");

    // Version
    tui::field("version", env!("CARGO_PKG_VERSION"));
    // Platform
    tui::field("os", std::env::consts::OS);
    tui::field("arch", std::env::consts::ARCH);

    println!();

    tui::info("Environment");

    // CEF path
    match std::env::var("CEF_PATH") {
        Ok(v) => tui::field("CEF_PATH", v),
        Err(_) => tui::field("CEF_PATH", "not set"),
    }

    println!();

    tui::info("Project");

    // Current project directory
    match std::env::current_dir() {
        Ok(dir) => tui::field("directory", dir.display()),
        Err(_) => tui::field("directory", "(unknown)"),
    }

    println!();

    Ok(())
}
