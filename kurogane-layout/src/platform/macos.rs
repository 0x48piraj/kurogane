//! macOS CEF runtime integration.
//!
//! Unbundled Chromium processes resolve ANGLE libraries from the executable
//! directory. Bundled processes resolve them from the CEF framework.

use std::fs;
use std::path::{Path, PathBuf};

use crate::CefError;

/// macOS CEF framework directory name.
const MACOS_FRAMEWORK: &str = "Chromium Embedded Framework.framework";

/// ANGLE libraries required by Chromium's GPU process.
const ANGLE_REQUIRED: &[&str] = &["libEGL.dylib", "libGLESv2.dylib"];

/// SwiftShader libraries backing Chromium's software fallback.
///
/// Absent from some distributions, so a missing entry is skipped rather than
/// reported as an invalid runtime.
const ANGLE_OPTIONAL: &[&str] = &["libvk_swiftshader.dylib", "vk_swiftshader_icd.json"];

/// Links ANGLE libraries into the executable directory for unbundled runs.
///
/// Chromium resolves ANGLE from the executable directory when the process is
/// not running inside an application bundle. CEF ships the libraries inside
/// the framework, so this exposes them at the path Chromium expects without
/// duplicating the files.
///
/// `dest` is the directory containing the executable.
///
/// Required libraries must exist in the CEF runtime. Optional SwiftShader
/// libraries are linked when present.
///
/// Existing links to the same source are preserved. Stale links and files are
/// replaced. Directories are not replaced.
pub fn link_unbundled_angle_libraries(
    runtime: &Path,
    dest: &Path,
) -> Result<Vec<&'static str>, CefError> {
    fs::create_dir_all(dest)?;

    let mut linked = Vec::new();

    for name in ANGLE_REQUIRED {
        let source = angle_source(runtime, name).ok_or_else(|| CefError::InvalidRuntime {
            root: runtime.to_path_buf(),
            missing: (*name).to_string(),
        })?;

        if link_when_stale(&source, &dest.join(name))? {
            linked.push(*name);
        }
    }

    for name in ANGLE_OPTIONAL {
        if let Some(source) = angle_source(runtime, name)
            && link_when_stale(&source, &dest.join(name))?
        {
            linked.push(*name);
        }
    }

    Ok(linked)
}

/// Locates an ANGLE library inside a CEF runtime.
///
/// The framework carries them in `Libraries/`; a flattened distribution may
/// place them at the runtime root instead.
fn angle_source(runtime: &Path, name: &str) -> Option<PathBuf> {
    [
        runtime.join(MACOS_FRAMEWORK).join("Libraries").join(name),
        runtime.join(name),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// Ensures `destination` links to `source`.
fn link_when_stale(source: &Path, destination: &Path) -> Result<bool, CefError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            if fs::read_link(destination).is_ok_and(|existing| existing == source) {
                return Ok(false);
            }

            fs::remove_file(destination)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    std::os::unix::fs::symlink(source, destination)?;

    Ok(true)
}

/// Writes a flattened ANGLE fixture for tests.
#[cfg(test)]
fn write_angle_fixture(dir: &Path) {
    fs::create_dir_all(dir).expect("create angle fixture dir");

    for name in ANGLE_REQUIRED.iter().chain(ANGLE_OPTIONAL) {
        fs::write(dir.join(name), *name).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn angle_libraries_are_linked_from_the_framework() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        write_angle_fixture(&runtime.join(MACOS_FRAMEWORK).join("Libraries"));

        let exe_dir = dir.path().join("bin");
        let installed = link_unbundled_angle_libraries(&runtime, &exe_dir).unwrap();

        assert_eq!(installed.len(), ANGLE_REQUIRED.len() + ANGLE_OPTIONAL.len());

        for name in ANGLE_REQUIRED.iter().chain(ANGLE_OPTIONAL) {
            let link = exe_dir.join(name);
            assert!(
                fs::read_link(&link).is_ok(),
                "{name} should be a symlink in the exe directory"
            );
            assert_eq!(
                fs::read_link(&link).unwrap(),
                runtime.join(MACOS_FRAMEWORK).join("Libraries").join(name),
                "{name} should point at the framework's Libraries entry"
            );
        }
    }

    #[test]
    fn angle_libraries_are_linked_from_a_flattened_runtime() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        write_angle_fixture(&runtime);

        let exe_dir = dir.path().join("bin");
        link_unbundled_angle_libraries(&runtime, &exe_dir).unwrap();

        let gles = exe_dir.join("libGLESv2.dylib");
        assert!(fs::read_link(&gles).is_ok());
        assert_eq!(
            fs::read_link(&gles).unwrap(),
            runtime.join("libGLESv2.dylib")
        );
    }

    #[test]
    fn angle_linking_is_idempotent() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        write_angle_fixture(&runtime.join(MACOS_FRAMEWORK).join("Libraries"));

        let exe_dir = dir.path().join("bin");
        link_unbundled_angle_libraries(&runtime, &exe_dir).unwrap();

        let installed = link_unbundled_angle_libraries(&runtime, &exe_dir).unwrap();
        assert!(installed.is_empty(), "identical symlinks are not recreated");
    }

    #[test]
    fn angle_linking_replaces_a_stale_library_file() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        write_angle_fixture(&runtime.join(MACOS_FRAMEWORK).join("Libraries"));

        let exe_dir = dir.path().join("bin");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::write(exe_dir.join("libGLESv2.dylib"), "stale").unwrap();

        let installed = link_unbundled_angle_libraries(&runtime, &exe_dir).unwrap();

        assert!(installed.contains(&"libGLESv2.dylib"));
        let gles = exe_dir.join("libGLESv2.dylib");
        assert!(
            fs::read_link(&gles).is_ok(),
            "plain file should be replaced by a symlink"
        );
        assert_eq!(
            fs::read_link(&gles).unwrap(),
            runtime
                .join(MACOS_FRAMEWORK)
                .join("Libraries")
                .join("libGLESv2.dylib")
        );
    }

    #[test]
    fn angle_linking_replaces_a_stale_or_broken_symlink() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        write_angle_fixture(&runtime.join(MACOS_FRAMEWORK).join("Libraries"));

        let exe_dir = dir.path().join("bin");
        fs::create_dir_all(&exe_dir).unwrap();
        std::os::unix::fs::symlink("/nonexistent/source", exe_dir.join("libGLESv2.dylib")).unwrap();

        let installed = link_unbundled_angle_libraries(&runtime, &exe_dir).unwrap();

        assert!(installed.contains(&"libGLESv2.dylib"));
        assert_eq!(
            fs::read_link(exe_dir.join("libGLESv2.dylib")).unwrap(),
            runtime
                .join(MACOS_FRAMEWORK)
                .join("Libraries")
                .join("libGLESv2.dylib")
        );
    }

    #[test]
    fn optional_angle_libraries_may_be_absent() {
        let dir = tmp();
        let libraries = dir
            .path()
            .join("rt")
            .join(MACOS_FRAMEWORK)
            .join("Libraries");
        fs::create_dir_all(&libraries).unwrap();

        for name in ANGLE_REQUIRED {
            fs::write(libraries.join(name), *name).unwrap();
        }

        let exe_dir = dir.path().join("bin");
        let installed = link_unbundled_angle_libraries(&dir.path().join("rt"), &exe_dir).unwrap();

        assert_eq!(installed.len(), ANGLE_REQUIRED.len());
        assert!(!exe_dir.join("libvk_swiftshader.dylib").exists());
    }

    #[test]
    fn missing_required_angle_library_is_rejected() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        fs::create_dir_all(&runtime).unwrap();

        match link_unbundled_angle_libraries(&runtime, &dir.path().join("bin")) {
            Err(CefError::InvalidRuntime { missing, .. }) => {
                assert_eq!(missing, "libEGL.dylib");
            }
            other => panic!("expected InvalidRuntime, got {other:?}"),
        }
    }
}
