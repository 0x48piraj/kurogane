use anyhow::Result;

pub fn run() -> Result<()> {
    println!("Kurogane Info\n");

    // Version
    println!("Version: {}", env!("CARGO_PKG_VERSION"));

    // Platform
    println!("OS: {}", std::env::consts::OS);
    println!("Arch: {}", std::env::consts::ARCH);

    // CEF path
    match std::env::var("CEF_PATH") {
        Ok(v) => println!("CEF_PATH: {}", v),
        Err(_) => println!("CEF_PATH: (not set)"),
    }

    // Current project directory
    match std::env::current_dir() {
        Ok(dir) => println!("Project dir: {}", dir.display()),
        Err(_) => println!("Project dir: (unknown)"),
    }

    println!();

    Ok(())
}
