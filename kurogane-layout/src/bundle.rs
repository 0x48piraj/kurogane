//! Canonical application bundle materialization.
//!
//! The bundle keeps the executable and CEF runtime together so the packaged
//! application can locate its runtime without environment-specific shims.
//!
//! On Windows, CEF is placed beside the executable so the Windows loader can
//! resolve its DLL dependencies normally. On Linux, CEF is placed under
//! `runtime/cef`, matching the executable's `$ORIGIN/cef` RPATH and runtime
//! discovery path.
//!
//! Linux bundles retain `chrome-sandbox` as part of the CEF runtime even though
//! Kurogane currently disables CEF's sandbox. This keeps the bundle compatible
//! with a future sandbox policy change without requiring a packaging change.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

use crate::{ResolvedDistribution, layout::copy_dir};

/// Errors raised while materializing or verifying a canonical bundle.
#[derive(Debug, Error)]
pub enum BundleError {
    #[error("frontend directory missing: {0}")]
    MissingFrontend(PathBuf),

    #[error("executable path has no file name: {0}")]
    InvalidExecutablePath(PathBuf),

    #[error("bundle executable missing at {0}")]
    MissingExecutable(PathBuf),

    #[error("content/index.html missing at {0}")]
    MissingContentIndex(PathBuf),

    #[error(transparent)]
    Cef(#[from] crate::cef::CefError),

    #[error(transparent)]
    Io(#[from] std::io::Error),
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
            fs::remove_dir_all(&self.root)?;
        }

        fs::create_dir_all(&self.root)?;

        #[cfg(target_os = "linux")]
        fs::create_dir_all(self.runtime_dir())?;

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

    pub fn content_dir(&self) -> PathBuf {
        self.root.join("content")
    }

    pub fn launcher_path(&self, exe_name: &OsStr) -> PathBuf {
        self.root.join(exe_name)
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

        copy_dir(src, &self.content_dir())?;
        Ok(())
    }

    /// Installs a materialized CEF runtime into the bundle.
    pub fn install_cef(&self, src: &Path) -> Result<(), BundleError> {
        copy_dir(src, &self.cef_dir())?;
        Ok(())
    }

    /// Writes the Linux launcher script for the bundle.
    #[cfg(target_os = "linux")]
    pub fn write_launcher(&self, exe_name: &OsStr) -> Result<(), BundleError> {
        let launcher = self.launcher_path(exe_name);

        let runtime_target = format!("runtime/{}", exe_name.to_string_lossy());

        // Optional library path override for non-standard runtime environments
        let extra_ld = std::env::var("KUROGANE_LD_LIBRARY_PATH").unwrap_or_default();

        let extra_ld_block = if extra_ld.is_empty() {
            String::new()
        } else {
            format!("export LD_LIBRARY_PATH=\"{extra_ld}:${{LD_LIBRARY_PATH:-}}\"\n")
        };

        let script = format!(
            r#"#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
cd "$ROOT"

{extra_ld_block}exec "$ROOT/{runtime_target}" "$@"
"#
        );

        fs::write(&launcher, script)?;

        let mut perms = fs::metadata(&launcher)?.permissions();

        perms.set_mode(0o755);

        fs::set_permissions(&launcher, perms)?;

        Ok(())
    }

    /// Materializes a resolved distribution into this bundle layout.
    ///
    /// Copies the executable, CEF runtime, frontend and any extra resources
    /// into the platform-specific directory structure.
    pub fn materialize(&self, dist: &ResolvedDistribution) -> Result<(), BundleError> {
        self.prepare()?;

        let exe_name = dist
            .executable
            .file_name()
            .ok_or_else(|| BundleError::InvalidExecutablePath(dist.executable.clone()))?;

        fs::copy(&dist.executable, self.executable_path(exe_name))?;

        #[cfg(target_os = "linux")]
        self.write_launcher(exe_name)?;

        self.install_cef(&dist.cef_runtime)?;

        if let Some(frontend) = &dist.frontend {
            self.install_frontend(frontend)?;
        }

        for resource in &dist.extra_resources {
            let dest = self.root.join(&resource.destination);
            if resource.source.is_dir() {
                copy_dir(&resource.source, &dest)?;
            } else {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&resource.source, &dest)?;
            }
        }

        Ok(())
    }

    /// Verifies that the bundle contains a valid executable, content and CEF runtime.
    pub fn verify(&self, exe_name: &OsStr) -> Result<(), BundleError> {
        let exe = self.executable_path(exe_name);

        if !exe.exists() {
            return Err(BundleError::MissingExecutable(exe));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppMetadata, ResolvedDistribution};
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    fn create_cef_fixture(dir: &Path) -> PathBuf {
        crate::cef::write_runtime_fixture(&dir.join("cef"))
    }

    /// Platform-correct executable file name.
    fn test_exe_name() -> &'static OsStr {
        if cfg!(target_os = "windows") {
            OsStr::new("myapp.exe")
        } else {
            OsStr::new("myapp")
        }
    }

    fn valid_distribution(dir: &Path) -> ResolvedDistribution {
        #[cfg(target_os = "windows")]
        let exe_name = "myapp.exe";
        #[cfg(not(target_os = "windows"))]
        let exe_name = "myapp";

        let exe = dir.join(exe_name);
        fs::write(&exe, "binary").unwrap();

        let frontend = dir.join("frontend/dist");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(frontend.join("index.html"), "<html></html>").unwrap();

        let cef = create_cef_fixture(dir);

        ResolvedDistribution {
            metadata: AppMetadata {
                name: "myapp".to_string(),
                version: "1.0.0".to_string(),
                exe_name: exe_name.to_string(),
                ..Default::default()
            },
            executable: exe,
            frontend: Some(frontend),
            cef_runtime: cef,
            extra_resources: Vec::new(),
        }
    }

    #[test]
    fn materialize_copies_executable() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);

        layout.materialize(&dist).unwrap();

        let exe = layout.executable_path(test_exe_name());
        assert!(exe.exists(), "executable should exist after materialize");
    }

    #[test]
    fn materialize_copies_cef() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
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
    fn materialize_copies_frontend() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);

        layout.materialize(&dist).unwrap();

        let index = layout.content_dir().join("index.html");
        assert!(index.exists(), "index.html should be present");
    }

    #[test]
    fn materialize_copies_extra_resources() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());

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
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
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
        let dir = tmp();
        let dist = valid_distribution(dir.path());
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
        let dir = tmp();
        let dist = valid_distribution(dir.path());
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
            content.contains("cd \"$ROOT\""),
            "launcher should anchor execution at the bundle root"
        );
        assert!(
            !content.contains("runtime/cef"),
            "launcher must not set LD_LIBRARY_PATH for CEF"
        );
    }

    #[test]
    fn verify_passes_with_valid_bundle() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);

        layout.materialize(&dist).unwrap();
        assert!(layout.verify(test_exe_name()).is_ok());
    }

    #[test]
    fn verify_requires_content_index_when_content_dir_exists() {
        let dir = tmp();
        let mut dist = valid_distribution(dir.path());
        dist.frontend = None;

        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);
        layout.materialize(&dist).unwrap();

        assert!(
            layout.verify(test_exe_name()).is_ok(),
            "verify should pass when content/ does not exist"
        );

        fs::create_dir(layout.content_dir()).unwrap();
        let result = layout.verify(test_exe_name());
        assert!(
            result.is_err(),
            "verify should fail when content/ exists without index.html"
        );
    }

    #[test]
    fn verify_fails_without_executable() {
        let dir = tmp();
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);
        fs::create_dir_all(layout.content_dir()).unwrap();
        fs::write(layout.content_dir().join("index.html"), "").unwrap();

        crate::cef::write_runtime_fixture(&layout.cef_dir());

        let result = layout.verify(test_exe_name());
        assert!(
            result.is_err(),
            "verify should fail when executable is missing"
        );
    }

    #[test]
    fn verify_fails_with_incomplete_cef_runtime() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());
        let out = dir.path().join("out");
        let layout = BundleLayout::new(&out);
        layout.materialize(&dist).unwrap();

        // Mess shit up
        fs::remove_file(layout.cef_dir().join("icudtl.dat")).unwrap();

        assert!(
            layout.verify(test_exe_name()).is_err(),
            "verify should fail when the CEF runtime is incomplete"
        );
    }

    #[test]
    fn exe_name_matches_executable_filename() {
        let dir = tmp();
        let dist = valid_distribution(dir.path());

        let actual_filename = dist.executable.file_name().unwrap().to_str().unwrap();

        assert_eq!(
            dist.metadata.exe_name, actual_filename,
            "exe_name should match the actual executable filename"
        );
    }
}
