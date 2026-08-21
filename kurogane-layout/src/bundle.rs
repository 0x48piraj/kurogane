//! Application bundle layout and materialization.
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

use anyhow::Result;

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

use crate::{ResolvedDistribution, layout::copy_dir};

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

    pub fn prepare(&self) -> Result<()> {
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

    pub fn install_frontend(&self, src: &Path) -> Result<()> {
        if !src.exists() {
            anyhow::bail!("frontend directory missing");
        }

        copy_dir(src, &self.content_dir())?;
        Ok(())
    }

    /// Installs a materialized CEF runtime into the bundle.
    pub fn install_cef(&self, src: &Path) -> Result<()> {
        copy_dir(src, &self.cef_dir())?;
        Ok(())
    }

    /// Writes the Linux launcher script for the bundle.
    #[cfg(target_os = "linux")]
    pub fn write_launcher(&self, exe_name: &OsStr) -> Result<()> {
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
    pub fn materialize(&self, dist: &ResolvedDistribution) -> Result<()> {
        self.prepare()?;

        let exe_name = dist
            .executable
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("executable has no file name"))?;

        fs::copy(&dist.executable, self.executable_path(exe_name))?;

        #[cfg(target_os = "linux")]
        self.write_launcher(exe_name)?;

        self.install_cef(&dist.cef_runtime)?;

        if let Some(frontend) = &dist.frontend {
            self.install_frontend(frontend)?;
        }

        for resource in &dist.extra_resources {
            let dest = self.root.join(
                resource
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("resource has no file name"))?,
            );
            if resource.is_dir() {
                copy_dir(resource, &dest)?;
            } else {
                fs::copy(resource, &dest)?;
            }
        }

        Ok(())
    }

    /// Verifies that the bundle contains a valid executable, content and CEF runtime.
    pub fn verify(&self, exe_name: &OsStr) -> Result<()> {
        let exe = self.executable_path(exe_name);

        if !exe.exists() {
            anyhow::bail!("bundle executable missing");
        }

        if self.content_dir().exists() {
            let index = self.content_dir().join("index.html");

            if !index.exists() {
                anyhow::bail!("content/index.html missing");
            }
        }

        crate::cef::validate_cef_runtime(&self.cef_dir())
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        Ok(())
    }
}
