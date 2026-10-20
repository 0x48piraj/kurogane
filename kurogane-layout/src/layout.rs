//! Bundle discovery for the running executable.
//!
//! A bundle is a directory marked with [`BUNDLE_MARKER`] beside its
//! executable, or a macOS application bundle. It runs the Chromium runtime and
//! resources inside it and no other.

use std::path::{Path, PathBuf};

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
    #[cfg(target_os = "macos")]
    {
        if let Some(contents) = app_bundle_contents(exe) {
            return Some(contents.join("Resources"));
        }
    }

    let dir = marked_bundle_dir(exe)?;

    // Linux keeps the executable in `runtime/`, below the bundle's root
    if cfg!(target_os = "linux") {
        dir.parent().map(Path::to_path_buf)
    } else {
        Some(dir.to_path_buf())
    }
}

/// A macOS application bundle without one helper app the runtime can start.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, thiserror::Error)]
#[error("{} in {}", helpers_found(.found), .frameworks.display())]
pub struct NoSingleHelper {
    /// The bundle's `Contents/Frameworks`.
    pub frameworks: PathBuf,

    /// Helper executables found there, none or several.
    pub found: Vec<PathBuf>,
}

/// Says what `found` holds for [`NoSingleHelper`]'s message.
#[cfg(any(target_os = "macos", test))]
fn helpers_found(found: &[PathBuf]) -> String {
    if found.is_empty() {
        return "no helper app".to_owned();
    }
    let names: Vec<String> = found
        .iter()
        .map(|exe| {
            exe.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    format!("more than one helper app ({})", names.join(", "))
}

/// Returns the helper executable of the macOS application bundle `exe`
/// belongs to, the one `<name> Helper.app` in its `Contents/Frameworks`
/// whatever the bundle is named. `None` outside an application bundle.
#[cfg(any(target_os = "macos", test))]
pub fn bundled_helper_path_for(exe: &Path) -> Result<Option<PathBuf>, NoSingleHelper> {
    let Some(contents) = app_bundle_contents(exe) else {
        return Ok(None);
    };
    let frameworks = contents.join("Frameworks");

    let mut found: Vec<PathBuf> = std::fs::read_dir(&frameworks)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| helper_executable(&entry.path()))
        .collect();
    found.sort();

    match <[PathBuf; 1]>::try_from(found) {
        Ok([helper]) => Ok(Some(helper)),
        Err(found) => Err(NoSingleHelper { frameworks, found }),
    }
}

/// Returns the executable of `app` when it is a bundle's main helper,
/// `<name> Helper.app/Contents/MacOS/<name> Helper`. Chromium derives the
/// other helpers (`<name> Helper (GPU).app` and the rest) from its path.
#[cfg(any(target_os = "macos", test))]
fn helper_executable(app: &Path) -> Option<PathBuf> {
    let name = app.file_name()?.to_str()?.strip_suffix(".app")?;
    if !name.ends_with(" Helper") {
        return None;
    }
    let exe = app.join("Contents").join("MacOS").join(name);
    exe.is_file().then_some(exe)
}

/// The file `kurogane bundle` and the sandbox's staged run leave beside a
/// bundle's executable. Such an application runs the Chromium runtime and
/// resources inside its bundle and no other.
pub const BUNDLE_MARKER: &str = "kurogane-bundle";

/// The directory of `exe` when [`BUNDLE_MARKER`] marks it as a bundle's.
fn marked_bundle_dir(exe: &Path) -> Option<&Path> {
    exe.parent().filter(|dir| dir.join(BUNDLE_MARKER).is_file())
}

/// The Chromium runtime of the bundle `exe` belongs to, whether or not it is
/// still in place, or `None` when `exe` belongs to no bundle.
///
/// A bundle is a directory marked with [`BUNDLE_MARKER`] beside the
/// executable, or a macOS application bundle.
pub fn bundle_cef_root_for(exe: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if let Some(contents) = app_bundle_contents(exe) {
            return Some(contents.join("Frameworks"));
        }
    }

    let dir = marked_bundle_dir(exe)?;

    // Linux keeps the runtime in `cef/` beside the executable; Windows keeps it
    // beside the executable itself
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
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join(BUNDLE_MARKER), b"").unwrap();

        assert_eq!(
            bundled_resource_root_for(&runtime.join("myapp")),
            Some(root)
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_bundle_resolves_its_resources_beside_its_runtime() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(BUNDLE_MARKER), b"").unwrap();

        let exe = dir.path().join("myapp");
        let resources = bundled_resource_root_for(&exe).unwrap();
        let runtime = bundle_cef_root_for(&exe).unwrap();

        assert!(
            runtime.starts_with(&resources),
            "one marker places both: resources {} and runtime {}",
            resources.display(),
            runtime.display()
        );
    }

    #[test]
    fn a_runtime_beside_an_unmarked_executable_has_no_resource_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("libcef.dll"), b"").unwrap();
        std::fs::write(dir.path().join("libcef.so"), b"").unwrap();
        std::fs::create_dir(dir.path().join("cef")).unwrap();

        assert_eq!(bundled_resource_root_for(&dir.path().join("myapp")), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_windows_bundle_resolves_to_the_executable_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(BUNDLE_MARKER), b"").unwrap();

        assert_eq!(
            bundled_resource_root_for(&dir.path().join("myapp.exe")),
            Some(dir.path().to_path_buf())
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

    /// Writes a helper app `<name>.app` into `frameworks` and returns its
    /// executable.
    fn write_helper(frameworks: &Path, name: &str) -> PathBuf {
        let exe = frameworks
            .join(format!("{name}.app"))
            .join("Contents")
            .join("MacOS")
            .join(name);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"mach-o").unwrap();
        exe
    }

    #[test]
    fn a_bundle_without_helpers_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("MyApp.app").join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();

        let refused = bundled_helper_path_for(&contents.join("MacOS").join("myapp")).unwrap_err();
        assert!(refused.found.is_empty());
        assert_eq!(refused.frameworks, contents.join("Frameworks"));
    }

    #[test]
    fn the_helper_is_found_whatever_the_bundle_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("MyApp 2.app").join("Contents");
        let frameworks = contents.join("Frameworks");
        let helper = write_helper(&frameworks, "MyApp Helper");
        for variant in [
            "MyApp Helper (GPU)",
            "MyApp Helper (Renderer)",
            "MyApp Helper (Plugin)",
        ] {
            write_helper(&frameworks, variant);
        }
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();

        let exe = contents.join("MacOS").join("myapp");
        assert_eq!(bundled_helper_path_for(&exe).unwrap(), Some(helper.clone()));
        assert_eq!(
            bundled_helper_path_for(&helper).unwrap(),
            Some(helper.clone()),
            "a helper finds it through the bundle it belongs to"
        );
    }

    #[test]
    fn several_helpers_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("MyApp.app").join("Contents");
        let frameworks = contents.join("Frameworks");
        let first = write_helper(&frameworks, "MyApp Helper");
        let second = write_helper(&frameworks, "Other Helper");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();

        let refused = bundled_helper_path_for(&contents.join("MacOS").join("myapp")).unwrap_err();
        assert_eq!(refused.found, [first, second]);
        assert!(
            refused
                .to_string()
                .starts_with("more than one helper app (MyApp Helper, Other Helper)")
        );
    }

    #[test]
    fn an_unbundled_executable_has_no_helper() {
        assert_eq!(
            bundled_helper_path_for(Path::new("/proj/target/release/myapp")).unwrap(),
            None
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
