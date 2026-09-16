//! Per-origin, per-operation filesystem capabilities for renderer code.
//!
//! Every `fs.*` call is subject to three independent checks:
//!
//! 1. Command: The command ACL permits the `fs.*` capability surface but
//!    neither grants nor revokes filesystem access.
//! 2. Operation: Each command performs one operation and requires the
//!    corresponding [`FsAccess`] bit in the origin's grants.
//! 3. Resource: The path must lie beneath an allow root carrying that bit
//!    and outside every deny rule held by the origin. The opened object's real
//!    location is checked again before use.
//!
//! Nothing is reachable without a grant. Links are never traversed, and
//! denials never disclose request contents.
//!
//! The public surface is [`Filesystem`], which defines scopes and grants and
//! is passed to [`App::filesystem`](crate::App::filesystem) to install the
//! `fs.*` commands. [`Filesystem::authorize`] produces the exact authority
//! granted to an origin for tests and headless checks.
//!
//! Modules:
//! - `policy`: capability bits and the `fs.*` command set.
//! - `path`: validated names, request parsing and comparison keys.
//! - `scope`: allow roots and deny rules.
//! - `safe`: platform-specific kernel-confined opening.
//! - `authorized`: filesystem configuration and per-origin authority.
//! - `commands`: IPC handlers.
//! - `audit`: cross-layer invariants (tests only).

mod authorized;
pub(crate) mod commands;
mod error;
// The `app://` asset scheme validates each URL segment as a Name
pub(crate) mod path;
pub(crate) mod policy;
mod safe;
mod scope;

#[cfg(all(test, any(target_os = "linux", windows, target_os = "macos")))]
mod audit;
#[cfg(all(test, any(target_os = "linux", windows, target_os = "macos")))]
mod test_support;

pub use authorized::{AuthorizedFs, Filesystem, FilesystemBuilder, ScopeId};
pub use error::{Denial, FsConfigError, FsError};
pub use policy::FsAccess;
pub use safe::{DirEntry, EntryKind};
pub use scope::ScopeBuilder;
