//! Credential storage backend selection.
//!
//! Chromium encrypts cookies and saved passwords with a key held by the
//! platform's credential store: the Keychain on macOS, kwallet or
//! gnome-keyring on Linux and DPAPI on Windows.
//!
//! Reaching those stores is not always possible or desirable. An unsigned
//! binary has no stable code identity, so macOS re-prompts for Keychain
//! access on every build; containers and CI machines frequently have no
//! keyring daemon at all. This module exposes that as an explicit policy
//! rather than silently weakening encryption.
//!
//! Called once during CEF command-line processing before any browser is created.

mod backend;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
mod macos;

pub use backend::CredentialStorage;

use crate::chromium_flags::ChromiumFlags;

/// Apply Chromium command-line flags for the configured credential storage.
pub(crate) fn apply_credential_flags(flags: &mut ChromiumFlags, storage: CredentialStorage) {
    backend::apply_credential_flags(flags, storage);
}
