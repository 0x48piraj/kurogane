//! Internal frontend resolution logic.

use std::path::PathBuf;
use cef::CefString;

use super::Source;

/// Resolve the frontend entrypoint.
///
/// Returns:
/// (optional asset root, start URL)
///
/// Priority:
/// 1. CEF_DEV_URL (live dev server)
/// 2. CEF_APP_PATH (custom frontend directory)
/// 3. examples/<name>/index.html (cargo run)
/// 4. assets/index.html next to the executable (release)
pub(crate) fn resolve(source: &Source) -> (Option<PathBuf>, CefString) {

    // Explicit URL (dev server or remote site)
    if let Source::Url(url) = source {
        return (None, CefString::from(url.as_str()));
    }

    // Dev override
    if let Ok(url) = std::env::var("CEF_DEV_URL") {
        return (None, CefString::from(url.as_str()));
    }

    // Explicit directory override
    if let Ok(path) = std::env::var("CEF_APP_PATH") {
        let dir = PathBuf::from(path);
        return (Some(dir), CefString::from("app://app/index.html"));
    }

    match source {
        Source::Path(dir) => {
            return (Some(dir.clone()), CefString::from("app://app/index.html"));
        }

        Source::Name(name) => {
            // Cargo dev environment
            if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
                let dir = PathBuf::from(manifest_dir)
                    .join("examples")
                    .join(name);

                if dir.join("index.html").exists() {
                    return (Some(dir), CefString::from("app://app/index.html"));
                }
            }

            // Packaged release
            let exe = std::env::current_exe().unwrap();
            let dir = exe.parent().unwrap().join("assets");

            (Some(dir), CefString::from("app://app/index.html"))
        }

        Source::Url(_) => unreachable!(),
    }
}
