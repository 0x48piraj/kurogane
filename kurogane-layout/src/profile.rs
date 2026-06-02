use std::path::{Path, PathBuf};

use crate::platform;

pub fn cache_root() -> PathBuf {
    platform::cache_dir().join("kurogane")
}

pub fn profile_dir(app_id: &str, exe: &Path) -> PathBuf {
    // Isolate the CEF cache per executable.
    // Reusing a profile across runs can trigger session restore leading to multiple on_context_initialized invocations.
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());

    let hash = fnv1a_64(&exe);

    let app_id = sanitize_name(app_id);

    cache_root()
        .join("profiles")
        .join(format!("{app_id}-{hash}"))
}

/// Computes a deterministic FNV-1a 64-bit hash of a filesystem path.
/// Intended for identity stability, not cryptographic use.
pub fn fnv1a_64(path: &Path) -> String {
    let mut hash: u64 = 14695981039346656037;

    for byte in path.to_string_lossy().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }

    format!("{:016x}", hash)
}

/// Sanitizes a user-provided name into a filesystem-safe identifier.
/// Returns "default" when the input cannot be reduced to a valid name.
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
            // never panics on any input
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
}
