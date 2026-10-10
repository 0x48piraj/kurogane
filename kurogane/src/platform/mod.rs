//! Platform-specific code.

#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;

pub(crate) mod embed;

/// Raises the soft open-file limit, capped at 10,240 on macOS.
///
/// Shared-memory messages retain file descriptors until received. Large bursts
/// can exhaust the default soft limit (1,024 on Linux, 256 on macOS).
/// Chromium processes started afterward inherit the updated limit.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn raise_open_file_limit() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is valid output space for one rlimit
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return;
    }
    // macOS refuses a soft limit above OPEN_MAX from <sys/syslimits.h>
    #[cfg(target_os = "macos")]
    let wanted = limit.rlim_max.min(10_240);
    #[cfg(target_os = "linux")]
    let wanted = limit.rlim_max;
    if limit.rlim_cur >= wanted {
        return;
    }
    let raised = libc::rlimit {
        rlim_cur: wanted,
        rlim_max: limit.rlim_max,
    };
    // SAFETY: setrlimit reads one valid rlimit
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } != 0 {
        tracing::debug!("the soft open file limit stays at {}", limit.rlim_cur);
    }
}
