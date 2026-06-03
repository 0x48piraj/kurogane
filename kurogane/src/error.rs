use std::fmt::{Display, Formatter};
use std::path::PathBuf;

#[derive(Debug)]
pub enum RuntimeError {
    InvalidAssetRoot(PathBuf),
    AssetRootMissing(PathBuf),
    CefInitializeFailed,
    CefNotInstalled,
    InvalidCefInstallation(String),
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

impl std::error::Error for RuntimeError {}

/// Errors returned when resolving an app:// request.
/// Each variant maps to an HTTP status code.
#[derive(Debug)]
pub enum ResolveError {
    /// The URL could not be parsed, or its scheme is not app
    InvalidUrl,
    /// The configured asset root is invalid (exists but is not a directory)
    InvalidRoot(PathBuf),
    /// The resolved path escapes the asset root (path-traversal attempt)
    Forbidden(PathBuf),
    /// The path is inside the root but the file does not exist
    NotFound(PathBuf),
    /// An I/O error occurred after validation
    Io(std::io::Error),
}

impl ResolveError {
    pub fn http_status(&self) -> i32 {
        match self {
            Self::InvalidUrl => 400,
            Self::InvalidRoot(_) => 500,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::Io(_) => 500,
        }
    }
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl => write!(f, "Invalid URL"),
            Self::InvalidRoot(p) => write!(f, "Invalid asset root: {}", p.display()),
            Self::Forbidden(p) => write!(f, "Forbidden: {}", p.display()),
            Self::NotFound(p) => write!(f, "Not found: {}", p.display()),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}
