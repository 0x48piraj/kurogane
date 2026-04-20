use anyhow::{Result, bail};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};

use crate::tui;

// Embed templates into the binary
static TEMPLATES: Dir = include_dir!("$CARGO_MANIFEST_DIR/templates");

pub fn run(name: Option<String>, template: Option<String>) -> Result<()> {
    tui::section("Kurogane project setup");

    let name = match name {
        Some(n) => n,
        None => {
            // Ask project name
            print!("Project name: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            input.trim().to_string()
        }
    };

    if name.is_empty() {
        bail!("Project name cannot be empty.");
    }

    let root = Path::new(&name);

    if root.exists() {
        bail!("Directory already exists.");
    }

    // Choose template
    let template = template.unwrap_or_else(|| "vanilla".to_string());

    tui::step("Creating project");
    tui::field("name", &name);
    tui::field("template", &template);

    // Extract template from embedded assets
    extract_template(&template, root)?;

    // .cargo/config.toml
    fs::create_dir_all(root.join(".cargo"))?;

    let cef_path = default_cef_path()?;

    fs::write(
        root.join(".cargo/config.toml"),
        format!(
            r#"[env]
CEF_PATH = {{ value = "{}", force = true }}

[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-Wl,-rpath,$ORIGIN/cef"]
"#,
            cef_path
        ),
    )?;

    tui::success("Project created");
    tui::field("name", &name);
    tui::field("template", &template);

    println!();

    tui::info("Next steps");
    println!("    cd {}", name);
    println!("    kurogane dev");

    println!();

    Ok(())
}

//
// Extract template from embedded dir
//
fn extract_template(name: &str, dest: &Path) -> Result<()> {
    let dir = TEMPLATES
        .get_dir(name)
        .ok_or_else(|| anyhow::anyhow!("Template '{}' not found", name))?;

    copy_embedded_dir(dir, dest)
}

//
// Copy embedded directory recursively
//
fn copy_embedded_dir(dir: &Dir, dest: &Path) -> Result<()> {
    for file in dir.files() {
        let rel_path = file.path();

        let stripped = rel_path
            .components()
            .skip(1) // remove template root
            .collect::<PathBuf>();

        let path = dest.join(stripped);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(path, file.contents())?;
    }

    for subdir in dir.dirs() {
        copy_embedded_dir(subdir, dest)?;
    }

    Ok(())
}

fn default_cef_path() -> Result<String> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("No home directory"))?;

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
