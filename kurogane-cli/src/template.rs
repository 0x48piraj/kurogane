//! Template resolution, caching and project generation.
//!
//! Resolves template references and delegates project generation to cargo-generate.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cargo_generate::{GenerateArgs, TemplatePath, Vcs};

use crate::cache;
use crate::tui;

/// A resolved template reference.
#[derive(Debug, Clone)]
pub enum TemplateSource {
    /// A template directory on the local filesystem.
    Local(PathBuf),
    /// A git URL or shorthand to be acquired through the cache.
    Git(String),
}

/// Resolve a user-supplied template reference.
pub fn resolve(reference: &str) -> TemplateSource {
    let path = Path::new(reference);
    // An existing filesystem path wins
    if path.exists() {
        TemplateSource::Local(path.to_owned())
    } else {
        TemplateSource::Git(reference.to_owned())
    }
}

/// Acquire the template directory for a resolved source.
///
/// Local sources are returned untouched. Git sources are cloned once into
/// the cache and reused from disk afterwards (no network on cache hits).
pub fn acquire(source: &TemplateSource) -> Result<PathBuf> {
    match source {
        TemplateSource::Local(path) => Ok(path.clone()),
        TemplateSource::Git(url) => {
            tui::step("Fetching template");
            tui::field("repository", url);

            let acquired = cache::acquire(url).map_err(|e| {
                anyhow::anyhow!(
                    "{e:#}\nExpected an existing local directory or a git-hosted template."
                )
            })?;

            tui::field("commit", &acquired.commit);
            tui::info("cached; re-runs reuse this copy without network access");

            Ok(acquired.path)
        }
    }
}

/// Detect hook scripts declared by a template's cargo-generate.toml.
///
/// Presence of declared hooks means generation may execute template code
/// i.e. Rhai scripts and shell commands behind prompts.
///
/// This checks configuration presence only; scripts are not inspected.
pub fn detect_hooks(template_dir: &Path) -> Vec<String> {
    let config_path = locate_config(template_dir);
    let Some(contents) = std::fs::read_to_string(config_path).ok() else {
        return Vec::new();
    };
    let Ok(value) = contents.parse::<toml::Table>() else {
        return Vec::new();
    };

    let mut hooks = Vec::new();
    if let Some(table) = value.get("hooks").and_then(|h| h.as_table()) {
        for stage in ["init", "pre", "post"] {
            if let Some(files) = table.get(stage).and_then(|s| s.as_array()) {
                hooks.extend(files.iter().filter_map(|f| f.as_str().map(str::to_owned)));
            }
        }
    }
    hooks
}

fn locate_config(template_dir: &Path) -> PathBuf {
    template_dir.join("cargo-generate.toml")
}

/// Require clear consent for templates that declare hooks.
///
/// Once cargo-generate exposes hook information directly through its API,
/// [`detect_hooks`] logic should be replaced by that native upstream signal.
pub fn confirm_hooks(template_dir: &Path, assume_yes: bool) -> Result<()> {
    use std::io::{IsTerminal, Write};

    let hooks = detect_hooks(template_dir);
    if hooks.is_empty() {
        return Ok(());
    }

    tui::warn("This template declares hooks that will execute during generation:");
    for hook in &hooks {
        tui::field("hook", hook);
    }

    if assume_yes {
        return Ok(());
    }

    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "Template declares hooks. Re-run with --yes to accept them in non-interactive mode."
        );
    }

    let confirmed = loop {
        print!("\nProceed with generation? [y/N]: ");
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        tui::blank();

        match input.trim() {
            "y" | "Y" | "yes" | "Yes" | "YES" => break true,
            "n" | "N" | "no" | "No" | "NO" | "" => break false,
            _ => {
                tui::warn("Please enter y or n");
                continue;
            }
        }
    };

    if !confirmed {
        anyhow::bail!("Aborted: template hooks not accepted.");
    }

    Ok(())
}

/// Generate a new project from a template.
pub fn generate_project(
    template_dir: &Path,
    name: &str,
    destination: &Path,
    overwrite: bool,
    defines: &[String],
) -> Result<PathBuf> {
    let args = GenerateArgs {
        template_path: TemplatePath {
            path: Some(template_dir.display().to_string()),
            ..TemplatePath::default()
        },
        name: Some(name.to_owned()),
        destination: Some(destination.to_owned()),
        vcs: Some(Vcs::None),
        no_workspace: true,
        overwrite,
        define: defines.to_vec(),
        ..GenerateArgs::default()
    };

    cargo_generate::generate(args).context("Project generation failed")
}

/// Generate a template into an existing project directory.
pub fn generate_into_existing_dir(
    template_dir: &Path,
    name: &str,
    destination: &Path,
    defines: &[String],
) -> Result<PathBuf> {
    let args = GenerateArgs {
        template_path: TemplatePath {
            path: Some(template_dir.display().to_string()),
            ..TemplatePath::default()
        },
        name: Some(name.to_owned()),
        destination: Some(destination.to_owned()),
        vcs: Some(Vcs::None),
        no_workspace: true,
        init: true,
        overwrite: false,
        define: defines.to_vec(),
        ..GenerateArgs::default()
    };

    cargo_generate::generate(args).context("Project generation failed")
}

/// Keep the bundled CEF runtime discoverable without environment shims.
pub(crate) fn write_cargo_config(project_dir: &Path) -> Result<()> {
    fs::create_dir_all(project_dir.join(".cargo"))?;

    fs::write(
        project_dir.join(".cargo/config.toml"),
        r#"[target.'cfg(all(unix, not(target_os = "macos")))']
rustflags = ["-C", "link-arg=-Wl,-rpath,$ORIGIN/cef"]

[target.'cfg(target_os = "macos")']
rustflags = ["-C", "link-arg=-Wl,-rpath,@executable_path"]
"#,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn layout_template(dir: &Path) {
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"placeholder\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    }

    #[test]
    fn existing_paths_resolve_locally() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            resolve(dir.path().to_str().unwrap()),
            TemplateSource::Local(_)
        ));
        assert!(matches!(resolve("."), TemplateSource::Local(_)));
    }

    #[test]
    fn everything_else_is_delegated_to_the_git_layer_verbatim() {
        for reference in [
            "https://github.com/foo/bar",
            "gh:foo/bar",
            "owner/repo",
            "git@github.com:foo/bar.git",
            "definitely-not-a-template",
        ] {
            match resolve(reference) {
                TemplateSource::Git(url) => assert_eq!(url, reference),
                other => panic!("expected git source for '{reference}', got {other:?}"),
            }
        }
    }

    #[test]
    fn generation_from_a_local_template_yields_a_usable_project() {
        let layout_dir = tempfile::tempdir().unwrap();
        layout_template(layout_dir.path());

        let destination = tempfile::tempdir().unwrap();
        let project =
            generate_project(layout_dir.path(), "my-app", destination.path(), false, &[]).unwrap();

        assert!(project.join("Cargo.toml").exists());
        assert!(project.join("src/main.rs").exists());
        assert!(!project.join(".git").exists());
    }

    #[test]
    fn non_kebab_names_are_kebab_cased_in_the_destination() {
        let layout_dir = tempfile::tempdir().unwrap();
        layout_template(layout_dir.path());

        let destination = tempfile::tempdir().unwrap();
        generate_project(layout_dir.path(), "My App", destination.path(), false, &[]).unwrap();

        assert!(
            destination
                .path()
                .join("my-app")
                .join("Cargo.toml")
                .exists()
        );
        assert!(!destination.path().join("My App").exists());
    }

    #[test]
    fn generation_does_not_mutate_a_parent_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .unwrap();

        let layout_dir = tempfile::tempdir().unwrap();
        layout_template(layout_dir.path());

        generate_project(
            layout_dir.path(),
            "member-app",
            workspace.path(),
            false,
            &[],
        )
        .unwrap();

        let manifest = fs::read_to_string(workspace.path().join("Cargo.toml")).unwrap();
        assert_eq!(manifest, "[workspace]\nmembers = []\n");
    }

    #[test]
    fn templates_without_hooks_require_no_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        layout_template(dir.path());
        assert!(detect_hooks(dir.path()).is_empty());
        confirm_hooks(dir.path(), false).unwrap();
    }

    #[test]
    fn declared_hooks_are_detected_from_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        layout_template(dir.path());
        fs::write(
            dir.path().join("cargo-generate.toml"),
            "[hooks]\npre = [\"pre.rhai\"]\npost = [\"post-a.rhai\", \"post-b.rhai\"]\n",
        )
        .unwrap();

        let hooks = detect_hooks(dir.path());
        assert_eq!(hooks, vec!["pre.rhai", "post-a.rhai", "post-b.rhai"]);
    }

    #[test]
    fn cargo_config_pins_rpath_to_the_bundled_cef_runtime() {
        let dir = tempfile::tempdir().unwrap();
        write_cargo_config(dir.path()).unwrap();

        let contents = fs::read_to_string(dir.path().join(".cargo/config.toml")).unwrap();
        assert!(contents.starts_with("[target."));
        assert!(contents.contains("$ORIGIN/cef"));
    }

    #[test]
    fn init_mode_generation_targets_the_destination_itself() {
        let shell_dir = tempfile::tempdir().unwrap();
        fs::write(
            shell_dir.path().join("Cargo.toml"),
            "[package]\nname = \"{{crate_name}}\"\nversion = \"0.0.0\"\n\n[workspace]\n",
        )
        .unwrap();

        let destination = tempfile::tempdir().unwrap();
        let project = generate_into_existing_dir(
            shell_dir.path(),
            "my-vite-app",
            destination.path(),
            &["frontend=dist".to_string()],
        )
        .unwrap();

        assert_eq!(project, destination.path());
        assert!(
            destination.path().join("Cargo.toml").exists(),
            "files land directly in the destination, no subfolder"
        );
    }
}
