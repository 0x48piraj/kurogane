//! Chromium sandbox configuration for application builds and launches.
//!
//! `sandbox = true` changes the application layout on Windows. The application
//! is built as a library and started through CEF's bootstrap. Linux uses the
//! application executable directly, while macOS requires an application
//! bundle.
//!
//! The runtime checks the platform-specific requirements before starting a
//! sandboxed application.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::{Message, Package, TargetKind};
use kurogane_layout::{Bootstrap, Executable, client_library_path, link_dir, stage_runtime};
use crate::config::{AppConfig, anchor_path};

use crate::launch;
use crate::tui;

/// What to add to Cargo.toml when a sandboxed application has nothing to load.
const NEEDS_A_LIBRARY: &str = r#"this crate has no cdylib target, so CEF's sandbox bootstrap has nothing to load.

  Add one to Cargo.toml, named apart from the binary so the two do not
  collide on Windows:

    [lib]
    name = "myapp_lib"
    crate-type = ["cdylib", "rlib"]

  then move the application into src/lib.rs and declare its entry points:

    kurogane::sandbox_entry!(run);
"#;

/// Returns whether the application starts through CEF's bootstrap.
pub(crate) fn uses_bootstrap(app: &AppConfig) -> bool {
    app.sandbox && cfg!(target_os = "windows")
}

/// Returns the name the application takes on disk, without an extension.
///
/// A sandboxed crate has both a binary and a library target, and the library
/// is usually named apart from the binary so Cargo does not collide their
/// debug symbols. The binary's name is the one an unsandboxed build produces,
/// so the bootstrap takes it and the two shapes stay recognizably the same
/// application.
pub(crate) fn app_name(package: &Package) -> Result<&str> {
    launch::find_target(package, TargetKind::Bin)
        .or_else(|| launch::find_target(package, TargetKind::CDyLib))
        .map(|target| target.name.as_str())
        .ok_or_else(|| anyhow!("no binary or library target to name the application after"))
}

/// Builds the application's library and returns the module the bootstrap
/// loads.
///
/// The bootstrap loads the application rather than being it, so only the
/// `cdylib` is built, and Cargo reports where it landed.
pub(crate) fn build_library(package: &Package, build_args: &[OsString]) -> Result<PathBuf> {
    let target =
        launch::find_target(package, TargetKind::CDyLib).ok_or_else(|| anyhow!(NEEDS_A_LIBRARY))?;

    tui::step("Building application library");

    let output = launch::cargo_command("build")
        .args(launch::strip_message_format(build_args))
        .arg("--lib")
        .arg("--message-format=json-render-diagnostics")
        .stderr(Stdio::inherit())
        .output()?;

    if !output.status.success() {
        let code = launch::describe_status(&output.status);
        bail!("cargo build failed (exit code: {code})");
    }

    built_library(&output.stdout, &target.name).ok_or_else(|| {
        anyhow!(
            "cargo built `{}` but reported no loadable library for it",
            target.name
        )
    })
}

/// Finds the module Cargo reports building for the `cdylib` target `name`.
///
/// A `cdylib` is not an executable, so Cargo lists it among the artifact's
/// file names, beside the import library it writes as `<name>.dll.lib`. The
/// last report wins: a rebuilt library is reported after any fresh one.
fn built_library(stdout: &[u8], name: &str) -> Option<PathBuf> {
    let mut library = None;

    for message in Message::parse_stream(stdout).flatten() {
        let Message::CompilerArtifact(artifact) = message else {
            continue;
        };

        if artifact.target.name != name || !artifact.target.kind.contains(&TargetKind::CDyLib) {
            continue;
        }

        let module = artifact.filenames.into_iter().find(|file| {
            file.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        });

        if let Some(module) = module {
            library = Some(module.into_std_path_buf());
        }
    }

    library
}

/// Returns what a bundle starts: CEF's windowed bootstrap, loading `library`.
pub(crate) fn bundled(cef: &Path, library: PathBuf) -> Result<Executable> {
    Ok(Executable::Bootstrap {
        bootstrap: bootstrap_in(cef, Bootstrap::Windowed)?,
        library,
    })
}

/// Returns a bootstrap from the CEF installation, or says how to restore it.
fn bootstrap_in(cef: &Path, bootstrap: Bootstrap) -> Result<PathBuf> {
    let path = bootstrap.path_in(cef);

    if !path.is_file() {
        bail!(
            "CEF's sandbox bootstrap is missing from {}.\n\n  \
             Reinstall the Chromium runtime with `kurogane install`.",
            cef.display()
        );
    }

    Ok(path)
}

/// Builds the application as a library and runs it under CEF's bootstrap.
///
/// The bootstrap finds both the application's library and `chrome_elf.dll`
/// in its own directory and nowhere else, so the run is staged beside the
/// build output rather than started in place.
///
/// The console bootstrap is used here, so the application keeps the terminal
/// it was started from. `kurogane bundle` ships the windowed one.
pub(crate) fn run(
    cef: &Path,
    cargo_args: &[OsString],
    package: &Package,
    project_root: &Path,
    app: &AppConfig,
) -> Result<ExitStatus> {
    let (build_args, app_args) = launch::split_cargo_args(cargo_args);

    let library = build_library(package, build_args)?;

    let artifact_dir = library
        .parent()
        .context("cargo reported a library with no directory")?;

    let staging = staging_dir(artifact_dir);

    tui::step("Staging sandbox bootstrap");

    stage_runtime(cef, &staging).with_context(|| {
        format!(
            "failed to stage the Chromium runtime into {}",
            staging.display()
        )
    })?;

    stage_frontend(project_root, app, &staging)?;

    let exe = staging.join(format!("{}.exe", app_name(package)?));
    let bootstrap = bootstrap_in(cef, Bootstrap::Console)?;

    std::fs::copy(&bootstrap, &exe).with_context(|| {
        format!(
            "failed to install {} as {}",
            bootstrap.display(),
            exe.display()
        )
    })?;

    // Rebuilt every run, so it is copied rather than shared with the build
    let installed = client_library_path(&exe);

    std::fs::copy(&library, &installed)
        .with_context(|| format!("failed to install {}", installed.display()))?;

    tui::field("bootstrap", tui::format_path(&exe));

    tui::blank();
    tui::step("Launching application");
    tui::blank();

    // The application finds the staged runtime beside its executable, the one
    // the bootstrap already loaded
    let status = Command::new(&exe)
        .args(app_args)
        .status()
        .with_context(|| format!("failed to run {}", exe.display()))?;

    Ok(status)
}

/// Directory a development run is staged into, beside the build output.
fn staging_dir(artifact_dir: &Path) -> PathBuf {
    artifact_dir.join("sandbox")
}

/// Stages the packaged frontend beside the bootstrap.
///
/// The bootstrap moves the browser process's working directory to its own,
/// as it does for a packaged application, so a relative asset root resolves
/// there rather than in the project. Installing the frontend under the
/// bundle's fixed `content/` name makes a development run resolve it exactly
/// as the bundle will.
///
/// The staged assets share their storage with the project's, which is safe
/// because an asset root is only ever read.
///
/// Nothing is staged before the frontend has been built, because an
/// application pointed at a dev server does not need it.
fn stage_frontend(project_root: &Path, app: &AppConfig, staging: &Path) -> Result<()> {
    let source = app
        .frontend_dist
        .as_ref()
        .map(|dist| anchor_path(project_root, dist));

    // A distribution configured inside the staging directory is already where
    // it needs to be, and clearing the destination would delete it
    if source
        .as_ref()
        .is_some_and(|source| source.starts_with(staging))
    {
        return Ok(());
    }

    let content = staging.join("content");

    // Rebuilt from the distribution each run, so a removed asset goes too,
    // and so does the whole copy once there is no distribution to stage
    match std::fs::remove_dir_all(&content) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to clear {}", content.display()));
        }
    }

    let Some(source) = source.filter(|source| source.is_dir()) else {
        return Ok(());
    };

    link_dir(&source, &content, &|_| true)
        .with_context(|| format!("failed to stage {} as content/", source.display()))?;

    tui::field("content", tui::format_path(&source));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `compiler-artifact` message as Cargo writes it.
    fn artifact(name: &str, kinds: &[&str], filenames: &[&str]) -> String {
        serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "path+file:///w#myapp@0.1.0",
            "manifest_path": "/w/Cargo.toml",
            "target": { "name": name, "kind": kinds, "src_path": "/w/src/lib.rs" },
            "profile": {
                "opt_level": "0",
                "debug_assertions": true,
                "overflow_checks": true,
                "test": false
            },
            "features": [],
            "filenames": filenames,
            "executable": null,
            "fresh": false
        })
        .to_string()
    }

    fn stream(messages: &[String]) -> Vec<u8> {
        messages.join("\n").into_bytes()
    }

    #[test]
    fn only_windows_starts_through_the_bootstrap() {
        let sandboxed = AppConfig {
            sandbox: true,
            ..Default::default()
        };

        assert_eq!(uses_bootstrap(&sandboxed), cfg!(target_os = "windows"));
        assert!(
            !uses_bootstrap(&AppConfig::default()),
            "the sandbox is opt-in"
        );
    }

    #[test]
    fn the_library_is_read_from_cargo_not_guessed() {
        let output = stream(&[
            artifact("dep", &["lib"], &["/w/target/debug/libdep.rlib"]),
            artifact(
                "myapp_lib",
                &["cdylib", "rlib"],
                &[
                    "/w/target/debug/myapp_lib.dll",
                    "/w/target/debug/myapp_lib.dll.lib",
                    "/w/target/debug/libmyapp_lib.rlib",
                ],
            ),
            r#"{"reason":"build-finished","success":true}"#.to_owned(),
        ]);

        assert_eq!(
            built_library(&output, "myapp_lib"),
            Some(PathBuf::from("/w/target/debug/myapp_lib.dll")),
            "the module is wanted, not the import library beside it"
        );
    }

    #[test]
    fn another_package_s_library_is_not_mistaken_for_the_application_s() {
        let output = stream(&[artifact(
            "plugin",
            &["cdylib"],
            &["/w/target/debug/plugin.dll"],
        )]);

        assert!(built_library(&output, "myapp_lib").is_none());
    }

    #[test]
    fn a_library_target_without_a_cdylib_is_not_loadable() {
        let output = stream(&[artifact(
            "myapp_lib",
            &["lib"],
            &["/w/target/debug/libmyapp_lib.rlib"],
        )]);

        assert!(
            built_library(&output, "myapp_lib").is_none(),
            "an rlib cannot be loaded by the bootstrap"
        );
    }

    #[test]
    fn a_rebuilt_library_replaces_an_earlier_report() {
        let output = stream(&[
            artifact("myapp_lib", &["cdylib"], &["/w/target/debug/stale.dll"]),
            artifact("myapp_lib", &["cdylib"], &["/w/target/debug/myapp_lib.dll"]),
        ]);

        assert_eq!(
            built_library(&output, "myapp_lib"),
            Some(PathBuf::from("/w/target/debug/myapp_lib.dll"))
        );
    }

    #[test]
    fn diagnostics_and_other_lines_are_ignored_rather_than_fatal() {
        let output = stream(&[
            "not json".to_owned(),
            artifact("myapp_lib", &["cdylib"], &["/w/target/debug/myapp_lib.dll"]),
        ]);

        assert!(built_library(&output, "myapp_lib").is_some());
    }

    #[test]
    fn a_run_is_staged_beside_its_build_output() {
        assert_eq!(
            staging_dir(Path::new("/w/target/debug")),
            PathBuf::from("/w/target/debug/sandbox"),
            "staging under the artifact directory follows --target and --release"
        );
    }
}
