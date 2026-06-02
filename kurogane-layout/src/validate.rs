use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CefValidationError {
    #[error("CEF root does not exist")]
    MissingRoot,

    #[error("missing required file: {0}")]
    MissingFile(&'static str),
}

pub fn validate_cef_root(root: &Path) -> Result<(), CefValidationError> {
    if !root.exists() {
        return Err(CefValidationError::MissingRoot);
    }

    #[cfg(target_os = "windows")]
    {
        require(root, "libcef.dll")?;
        require(root, "locales")?;
        require(root, "chrome_elf.dll")?;
        require(root, "icudtl.dat")?;
    }

    #[cfg(target_os = "linux")]
    {
        require(root, "libcef.so")?;
        require(root, "locales")?;
        require(root, "icudtl.dat")?;
        require(root, "chrome-sandbox")?;
    }

    #[cfg(target_os = "macos")]
    {
        require(root, "Chromium Embedded Framework.framework")?;
    }

    Ok(())
}

fn require(root: &Path, name: &'static str) -> Result<(), CefValidationError> {
    if root.join(name).exists() {
        Ok(())
    } else {
        Err(CefValidationError::MissingFile(name))
    }
}
