//! CEF distribution resolution, provenance and validation.
//!
//! This module knows how to recognize the CEF distribution a bundle packages,
//! validate its platform and version metadata and tell its runtime files from
//! the rest.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bootstrap::Bootstrap;

/// The source of a resolved CEF distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CefSource {
    /// The distribution `CEF_PATH` names.
    CefPath,

    /// tetsu's shared installation of the CEF version.
    Installed,
}

impl CefSource {
    /// What to do about a distribution from here without provenance.
    fn unverifiable_advice(self) -> &'static str {
        match self {
            Self::CefPath => {
                "Point CEF_PATH at a distribution `export-cef-dir` wrote, or unset it to \
                 package the installed one."
            }
            Self::Installed => "Run `kurogane install` to reinstall it.",
        }
    }
}

/// The CEF distribution `CEF_PATH` names, when it is set and not empty.
pub fn cef_path() -> Option<PathBuf> {
    std::env::var_os("CEF_PATH")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Provenance information for a CEF distribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CefProvenance {
    /// The CEF version.
    pub cef_version: String,

    /// The Chromium version, when available.
    pub chromium_version: Option<String>,

    /// The target platform, when available.
    pub platform: Option<String>,

    /// The distribution type.
    pub distribution: String,

    /// The source artifact name.
    pub artifact: String,
}

impl CefProvenance {
    /// Returns whether the provenance matches the requested CEF version.
    pub fn matches_version(&self, expected: &str) -> bool {
        self.cef_version == expected
            || self
                .cef_version
                .strip_prefix(expected)
                .is_some_and(|rest| rest.starts_with('+'))
    }

    /// Returns whether the provenance matches the current target platform.
    pub fn matches_current_platform(&self) -> bool {
        match (self.platform.as_deref(), current_platform_name()) {
            // Unknown platform information cannot prove a mismatch
            (_, None) | (None, _) => true,
            (Some(mine), Some(current)) => mine == current,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ArchiveJson {
    #[serde(rename = "type")]
    file_type: String,
    name: String,
}

/// Reads provenance information from a CEF distribution.
pub fn read_provenance(root: &Path) -> Result<Option<CefProvenance>, CefError> {
    let path = root.join("archive.json");
    if !path.exists() {
        return Ok(None);
    }

    let file = fs::File::open(&path).map_err(CefError::io("read", &path))?;
    let archive: ArchiveJson =
        serde_json::from_reader(file).map_err(|e| CefError::InvalidDistribution {
            root: root.to_path_buf(),
            reason: format!("unreadable archive.json: {e}"),
        })?;

    Ok(
        parse_archive_name(&archive.name).map(|(cef_version, chromium_version, platform)| {
            CefProvenance {
                cef_version,
                chromium_version,
                platform,
                distribution: archive.file_type,
                artifact: archive.name,
            }
        }),
    )
}

/// Parses a CEF archive filename.
/// Format: `cef_binary_<ver>+g<rev>+chromium-<cv>_<platform>_<dist>.tar.bz2`
fn parse_archive_name(name: &str) -> Option<(String, Option<String>, Option<String>)> {
    let stem = name.strip_suffix(".tar.bz2")?;
    let rest = stem.strip_prefix("cef_binary_")?;

    let (cef_version, tail) = rest.split_once("+chromium-")?;

    let mut parts = tail.rsplitn(3, '_');
    let _distribution = parts.next()?;
    let platform = parts.next().map(str::to_string);
    let chromium = parts.next().map(str::to_string);

    Some((cef_version.to_string(), chromium, platform))
}

/// Returns the CEF platform name for the current target.
pub fn current_platform_name() -> Option<&'static str> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Some("linux64")
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        Some("linuxarm64")
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        Some("linuxarm")
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Some("windows64")
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        Some("windowsarm64")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some("macosarm64")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Some("macosx64")
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "arm"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
    )))]
    {
        None
    }
}

/// A CEF distribution resolved for release packaging.
#[derive(Debug, Clone)]
pub struct ResolvedCef {
    /// The distribution root.
    pub root: PathBuf,

    /// The source of the distribution.
    pub source: CefSource,

    /// Provenance information.
    pub provenance: CefProvenance,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CefError {
    #[error("CEF {version} is not installed at {}; run `kurogane install`", .path.display())]
    NotInstalled { version: String, path: PathBuf },

    #[error("CEF_PATH names {}, which is not a directory", .0.display())]
    CefPathMissing(PathBuf),

    #[error(
        "{} has no archive.json naming its CEF build, so it cannot be verified. {}",
        .path.display(),
        .from.unverifiable_advice()
    )]
    Unverifiable { path: PathBuf, from: CefSource },

    #[error("CEF version mismatch at {path}: expected {expected}, found {found}")]
    VersionMismatch {
        expected: String,
        found: String,
        path: PathBuf,
    },

    #[error("CEF platform mismatch at {path}: expected {expected}, found {found}")]
    PlatformMismatch {
        expected: String,
        found: String,
        path: PathBuf,
    },

    #[error("invalid CEF distribution at {root}: {reason}")]
    InvalidDistribution { root: PathBuf, reason: String },

    #[error("invalid CEF runtime at {root}: missing {missing}")]
    InvalidRuntime { root: PathBuf, missing: String },

    /// A file operation failed: what was being done, and to which path.
    #[error("failed to {action} {}", .path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl CefError {
    /// An I/O error of the file operation `action` on `path`.
    pub(crate) fn io(action: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.to_path_buf();
        move |source| Self::Io {
            action,
            path,
            source,
        }
    }
}

/// Resolves and validates a CEF distribution root.
///
/// The selected distribution must provide verifiable provenance, match the
/// requested CEF version and current target platform and have a recognized
/// CEF distribution layout.
fn resolve_provenanced_root(
    root: PathBuf,
    version: &str,
    from: CefSource,
) -> Result<CefProvenance, CefError> {
    let provenance = read_provenance(&root)?.ok_or_else(|| CefError::Unverifiable {
        path: root.clone(),
        from,
    })?;

    verify_provenanced_version_and_platform(&provenance, &root, version)?;

    validate_distribution(&root)?;

    Ok(provenance)
}

/// Resolves the CEF distribution a bundle packages, the one `CEF_PATH`
/// names, else the installation at `installed` of CEF `version`.
pub fn resolve_cef_for_bundle(version: &str, installed: &Path) -> Result<ResolvedCef, CefError> {
    resolve_cef(version, cef_path(), installed)
}

/// Checks that `root` is a complete installation of CEF `version` whose
/// `archive.json` names that version and this platform.
pub fn verify_installation(root: &Path, version: &str) -> Result<CefProvenance, CefError> {
    let provenance = resolve_provenanced_root(root.to_path_buf(), version, CefSource::Installed)?;
    validate_cef_runtime(root)?;

    Ok(provenance)
}

/// Resolves the CEF distribution for release packaging.
///
/// `CEF_PATH` comes before the installation. Both are validated for
/// provenance, version, platform and distribution layout. A set `CEF_PATH`
/// naming no directory is an error rather than a fallback to the
/// installation.
fn resolve_cef(
    version: &str,
    cef_path: Option<PathBuf>,
    installed: &Path,
) -> Result<ResolvedCef, CefError> {
    if let Some(root) = cef_path {
        if !root.is_dir() {
            return Err(CefError::CefPathMissing(root));
        }

        let provenance = resolve_provenanced_root(root.clone(), version, CefSource::CefPath)?;

        return Ok(ResolvedCef {
            root,
            source: CefSource::CefPath,
            provenance,
        });
    }

    if installed.is_dir() {
        let root = installed.to_path_buf();
        let provenance = resolve_provenanced_root(root.clone(), version, CefSource::Installed)?;

        return Ok(ResolvedCef {
            root,
            source: CefSource::Installed,
            provenance,
        });
    }

    Err(CefError::NotInstalled {
        version: version.to_string(),
        path: installed.to_path_buf(),
    })
}

/// Checks provenance version and platform against expectations.
fn verify_provenanced_version_and_platform(
    provenance: &CefProvenance,
    root: &Path,
    version: &str,
) -> Result<(), CefError> {
    if !provenance.matches_version(version) {
        return Err(CefError::VersionMismatch {
            expected: version.to_string(),
            found: provenance.cef_version.clone(),
            path: root.to_path_buf(),
        });
    }

    if !provenance.matches_current_platform() {
        return Err(CefError::PlatformMismatch {
            expected: current_platform_name().unwrap_or("unknown").to_string(),
            found: provenance
                .platform
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            path: root.to_path_buf(),
        });
    }

    Ok(())
}

/// Development-only artifacts.
///
/// Headers, CMake files and the import library used to build against CEF.
const DEV_ARTIFACTS: &[&str] = &[
    "include",
    "cmake",
    "libcef_dll",
    "CMakeLists.txt",
    "CREDITS.html",
    "libcef.lib",
];

/// Download-cache residue.
fn is_download_cache_artifact(name: &str) -> bool {
    name == "archive.json" || name.ends_with(".tar.bz2")
}

/// Returns whether a file in a CEF distribution is part of the runtime.
///
/// Excludes development artifacts, download-cache files and CEF's own
/// sandbox bootstraps. Names are compared without regard to ASCII case.
pub(crate) fn is_runtime_artifact(name: &str) -> bool {
    let is = |excluded: &str| excluded.eq_ignore_ascii_case(name);

    !DEV_ARTIFACTS.iter().any(|artifact| is(artifact))
        && !Bootstrap::ALL
            .iter()
            .any(|bootstrap| is(bootstrap.file_name()))
        && !is_download_cache_artifact(name)
}

pub(crate) fn cef_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "libcef.dll"
    } else if cfg!(target_os = "macos") {
        "Chromium Embedded Framework.framework/Chromium Embedded Framework"
    } else {
        "libcef.so"
    }
}

/// Validates that a directory is a CEF distribution as tetsu writes it, with
/// libcef at its root.
pub fn validate_distribution(root: &Path) -> Result<(), CefError> {
    if !root.is_dir() {
        return Err(CefError::InvalidDistribution {
            root: root.to_path_buf(),
            reason: "not a directory".into(),
        });
    }

    if root.join(cef_binary_name()).exists() {
        Ok(())
    } else {
        Err(CefError::InvalidDistribution {
            root: root.to_path_buf(),
            reason: format!(
                "no {} at its root, as `export-cef-dir` and `kurogane install` lay it out",
                cef_binary_name()
            ),
        })
    }
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
pub fn validate_cef_runtime(runtime: &Path) -> Result<(), CefError> {
    validate_cef_runtime_for(runtime, current_platform())
}

/// Validates a CEF runtime against a platform's expected layout.
fn validate_cef_runtime_for(runtime: &Path, platform: Platform) -> Result<(), CefError> {
    let mut missing: Vec<&'static str> = Vec::new();

    let require = |missing: &mut Vec<&'static str>, name: &'static str| {
        if !runtime.join(name).exists() {
            missing.push(name);
        }
    };

    match platform {
        Platform::Windows => {
            require(&mut missing, "libcef.dll");
            require(&mut missing, "chrome_elf.dll");
            require(&mut missing, "icudtl.dat");
            require(&mut missing, "locales");

            if !V8_SNAPSHOTS.iter().any(|s| runtime.join(s).exists()) {
                missing.push("v8_context_snapshot.bin");
            }
        }
        Platform::MacOs => {
            require(
                &mut missing,
                "Chromium Embedded Framework.framework/Chromium Embedded Framework",
            );

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
            require(&mut missing, "libcef.so");
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
        Err(CefError::InvalidRuntime {
            root: runtime.to_path_buf(),
            missing: missing.join(", "),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        crate::test_fixtures::tmp_dir()
    }

    // Provenance parsing

    #[test]
    fn parses_official_archive_name() {
        let name = "cef_binary_1.2.3+g6a8d2b7+chromium-131.0.6778.204_linux64_minimal.tar.bz2";
        let (cef, chromium, platform) = parse_archive_name(name).unwrap();
        assert_eq!(cef, "1.2.3+g6a8d2b7");
        assert_eq!(chromium.as_deref(), Some("131.0.6778.204"));
        assert_eq!(platform.as_deref(), Some("linux64"));
    }

    #[test]
    fn rejects_non_archive_names() {
        assert!(parse_archive_name("random.tar.bz2").is_none());
        assert!(parse_archive_name("cef_binary_1.2.3_linux64_minimal.zip").is_none());
    }

    #[test]
    fn version_match_accepts_full_and_prefix() {
        let p = CefProvenance {
            cef_version: "1.2.3+g6a8d2b7".into(),
            chromium_version: None,
            platform: Some("linux64".into()),
            distribution: "minimal".into(),
            artifact: "x.tar.bz2".into(),
        };
        assert!(p.matches_version("1.2.3"));
        assert!(p.matches_version("1.2.3+g6a8d2b7"));
        assert!(!p.matches_version("127.1.1"));
        assert!(!p.matches_version("131.3"));
    }

    // Distribution validation

    #[test]
    fn flat_distribution_shape_is_valid() {
        let dir = tmp();

        let binary = dir.path().join(cef_binary_name());
        if let Some(parent) = binary.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&binary, "").unwrap();

        assert!(validate_distribution(dir.path()).is_ok());
    }

    #[test]
    fn unrecognized_directory_is_invalid_distribution() {
        let dir = tmp();
        fs::create_dir_all(dir.path().join("stuff")).unwrap();
        let err = validate_distribution(dir.path()).unwrap_err();
        assert!(matches!(err, CefError::InvalidDistribution { .. }));
    }

    // Runtime files, as a bundle copies them

    /// Copies `dist` to `dest` as a bundle does, runtime files only.
    fn copy_runtime(dist: &Path, dest: &Path) {
        crate::layout::copy_dir_filtered(dist, dest, &is_runtime_artifact).unwrap();
    }

    #[test]
    fn flat_distribution_strips_development_material() {
        let dir = tmp();
        let dist = crate::test_fixtures::cef_runtime(&dir.path().join("managed"));
        fs::create_dir_all(dist.join("include").join("cef")).unwrap();
        fs::write(dist.join("include").join("cef").join("cef_app.h"), "h").unwrap();
        fs::create_dir_all(dist.join("cmake")).unwrap();
        fs::create_dir_all(dist.join("libcef_dll")).unwrap();
        fs::write(dist.join("CMakeLists.txt"), "cmake").unwrap();
        fs::write(dist.join("CREDITS.html"), "credits").unwrap();

        let dest = dir.path().join("runtime");
        copy_runtime(&dist, &dest);

        assert!(dest.join(cef_binary_name()).exists());
        assert!(!dest.join("include").exists());
        assert!(!dest.join("cmake").exists());
        assert!(!dest.join("libcef_dll").exists());
        assert!(!dest.join("CMakeLists.txt").exists());
        assert!(!dest.join("CREDITS.html").exists());
    }

    #[test]
    fn distributions_leave_the_bootstraps_and_import_library_behind() {
        let dir = tmp();
        let dist = crate::test_fixtures::cef_runtime(&dir.path().join("managed"));
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
        for name in ["libcef.dll", "chrome_elf.dll", "icudtl.dat", "locales"] {
            assert!(is_runtime_artifact(name), "{name} is loaded at runtime");
        }

        for name in [
            "bootstrap.exe",
            "bootstrapc.exe",
            "libcef.lib",
            "include",
            "CMakeLists.txt",
            "archive.json",
        ] {
            assert!(
                !is_runtime_artifact(name),
                "{name} is not loaded at runtime"
            );
        }
    }

    #[test]
    fn runtime_artifacts_ignore_case_like_windows_does() {
        assert!(!is_runtime_artifact("Bootstrap.exe"));
        assert!(!is_runtime_artifact("LIBCEF.LIB"));
    }

    #[test]
    fn flat_distribution_strips_download_cache_residue() {
        let dir = tmp();
        let dist = crate::test_fixtures::cef_runtime(&dir.path().join("managed"));
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
            Err(CefError::InvalidRuntime { missing, .. }) => {
                assert!(missing.contains(cef_binary_name()));
                assert!(missing.contains("icudtl.dat"));
                if cfg!(target_os = "macos") {
                    assert!(!missing.contains("locales"));
                    assert!(missing.contains("*.lproj"));
                } else {
                    assert!(missing.contains("locales"));
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
        fs::write(runtime.join(cef_binary_name()), "").unwrap();
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
            Err(CefError::InvalidRuntime { .. })
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
            Err(CefError::InvalidRuntime { missing, .. }) => {
                assert!(missing.contains("v8 snapshot"));
            }
            other => panic!("expected InvalidRuntime, got {other:?}"),
        }

        // Missing locale bundle is reported
        fs::write(resources.join("v8_context_snapshot.arm64.bin"), "v8").unwrap();
        fs::remove_dir_all(resources.join("en.lproj")).unwrap();
        match validate_cef_runtime_for(&runtime, Platform::MacOs) {
            Err(CefError::InvalidRuntime { missing, .. }) => {
                assert!(missing.contains("*.lproj"));
            }
            other => panic!("expected InvalidRuntime, got {other:?}"),
        }
    }

    // Resolution policy
    //
    // Tests pass CEF_PATH and the installation in as values, so no test
    // mutates process-global environment state.

    #[test]
    fn resolution_fails_without_an_installation_or_cef_path() {
        let dir = tmp();
        let err = resolve_cef("0.0.0-nonexistent", None, &dir.path().join("absent")).unwrap_err();
        assert!(matches!(err, CefError::NotInstalled { .. }));
    }

    #[test]
    fn an_unverifiable_cef_path_is_rejected() {
        let dir = tmp();
        let fake = dir.path().join("dev-cef");
        crate::test_fixtures::cef_runtime(&fake); // looks like CEF but has no archive.json

        let err = resolve_cef("1.2.3", Some(fake.clone()), &dir.path().join("absent")).unwrap_err();

        assert!(matches!(
            err,
            CefError::Unverifiable {
                from: CefSource::CefPath,
                ..
            }
        ));
    }

    #[test]
    fn a_version_mismatched_cef_path_is_rejected() {
        let dir = tmp();
        let fake = crate::test_fixtures::cef_runtime(&dir.path().join("dev-cef"));
        fs::write(
            fake.join("archive.json"),
            r#"{"type":"minimal","name":"cef_binary_127.1.1+gabcdef+chromium-127.0.1.2_linux64_minimal.tar.bz2","sha1":"x"}"#,
        )
        .unwrap();

        let err = resolve_cef("1.2.3", Some(fake.clone()), &dir.path().join("absent")).unwrap_err();

        assert!(matches!(err, CefError::VersionMismatch { .. }));
    }

    #[test]
    fn a_verified_cef_path_with_matching_provenance_is_accepted() {
        let dir = tmp();
        let fake = provenance_fixture(&dir.path().join("dev"));

        let resolved =
            resolve_cef("1.2.3", Some(fake.clone()), &dir.path().join("absent")).unwrap();

        assert_eq!(resolved.source, CefSource::CefPath);
        assert_eq!(
            resolved.provenance.chromium_version.as_deref(),
            Some("131.0.6778.204")
        );
    }

    // The installation

    fn provenance_fixture(dir: &Path) -> PathBuf {
        let installed = crate::test_fixtures::cef_runtime(&dir.join("installed"));
        let platform = current_platform_name().unwrap_or("linux64");
        let archive_name =
            format!("cef_binary_1.2.3+g6a8d2b7+chromium-131.0.6778.204_{platform}_minimal.tar.bz2");
        fs::write(
            installed.join("archive.json"),
            serde_json::json!({ "type": "minimal", "name": archive_name, "sha1": "x" }).to_string(),
        )
        .unwrap();
        installed
    }

    #[test]
    fn a_valid_installation_is_accepted_with_provenance() {
        let dir = tmp();
        let installed = provenance_fixture(dir.path());

        let resolved = resolve_cef("1.2.3", None, &installed).unwrap();

        assert_eq!(resolved.source, CefSource::Installed);
        assert_eq!(resolved.root, installed);
        assert_eq!(resolved.provenance.cef_version, "1.2.3+g6a8d2b7");
    }

    #[test]
    fn a_verified_installation_names_its_version() {
        let dir = tmp();
        let installed = provenance_fixture(dir.path());

        let provenance = verify_installation(&installed, "1.2.3").unwrap();

        assert_eq!(provenance.cef_version, "1.2.3+g6a8d2b7");
    }

    #[test]
    fn an_installation_is_not_verified_without_archive_json() {
        let dir = tmp();
        let installed = crate::test_fixtures::cef_runtime(&dir.path().join("installed"));

        assert!(matches!(
            verify_installation(&installed, "1.2.3"),
            Err(CefError::Unverifiable {
                from: CefSource::Installed,
                ..
            })
        ));
    }

    #[test]
    fn an_installation_of_another_version_is_not_verified() {
        let dir = tmp();
        let installed = provenance_fixture(dir.path());

        assert!(matches!(
            verify_installation(&installed, "1.2.4"),
            Err(CefError::VersionMismatch { .. })
        ));
    }

    #[test]
    fn an_incomplete_installation_is_not_verified_despite_its_archive_json() {
        let dir = tmp();
        let installed = provenance_fixture(dir.path());
        let icu = if cfg!(target_os = "macos") {
            installed.join("Chromium Embedded Framework.framework/Resources/icudtl.dat")
        } else {
            installed.join("icudtl.dat")
        };
        fs::remove_file(icu).unwrap();

        assert!(matches!(
            verify_installation(&installed, "1.2.3"),
            Err(CefError::InvalidRuntime { .. })
        ));
    }

    #[test]
    fn an_installation_without_provenance_is_rejected() {
        let dir = tmp();
        let installed = crate::test_fixtures::cef_runtime(&dir.path().join("installed"));

        let err = resolve_cef("1.2.3", None, &installed).unwrap_err();

        assert!(
            matches!(
                err,
                CefError::Unverifiable { ref path, from: CefSource::Installed } if path == &installed
            ),
            "expected Unverifiable, got: {err}"
        );
    }

    #[test]
    fn a_version_mismatched_installation_is_rejected() {
        let dir = tmp();
        let installed = provenance_fixture(dir.path());

        let err = resolve_cef("127.1.1", None, &installed).unwrap_err();

        assert!(
            matches!(err, CefError::VersionMismatch { .. }),
            "expected VersionMismatch, got: {err}"
        );
    }

    #[test]
    fn a_platform_mismatched_installation_is_rejected() {
        let dir = tmp();
        let installed = crate::test_fixtures::cef_runtime(&dir.path().join("installed"));
        let wrong_platform = if current_platform_name() == Some("linux64") {
            "windowsarm64"
        } else {
            "linux64"
        };
        let archive_name = format!(
            "cef_binary_1.2.3+g6a8d2b7+chromium-131.0.6778.204_{wrong_platform}_minimal.tar.bz2"
        );
        fs::write(
            installed.join("archive.json"),
            serde_json::json!({ "type": "minimal", "name": archive_name, "sha1": "x" }).to_string(),
        )
        .unwrap();

        let err = resolve_cef("1.2.3", None, &installed).unwrap_err();

        assert!(
            matches!(err, CefError::PlatformMismatch { .. }),
            "expected PlatformMismatch, got: {err}"
        );
    }

    #[test]
    fn cef_path_comes_before_the_installation() {
        let dir = tmp();
        let cef_path = provenance_fixture(&dir.path().join("cef-path"));
        let installed = provenance_fixture(&dir.path().join("installed"));

        let resolved = resolve_cef("1.2.3", Some(cef_path.clone()), &installed).unwrap();

        assert_eq!(resolved.source, CefSource::CefPath);
        assert_eq!(resolved.root, cef_path);
    }

    #[test]
    fn a_missing_cef_path_is_an_error_even_with_an_installation() {
        let dir = tmp();
        let installed = provenance_fixture(&dir.path().join("installed"));
        let missing = dir.path().join("does-not-exist");

        let err = resolve_cef("1.2.3", Some(missing.clone()), &installed).unwrap_err();

        assert!(
            matches!(err, CefError::CefPathMissing(ref p) if p == &missing),
            "expected CefPathMissing, got: {err}"
        );
    }
}
