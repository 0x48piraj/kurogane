//! Chromium runtime validation.
//!
//! A runtime is complete when it holds every file CEF needs to start on the
//! current platform.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// A Chromium runtime without files CEF needs to start.
#[derive(Debug, Error)]
#[error("invalid CEF runtime at {}: missing {}", .root.display(), .missing.join(", "))]
pub struct IncompleteRuntime {
    /// Runtime directory.
    pub root: PathBuf,

    /// Files the runtime lacks, relative to its directory.
    pub missing: Vec<&'static str>,
}

/// V8 snapshot file names across CEF versions.
const V8_SNAPSHOTS: &[&str] = &["v8_context_snapshot.bin", "snapshot_blob.bin"];

/// The platform for which a CEF runtime layout is validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Windows,
    Linux,
    MacOs,
}

impl Platform {
    /// Returns CEF's library, relative to the runtime root.
    fn libcef(self) -> &'static str {
        match self {
            Platform::Windows => "libcef.dll",
            Platform::Linux => "libcef.so",
            Platform::MacOs => "Chromium Embedded Framework.framework/Chromium Embedded Framework",
        }
    }
}

/// Returns the current platform.
fn current_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

/// Validates the required files in a CEF runtime.
pub fn validate_cef_runtime(runtime: &Path) -> Result<(), IncompleteRuntime> {
    validate_cef_runtime_for(runtime, current_platform())
}

/// Validates a CEF runtime against a platform's expected layout.
fn validate_cef_runtime_for(runtime: &Path, platform: Platform) -> Result<(), IncompleteRuntime> {
    let mut missing: Vec<&'static str> = Vec::new();

    let require = |missing: &mut Vec<&'static str>, name: &'static str| {
        if !runtime.join(name).exists() {
            missing.push(name);
        }
    };

    match platform {
        Platform::Windows => {
            require(&mut missing, platform.libcef());
            require(&mut missing, "chrome_elf.dll");
            require(&mut missing, "icudtl.dat");
            require(&mut missing, "locales");

            if !V8_SNAPSHOTS.iter().any(|s| runtime.join(s).exists()) {
                missing.push("v8_context_snapshot.bin");
            }
        }
        Platform::MacOs => {
            require(&mut missing, platform.libcef());

            // Resources, locales and V8 snapshots ship inside the framework on
            // macOS rather than at the runtime root
            let resources = runtime.join("Chromium Embedded Framework.framework/Resources");

            if !resources.join("icudtl.dat").exists() {
                missing.push("Chromium Embedded Framework.framework/Resources/icudtl.dat");
            }

            let has_locale = fs::read_dir(&resources).is_ok_and(|entries| {
                entries.filter_map(Result::ok).any(|entry| {
                    entry.path().is_dir() && entry.file_name().to_string_lossy().ends_with(".lproj")
                })
            });

            if !has_locale {
                missing.push("Chromium Embedded Framework.framework/Resources/*.lproj");
            }

            let has_snapshot = fs::read_dir(&resources).is_ok_and(|entries| {
                entries.filter_map(Result::ok).any(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name == "snapshot_blob.bin" || name.starts_with("v8_context_snapshot.")
                })
            });

            if !has_snapshot {
                missing.push("Chromium Embedded Framework.framework/Resources/v8 snapshot");
            }
        }
        Platform::Linux => {
            require(&mut missing, platform.libcef());
            require(&mut missing, "chrome-sandbox");
            require(&mut missing, "icudtl.dat");
            require(&mut missing, "locales");

            if !V8_SNAPSHOTS.iter().any(|s| runtime.join(s).exists()) {
                missing.push("v8_context_snapshot.bin");
            }
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(IncompleteRuntime {
            root: runtime.to_path_buf(),
            missing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns whether a missing file's path holds `part`.
    fn lacks(missing: &[&str], part: &str) -> bool {
        missing.iter().any(|file| file.contains(part))
    }

    fn tmp() -> tempfile::TempDir {
        crate::test_fixtures::tmp_dir()
    }

    // Runtime validation

    #[test]
    fn complete_runtime_passes_validation() {
        let dir = tmp();
        let runtime = crate::test_fixtures::cef_runtime(&dir.path().join("rt"));
        assert!(validate_cef_runtime(&runtime).is_ok());
    }

    #[test]
    fn every_platform_fixture_passes_its_own_validation() {
        for platform in [Platform::Windows, Platform::MacOs, Platform::Linux] {
            let dir = tmp();
            let target = match platform {
                Platform::Windows => crate::test_fixtures::Target::Windows,
                Platform::MacOs => crate::test_fixtures::Target::MacOs,
                Platform::Linux => crate::test_fixtures::Target::Linux,
            };
            let runtime = crate::test_fixtures::cef_runtime_for(dir.path(), target);

            validate_cef_runtime_for(&runtime, platform)
                .unwrap_or_else(|e| panic!("{platform:?} fixture failed its own validation: {e}"));
        }
    }

    #[test]
    fn missing_required_files_are_reported_together() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        fs::create_dir_all(&runtime).unwrap();

        match validate_cef_runtime(&runtime) {
            Err(IncompleteRuntime { missing, .. }) => {
                assert!(lacks(&missing, current_platform().libcef()));
                assert!(lacks(&missing, "icudtl.dat"));
                if cfg!(target_os = "macos") {
                    assert!(!lacks(&missing, "locales"));
                    assert!(lacks(&missing, "*.lproj"));
                } else {
                    assert!(lacks(&missing, "locales"));
                }
            }
            other => panic!("expected InvalidRuntime, got {other:?}"),
        }
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn either_v8_snapshot_satisfies_requirement() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join(current_platform().libcef()), "").unwrap();
        fs::write(runtime.join("icudtl.dat"), "").unwrap();
        fs::create_dir_all(runtime.join("locales")).unwrap();
        if cfg!(target_os = "windows") {
            fs::write(runtime.join("chrome_elf.dll"), "").unwrap();
        } else {
            fs::write(runtime.join("chrome-sandbox"), "").unwrap();
        }

        // Legacy snapshot name only
        fs::write(runtime.join("snapshot_blob.bin"), "").unwrap();
        assert!(validate_cef_runtime(&runtime).is_ok());

        // Modern snapshot name only
        fs::remove_file(runtime.join("snapshot_blob.bin")).unwrap();
        fs::write(runtime.join("v8_context_snapshot.bin"), "").unwrap();
        assert!(validate_cef_runtime(&runtime).is_ok());

        // Neither
        fs::remove_file(runtime.join("v8_context_snapshot.bin")).unwrap();
        assert!(matches!(
            validate_cef_runtime(&runtime),
            Err(IncompleteRuntime { .. })
        ));
    }

    #[test]
    fn macos_framework_layout_passes_validation() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        let fw = runtime.join("Chromium Embedded Framework.framework");
        let resources = fw.join("Resources");
        fs::create_dir_all(resources.join("en.lproj")).unwrap();
        fs::write(resources.join("en.lproj").join("locale.pak"), "pak").unwrap();
        fs::write(fw.join("Chromium Embedded Framework"), "cef").unwrap();
        fs::write(resources.join("icudtl.dat"), "icu").unwrap();

        // Architecture-suffixed snapshot name satisfies the requirement
        fs::write(resources.join("v8_context_snapshot.arm64.bin"), "v8").unwrap();
        assert!(validate_cef_runtime_for(&runtime, Platform::MacOs).is_ok());

        // Missing snapshot is reported
        fs::remove_file(resources.join("v8_context_snapshot.arm64.bin")).unwrap();
        match validate_cef_runtime_for(&runtime, Platform::MacOs) {
            Err(IncompleteRuntime { missing, .. }) => {
                assert!(lacks(&missing, "v8 snapshot"));
            }
            other => panic!("expected InvalidRuntime, got {other:?}"),
        }

        // Missing locale bundle is reported
        fs::write(resources.join("v8_context_snapshot.arm64.bin"), "v8").unwrap();
        fs::remove_dir_all(resources.join("en.lproj")).unwrap();
        match validate_cef_runtime_for(&runtime, Platform::MacOs) {
            Err(IncompleteRuntime { missing, .. }) => {
                assert!(lacks(&missing, "*.lproj"));
            }
            other => panic!("expected InvalidRuntime, got {other:?}"),
        }
    }
}
