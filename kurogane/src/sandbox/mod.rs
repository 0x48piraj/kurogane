//! Chromium process sandbox policy.
//!
//! [`SandboxMode`] controls the sandbox policy for the Chromium process tree.
//! The selected policy is reflected in CEF configuration, platform-specific
//! switches and startup validation; a requested sandbox either starts or
//! returns an error.
//!
//! - `apply_disabled` adds the switches that turn its sandbox off.
//! - `preflight` checks that the sandbox is available before CEF starts.
//! - `sandbox_info` returns the state CEF's entry points take, or null.

use std::path::Path;

use crate::chromium_flags::ChromiumFlags;
use crate::error::RuntimeError;
use crate::spec::SandboxMode;

mod entry;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "windows")]
pub(crate) mod windows;

#[cfg(target_os = "macos")]
pub(crate) mod macos;

#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(target_os = "macos")]
use macos as platform;

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

/// Returns the sandbox state to pass to CEF's process entry points.
///
/// The same value must reach both `execute_process` and `initialize`.
pub(crate) fn cef_sandbox_info(mode: SandboxMode) -> *mut u8 {
    match mode {
        SandboxMode::Disabled => std::ptr::null_mut(),
        SandboxMode::Chromium => platform::sandbox_info(),
    }
}

/// Applies the platform sandbox switches for the policy.
///
/// [`SandboxMode::Chromium`] adds no switches so the sandbox runs under
/// Chromium's own defaults.
pub(crate) fn apply_sandbox_flags(flags: &mut ChromiumFlags, mode: SandboxMode) {
    match mode {
        SandboxMode::Disabled => platform::apply_disabled(flags),
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
        SandboxMode::Chromium => platform::preflight(cef_root),
    }
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
    fn a_prefixed_user_switch_that_weakens_the_sandbox_is_reported() {
        let mut flags = ChromiumFlags::default();
        flags.extend_user_flags(&[ChromiumFlag::Present("--no-sandbox".into())]);

        assert_eq!(
            sandbox_overrides(&flags, SandboxMode::Chromium),
            ["no-sandbox"]
        );
    }

    #[test]
    fn disabled_policy_always_passes_preflight() {
        assert!(preflight(SandboxMode::Disabled, Path::new("/nonexistent")).is_ok());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn chromium_policy_needs_the_bootstrap_on_windows() {
        // The test binary is not loaded by CEF's bootstrap
        assert!(matches!(
            preflight(SandboxMode::Chromium, Path::new(".")),
            Err(RuntimeError::SandboxUnsupported { .. })
        ));
    }

    #[test]
    fn a_disabled_sandbox_passes_no_sandbox_state() {
        assert!(cef_sandbox_info(SandboxMode::Disabled).is_null());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn only_windows_passes_sandbox_state() {
        assert!(cef_sandbox_info(SandboxMode::Chromium).is_null());
    }
}
