//! Typed errors of the capability layer.
//!
//! The security class of a failure is a variant, never an `io::ErrorKind`
//! inferred after the fact: an OS `EACCES` inside the grant is [`FsError::Io`],
//! not a scope denial. Denials carry no path data at all; each renders one
//! fixed, class-level message, so it cannot echo the request.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::ipc::{ErrorCode, IpcError};

/// Why an in-grant request was refused. Each variant renders one fixed,
/// class-level message.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    /// The request lies outside every allow root that carries the capability,
    /// or `..` pops above its anchor.
    OutsideRoots,
    /// The request matches a deny rule of one of the origin's scopes.
    DenyRule,
    /// The opened object (or a mutation's target) is located in a denied
    /// subtree, or is a link / reparse point.
    ObjectLocation,
}

impl Denial {
    /// The class-level message surfaced to the renderer.
    pub const fn message(self) -> &'static str {
        match self {
            Denial::OutsideRoots => "path is not within any allowed root",
            Denial::DenyRule => "path is denied by the scope's deny patterns",
            Denial::ObjectLocation => "filesystem access denied",
        }
    }
}

/// Failure of one filesystem operation performed for an origin.
#[non_exhaustive]
#[derive(Debug)]
pub enum FsError {
    /// No grant of the origin carries the capability the operation needs.
    CapabilityDenied,
    /// A grant carries the capability, but not for this path.
    PathDenied(Denial),
    /// The request path is malformed. The reason is a category, never the path.
    InvalidPath(&'static str),
    /// The file is larger than the transfer limit
    /// ([`FilesystemBuilder::max_file_size`](crate::capability::FilesystemBuilder::max_file_size)).
    TooLarge { limit: u64 },
    /// The authorized operation failed in the OS. The caller is inside the
    /// grant for this object, so the OS detail may be shown.
    Io(io::Error),
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::CapabilityDenied => f.write_str("capability not granted for this origin"),
            FsError::PathDenied(denial) => f.write_str(denial.message()),
            FsError::InvalidPath(reason) => f.write_str(reason),
            FsError::TooLarge { limit } => {
                write!(f, "file is larger than the {limit}-byte transfer limit")
            }
            FsError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FsError::Io(e) => Some(e),
            FsError::CapabilityDenied
            | FsError::PathDenied(_)
            | FsError::InvalidPath(_)
            | FsError::TooLarge { .. } => None,
        }
    }
}

impl From<io::Error> for FsError {
    fn from(e: io::Error) -> Self {
        FsError::Io(e)
    }
}

impl From<FsError> for IpcError {
    fn from(e: FsError) -> Self {
        let code = match &e {
            FsError::CapabilityDenied => ErrorCode::Capability,
            FsError::PathDenied(_) => ErrorCode::PathDenied,
            FsError::InvalidPath(_) => ErrorCode::PathInvalid,
            FsError::TooLarge { .. } => ErrorCode::TooLarge,
            FsError::Io(_) => ErrorCode::Handler,
        };
        IpcError::with_code(e.to_string(), code)
    }
}

/// A `Filesystem` configuration that cannot be built. These are
/// developer-facing and may name paths; they never reach a renderer.
#[non_exhaustive]
#[derive(Debug)]
pub enum FsConfigError {
    /// A grant uses a [`ScopeId`](crate::capability::ScopeId) declared on
    /// another builder.
    ForeignScope,
    /// A grant targets the opaque origin, which would match every frame
    /// without a host.
    OpaqueOrigin,
    /// An allow root cannot be opened as a directory.
    Root { path: PathBuf, source: io::Error },
    /// A `deny_path` lies outside every allow root of its scope.
    DenyOutsideRoots { scope: String, path: PathBuf },
    /// A `deny_glob` pattern is malformed.
    InvalidGlob {
        scope: String,
        pattern: String,
        reason: &'static str,
    },
}

impl fmt::Display for FsConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsConfigError::ForeignScope => {
                f.write_str("a grant uses a scope declared on another FilesystemBuilder")
            }
            FsConfigError::OpaqueOrigin => f.write_str("the opaque origin cannot hold a grant"),
            FsConfigError::Root { path, .. } => {
                write!(f, "cannot open allow root {}", path.display())
            }
            FsConfigError::DenyOutsideRoots { scope, path } => write!(
                f,
                "deny path {} is outside every allow root of scope '{scope}'",
                path.display()
            ),
            FsConfigError::InvalidGlob {
                scope,
                pattern,
                reason,
            } => {
                write!(
                    f,
                    "invalid deny glob '{pattern}' in scope '{scope}': {reason}"
                )
            }
        }
    }
}

impl std::error::Error for FsConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FsConfigError::Root { source, .. } => Some(source),
            FsConfigError::ForeignScope
            | FsConfigError::OpaqueOrigin
            | FsConfigError::DenyOutsideRoots { .. }
            | FsConfigError::InvalidGlob { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_class_maps_to_its_ipc_code() {
        let cases = [
            (FsError::CapabilityDenied, ErrorCode::Capability),
            (FsError::PathDenied(Denial::DenyRule), ErrorCode::PathDenied),
            (
                FsError::InvalidPath("path is empty"),
                ErrorCode::PathInvalid,
            ),
            (FsError::TooLarge { limit: 4 }, ErrorCode::TooLarge),
            (
                FsError::Io(io::Error::from(io::ErrorKind::PermissionDenied)),
                ErrorCode::Handler,
            ),
        ];
        for (err, code) in cases {
            assert_eq!(IpcError::from(err).code(), code);
        }
    }

    #[test]
    fn os_permission_errors_are_not_scope_denials() {
        let err = IpcError::from(FsError::from(io::Error::from(
            io::ErrorKind::PermissionDenied,
        )));
        assert_ne!(err.code(), ErrorCode::PathDenied);
    }
}
