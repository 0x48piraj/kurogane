use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

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

    pub fn cef_dir(&self) -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            self.root.clone()
        }

        #[cfg(target_os = "linux")]
        {
            self.runtime_dir().join("cef")
        }

        #[cfg(target_os = "macos")]
        {
            self.root.clone()
        }
    }

    pub fn content_dir(&self) -> PathBuf {
        self.root.join("content")
    }

    pub fn launcher_path(&self, exe_name: &OsStr) -> PathBuf {
        self.root.join(exe_name)
    }

    pub fn executable_path(&self, exe_name: &OsStr) -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            self.root.join(exe_name)
        }

        #[cfg(target_os = "linux")]
        {
            self.runtime_dir().join(exe_name)
        }

        #[cfg(target_os = "macos")]
        {
            self.root.join(exe_name)
        }
    }

    pub fn install_frontend(&self, src: &Path) -> Result<()> {
        if !src.exists() {
            anyhow::bail!("frontend directory missing");
        }

        copy_dir(src, &self.content_dir())
    }

    /// Installs the Chromium Embedded Framework runtime
    /// into the bundle.
    ///
    /// Platform layout differs intentionally:
    ///
    /// - Windows places CEF beside the executable because
    ///   the Windows loader searches the executable directory
    ///   for DLL dependencies automatically.
    ///
    /// - Linux places CEF inside runtime/cef/ and relies on
    ///   RPATH ($ORIGIN/cef) plus a lightweight launcher script.
    ///   This keeps the runtime self-contained while avoiding
    ///   global configuration (no PATH hacks or env vars).
    ///
    /// Linux may require chrome-sandbox to have setuid permissions
    /// for proper Chromium sandboxing support.
    pub fn install_cef(&self, src: &Path) -> Result<()> {
        copy_dir(src, &self.cef_dir())?;

        #[cfg(target_os = "linux")]
        {
            // Sandbox permissions (required by CEF)
            let sandbox = self.cef_dir().join("chrome-sandbox");
            let _ = std::process::Command::new("chmod")
                .arg("4755")
                .arg(&sandbox)
                .status();
        }

        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn write_launcher(&self, exe_name: &OsStr) -> Result<()> {
        let launcher = self.launcher_path(exe_name);

        let runtime_target = format!("runtime/{}", exe_name.to_string_lossy());

        // Optional extra runtime libraries (for NixOS runtime closures)
        let extra_ld = std::env::var("KUROGANE_LD_LIBRARY_PATH").unwrap_or_default();

        let script = format!(
r#"#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

if [ -n "{extra_ld}" ]; then
    export LD_LIBRARY_PATH="{extra_ld}:$ROOT/runtime/cef${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}"
else
    export LD_LIBRARY_PATH="$ROOT/runtime/cef${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}"
fi

exec "$ROOT/{runtime_target}" "$@"
"#,
        );

        fs::write(&launcher, script)?;

        let mut perms = fs::metadata(&launcher)?.permissions();

        perms.set_mode(0o755);

        fs::set_permissions(&launcher, perms)?;

        Ok(())
    }

    pub fn verify(&self, exe_name: &OsStr) -> Result<()> {
        let exe = self.executable_path(exe_name);

        if !exe.exists() {
            anyhow::bail!("bundle executable missing");
        }

        let index = self.content_dir().join("index.html");

        if !index.exists() {
            anyhow::bail!("content/index.html missing");
        }

        #[cfg(target_os = "windows")]
        {
            if !self.cef_dir().join("libcef.dll").exists() {
                anyhow::bail!("libcef.dll missing");
            }
        }

        #[cfg(target_os = "linux")]
        {
            if !self.cef_dir().join("libcef.so").exists() {
                anyhow::bail!("libcef.so missing");
            }
        }

        Ok(())
    }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;

        let path = entry.path();

        let dest = dst.join(entry.file_name());

        if path.is_dir() {
            copy_dir(&path, &dest)?;
        } else {
            fs::copy(&path, &dest)?;
        }
    }

    Ok(())
}
