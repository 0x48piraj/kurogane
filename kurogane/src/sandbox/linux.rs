//! Linux sandbox switches and preflight.
//!
//! Chromium sandboxes Linux helpers with unprivileged user namespaces when
//! the kernel allows it and otherwise falls back to the setuid `chrome-sandbox` helper.

use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::chromium_flags::ChromiumFlags;
use crate::error::RuntimeError;

pub(super) fn apply_disabled(flags: &mut ChromiumFlags) {
    flags.set("disable-setuid-sandbox");
}

/// Linux configures its sandbox through settings and switches alone.
pub(super) fn sandbox_info() -> *mut u8 {
    std::ptr::null_mut()
}

/// Confirms Chromium can sandbox its helpers on this machine.
pub(super) fn preflight(cef_root: &Path) -> Result<(), RuntimeError> {
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
    // SAFETY: `getuid` and `getgid` take no arguments and have no failure mode.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };

    // Formatted before fork; the child only makes async-signal-safe calls
    let uid_map = format!("0 {uid} 1");
    let gid_map = format!("0 {gid} 1");

    // SAFETY: `fork` requires no prior state. To satisfy POSIX multi-threading
    // semantics, the resulting child process executes exclusively
    // async-signal-safe routines with no allocations.
    let pid = unsafe { libc::fork() };

    if pid < 0 {
        return false;
    }

    if pid == 0 {
        // SAFETY: `libc::unshare` takes integer flags with no memory preconditions
        // and is async-signal-safe. State mutations are strictly bounded to this child.
        let mapped =
            unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWPID | libc::CLONE_NEWNET) }
                == 0
                && write_proc(c"/proc/self/setgroups", b"deny")
                && write_proc(c"/proc/self/uid_map", uid_map.as_bytes())
                && write_proc(c"/proc/self/gid_map", gid_map.as_bytes());

        // SAFETY: `libc::_exit` is async-signal-safe. Immediate termination bypasses
        // user-space destructors, preventing the child from corrupting shared parent state.
        unsafe { libc::_exit(if mapped { 0 } else { 1 }) };
    }

    let mut status = 0;

    // Retry when a signal interrupts the wait
    let waited = loop {
        // SAFETY: `pid` is a directly owned child process. `status` is a valid,
        // stack-allocated integer whose memory strictly outlives the FFI call.
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };

        if result != -1 || io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break result;
        }
    };

    waited == pid && libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
}

/// Writes `data` to a procfs file using only async-signal-safe calls.
fn write_proc(path: &CStr, data: &[u8]) -> bool {
    // SAFETY: `path` is a valid, null-terminated C-string. `open` is async-signal-safe.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY) };

    if fd < 0 {
        return false;
    }

    // SAFETY: `fd` is a valid, open file descriptor. `data` pointer and length match
    // exactly and the backing memory outlives the async-signal-safe `write` call.
    let written = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };

    // SAFETY: `fd` is exclusively owned by this scope and closed exactly once.
    unsafe { libc::close(fd) };

    usize::try_from(written) == Ok(data.len())
}

/// Describes why the setuid helper is unusable, or `None` when it is usable.
fn setuid_helper_problem(helper: &Path) -> Option<String> {
    let meta = match std::fs::metadata(helper) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Some(format!("{} is missing", helper.display()));
        }
        Err(e) => return Some(format!("{} cannot be inspected: {e}", helper.display())),
    };

    let problem = if meta.uid() != 0 {
        "is not owned by root"
    } else if meta.mode() & 0o4000 == 0 {
        "does not have the setuid bit"
    } else if meta.mode() & 0o111 == 0 {
        "is not executable"
    } else {
        match mounted_nosuid(helper) {
            Ok(false) => return None,
            Ok(true) => "is on a filesystem mounted nosuid",
            Err(e) => {
                return Some(format!(
                    "{}: its filesystem's mount flags cannot be read: {e}",
                    helper.display()
                ));
            }
        }
    };

    Some(format!("{} {problem}", helper.display()))
}

/// Returns whether the filesystem holding `path` ignores setuid bits.
fn mounted_nosuid(path: &Path) -> io::Result<bool> {
    let path = CString::new(path.as_os_str().as_bytes())?;

    // SAFETY: `libc::statvfs` is a Plain Old Data (POD) C-struct.
    // All-zero byte initialization is a strictly valid memory representation.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };

    // SAFETY: `path` is a valid, null-terminated C-string. `stat` is a valid,
    // stack-allocated struct whose memory strictly outlives the FFI call.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(stat.f_flag & libc::ST_NOSUID != 0)
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
