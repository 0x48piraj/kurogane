//! Project creation from starters and templates.
//!
//! Resolves project sources and generates new applications through
//! cargo-generate with Kurogane-specific project post-generation.

use anyhow::{Result, bail};
use std::io::{self, Write};
use std::path::Path;

use crate::starters;
use crate::template;
use crate::tui;

pub fn run(starter: Option<String>, template_src: Option<String>, assume_yes: bool) -> Result<()> {
    tui::section("Kurogane project setup");

    let (source, language) = resolve_source(starter, template_src)?;

    let name = prompt_project_name()?;

    tui::step("Creating project");
    tui::field("name", &name);

    let resolved = template::resolve(&source);
    let template_dir = template::acquire(&resolved)?;
    template::confirm_hooks(&template_dir, assume_yes)?;

    let defines = language
        .map(|l| vec![format!("language={l}")])
        .unwrap_or_default();
    let destination = std::env::current_dir()?;
    let project = template::generate_project(&template_dir, &name, &destination, false, &defines)?;

    template::write_cargo_config(&project)?;

    let generated_config = kurogane_layout::PackagingConfig::load(&project)?;

    tui::success("Project created");
    tui::field("name", &name);
    tui::blank();

    tui::info("Next steps");
    println!("    cd {}", name);

    if project.join("package.json").exists() {
        println!("    npm install");
    }

    if let Some(cmd) = &generated_config.app.frontend_build {
        println!("    {cmd}");
    }

    println!("    kurogane dev  # in another terminal");
    tui::blank();

    Ok(())
}

fn resolve_source(
    starter: Option<String>,
    template_src: Option<String>,
) -> Result<(String, Option<String>)> {
    match (starter, template_src) {
        (None, None) => {
            let s = starters::choose()?;
            let lang = starters::choose_language(s)?;
            Ok((s.source.to_owned(), lang.map(str::to_owned)))
        }
        (None, Some(tpl)) => Ok((tpl, None)),
        (Some(name), None) => {
            if name == "showcase" {
                bail!("Use 'kurogane showcase' to run the showcase demo");
            }
            let s = starters::find(&name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown starter '{name}'\n\n\
                     Available starters:\n  minimal\n  react\n  svelte\n  vue\n\n\
                     For a custom template, use --template:\n  \
                     kurogane new --template gh:owner/repository"
                )
            })?;
            let lang = starters::choose_language(s)?;
            Ok((s.source.to_owned(), lang.map(str::to_owned)))
        }
        (Some(_), Some(_)) => {
            bail!("--template and a starter name are mutually exclusive");
        }
    }
}

fn prompt_project_name() -> Result<String> {
    use std::io::IsTerminal;

    if !std::io::stdin().is_terminal() {
        bail!("Project name is required in non-interactive mode");
    }

    print!("Project name: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let name = input.trim().to_string();

    if name.is_empty() {
        bail!("Project name cannot be empty.");
    }

    if Path::new(&name).exists() {
        bail!("Directory already exists.");
    }

    Ok(name)
}
