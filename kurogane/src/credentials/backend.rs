use crate::chromium_flags::ChromiumFlags;

#[cfg(target_os = "linux")]
use super::linux as platform;

#[cfg(target_os = "windows")]
use super::windows as platform;

#[cfg(target_os = "macos")]
use super::macos as platform;

/// Backend used to protect cookies and saved passwords at rest.
///
/// Pass to App::credential_storage to control whether the runtime reaches for
/// the platform credential store (default = CredentialStorage::System).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CredentialStorage {
    /// Use the platform credential store.
    ///
    /// Cookies and passwords are encrypted with a key held by the Keychain
    /// (macOS), kwallet or gnome-keyring (Linux) or DPAPI (Windows).
    ///
    /// Unsigned macOS builds prompt for Keychain access on every run because
    /// they have no stable code identity, and Linux hosts without a keyring
    /// daemon fall back to unencrypted storage on their own.
    #[default]
    System,

    /// Bypass the platform credential store.
    ///
    /// Chromium protects data with a built-in fixed key instead, which is
    /// obfuscation rather than encryption: anything readable by the process is
    /// readable by anyone with access to the profile directory.
    ///
    /// Suited to development, containers and CI, where the credential store is
    /// either absent or prompts on every run. Not suited to profiles holding
    /// data worth protecting.
    Basic,
}

/// Apply Chromium command-line flags for the configured credential storage.
pub(crate) fn apply_credential_flags(flags: &mut ChromiumFlags, storage: CredentialStorage) {
    match storage {
        // Chromium reaches for the platform store by default; nothing to add.
        CredentialStorage::System => {}

        CredentialStorage::Basic => platform::apply_basic(flags),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switches(storage: CredentialStorage) -> String {
        let mut flags = ChromiumFlags::default();
        apply_credential_flags(&mut flags, storage);
        flags.to_string()
    }

    #[test]
    fn system_storage_is_the_default() {
        assert_eq!(CredentialStorage::default(), CredentialStorage::System);
    }

    #[test]
    fn system_storage_adds_no_switches() {
        assert!(
            switches(CredentialStorage::System).is_empty(),
            "the platform default needs no switches"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn basic_storage_bypasses_the_keychain() {
        assert_eq!(switches(CredentialStorage::Basic), "--use-mock-keychain\n");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn basic_storage_bypasses_the_keyring() {
        assert_eq!(
            switches(CredentialStorage::Basic),
            "--password-store=basic\n"
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn basic_storage_is_inert_where_dpapi_never_prompts() {
        assert!(switches(CredentialStorage::Basic).is_empty());
    }
}
