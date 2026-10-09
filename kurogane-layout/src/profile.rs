//! Per-application runtime profile paths.
//!
//! An application's profile holds what its user would miss (cookies,
//! storage, permissions), so it lives in the local data directory, not in
//! the cache that cleaners empty. Its name is derived from the application's
//! identity and remains stable across executable moves and updates.

use std::path::PathBuf;

use crate::platform;

/// Where every application's profile lives: `kurogane/profiles` in the
/// local data directory (`~/.local/share` on Linux, `~/Library/Application
/// Support` on macOS, `%LOCALAPPDATA%` on Windows).
pub fn profiles_root() -> PathBuf {
    platform::data_local_dir().join("kurogane").join("profiles")
}

/// Returns the profile directory of the application identified by `app_id`.
pub fn profile_dir(app_id: &str) -> PathBuf {
    profiles_root().join(sanitize_name(app_id))
}

/// Sanitizes a user-provided name into a filesystem-safe identifier.
/// Returns "kurogane-app" when the input cannot be reduced to a valid name.
pub fn sanitize_name(name: &str) -> String {
    // Windows reserved names
    const WINDOWS_RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    // Replace forbidden/control chars with _
    let replaced = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            _ if c.is_control() => '_',
            _ => c,
        })
        .collect::<String>();

    // Collapse consecutive _ for aesthetics
    let mut sanitized = replaced.chars().fold(String::new(), |mut acc, c| {
        if c == '_' && acc.ends_with('_') {
            acc
        } else {
            acc.push(c);
            acc
        }
    });

    // Trim Windows-invalid endings
    sanitized = sanitized.trim_end_matches(['.', ' ']).to_string();

    // Trim leading dots
    sanitized = sanitized.trim_start_matches('.').to_string();

    let stem = sanitized.split('.').next().unwrap();

    if WINDOWS_RESERVED
        .iter()
        .any(|&r| r.eq_ignore_ascii_case(stem))
    {
        sanitized = format!("_{sanitized}");
    }

    // Character length limit
    const MAX_LEN: usize = 64;
    sanitized = sanitized.chars().take(MAX_LEN).collect();

    // Fallback if empty, or _ for aesthetics, again
    if sanitized.is_empty() || sanitized.chars().all(|c| c == '_') {
        return "kurogane-app".to_string();
    }

    sanitized
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn sanitize_name_never_panics(s in ".*") {
            let _ = sanitize_name(&s);
        }

        #[test]
        fn sanitize_name_output_is_valid_utf8(s in ".*") {
            let result = sanitize_name(&s);
            prop_assert!(result.chars().count() <= 64);
            assert!(!result.is_empty());
        }

        #[test]
        fn sanitize_name_no_forbidden_chars(s in ".*") {
            let result = sanitize_name(&s);
            for c in result.chars() {
                prop_assert!(!matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'));
            }
        }
    }

    #[test]
    fn profile_dir_is_named_after_the_application() {
        let dir = profile_dir("my-app");

        assert_eq!(dir.file_name().unwrap(), "my-app");
        assert_eq!(dir.parent().unwrap(), profiles_root());
    }

    #[test]
    fn profiles_live_with_the_data_not_the_cache() {
        let root = profiles_root();
        assert!(
            root.starts_with(platform::data_local_dir()),
            "{}",
            root.display()
        );
        // Windows keeps both in %LOCALAPPDATA%
        if cfg!(not(windows)) {
            let cache = dirs::cache_dir().unwrap();
            assert!(!root.starts_with(cache), "{}", root.display());
        }
    }

    #[test]
    fn profile_dir_is_separate_per_application() {
        assert_ne!(profile_dir("my-app"), profile_dir("other-app"));
    }

    #[test]
    fn profile_dir_sanitizes_the_identity() {
        assert_eq!(profile_dir("a/b").file_name().unwrap(), "a_b");
    }
}
