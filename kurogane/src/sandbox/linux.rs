//! Linux sandbox switches and preflight.
//!
//! Chromium sandboxes Linux helpers with unprivileged user namespaces when
//! the kernel allows it and otherwise falls back to the setuid `chrome-sandbox` helper.

use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::chromium_flags::ChromiumFlags;
use crate::error::RuntimeError;

pub(crate) fn apply_sandbox_flags(flags: &mut ChromiumFlags) {
    flags.set("disable-setuid-sandbox");
}

/// Confirms Chromium can sandbox its helpers on this machine.
pub(crate) fn preflight(cef_root: &Path) -> Result<(), RuntimeError> {
    if user_namespaces_available() {
        return Ok(());
    }

    let helper = sandbox_helper_path(cef_root);

    match setuid_helper_problem(&helper) {
        None => Ok(()),
        Some(problem) => Err(RuntimeError::SandboxUnavailable {
            reason: format!(
                concat!(
                    "Unprivileged user namespaces are unavailable and {}.\n\n",
                    "Enable one of:\n\n",
                    "  User namespaces (e.g. lift the Ubuntu 24.04 AppArmor restriction):\n",
                    "    sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0\n\n",
                    "  The setuid helper:\n",
                    "    sudo chown root:root {helper}\n",
                    "    sudo chmod 4755 {helper}"
                ),
                problem,
                helper = helper.display(),
            ),
        }),
    }
}

/// Returns the setuid helper Chromium will use.
///
/// `CHROME_DEVEL_SANDBOX` overrides the helper shipped with CEF.
fn sandbox_helper_path(cef_root: &Path) -> PathBuf {
    std::env::var_os("CHROME_DEVEL_SANDBOX")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| cef_root.join("chrome-sandbox"))
}

/// Returns whether this process can create the namespaces Chromium's sandbox
/// needs.
///
/// Mirrors Chromium's own probe in a forked child so this process's
/// namespaces never change: unshare, then map the current user, which is the
/// step AppArmor's user-namespace restriction denies.
fn user_namespaces_available() -> bool {
    // Formatted before fork; the child only makes async-signal-safe calls
    let uid_map = format!("0 {} 1", unsafe { libc::getuid() });
    let gid_map = format!("0 {} 1", unsafe { libc::getgid() });

    unsafe {
        let pid = libc::fork();

        if pid < 0 {
            return false;
        }

        if pid == 0 {
            let mapped = libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWPID | libc::CLONE_NEWNET)
                == 0
                && write_proc(c"/proc/self/setgroups", b"deny")
                && write_proc(c"/proc/self/uid_map", uid_map.as_bytes())
                && write_proc(c"/proc/self/gid_map", gid_map.as_bytes());

            libc::_exit(if mapped { 0 } else { 1 });
        }

        let mut status = 0;

        libc::waitpid(pid, &mut status, 0) == pid
            && libc::WIFEXITED(status)
            && libc::WEXITSTATUS(status) == 0
    }
}

/// Writes `data` to a procfs file using only async-signal-safe calls.
unsafe fn write_proc(path: &CStr, data: &[u8]) -> bool {
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY);

        if fd < 0 {
            return false;
        }

        let written = libc::write(fd, data.as_ptr().cast(), data.len());
        libc::close(fd);

        written == data.len() as isize
    }
}

/// Describes why the setuid helper is unusable, or `None` when it is usable.
fn setuid_helper_problem(helper: &Path) -> Option<String> {
    let Ok(meta) = std::fs::metadata(helper) else {
        return Some(format!("{} is missing", helper.display()));
    };

    let problem = if meta.uid() != 0 {
        "is not owned by root"
    } else if meta.mode() & 0o4000 == 0 {
        "does not have the setuid bit"
    } else if meta.mode() & 0o111 == 0 {
        "is not executable"
    } else if mounted_nosuid(helper) {
        "is on a filesystem mounted nosuid"
    } else {
        return None;
    };

    Some(format!("{} {problem}", helper.display()))
}

/// Returns whether the filesystem holding `path` ignores setuid bits.
fn mounted_nosuid(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };

    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };

    unsafe { libc::statvfs(path.as_ptr(), &mut stat) == 0 && stat.f_flag & libc::ST_NOSUID != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_helper_is_reported() {
        let dir = tempfile::tempdir().unwrap();

        let problem = setuid_helper_problem(&dir.path().join("chrome-sandbox")).unwrap();

        assert!(problem.ends_with("is missing"));
    }

    #[test]
    fn helper_without_root_setuid_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("chrome-sandbox");
        std::fs::write(&helper, "").unwrap();

        // Owned by the test user, or by root without the setuid bit
        assert!(setuid_helper_problem(&helper).is_some());
    }
}
