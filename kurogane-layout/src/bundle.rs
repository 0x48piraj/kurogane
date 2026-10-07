//! Canonical application bundle materialization.
//!
//! The bundle keeps the executable and CEF runtime together so the packaged
//! application can locate its runtime without environment-specific shims.
//!
//! On Windows, CEF is placed beside the executable, where CEF's sandbox
//! bootstrap requires it. On Linux, CEF is placed under `runtime/cef`, where
//! the runtime looks for a bundle's runtime.
//!
//! Linux bundles include `chrome-sandbox` with the CEF runtime.
//! Used by `SandboxMode::Chromium` when unprivileged user namespaces are unavailable.
//!
//! The executable is installed under the application's name
//! ([`AppMetadata::exe_name`](crate::AppMetadata::exe_name)), whatever the
//! built file is called. A sandboxed Windows application is CEF's bootstrap
//! under that name, with the application's library beside it; see
//! [`Executable`](crate::Executable).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

use crate::cef::is_runtime_artifact;
use crate::layout::{copy_dir, copy_dir_filtered};
use crate::ResolvedDistribution;

/// Errors raised while materializing or verifying a canonical bundle.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BundleError {
    #[error("frontend directory missing: {0}")]
    MissingFrontend(PathBuf),

    #[error("the application has no executable name to install under")]
    MissingExeName,

    #[error("bundle executable missing at {0}")]
    MissingExecutable(PathBuf),

    #[error("bundle client library missing at {0}")]
    MissingClientLibrary(PathBuf),

    #[error("content/index.html missing at {0}")]
    MissingContentIndex(PathBuf),

    /// Without it the application would not know it is a bundle, and would
    /// look for Chromium outside it.
    #[error("bundle marker missing at {0}")]
    MissingMarker(PathBuf),

    #[error(transparent)]
    Cef(#[from] crate::cef::CefError),

    /// A file operation failed: what was being done, and to which path.
    #[error("failed to {action} {}", .path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl BundleError {
    /// An I/O error of the file operation `action` on `path`.
    fn io(action: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.to_path_buf();
        move |source| Self::Io {
            action,
            path,
            source,
        }
    }
}

pub struct BundleLayout {
    root: PathBuf,
}

impl BundleLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn prepare(&self) -> Result<(), BundleError> {
        // Cleaning build directory
        if self.root.exists() {
            fs::remove_dir_all(&self.root).map_err(BundleError::io("clear", &self.root))?;
        }

        fs::create_dir_all(&self.root).map_err(BundleError::io("create", &self.root))?;

        #[cfg(target_os = "linux")]
        {
            let runtime = self.runtime_dir();
            fs::create_dir_all(&runtime).map_err(BundleError::io("create", &runtime))?;
        }

        Ok(())
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }

    #[cfg(target_os = "windows")]
    pub fn cef_dir(&self) -> PathBuf {
        self.root.clone()
    }

    #[cfg(target_os = "linux")]
    pub fn cef_dir(&self) -> PathBuf {
        self.runtime_dir().join("cef")
    }

    #[cfg(target_os = "macos")]
    pub fn cef_dir(&self) -> PathBuf {
        self.root.clone()
    }

    /// Path of the library a bootstrap installed as `exe_name` loads.
    ///
    /// CEF looks it up beside the bootstrap, under the bootstrap's own name,
    /// so it is never on a search path and never shared between applications.
    pub fn client_library_path(&self, exe_name: &OsStr) -> PathBuf {
        crate::client_library_path(&self.executable_path(exe_name))
    }

    pub fn content_dir(&self) -> PathBuf {
        self.root.join("content")
    }

    pub fn launcher_path(&self, exe_name: &OsStr) -> PathBuf {
        self.root.join(exe_name)
    }

    /// Path of the marker that tells the application it runs from this
    /// bundle, beside the executable.
    pub fn marker_path(&self, exe_name: &OsStr) -> PathBuf {
        self.executable_path(exe_name)
            .with_file_name(crate::layout::BUNDLE_MARKER)
    }

    #[cfg(target_os = "windows")]
    pub fn executable_path(&self, exe_name: &OsStr) -> PathBuf {
        self.root.join(exe_name)
    }

    #[cfg(target_os = "linux")]
    pub fn executable_path(&self, exe_name: &OsStr) -> PathBuf {
        self.runtime_dir().join(exe_name)
    }

    #[cfg(target_os = "macos")]
    pub fn executable_path(&self, exe_name: &OsStr) -> PathBuf {
        self.root.join(exe_name)
    }

    pub fn install_frontend(&self, src: &Path) -> Result<(), BundleError> {
        if !src.exists() {
            return Err(BundleError::MissingFrontend(src.to_path_buf()));
        }

        let content = self.content_dir();
        copy_dir(src, &content).map_err(BundleError::io("copy the frontend to", &content))?;
        Ok(())
    }

    /// Installs a materialized CEF runtime into the bundle.
    ///
    /// Only runtime artifacts are included.
    pub fn install_cef(&self, src: &Path) -> Result<(), BundleError> {
        let cef = self.cef_dir();
        copy_dir_filtered(src, &cef, &is_runtime_artifact)
            .map_err(BundleError::io("copy the CEF runtime to", &cef))?;
        Ok(())
    }

    /// Writes the Linux launcher script for the bundle.
    #[cfg(target_os = "linux")]
    pub fn write_launcher(&self, exe_name: &OsStr) -> Result<(), BundleError> {
        let launcher = self.launcher_path(exe_name);

        // One quoted word: the name expands nothing in the script
        let runtime_target = crate::sh_quote(&format!("runtime/{}", exe_name.to_string_lossy()));

        // The library path override is the running machine's, so the script
        // reads it when it starts; an unset LD_LIBRARY_PATH gains no empty
        // entry, which the loader would read as the working directory
        let script = format!(
            r#"#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

# Opt-in library path for libraries the system's loader does not find
if [ -n "${{KUROGANE_LD_LIBRARY_PATH:-}}" ]; then
    export LD_LIBRARY_PATH="$KUROGANE_LD_LIBRARY_PATH${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}"
fi

exec "$ROOT"/{runtime_target} "$@"
"#
        );

        fs::write(&launcher, script).map_err(BundleError::io("write", &launcher))?;

        let executable = BundleError::io("make executable", &launcher);
        let mut perms = fs::metadata(&launcher).map_err(executable)?.permissions();

        perms.set_mode(0o755);

        fs::set_permissions(&launcher, perms)
            .map_err(BundleError::io("make executable", &launcher))?;

        Ok(())
    }

    /// Materializes a resolved distribution into this bundle layout.
    ///
    /// Copies the executable, CEF runtime, frontend and any extra resources
    /// into the platform-specific directory structure.
    pub fn materialize(&self, dist: &ResolvedDistribution) -> Result<(), BundleError> {
        self.prepare()?;

        let exe_name = exe_name(dist)?;

        let executable = self.executable_path(exe_name);
        fs::copy(dist.executable.binary(), &executable)
            .map_err(BundleError::io("copy the executable to", &executable))?;

        if let Some(library) = dist.executable.library() {
            let installed = self.client_library_path(exe_name);
            fs::copy(library, &installed).map_err(BundleError::io(
                "copy the application library to",
                &installed,
            ))?;
        }

        #[cfg(target_os = "linux")]
        self.write_launcher(exe_name)?;

        // The application runs the runtime installed below and no other
        let marker = self.marker_path(exe_name);
        fs::write(&marker, b"").map_err(BundleError::io("write", &marker))?;

        self.install_cef(&dist.cef_runtime)?;

        if let Some(frontend) = &dist.frontend {
            self.install_frontend(frontend)?;
        }

        for resource in &dist.extra_resources {
            let dest = self.root.join(&resource.destination);
            let copy = BundleError::io("copy a resource to", &dest);
            if resource.source.is_dir() {
                copy_dir(&resource.source, &dest).map_err(copy)?;
            } else {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(BundleError::io("create", parent))?;
                }
                fs::copy(&resource.source, &dest).map_err(copy)?;
            }
        }

        Ok(())
    }

    /// Verifies that the bundle contains the application, its client library
    /// when required and a complete CEF runtime.
    pub fn verify(&self, dist: &ResolvedDistribution) -> Result<(), BundleError> {
        let exe_name = exe_name(dist)?;
        let exe = self.executable_path(exe_name);

        if !exe.exists() {
            return Err(BundleError::MissingExecutable(exe));
        }

        let marker = self.marker_path(exe_name);

        if !marker.is_file() {
            return Err(BundleError::MissingMarker(marker));
        }

        if dist.executable.library().is_some() {
            let library = self.client_library_path(exe_name);

            if !library.exists() {
                return Err(BundleError::MissingClientLibrary(library));
            }
        }

        if self.content_dir().exists() {
            let index = self.content_dir().join("index.html");

            if !index.exists() {
                return Err(BundleError::MissingContentIndex(index));
            }
        }

        crate::cef::validate_cef_runtime(&self.cef_dir())?;

        Ok(())
    }
}

/// Returns the file name the executable is installed under.
fn exe_name(dist: &ResolvedDistribution) -> Result<&OsStr, BundleError> {
    match dist.metadata.exe_name.as_str() {
        "" => Err(BundleError::MissingExeName),
        name => Ok(OsStr::new(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Platform-correct executable file name.
    fn test_exe_name() -> &'static OsStr {
        if cfg!(target_os = "windows") {
            OsStr::new("myapp.exe")
        } else {
            OsStr::new("myapp")
        }
    }

    #[test]
    fn materialize_copies_executable() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);

        layout.materialize(&dist).unwrap();

        let exe = layout.executable_path(test_exe_name());
        assert!(exe.exists(), "executable should exist after materialize");
    }

    #[test]
    fn a_failed_copy_names_what_it_was_copying_and_where() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        std::fs::remove_file(dist.executable.binary()).unwrap();
        let layout = BundleLayout::new(dir.path().join("out"));

        let err = layout.materialize(&dist).unwrap_err();

        let executable = layout.executable_path(test_exe_name());
        assert_eq!(
            err.to_string(),
            format!("failed to copy the executable to {}", executable.display())
        );
        assert!(
            std::error::Error::source(&err).is_some(),
            "the operating system's reason stays as the cause"
        );
    }

    #[test]
    fn materialize_copies_cef() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);

        layout.materialize(&dist).unwrap();

        let cef_dir = layout.cef_dir();
        assert!(cef_dir.exists(), "CEF directory should exist");

        #[cfg(target_os = "linux")]
        assert!(
            cef_dir.join("libcef.so").exists(),
            "libcef.so should be present"
        );
        #[cfg(target_os = "windows")]
        assert!(
            cef_dir.join("libcef.dll").exists(),
            "libcef.dll should be present"
        );
    }

    #[test]
    fn a_bundle_is_marked_to_run_the_runtime_it_installed() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let layout = BundleLayout::new(dir.path().join("out"));

        layout.materialize(&dist).unwrap();

        let exe = layout.executable_path(test_exe_name());
        assert_eq!(
            crate::layout::bundle_cef_root_for(&exe),
            Some(layout.cef_dir())
        );
    }

    #[test]
    fn a_bundle_without_its_marker_fails_verification() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let layout = BundleLayout::new(dir.path().join("out"));
        layout.materialize(&dist).unwrap();

        let marker = layout.marker_path(test_exe_name());
        fs::remove_file(&marker).unwrap();

        assert!(matches!(
            layout.verify(&dist),
            Err(BundleError::MissingMarker(path)) if path == marker
        ));
    }

    #[test]
    fn materialize_copies_frontend() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);

        layout.materialize(&dist).unwrap();

        let index = layout.content_dir().join("index.html");
        assert!(index.exists(), "index.html should be present");
    }

    #[test]
    fn materialize_copies_extra_resources() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());

        let res_file = dir.path().join("data.txt");
        fs::write(&res_file, "resource content").unwrap();
        dist.extra_resources.push(crate::ResolvedResource {
            source: res_file.clone(),
            destination: "data.txt".into(),
        });

        let res_dir = dir.path().join("assets");
        fs::create_dir_all(res_dir.join("sub")).unwrap();
        fs::write(res_dir.join("sub").join("file.txt"), "nested").unwrap();
        dist.extra_resources.push(crate::ResolvedResource {
            source: res_dir.clone(),
            destination: "assets".into(),
        });

        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);
        layout.materialize(&dist).unwrap();

        assert!(
            layout.root().join("data.txt").exists(),
            "extra file should be in bundle root"
        );
        assert!(
            layout.root().join("assets").exists(),
            "extra directory should be in bundle root"
        );
        assert!(
            layout
                .root()
                .join("assets")
                .join("sub")
                .join("file.txt")
                .exists(),
            "nested file should be preserved"
        );
    }

    #[test]
    fn materialize_no_frontend_does_not_fabricate_content_dir() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.frontend = None;

        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);
        layout.materialize(&dist).unwrap();

        assert!(
            !layout.content_dir().exists(),
            "content directory should not be created when frontend is None"
        );
    }

    #[test]
    fn materialize_over_existing_output() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("out");

        // First materialization
        let layout = BundleLayout::new(&out);
        layout.materialize(&dist).unwrap();
        let exe = layout.executable_path(test_exe_name());
        assert!(exe.exists());

        // Second materialization over existing output
        layout.materialize(&dist).unwrap();
        assert!(exe.exists(), "executable should exist after re-materialize");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn materialize_creates_launcher_script() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);

        layout.materialize(&dist).unwrap();

        let launcher = layout.launcher_path(test_exe_name());
        assert!(launcher.exists(), "launcher script should exist on Linux");

        let content = fs::read_to_string(&launcher).unwrap();
        assert!(
            content.starts_with("#!/usr/bin/env sh"),
            "launcher should be a shell script"
        );
        assert!(
            content.contains("runtime/myapp"),
            "launcher should reference runtime/myapp"
        );
        assert!(
            !content.contains("cd \"$ROOT\""),
            "the launcher must not change the working directory: a bundle \
             resolves its resources from the executable, not the CWD"
        );
        assert!(
            !content.contains("runtime/cef"),
            "launcher must not set LD_LIBRARY_PATH for CEF"
        );
    }

    // KUROGANE_LD_LIBRARY_PATH belongs to the machine the application runs
    // on: the launcher reads it when it starts, keeps it a plain value, and
    // leaves an unset LD_LIBRARY_PATH without an empty entry
    #[cfg(target_os = "linux")]
    #[test]
    fn the_launcher_reads_the_library_path_override_when_it_runs() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let layout = BundleLayout::new(dir.path().join("out"));
        layout.materialize(&dist).unwrap();

        // The executable the launcher starts reports what it was given
        let target = layout.executable_path(test_exe_name());
        fs::write(
            &target,
            "#!/bin/sh\nprintf '%s' \"${LD_LIBRARY_PATH-unset}\"\n",
        )
        .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let launcher = layout.launcher_path(test_exe_name());
        let run = |vars: &[(&str, &str)]| {
            let output = std::process::Command::new(&launcher)
                .env_remove("KUROGANE_LD_LIBRARY_PATH")
                .env_remove("LD_LIBRARY_PATH")
                .envs(vars.iter().copied())
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap()
        };

        assert_eq!(run(&[]), "unset");
        assert_eq!(run(&[("KUROGANE_LD_LIBRARY_PATH", "/opt/a")]), "/opt/a");
        assert_eq!(
            run(&[
                ("KUROGANE_LD_LIBRARY_PATH", "/opt/a"),
                ("LD_LIBRARY_PATH", "/usr/b")
            ]),
            "/opt/a:/usr/b"
        );
        assert_eq!(run(&[("LD_LIBRARY_PATH", "/usr/b")]), "/usr/b");
        assert_eq!(
            run(&[("KUROGANE_LD_LIBRARY_PATH", "$(false)\"")]),
            "$(false)\""
        );
    }

    #[test]
    fn the_executable_is_installed_under_the_application_s_name() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.metadata.exe_name = "renamed".to_string();

        let layout = BundleLayout::new(dir.path().join("out"));
        layout.materialize(&dist).unwrap();

        assert!(
            layout.executable_path(OsStr::new("renamed")).is_file(),
            "every package format starts the executable by this name"
        );
        assert!(layout.verify(&dist).is_ok());
    }

    #[test]
    fn an_executable_without_a_name_is_refused() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.metadata.exe_name.clear();

        let layout = BundleLayout::new(dir.path().join("out"));

        assert!(matches!(
            layout.materialize(&dist),
            Err(BundleError::MissingExeName)
        ));
    }

    #[test]
    fn a_sandboxed_bundle_installs_the_bootstrap_and_its_library() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sandboxed_distribution(dir.path());
        let layout = BundleLayout::new(dir.path().join("out"));

        layout.materialize(&dist).unwrap();

        let exe = layout.executable_path(OsStr::new("myapp.exe"));
        assert!(
            exe.is_file(),
            "CEF's bootstrap takes the application's name"
        );
        assert!(
            !layout.executable_path(OsStr::new("bootstrap.exe")).exists(),
            "and never reaches the bundle under CEF's name"
        );
        assert!(
            exe.with_file_name("myapp.dll").is_file(),
            "CEF loads the library beside the bootstrap, under its name"
        );
        assert!(layout.verify(&dist).is_ok());
    }

    #[test]
    fn a_sandboxed_bundle_without_its_library_fails_verification() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sandboxed_distribution(dir.path());
        let layout = BundleLayout::new(dir.path().join("out"));

        layout.materialize(&dist).unwrap();
        fs::remove_file(layout.client_library_path(OsStr::new("myapp.exe"))).unwrap();

        assert!(matches!(
            layout.verify(&dist),
            Err(BundleError::MissingClientLibrary(_))
        ));
    }

    #[test]
    fn cef_s_bootstraps_do_not_reach_the_bundle() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());

        // As a runtime materialized by an older Kurogane still carries them
        for name in ["bootstrap.exe", "bootstrapc.exe", "libcef.lib"] {
            fs::write(dist.cef_runtime.join(name), "build artifact").unwrap();
        }

        let layout = BundleLayout::new(dir.path().join("out"));
        layout.materialize(&dist).unwrap();

        for name in ["bootstrap.exe", "bootstrapc.exe", "libcef.lib"] {
            assert!(
                !layout.cef_dir().join(name).exists(),
                "{name} is not loaded at runtime"
            );
        }

        // The runtime itself still arrives intact
        assert!(
            layout
                .cef_dir()
                .join(crate::cef::cef_binary_name())
                .is_file()
        );
    }

    #[test]
    fn verify_passes_with_valid_bundle() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);

        layout.materialize(&dist).unwrap();
        assert!(layout.verify(&dist).is_ok());
    }

    #[test]
    fn verify_requires_content_index_when_content_dir_exists() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.frontend = None;

        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);
        layout.materialize(&dist).unwrap();

        assert!(
            layout.verify(&dist).is_ok(),
            "verify should pass when content/ does not exist"
        );

        fs::create_dir(layout.content_dir()).unwrap();
        let result = layout.verify(&dist);
        assert!(
            result.is_err(),
            "verify should fail when content/ exists without index.html"
        );
    }

    #[test]
    fn verify_fails_without_executable() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);
        fs::create_dir_all(layout.content_dir()).unwrap();
        fs::write(layout.content_dir().join("index.html"), "").unwrap();

        crate::test_fixtures::cef_runtime(&layout.cef_dir());

        let result = layout.verify(&dist);
        assert!(
            result.is_err(),
            "verify should fail when executable is missing"
        );
    }

    #[test]
    fn verify_fails_with_incomplete_cef_runtime() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);
        layout.materialize(&dist).unwrap();

        // Mess shit up
        fs::remove_file(layout.cef_dir().join(crate::cef::cef_binary_name())).unwrap();

        assert!(
            layout.verify(&dist).is_err(),
            "verify should fail when the CEF runtime is incomplete"
        );
    }

    #[test]
    fn exe_name_matches_executable_filename() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());

        let actual_filename = dist
            .executable
            .binary()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();

        assert_eq!(
            dist.metadata.exe_name, actual_filename,
            "exe_name should match the actual executable filename"
        );
    }
}
