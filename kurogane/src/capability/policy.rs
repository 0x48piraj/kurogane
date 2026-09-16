//! Filesystem operations and the [`FsAccess`] bits required to perform them.
//!
//! [`FsAccess`] grants native filesystem operations. The command ACL is a
//! separate layer and never grants or revokes an [`FsAccess`] bit.
//!
//! Each [`FsCommand`] performs exactly one filesystem operation with the
//! required capability enforced by `authorized.rs`:
//!
//! ```
//! fs.read_file    READ
//! fs.write_file   WRITE (existing) | CREATE (new)
//! fs.size         METADATA
//! fs.create_dir   CREATE
//! fs.exists       METADATA
//! fs.remove_file  DELETE
//! fs.read_dir     LIST
//! fs.remove_dir   DELETE
//! fs.copy_file    READ (src) + CREATE (dst, never overwrites)
//! fs.rename_file  RENAME (src) + RENAME (dst)
//! ```
//!
//! Commands not listed here are not part of the filesystem capability.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

/// A set of operational filesystem capabilities.
///
/// The default is [`FsAccess::NONE`]; native access is granted without
/// restriction.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct FsAccess(u8);

impl FsAccess {
    /// No native filesystem capability.
    pub const NONE: FsAccess = FsAccess(0);
    /// Read a regular file's contents.
    pub const READ: FsAccess = FsAccess(1 << 0);
    /// Read metadata (`exists` / `size`), never contents.
    pub const METADATA: FsAccess = FsAccess(1 << 1);
    /// Enumerate a directory under the strict visibility policy.
    pub const LIST: FsAccess = FsAccess(1 << 2);
    /// Replace the contents of an existing file.
    pub const WRITE: FsAccess = FsAccess(1 << 3);
    /// Create new files and directories.
    pub const CREATE: FsAccess = FsAccess(1 << 4);
    /// Remove files and empty directories.
    pub const DELETE: FsAccess = FsAccess(1 << 5);
    /// Rename or move entries.
    pub const RENAME: FsAccess = FsAccess(1 << 6);
    /// Every capability.
    pub const ALL: FsAccess = FsAccess(0x7F);

    /// Returns true when every bit of `other` is present.
    pub const fn contains(self, other: FsAccess) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the union of both sets; the `const` form of `|`.
    pub const fn union(self, other: FsAccess) -> FsAccess {
        FsAccess(self.0 | other.0)
    }

    /// The capabilities of `self` that `other` lacks.
    pub(crate) const fn without(self, other: FsAccess) -> FsAccess {
        FsAccess(self.0 & !other.0)
    }

    /// Whether no capability is set.
    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for FsAccess {
    type Output = FsAccess;

    fn bitor(self, rhs: FsAccess) -> FsAccess {
        self.union(rhs)
    }
}

impl BitOrAssign for FsAccess {
    fn bitor_assign(&mut self, rhs: FsAccess) {
        *self = self.union(rhs);
    }
}

impl fmt::Debug for FsAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [(FsAccess, &str); 7] = [
            (FsAccess::READ, "READ"),
            (FsAccess::METADATA, "METADATA"),
            (FsAccess::LIST, "LIST"),
            (FsAccess::WRITE, "WRITE"),
            (FsAccess::CREATE, "CREATE"),
            (FsAccess::DELETE, "DELETE"),
            (FsAccess::RENAME, "RENAME"),
        ];
        let mut first = true;
        for (bit, name) in NAMES {
            if self.contains(bit) {
                if !first {
                    f.write_str("|")?;
                }
                first = false;
                f.write_str(name)?;
            }
        }
        if first {
            f.write_str("NONE")?;
        }
        Ok(())
    }
}

/// The `fs.*` command surface. The only place command names are spelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FsCommand {
    ReadFile,
    WriteFile,
    ReadDir,
    Size,
    Exists,
    CreateDir,
    RemoveFile,
    RemoveDir,
    CopyFile,
    RenameFile,
}

impl FsCommand {
    pub(crate) const ALL: [FsCommand; 10] = [
        FsCommand::ReadFile,
        FsCommand::WriteFile,
        FsCommand::ReadDir,
        FsCommand::Size,
        FsCommand::Exists,
        FsCommand::CreateDir,
        FsCommand::RemoveFile,
        FsCommand::RemoveDir,
        FsCommand::CopyFile,
        FsCommand::RenameFile,
    ];

    /// The IPC command name.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            FsCommand::ReadFile => "fs.read_file",
            FsCommand::WriteFile => "fs.write_file",
            FsCommand::ReadDir => "fs.read_dir",
            FsCommand::Size => "fs.size",
            FsCommand::Exists => "fs.exists",
            FsCommand::CreateDir => "fs.create_dir",
            FsCommand::RemoveFile => "fs.remove_file",
            FsCommand::RemoveDir => "fs.remove_dir",
            FsCommand::CopyFile => "fs.copy_file",
            FsCommand::RenameFile => "fs.rename_file",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_are_independent_and_composable() {
        let g = FsAccess::READ | FsAccess::WRITE;
        assert!(g.contains(FsAccess::READ));
        assert!(g.contains(FsAccess::WRITE));
        assert!(!g.contains(FsAccess::CREATE));
        assert!(!g.contains(FsAccess::READ | FsAccess::CREATE));
        assert_eq!(FsAccess::default(), FsAccess::NONE);
        assert!(FsAccess::ALL.contains(g));
    }

    #[test]
    fn debug_names_only_set_bits() {
        assert_eq!(format!("{:?}", FsAccess::NONE), "NONE");
        assert_eq!(
            format!("{:?}", FsAccess::READ | FsAccess::CREATE),
            "READ|CREATE"
        );
        assert_eq!(
            format!("{:?}", FsAccess::ALL),
            "READ|METADATA|LIST|WRITE|CREATE|DELETE|RENAME"
        );
    }

    #[test]
    fn command_names_are_distinct_and_namespaced() {
        let names: Vec<_> = FsCommand::ALL.iter().map(|c| c.name()).collect();
        let unique: std::collections::HashSet<_> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len());
        assert!(names.iter().all(|n| n.starts_with("fs.")));
    }
}
