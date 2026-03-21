use anyhow::Result;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub fn run() -> Result<()> {
    println!("Kurogane project setup");

    // Ask project name
    print!("Project name: ");
    io::stdout().flush()?;

    let mut name = String::new();
    io::stdin().read_line(&mut name)?;
    let name = name.trim();

    let root = Path::new(name);

    if root.exists() {
        anyhow::bail!("Directory already exists");
    }

    // Create structure
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("content"))?;
    fs::create_dir_all(root.join(".cargo"))?;

    // Cargo.toml
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{}"
version = "0.1.0"
edition = "2024"

[dependencies]
kurogane = {{ git = "https://github.com/0x48piraj/kurogane" }}
"#,
            name
        ),
    )?;

    // main.rs
    fs::write(
        root.join("src/main.rs"),
        r#"use kurogane::App;

fn main() {
    App::path("content").run_or_exit();
}
"#,
    )?;

    // frontend
    fs::write(
        root.join("content/index.html"),
        r#"<!DOCTYPE html>
<html>
<head>
  <title>Kurogane App</title>
</head>
<body>
  <h1>Hello from Kurogane.</h1>
</body>
</html>
"#,
    )?;

    // kurogane.toml
    fs::write(
        root.join("kurogane.toml"),
        r#"[app]
name = "MyApp"
frontend = "content"
dev_url = ""
"#,
    )?;

    // .cargo/config.toml
    let cef_path = default_cef_path()?;

    let cargo_config = format!(
        r#"[env]
CEF_PATH = {{ value = "{}", force = true }}
"#,
        cef_path
    );

    fs::write(root.join(".cargo/config.toml"), cargo_config)?;

    // .gitignore
    fs::write(
        root.join(".gitignore"),
        r#"target/
dist/
.DS_Store
"#,
    )?;

    println!("\nProject `{}` created!", name);
    println!("Next steps:");
    println!("  cd {}", name);
    println!("  kurogane install # one-time install");
    println!("  kurogane dev");

    Ok(())
}

fn default_cef_path() -> Result<String> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home directory"))?;

    let path: PathBuf = home.join(".local").join("share").join("cef");

    #[cfg(target_os = "windows")]
    {
        Ok(path.display().to_string().replace("\\", "\\\\"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(path.display().to_string())
    }
}
