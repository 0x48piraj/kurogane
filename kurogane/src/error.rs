use std::fmt::{Display, Formatter};
use std::path::PathBuf;

#[derive(Debug)]
pub enum RuntimeError {
    InvalidAssetRoot(PathBuf),
    AssetRootMissing(PathBuf),
    AssetRootUnavailable {
        path: PathBuf,
        source: std::io::Error,
    },

    CefInitializeFailed,
    CefNotInstalled,
    InvalidCefInstallation(String),
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::InvalidAssetRoot(path) => write!(
                f,
                concat!(
                    "Invalid frontend directory:\n\n",
                    "  {}\n\n",
                    "The path exists but is not a directory.\n\n",
                    "Ensure you pass a directory containing your frontend build (with index.html)."
                ),
                path.display()
            ),

            RuntimeError::AssetRootMissing(path) => write!(
                f,
                concat!(
                    "Frontend directory does not exist:\n\n",
                    "  {}\n\n",
                    "Possible fixes:\n",
                    "  - Make sure your app is using App::new(\"your-frontend-directory\")\n",
                    "  - Use a dev server URL: App::url(\"http://your-dev-server\")\n\n",
                    "Make sure your frontend build exists and contains index.html."
                ),
                path.display()
            ),

            RuntimeError::AssetRootUnavailable { path, source } => write!(
                f,
                concat!(
                    "Unable to access frontend directory:\n\n",
                    "  {}\n\n",
                    "OS error:\n",
                    "  {}\n\n",
                    "Check filesystem permissions and ensure the path is accessible."
                ),
                path.display(),
                source,
            ),

            RuntimeError::CefInitializeFailed => write!(
                f,
                concat!(
                    "Chromium failed to initialize.\n\n",
                    "This usually means required CEF resources are missing next to the executable."
                )
            ),

            RuntimeError::CefNotInstalled => write!(
                f,
                concat!(
                    "Chromium is not installed.\n\n",
                    "Install it with:\n\n",
                    "  kurogane install\n\n",
                    "Then run your application again."
                )
            ),

            RuntimeError::InvalidCefInstallation(reason) => write!(
                f,
                concat!(
                    "Chromium installation is invalid.\n\n",
                    "Reason:\n",
                    "  {}\n\n",
                    "Try reinstalling Chromium:\n\n",
                    "  kurogane install"
                ),
                reason
            ),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RuntimeError::AssetRootUnavailable { source, .. } => Some(source),
            _ => None,
        }
    }
}
