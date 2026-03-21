use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum RuntimeError {
    AssetRootNotSet,
    AssetRootMissing(std::path::PathBuf),
    CefInitializeFailed,
    CefNotInstalled,
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::AssetRootNotSet => write!(
                f,
                "No frontend assets were configured.

You attempted to launch a local application, but no asset directory was set.

Possible fixes:
  - Run using App::new(\"demo\") inside a project containing examples/demo/index.html
  - Set environment variable CEF_APP_PATH to your frontend directory
  - Use a dev server URL: App::new(\"http://localhost:8000\")"
            ),

            RuntimeError::AssetRootMissing(p) => write!(
                f,
                "Frontend directory does not exist:

  {}

Ensure your frontend build output exists before launching the runtime.",
                p.display()
            ),

            RuntimeError::CefInitializeFailed => write!(
                f,
                "Chromium Embedded Framework failed to initialize.

This usually means required CEF resources (locales, icudtl.dat, snapshot blobs)
are missing next to the executable."
            ),

            RuntimeError::CefNotInstalled => write!(
                f,
                "Chromium Embedded Framework is not installed.

Install it with:

    cargo run -p rust-cef-installer

Then run your application again."
            ),
        }
    }
}

impl std::error::Error for RuntimeError {}
