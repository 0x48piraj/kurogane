use std::fmt::{Display, Formatter};
use std::path::PathBuf;

#[derive(Debug)]
#[non_exhaustive]
pub enum RuntimeError {
    InvalidAssetRoot(PathBuf),
    InvalidFrontendUrl(String),
    AssetRootMissing(PathBuf),
    AssetRootUnavailable {
        path: PathBuf,
        source: std::io::Error,
    },

    CefInitializeFailed,
    CefNotInstalled,
    InvalidCefInstallation(String),

    CacheUnavailable {
        path: PathBuf,
        source: std::io::Error,
    },

    BrowserCreationFailed,
    WindowCreationFailed,

    /// The requested sandbox cannot be enforced on this platform or layout.
    SandboxUnsupported {
        reason: String,
    },

    /// The requested sandbox is supported but not usable on this machine.
    SandboxUnavailable {
        reason: String,
    },
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

            RuntimeError::InvalidFrontendUrl(url) => write!(
                f,
                concat!(
                    "Invalid URL:\n\n",
                    "  {}\n\n",
                    "Use a fully qualified URL such as:\n\n",
                    "  http://localhost:3000\n",
                    "  https://example.com"
                ),
                url
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

            RuntimeError::CacheUnavailable { path, source } => write!(
                f,
                concat!(
                    "Unable to create cache directory:\n\n",
                    "  {}\n\n",
                    "OS error:\n",
                    "  {}\n\n",
                    "Check filesystem permissions or free up disk space."
                ),
                path.display(),
                source,
            ),

            RuntimeError::BrowserCreationFailed => write!(
                f,
                concat!(
                    "Failed to create browser.\n\n",
                    "This usually indicates a Chromium internal error."
                )
            ),

            RuntimeError::WindowCreationFailed => write!(
                f,
                concat!(
                    "Failed to create window.\n\n",
                    "This usually indicates a Chromium internal error."
                )
            ),

            RuntimeError::SandboxUnsupported { reason } => write!(
                f,
                concat!(
                    "Chromium sandbox is not supported here.\n\n",
                    "Reason:\n",
                    "  {}\n\n",
                    "Use SandboxMode::Disabled (the default) in this environment."
                ),
                reason
            ),

            RuntimeError::SandboxUnavailable { reason } => write!(
                f,
                "Chromium sandbox is unavailable.\n\n{}",
                reason
            ),
        }
    }
}

impl std::error::Error for RuntimeError {
    /// Returns the underlying error that caused this runtime error, when available.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RuntimeError::AssetRootUnavailable { source, .. }
            | RuntimeError::CacheUnavailable { source, .. } => Some(source),

            RuntimeError::InvalidAssetRoot(_)
            | RuntimeError::InvalidFrontendUrl(_)
            | RuntimeError::AssetRootMissing(_)
            | RuntimeError::CefInitializeFailed
            | RuntimeError::CefNotInstalled
            | RuntimeError::InvalidCefInstallation(_)
            | RuntimeError::BrowserCreationFailed
            | RuntimeError::WindowCreationFailed
            | RuntimeError::SandboxUnsupported { .. }
            | RuntimeError::SandboxUnavailable { .. } => None,
        }
    }
}
