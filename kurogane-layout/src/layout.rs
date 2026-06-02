use std::path::{Path, PathBuf};

use crate::platform;

pub fn install_root() -> PathBuf {
    platform::data_local_dir().join("kurogane").join("cef")
}

pub fn cef_install_dir(version: &str) -> PathBuf {
    install_root().join(version)
}

/// Resolves a versioned managed CEF installation if it exists locally.
pub fn installed_cef_root(version: &str) -> Option<PathBuf> {
    let root = cef_install_dir(version);

    root.exists().then_some(root)
}

pub fn bundled_cef_root() -> Result<Option<PathBuf>, std::io::Error> {
    let exe = std::env::current_exe()?;

    let dir = exe.parent().unwrap_or(Path::new("."));

    #[cfg(target_os = "windows")]
    {
        // Windows bundle: CEF is flattened next to the exe.
        let libcef = dir.join("libcef.dll");

        if libcef.exists() {
            return Ok(Some(dir.to_path_buf()));
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: CEF lives in a cef/ subdirectory.
        let cef = dir.join("cef");

        if cef.exists() {
            return Ok(Some(cef));
        }
    }

    #[cfg(target_os = "macos")]
    {
        let framework = dir.join("Chromium Embedded Framework.framework");

        if framework.exists() {
            return Ok(Some(dir.to_path_buf()));
        }
    }

    Ok(None)
}
