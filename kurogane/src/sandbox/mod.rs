//! Chromium process sandbox policy.
//!
//! [`SandboxMode`] controls the sandbox policy for the Chromium process tree.
//! The selected policy is reflected in CEF configuration, platform-specific
//! switches and startup validation; a requested sandbox is never silently
//! ignored when it cannot be enforced.

use std::path::Path;

use crate::chromium_flags::ChromiumFlags;
use crate::error::RuntimeError;
use crate::spec::SandboxMode;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub(crate) mod macos;

/// Chromium switches that turn off all or part of the sandbox.
const SANDBOX_DISABLING_SWITCHES: [&str; 3] = [
    "no-sandbox",
    "disable-setuid-sandbox",
    "disable-gpu-sandbox",
];

/// Returns the CEF `Settings::no_sandbox` value for the policy.
pub(crate) fn cef_no_sandbox(mode: SandboxMode) -> i32 {
    match mode {
        SandboxMode::Disabled => 1,
        SandboxMode::Chromium => 0,
    }
}

/// Applies the platform sandbox switches for the policy.
///
/// [`SandboxMode::Chromium`] adds no switches so the sandbox runs under
/// Chromium's own defaults.
pub(crate) fn apply_sandbox_flags(flags: &mut ChromiumFlags, mode: SandboxMode) {
    match mode {
        SandboxMode::Disabled => {
            #[cfg(target_os = "linux")]
            linux::apply_sandbox_flags(flags);

            #[cfg(target_os = "windows")]
            windows::apply_sandbox_flags(flags);

            #[cfg(target_os = "macos")]
            macos::apply_sandbox_flags(flags);
        }
        SandboxMode::Chromium => {}
    }
}

/// Returns the user switches that weaken an enabled sandbox.
///
/// User flags take precedence over runtime policy, so these are reported
/// rather than removed.
pub(crate) fn sandbox_overrides(flags: &ChromiumFlags, mode: SandboxMode) -> Vec<&'static str> {
    match mode {
        SandboxMode::Disabled => Vec::new(),
        SandboxMode::Chromium => SANDBOX_DISABLING_SWITCHES
            .into_iter()
            .filter(|name| flags.contains(name))
            .collect(),
    }
}

/// Confirms the policy can be enforced before CEF starts.
///
/// Runs in the browser process only.
pub(crate) fn preflight(mode: SandboxMode, cef_root: &Path) -> Result<(), RuntimeError> {
    match mode {
        SandboxMode::Disabled => Ok(()),
        SandboxMode::Chromium => chromium_preflight(cef_root),
    }
}

#[cfg(target_os = "linux")]
fn chromium_preflight(cef_root: &Path) -> Result<(), RuntimeError> {
    linux::preflight(cef_root)
}

#[cfg(target_os = "macos")]
fn chromium_preflight(_cef_root: &Path) -> Result<(), RuntimeError> {
    macos::preflight()
}

#[cfg(target_os = "windows")]
fn chromium_preflight(_cef_root: &Path) -> Result<(), RuntimeError> {
    windows::preflight()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chromium_flags::ChromiumFlag;

    #[test]
    fn no_sandbox_setting_follows_policy() {
        assert_eq!(cef_no_sandbox(SandboxMode::Disabled), 1);
        assert_eq!(cef_no_sandbox(SandboxMode::Chromium), 0);
    }

    #[test]
    fn chromium_policy_adds_no_switches() {
        let mut flags = ChromiumFlags::default();
        apply_sandbox_flags(&mut flags, SandboxMode::Chromium);
        assert_eq!(flags.to_string(), "");
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn disabled_policy_adds_disabling_switches() {
        let mut flags = ChromiumFlags::default();
        apply_sandbox_flags(&mut flags, SandboxMode::Disabled);
        assert!(
            SANDBOX_DISABLING_SWITCHES
                .iter()
                .any(|name| flags.contains(name))
        );
    }

    #[test]
    fn user_switches_that_weaken_the_sandbox_are_reported() {
        let mut flags = ChromiumFlags::default();
        flags.extend_user_flags(&[
            ChromiumFlag::Present("no-sandbox".into()),
            ChromiumFlag::Present("disable-gpu".into()),
        ]);

        assert_eq!(
            sandbox_overrides(&flags, SandboxMode::Chromium),
            ["no-sandbox"]
        );
        assert!(sandbox_overrides(&flags, SandboxMode::Disabled).is_empty());
    }

    #[test]
    fn disabled_policy_always_passes_preflight() {
        assert!(preflight(SandboxMode::Disabled, Path::new("/nonexistent")).is_ok());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn chromium_policy_is_rejected_on_windows() {
        assert!(matches!(
            preflight(SandboxMode::Chromium, Path::new(".")),
            Err(RuntimeError::SandboxUnsupported { .. })
        ));
    }
}
