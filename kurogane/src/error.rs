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

You attempted to launch a local application but no frontend backend was set.

Possible fixes:
  - Make sure your app is using App::new(\"your-frontend-directory\")
  - Use a dev server URL: App::url(\"http://your-dev-server\")
  - Set environment variable CEF_APP_PATH to your frontend directory"
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

    kurogane install

Then run your application again."
            ),
        }
    }
}

impl std::error::Error for RuntimeError {}
