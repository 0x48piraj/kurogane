//! Internal frontend resolution logic.

use std::path::PathBuf;
use cef::CefString;

use super::Source;

use crate::error::RuntimeError;

/// Resolve the frontend entrypoint.
///
/// Priority:
/// 1. Explicit URL (App::url)
/// 2. CEF_DEV_URL (live dev server)
/// 3. CEF_APP_PATH (custom frontend directory)
/// 4. Explicit path via App::new (must contain index.html)
///
/// Errors if no valid frontend is found.
pub(crate) fn resolve(source: &Source) -> Result<(Option<PathBuf>, CefString), RuntimeError> {

    const APP_URL: &str = "app://app/index.html";

    // Explicit URL via App::url (dev server or remote site)
    if let Source::Url(url) = source {
        return Ok((None, CefString::from(url.as_str())));
    }

    // Dev server override
    if let Ok(url) = std::env::var("CEF_DEV_URL") {
        return Ok((None, CefString::from(url.as_str())));
    }

    // Explicit directory override
    if let Ok(path) = std::env::var("CEF_APP_PATH") {
        let dir = PathBuf::from(path);

        if dir.join("index.html").exists() {
            return Ok((Some(dir), CefString::from(APP_URL)));
        } else {
            return Err(RuntimeError::AssetRootMissing(dir));
        }
    }

    // Explicit path via App::new
    if let Source::Path(dir) = source {
        let dir = if dir.is_absolute() {
            dir.clone()
        } else {
            std::env::current_dir()
                .map_err(|_| RuntimeError::AssetRootMissing(dir.clone()))?
                .join(dir)
        };

        if dir.join("index.html").exists() {
            return Ok((Some(dir), CefString::from(APP_URL)));
        } else {
            return Err(RuntimeError::AssetRootMissing(dir));
        }
    }

    let fallback = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("content");

    Err(RuntimeError::AssetRootMissing(fallback))
}
