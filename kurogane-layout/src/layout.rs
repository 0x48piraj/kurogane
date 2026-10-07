//! Filesystem layout and low-level bundle utilities.
//!
//! This module owns bundled runtime discovery and recursive directory
//! copying and linking.
//!
//! It does not define package formats or application metadata.

use std::path::{Path, PathBuf};

/// Mirrors `src` into `dst`, reusing files where possible.
///
/// A staged CEF runtime is hundreds of megabytes of read-only files that are
/// already on disk, so it is hard linked rather than copied. Linking is
/// refused across volumes and on filesystems without hard links and each
/// file falls back to a copy.
///
/// `keep` is called for every entry by name at each directory level.
pub fn link_dir(src: &Path, dst: &Path, keep: &dyn Fn(&str) -> bool) -> std::io::Result<()> {
    mirror_dir(src, dst, keep, &link_file)
}

pub fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    copy_dir_filtered(src, dst, &|_| true)
}

/// Copies `src` into `dst`, leaving out every entry `keep` refuses.
///
/// `keep` is asked about every entry by name, at every level.
pub(crate) fn copy_dir_filtered(
    src: &Path,
    dst: &Path,
    keep: &dyn Fn(&str) -> bool,
) -> std::io::Result<()> {
    mirror_dir(src, dst, keep, &|src, dst| {
        std::fs::copy(src, dst).map(drop)
    })
}

/// Mirrors the directory structure from `src` into `dst`.
///
/// Kept files are passed to `place` for copying or linking.
fn mirror_dir(
    src: &Path,
    dst: &Path,
    keep: &dyn Fn(&str) -> bool,
    place: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();

        if !keep(&name.to_string_lossy()) {
            continue;
        }

        let path = entry.path();
        let dest = dst.join(&name);

        if path.is_dir() {
            mirror_dir(&path, &dest, keep, place)?;
        } else {
            place(&path, &dest)?;
        }
    }

    Ok(())
}

/// Links one file into place, leaving an up-to-date destination alone.
///
/// Reuses the destination when its metadata matches the source.
fn link_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    let source = std::fs::metadata(src)?;

    if is_same_file(&source, dst) {
        return Ok(());
    }

    // A stale link has to go before a fresh one can take its name
    match std::fs::remove_file(dst) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    if std::fs::hard_link(src, dst).is_ok() {
        return Ok(());
    }

    std::fs::copy(src, dst)?;

    Ok(())
}

/// Returns whether `dst` already holds what `source` describes.
fn is_same_file(source: &std::fs::Metadata, dst: &Path) -> bool {
    let Ok(existing) = std::fs::metadata(dst) else {
        return false;
    };

    if existing.len() != source.len() {
        return false;
    }

    match (existing.modified(), source.modified()) {
        (Ok(existing), Ok(source)) => existing == source,
        _ => false,
    }
}

/// Returns the `Contents` directory of the `.app` directly containing `exe`.
#[cfg(any(target_os = "macos", test))]
fn owning_bundle_contents(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;

    if macos.file_name()? != "MacOS"
        || contents.file_name()? != "Contents"
        || app.extension()? != "app"
    {
        return None;
    }

    Some(contents.to_path_buf())
}

/// Returns the enclosing application's `Contents` directory.
#[cfg(any(target_os = "macos", test))]
fn enclosing_application_contents(helper_contents: &Path) -> Option<PathBuf> {
    let frameworks = helper_contents.parent()?.parent()?;
    let contents = frameworks.parent()?;

    if frameworks.file_name()? != "Frameworks"
        || contents.file_name()? != "Contents"
        || contents.parent()?.extension()? != "app"
    {
        return None;
    }

    Some(contents.to_path_buf())
}

/// Returns the `Contents` directory for the application bundle.
///
/// Helper bundles resolve to their enclosing application bundle.
#[cfg(any(target_os = "macos", test))]
fn app_bundle_contents(exe: &Path) -> Option<PathBuf> {
    let contents = owning_bundle_contents(exe)?;

    Some(enclosing_application_contents(&contents).unwrap_or(contents))
}

/// Returns the bundled resource directory, if running from a bundle.
pub fn bundled_resource_root() -> Result<Option<PathBuf>, std::io::Error> {
    Ok(bundled_resource_root_for(&std::env::current_exe()?))
}

/// Returns the resource root for a bundled executable.
fn bundled_resource_root_for(exe: &Path) -> Option<PathBuf> {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let dir = exe.parent()?;

    #[cfg(target_os = "windows")]
    {
        // Flat bundle; executable, CEF and resources share a directory
        if dir.join("libcef.dll").exists() {
            return Some(dir.to_path_buf());
        }
    }

    #[cfg(target_os = "linux")]
    {
        // The executable is under `runtime/`; `cef/` identifies the bundle
        if dir.join("cef").is_dir() {
            return dir.parent().map(Path::to_path_buf);
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(contents) = app_bundle_contents(exe) {
            return Some(contents.join("Resources"));
        }
    }

    None
}

/// Returns the bundled macOS helper executable, if present.
pub fn bundled_helper_path() -> Result<Option<PathBuf>, std::io::Error> {
    #[cfg(target_os = "macos")]
    {
        Ok(bundled_helper_path_for(&std::env::current_exe()?))
    }

    #[cfg(not(target_os = "macos"))]
    Ok(None)
}

/// Returns the bundled helper executable for the application containing `exe`.
#[cfg(any(target_os = "macos", test))]
pub fn bundled_helper_path_for(exe: &Path) -> Option<PathBuf> {
    let contents = app_bundle_contents(exe)?;
    let name = contents.parent()?.file_stem()?.to_str()?;

    let helper = format!("{name} Helper");

    let path = contents
        .join("Frameworks")
        .join(format!("{helper}.app"))
        .join("Contents")
        .join("MacOS")
        .join(&helper);

    path.is_file().then_some(path)
}

/// The file `kurogane bundle` leaves beside a bundle's executable. Such an
/// application runs the Chromium runtime inside its bundle and no other.
pub(crate) const BUNDLE_MARKER: &str = "kurogane-bundle";

/// The Chromium runtime of the bundle `exe` belongs to, whether or not it is
/// still in place, or `None` when `exe` belongs to no bundle.
///
/// A bundle is a directory `kurogane bundle` made, which it marks with
/// [`BUNDLE_MARKER`] beside the executable, or a macOS application bundle.
pub(crate) fn bundle_cef_root_for(exe: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if let Some(contents) = app_bundle_contents(exe) {
            return Some(contents.join("Frameworks"));
        }
    }

    let dir = exe.parent()?;

    if !dir.join(BUNDLE_MARKER).is_file() {
        return None;
    }

    // Linux keeps the runtime in `cef/` beside the executable; Windows and
    // macOS directory bundles keep it beside the executable itself
    if cfg!(target_os = "linux") {
        Some(dir.join("cef"))
    } else {
        Some(dir.to_path_buf())
    }
}

/// The Chromium runtime of the bundle the running executable belongs to,
/// whether or not it is still in place; a bundle runs it and no other.
pub fn bundle_cef_root() -> std::io::Result<Option<PathBuf>> {
    Ok(bundle_cef_root_for(&std::env::current_exe()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_helper_resolves_to_the_application_that_owns_it() {
        assert_eq!(
            app_bundle_contents(Path::new(
                "/Apps/MyApp.app/Contents/Frameworks/MyApp Helper.app/Contents/MacOS/MyApp Helper"
            )),
            Some(PathBuf::from("/Apps/MyApp.app/Contents"))
        );
    }

    #[test]
    fn a_bundle_outside_frameworks_resolves_to_itself() {
        assert_eq!(
            app_bundle_contents(Path::new(
                "/Apps/Outer.app/Contents/Resources/Inner.app/Contents/MacOS/inner"
            )),
            Some(PathBuf::from(
                "/Apps/Outer.app/Contents/Resources/Inner.app/Contents"
            ))
        );
    }

    #[test]
    fn app_bundle_contents_found_for_a_bundled_executable() {
        assert_eq!(
            app_bundle_contents(Path::new("/Apps/MyApp.app/Contents/MacOS/myapp")),
            Some(PathBuf::from("/Apps/MyApp.app/Contents"))
        );
    }

    #[test]
    fn app_bundle_contents_rejects_non_bundle_layouts() {
        for exe in [
            "/proj/target/release/myapp",
            "/Apps/MyApp.app/Contents/myapp",
            "/Apps/MyApp/Contents/MacOS/myapp",
            "/myapp",
        ] {
            assert_eq!(
                app_bundle_contents(Path::new(exe)),
                None,
                "{exe} is not inside an .app bundle"
            );
        }
    }

    #[test]
    fn a_cargo_target_directory_is_not_a_bundle() {
        assert_eq!(bundled_resource_root().unwrap(), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_linux_bundle_resolves_to_the_bundle_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dist");
        let runtime = root.join("runtime");
        std::fs::create_dir_all(runtime.join("cef")).unwrap();

        assert_eq!(
            bundled_resource_root_for(&runtime.join("myapp")),
            Some(root)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_linux_executable_with_no_cef_sibling_is_not_a_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target").join("debug");
        std::fs::create_dir_all(&target).unwrap();

        assert_eq!(bundled_resource_root_for(&target.join("myapp")), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_windows_bundle_resolves_to_the_executable_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("libcef.dll"), b"").unwrap();

        assert_eq!(
            bundled_resource_root_for(&dir.path().join("myapp.exe")),
            Some(dir.path().to_path_buf())
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_windows_executable_with_no_libcef_is_not_a_bundle() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(
            bundled_resource_root_for(&dir.path().join("myapp.exe")),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_app_bundle_resolves_to_contents_resources() {
        assert_eq!(
            bundled_resource_root_for(Path::new("/Apps/MyApp.app/Contents/MacOS/myapp")),
            Some(PathBuf::from("/Apps/MyApp.app/Contents/Resources"))
        );
    }

    #[test]
    fn a_bundle_without_helpers_has_no_helper_path() {
        let dir = tempfile::tempdir().unwrap();
        let macos = dir.path().join("MyApp.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();

        assert_eq!(bundled_helper_path_for(&macos.join("myapp")), None);
    }

    #[test]
    fn the_helper_is_named_after_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("MyApp.app").join("Contents");
        let helper_exe = contents
            .join("Frameworks")
            .join("MyApp Helper.app")
            .join("Contents")
            .join("MacOS")
            .join("MyApp Helper");

        std::fs::create_dir_all(helper_exe.parent().unwrap()).unwrap();
        std::fs::write(&helper_exe, b"mach-o").unwrap();
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();

        assert_eq!(
            bundled_helper_path_for(&contents.join("MacOS").join("myapp")),
            Some(helper_exe)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_unbundled_macos_executable_is_not_a_bundle() {
        assert_eq!(
            bundled_resource_root_for(Path::new("/proj/target/release/myapp")),
            None
        );
    }

    #[test]
    fn the_marker_beside_an_executable_names_its_bundle_s_runtime() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(BUNDLE_MARKER), b"").unwrap();

        // The runtime itself may be gone: the marker alone decides
        let expected = if cfg!(target_os = "linux") {
            dir.path().join("cef")
        } else {
            dir.path().to_path_buf()
        };
        assert_eq!(
            bundle_cef_root_for(&dir.path().join("myapp")),
            Some(expected)
        );
    }

    #[test]
    fn a_runtime_beside_an_unmarked_executable_is_no_bundle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("libcef.dll"), b"").unwrap();
        std::fs::create_dir(dir.path().join("cef")).unwrap();

        assert_eq!(bundle_cef_root_for(&dir.path().join("myapp")), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_macos_application_bundle_needs_no_marker() {
        assert_eq!(
            bundle_cef_root_for(Path::new("/Apps/MyApp.app/Contents/MacOS/myapp")),
            Some(PathBuf::from("/Apps/MyApp.app/Contents/Frameworks"))
        );
    }
}
