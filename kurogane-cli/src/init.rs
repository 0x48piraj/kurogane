use std::fs;
use anyhow::{Result, bail};
use std::io::{self, Write};
use std::path::Path;

use crate::tui;
use crate::templates::extract_template;

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

    if template == "showcase" {
        bail!("Use 'kurogane showcase' to run the showcase demo");
    }

    tui::step("Creating project");
    tui::field("name", &name);
    tui::field("template", &template);

    // Extract template from embedded assets
    fs::create_dir_all(&root)?;
    extract_template(&template, root)?;

    // .cargo/config.toml
    fs::create_dir_all(root.join(".cargo"))?;

    fs::write(
        root.join(".cargo/config.toml"),
        r#"[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-Wl,-rpath,$ORIGIN/cef"]
"#,
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
