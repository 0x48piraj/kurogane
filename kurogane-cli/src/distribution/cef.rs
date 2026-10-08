//! CEF files a bundle carries.
//!
//! A CEF installation also holds what builds against CEF use and the record
//! of the archive it came from. A bundle carries the runtime and CEF's
//! licence and credits.

use super::bootstrap::Bootstrap;

/// Files only builds against CEF use: headers, CMake files, the C++ wrapper's
/// sources and the import library.
const DEV_ARTIFACTS: &[&str] = &[
    "include",
    "cmake",
    "libcef_dll",
    "CMakeLists.txt",
    "libcef.lib",
];

/// Returns whether `name` records the download an installation came from.
fn is_download_record(name: &str) -> bool {
    name.eq_ignore_ascii_case("archive.json") || name.to_ascii_lowercase().ends_with(".tar.bz2")
}

/// Returns whether a file in a CEF distribution goes into a bundle.
///
/// Excludes development artifacts, download records and CEF's own sandbox
/// bootstraps. Names are compared without regard to ASCII case.
pub(crate) fn is_runtime_artifact(name: &str) -> bool {
    let is = |excluded: &str| excluded.eq_ignore_ascii_case(name);

    !DEV_ARTIFACTS.iter().any(|artifact| is(artifact))
        && !Bootstrap::ALL
            .iter()
            .any(|bootstrap| is(bootstrap.file_name()))
        && !is_download_record(name)
}

/// Returns libcef's path within a runtime on this platform.
#[cfg(test)]
pub(crate) fn cef_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "libcef.dll"
    } else if cfg!(target_os = "macos") {
        "Chromium Embedded Framework.framework/Chromium Embedded Framework"
    } else {
        "libcef.so"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::NOTICES;
    use std::fs;
    use std::path::Path;

    fn tmp() -> tempfile::TempDir {
        crate::distribution::test_fixtures::tmp_dir()
    }

    /// Copies `dist` to `dest` as a bundle does, runtime files only.
    fn copy_runtime(dist: &Path, dest: &Path) {
        crate::distribution::files::copy_dir_filtered(dist, dest, &is_runtime_artifact).unwrap();
    }

    #[test]
    fn flat_distribution_strips_development_material() {
        let dir = tmp();
        let dist = crate::distribution::test_fixtures::cef_runtime(&dir.path().join("managed"));
        fs::create_dir_all(dist.join("include").join("cef")).unwrap();
        fs::write(dist.join("include").join("cef").join("cef_app.h"), "h").unwrap();
        fs::create_dir_all(dist.join("cmake")).unwrap();
        fs::create_dir_all(dist.join("libcef_dll")).unwrap();
        fs::write(dist.join("CMakeLists.txt"), "cmake").unwrap();

        let dest = dir.path().join("runtime");
        copy_runtime(&dist, &dest);

        assert!(dest.join(cef_binary_name()).exists());
        assert!(!dest.join("include").exists());
        assert!(!dest.join("cmake").exists());
        assert!(!dest.join("libcef_dll").exists());
        assert!(!dest.join("CMakeLists.txt").exists());
    }

    #[test]
    fn a_bundle_passes_on_cef_s_licence_and_credits() {
        let dir = tmp();
        let dist = crate::distribution::test_fixtures::cef_runtime(&dir.path().join("managed"));
        for notice in NOTICES {
            fs::write(dist.join(notice), "notice").unwrap();
        }

        let dest = dir.path().join("runtime");
        copy_runtime(&dist, &dest);

        for notice in NOTICES {
            assert!(
                dest.join(notice).is_file(),
                "{notice} goes with the runtime"
            );
        }
    }

    #[test]
    fn distributions_leave_the_bootstraps_and_import_library_behind() {
        let dir = tmp();
        let dist = crate::distribution::test_fixtures::cef_runtime(&dir.path().join("managed"));
        for name in ["bootstrap.exe", "bootstrapc.exe", "libcef.lib"] {
            fs::write(dist.join(name), "build artifact").unwrap();
        }

        let dest = dir.path().join("runtime");
        copy_runtime(&dist, &dest);

        for name in ["bootstrap.exe", "bootstrapc.exe", "libcef.lib"] {
            assert!(!dest.join(name).exists(), "{name} is not loaded at runtime");
        }
        assert!(dest.join(cef_binary_name()).exists());
    }

    #[test]
    fn runtime_artifacts_are_told_apart_by_name() {
        for name in [
            "libcef.dll",
            "chrome_elf.dll",
            "icudtl.dat",
            "locales",
            "LICENSE.txt",
            "CREDITS.html",
        ] {
            assert!(is_runtime_artifact(name), "{name} goes into a bundle");
        }

        for name in [
            "bootstrap.exe",
            "bootstrapc.exe",
            "libcef.lib",
            "include",
            "CMakeLists.txt",
            "archive.json",
        ] {
            assert!(!is_runtime_artifact(name), "{name} stays out of a bundle");
        }
    }

    #[test]
    fn runtime_artifacts_ignore_case_like_windows_does() {
        assert!(!is_runtime_artifact("Bootstrap.exe"));
        assert!(!is_runtime_artifact("LIBCEF.LIB"));
    }

    #[test]
    fn flat_distribution_strips_download_records() {
        let dir = tmp();
        let dist = crate::distribution::test_fixtures::cef_runtime(&dir.path().join("managed"));
        fs::write(
            dist.join("archive.json"),
            r#"{"type":"minimal","name":"x.tar.bz2","sha1":"0"}"#,
        )
        .unwrap();
        fs::write(
            dist.join("cef_binary_1.2.3_linux64_minimal.tar.bz2"),
            "100MB of archive",
        )
        .unwrap();

        let dest = dir.path().join("runtime");
        copy_runtime(&dist, &dest);

        assert!(!dest.join("archive.json").exists());
        assert!(
            !dest
                .join("cef_binary_1.2.3_linux64_minimal.tar.bz2")
                .exists()
        );
        assert!(
            dest.join(cef_binary_name()).exists(),
            "runtime files unaffected"
        );
    }
}
