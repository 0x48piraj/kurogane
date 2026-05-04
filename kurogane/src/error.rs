use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum RuntimeError {
    InvalidAssetRoot(std::path::PathBuf),
    AssetRootMissing(std::path::PathBuf),
    CefInitializeFailed,
    CefNotInstalled,
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::InvalidAssetRoot(p) => write!(
                f,
                concat!(
                    "Invalid frontend directory:\n\n",
                    "  {}\n\n",
                    "The path exists but is not a directory.\n\n",
                    "Ensure you pass a directory containing your frontend build (with index.html)."
                ),
                p.display()
            ),

            RuntimeError::AssetRootMissing(p) => write!(
                f,
                concat!(
                    "Frontend directory does not exist:\n\n",
                    "  {}\n\n",
                    "Possible fixes:\n",
                    "  - Make sure your app is using App::new(\"your-frontend-directory\")\n",
                    "  - Use a dev server URL: App::url(\"http://your-dev-server\")\n\n",
                    "Make sure your frontend build exists and contains index.html."
                ),
                p.display()
            ),

            RuntimeError::CefInitializeFailed => write!(
                f,
                concat!(
                    "Chromium Embedded Framework failed to initialize.\n\n",
                    "This usually means required CEF resources are missing next to the executable."
                )
            ),

            RuntimeError::CefNotInstalled => write!(
                f,
                concat!(
                    "Chromium Embedded Framework is not installed.\n\n",
                    "Install it with:\n\n",
                    "  kurogane install\n\n",
                    "Then run your application again."
                )
            ),
        }
    }
}

impl std::error::Error for RuntimeError {}
