use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use crate::capability::FsConfigError;

#[derive(Debug)]
#[non_exhaustive]
pub enum RuntimeError {
    InvalidAssetRoot(PathBuf),
    /// The frontend URL could not be parsed; `source` says why.
    InvalidFrontendUrl {
        url: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    AssetRootMissing(PathBuf),
    /// The frontend directory exists but has no `index.html` to serve; none,
    /// a directory by that name, or a link out of the directory.
    EntrypointMissing(PathBuf),
    AssetRootUnavailable {
        path: PathBuf,
        source: std::io::Error,
    },

    CefInitializeFailed,
    CefNotInstalled,
    /// The Chromium runtime at `path` cannot be used; `source` says why.
    InvalidCefInstallation {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The path to the running executable could not be determined.
    ExecutableUnavailable(std::io::Error),

    CacheUnavailable {
        path: PathBuf,
        source: std::io::Error,
    },

    BrowserCreationFailed,
    WindowCreationFailed,

    /// The window given to
    /// [`AppInstance::create_child_browser`](crate::AppInstance::create_child_browser)
    /// cannot parent a browser: CEF embeds only in a Win32 window, an AppKit
    /// view or an X11 window, never, for one, in a Wayland surface.
    UnsupportedParentWindow,

    /// The requested sandbox cannot be enforced on this platform or layout.
    SandboxUnsupported {
        reason: String,
    },

    /// The requested sandbox is supported but not usable on this machine.
    SandboxUnavailable {
        reason: String,
    },

    /// The [`App`](crate::App) builder was misconfigured. Every problem is
    /// listed; nothing was started.
    InvalidConfiguration(Vec<ConfigError>),

    /// The filesystem configuration passed to
    /// [`App::filesystem`](crate::App::filesystem) was rejected; nothing was
    /// started.
    InvalidFilesystem(FsConfigError),
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

            RuntimeError::InvalidFrontendUrl { url, .. } => write!(
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

            RuntimeError::EntrypointMissing(path) => write!(
                f,
                concat!(
                    "Frontend directory has no index.html:\n\n",
                    "  {}\n\n",
                    "Build your frontend into this directory, or pass the directory ",
                    "that holds its index.html to App::new."
                ),
                path.display()
            ),

            RuntimeError::AssetRootUnavailable { path, .. } => write!(
                f,
                concat!(
                    "Unable to access the frontend:\n\n",
                    "  {}\n\n",
                    "Check filesystem permissions and ensure the path is accessible."
                ),
                path.display(),
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

            RuntimeError::InvalidCefInstallation { path, .. } => write!(
                f,
                concat!(
                    "Chromium installation is invalid:\n\n",
                    "  {}\n\n",
                    "Try reinstalling Chromium:\n\n",
                    "  kurogane install"
                ),
                path.display()
            ),

            RuntimeError::ExecutableUnavailable(_) => write!(
                f,
                concat!(
                    "Unable to locate the running executable.\n\n",
                    "Kurogane finds its Chromium runtime and profile cache from the executable's path."
                )
            ),

            RuntimeError::CacheUnavailable { path, .. } => write!(
                f,
                concat!(
                    "Unable to create cache directory:\n\n",
                    "  {}\n\n",
                    "Check filesystem permissions or free up disk space."
                ),
                path.display(),
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

            RuntimeError::UnsupportedParentWindow => write!(
                f,
                concat!(
                    "The window cannot host a Chromium browser.\n\n",
                    "CEF embeds a browser in a Win32 window, an AppKit view or an X11 window. ",
                    "On Linux, ask winit for X11 (with_x11()); a Wayland session runs it under XWayland."
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

            RuntimeError::SandboxUnavailable { reason } => {
                write!(f, "Chromium sandbox is unavailable.\n\n{}", reason)
            }

            RuntimeError::InvalidConfiguration(problems) => {
                f.write_str("Invalid application configuration:\n\n")?;
                for problem in problems {
                    writeln!(f, "  - {problem}")?;
                }
                f.write_str("\nNothing was started. Fix the builder calls above.")
            }

            RuntimeError::InvalidFilesystem(_) => {
                f.write_str("Invalid filesystem configuration. Nothing was started.")
            }
        }
    }
}

/// `Display` names the failure and `source()` holds its cause, so a report
/// that walks the chain ([`App::run_or_exit`](crate::App::run_or_exit) does)
/// shows each cause once.
impl std::error::Error for RuntimeError {
    /// Returns the underlying error that caused this runtime error, when available.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RuntimeError::AssetRootUnavailable { source, .. }
            | RuntimeError::CacheUnavailable { source, .. } => Some(source),

            RuntimeError::ExecutableUnavailable(source) => Some(source),

            RuntimeError::InvalidFrontendUrl { source, .. }
            | RuntimeError::InvalidCefInstallation { source, .. } => Some(&**source),

            RuntimeError::InvalidFilesystem(error) => Some(error),

            RuntimeError::InvalidAssetRoot(_)
            | RuntimeError::AssetRootMissing(_)
            | RuntimeError::EntrypointMissing(_)
            | RuntimeError::CefInitializeFailed
            | RuntimeError::CefNotInstalled
            | RuntimeError::BrowserCreationFailed
            | RuntimeError::WindowCreationFailed
            | RuntimeError::UnsupportedParentWindow
            | RuntimeError::SandboxUnsupported { .. }
            | RuntimeError::SandboxUnavailable { .. }
            | RuntimeError::InvalidConfiguration(_) => None,
        }
    }
}

/// One problem in the [`App`](crate::App) builder configuration, reported
/// by `build()` / `run()` / `start_embedded()` as [`RuntimeError::InvalidConfiguration`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigError {
    /// Two handlers share a name. Commands, async and binary commands,
    /// streams and capability commands share one namespace.
    DuplicateHandler(String),
    /// A custom scheme name is invalid or reserved.
    InvalidScheme { name: String, reason: &'static str },
    /// Two custom schemes share a name.
    DuplicateScheme(String),
    /// An ACL rule and a native capability both claim a command.
    CapabilityCommand(String),
    /// An ACL rule names the opaque origin, which would match every frame
    /// without a host.
    OpaqueOrigin(String),
    /// [`App::run`](crate::App::run) was given a pump scheduler, which requires
    /// the application to drive CEF's message loop itself.
    SchedulerWithRunLoop,
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::DuplicateHandler(name) => {
                write!(f, "handler '{name}' is registered twice")
            }
            ConfigError::InvalidScheme { name, reason } => {
                write!(f, "invalid custom scheme '{name}': {reason}")
            }
            ConfigError::DuplicateScheme(name) => {
                write!(f, "custom scheme '{name}' is registered twice")
            }
            ConfigError::CapabilityCommand(name) => write!(
                f,
                "'{name}' is authorized by a native capability: grant access with \
                 Filesystem::grant, not App::permit"
            ),
            ConfigError::OpaqueOrigin(name) => write!(
                f,
                "the rule for '{name}' names the opaque origin, which matches every frame without a host"
            ),
            ConfigError::SchedulerWithRunLoop => f.write_str(
                "App::scheduler is for an application that pumps CEF from its own loop: \
                 start it with App::start or App::start_embedded, not App::run",
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn a_cause_is_reported_once() {
        let denied = || std::io::Error::new(std::io::ErrorKind::PermissionDenied, "no entry");
        let errors = [
            RuntimeError::AssetRootUnavailable {
                path: "web".into(),
                source: denied(),
            },
            RuntimeError::ExecutableUnavailable(denied()),
            RuntimeError::CacheUnavailable {
                path: "cache".into(),
                source: denied(),
            },
            RuntimeError::InvalidCefInstallation {
                path: "cef".into(),
                source: Box::new(denied()),
            },
            RuntimeError::InvalidFrontendUrl {
                url: "not a url".into(),
                source: Box::new(denied()),
            },
        ];
        for error in errors {
            assert!(!error.to_string().contains("no entry"), "{error}");
            assert_eq!(
                error.source().map(ToString::to_string).as_deref(),
                Some("no entry"),
                "{error}"
            );
        }
    }
}
