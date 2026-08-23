//! CEF distribution resolution, provenance, validation and runtime materialization.
//!
//! This module knows how to recognize CEF distributions, validate their
//! platform and version metadata and produce the runnable CEF runtime used
//! by packaged applications.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::layout::copy_dir;

/// The source of a resolved CEF distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CefSource {
    // A managed CEF installation
    ManagedCache,

    // A CEF distribution supplied through `CEF_PATH`
    EnvironmentOverride,
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

    let file = fs::File::open(&path)?;
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

    /// Provenance information, when available.
    pub provenance: Option<CefProvenance>,
}

#[derive(Debug, Error)]
pub enum CefError {
    #[error("no usable CEF distribution — run `kurogane install` (expected {expected} at {path})")]
    NotFound { expected: String, path: PathBuf },

    #[error("CEF_PATH does not exist: {0}")]
    OverrideMissing(PathBuf),

    #[error(
        "CEF_PATH has no archive.json provenance; refusing to package an unverifiable CEF tree ({0}). \
         Run `kurogane install` or point CEF_PATH at a managed installation."
    )]
    UnverifiableOverride(PathBuf),

    #[error(
        "managed CEF installation at {0} has no archive.json provenance; refusing to package an \
         unverifiable CEF tree. Re-run `kurogane install`."
    )]
    UnverifiableManaged(PathBuf),

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

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Resolves and validates a CEF distribution root.
///
/// The selected distribution must provide verifiable provenance, match the
/// requested CEF version and current target platform and have a recognized
/// CEF distribution layout.
fn resolve_provenanced_root(
    root: PathBuf,
    version: &str,
    unverifiable: fn(PathBuf) -> CefError,
) -> Result<CefProvenance, CefError> {
    let provenance = read_provenance(&root)?.ok_or_else(|| unverifiable(root.clone()))?;

    verify_provenanced_version_and_platform(&provenance, &root, version)?;

    validate_distribution(&root)?;

    Ok(provenance)
}

/// Resolves the CEF distribution for release packaging.
pub fn resolve_cef_for_bundle(version: &str) -> Result<ResolvedCef, CefError> {
    resolve_cef(
        version,
        || std::env::var("CEF_PATH").ok(),
        crate::layout::installed_cef_root,
    )
}

/// Resolves the CEF distribution for release packaging.
///
/// An explicit `CEF_PATH` is preferred over the managed installation. Both
/// sources are validated for provenance, version, platform and distribution
/// layout. An invalid `CEF_PATH` causes resolution to fail rather than falling
/// back to the managed installation.
fn resolve_cef(
    version: &str,
    override_path: impl Fn() -> Option<String>,
    installed_root: impl Fn(&str) -> Option<PathBuf>,
) -> Result<ResolvedCef, CefError> {
    // Environment override takes precedence; a set-but-broken override is an
    // error rather than a silent fallback to the managed installation
    if let Some(path) = override_path() {
        let root = PathBuf::from(path);
        if !root.exists() {
            return Err(CefError::OverrideMissing(root));
        }

        let provenance =
            resolve_provenanced_root(root.clone(), version, CefError::UnverifiableOverride)?;

        return Ok(ResolvedCef {
            root,
            source: CefSource::EnvironmentOverride,
            provenance: Some(provenance),
        });
    }

    if let Some(root) = installed_root(version) {
        let provenance =
            resolve_provenanced_root(root.clone(), version, CefError::UnverifiableManaged)?;

        return Ok(ResolvedCef {
            root,
            source: CefSource::ManagedCache,
            provenance: Some(provenance),
        });
    }

    Err(CefError::NotFound {
        expected: version.to_string(),
        path: crate::layout::cef_install_dir(version),
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
const DEV_ARTIFACTS: &[&str] = &[
    "include",
    "cmake",
    "libcef_dll",
    "CMakeLists.txt",
    "CREDITS.html",
];

/// Download-cache residue.
fn is_download_cache_artifact(name: &str) -> bool {
    name == "archive.json" || name.ends_with(".tar.bz2")
}

fn libcef_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "libcef.dll"
    } else {
        "libcef.so"
    }
}

/// Validates that a directory has a recognized CEF distribution shape.
pub fn validate_distribution(root: &Path) -> Result<(), CefError> {
    if !root.is_dir() {
        return Err(CefError::InvalidDistribution {
            root: root.to_path_buf(),
            reason: "not a directory".into(),
        });
    }

    let raw_shape = root.join("Release").is_dir() && root.join("Resources").is_dir();
    let flat_shape = root.join(libcef_name()).exists();

    if raw_shape || flat_shape {
        Ok(())
    } else {
        Err(CefError::InvalidDistribution {
            root: root.to_path_buf(),
            reason: format!("neither Release/+Resources/ nor {} found", libcef_name()),
        })
    }
}

/// Prepares the runtime files required by a packaged application.
pub fn materialize_cef_runtime(
    distribution_root: &Path,
    destination: &Path,
) -> Result<PathBuf, CefError> {
    if destination.exists() && validate_cef_runtime(destination).is_ok() {
        return Ok(destination.to_path_buf());
    }

    if destination.exists() {
        fs::remove_dir_all(destination)?;
    }

    let release = distribution_root.join("Release");
    let resources = distribution_root.join("Resources");

    if release.is_dir() && resources.is_dir() {
        // Raw official distribution
        copy_dir(&release, destination)?;
        merge_copy_dir(&resources, destination)?;
    } else if distribution_root.join(libcef_name()).exists() {
        // Already-flattened distribution
        fs::create_dir_all(destination)?;

        for entry in fs::read_dir(distribution_root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if DEV_ARTIFACTS.iter().any(|d| d == &name_str) || is_download_cache_artifact(&name_str)
            {
                continue;
            }

            let path = entry.path();
            let dest = destination.join(&name);
            if path.is_dir() {
                copy_dir(&path, &dest)?;
            } else {
                fs::copy(&path, &dest)?;
            }
        }
    } else {
        return Err(CefError::InvalidDistribution {
            root: distribution_root.to_path_buf(),
            reason: format!("neither Release/+Resources/ nor {} found", libcef_name()),
        });
    }

    validate_cef_runtime(destination)?;
    Ok(destination.to_path_buf())
}

fn merge_copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let dest = dst.join(entry.file_name());
        let path = entry.path();

        if path.is_dir() {
            copy_dir(&path, &dest)?;
        } else {
            fs::copy(&path, &dest)?;
        }
    }

    Ok(())
}

/// V8 snapshot file names across CEF versions.
const V8_SNAPSHOTS: &[&str] = &["v8_context_snapshot.bin", "snapshot_blob.bin"];

/// Validates the required files in a CEF runtime.
pub fn validate_cef_runtime(runtime: &Path) -> Result<(), CefError> {
    let mut missing: Vec<&'static str> = Vec::new();

    let require = |missing: &mut Vec<&'static str>, name: &'static str| {
        if !runtime.join(name).exists() {
            missing.push(name);
        }
    };

    if cfg!(target_os = "windows") {
        require(&mut missing, "libcef.dll");
        require(&mut missing, "chrome_elf.dll");
    } else {
        require(&mut missing, "libcef.so");
        require(&mut missing, "chrome-sandbox");
    }

    require(&mut missing, "icudtl.dat");
    require(&mut missing, "locales");

    if !V8_SNAPSHOTS.iter().any(|s| runtime.join(s).exists()) {
        missing.push("v8_context_snapshot.bin");
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
pub(crate) fn write_runtime_fixture(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("create fixture dir");
    let libcef = if cfg!(target_os = "windows") {
        dir.join("libcef.dll")
    } else {
        dir.join("libcef.so")
    };
    fs::write(libcef, "cef").unwrap();
    fs::write(dir.join("icudtl.dat"), "icu").unwrap();
    fs::write(dir.join("v8_context_snapshot.bin"), "v8").unwrap();
    fs::create_dir_all(dir.join("locales")).unwrap();
    fs::write(dir.join("locales").join("en-US.pak"), "pak").unwrap();
    if cfg!(target_os = "windows") {
        fs::write(dir.join("chrome_elf.dll"), "elf").unwrap();
    } else {
        fs::write(dir.join("chrome-sandbox"), "sandbox").unwrap();
    }
    dir.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    // Provenance parsing

    #[test]
    fn parses_official_archive_name() {
        let name = "cef_binary_131.3.5+g6a8d2b7+chromium-131.0.6778.204_linux64_minimal.tar.bz2";
        let (cef, chromium, platform) = parse_archive_name(name).unwrap();
        assert_eq!(cef, "131.3.5+g6a8d2b7");
        assert_eq!(chromium.as_deref(), Some("131.0.6778.204"));
        assert_eq!(platform.as_deref(), Some("linux64"));
    }

    #[test]
    fn rejects_non_archive_names() {
        assert!(parse_archive_name("random.tar.bz2").is_none());
        assert!(parse_archive_name("cef_binary_131.3.5_linux64_minimal.zip").is_none());
    }

    #[test]
    fn version_match_accepts_full_and_prefix() {
        let p = CefProvenance {
            cef_version: "131.3.5+g6a8d2b7".into(),
            chromium_version: None,
            platform: Some("linux64".into()),
            distribution: "minimal".into(),
            artifact: "x.tar.bz2".into(),
        };
        assert!(p.matches_version("131.3.5"));
        assert!(p.matches_version("131.3.5+g6a8d2b7"));
        assert!(!p.matches_version("127.1.1"));
        assert!(!p.matches_version("131.3"));
    }

    // Distribution validation

    #[test]
    fn raw_distribution_shape_is_valid() {
        let dir = tmp();
        fs::create_dir_all(dir.path().join("Release")).unwrap();
        fs::create_dir_all(dir.path().join("Resources")).unwrap();
        fs::write(dir.path().join("Release").join(libcef_name()), "").unwrap();

        assert!(validate_distribution(dir.path()).is_ok());
    }

    #[test]
    fn flat_distribution_shape_is_valid() {
        let dir = tmp();
        fs::write(dir.path().join(libcef_name()), "").unwrap();
        assert!(validate_distribution(dir.path()).is_ok());
    }

    #[test]
    fn unrecognized_directory_is_invalid_distribution() {
        let dir = tmp();
        fs::create_dir_all(dir.path().join("stuff")).unwrap();
        let err = validate_distribution(dir.path()).unwrap_err();
        assert!(matches!(err, CefError::InvalidDistribution { .. }));
    }

    // Materialization

    #[cfg(target_os = "linux")]
    #[test]
    fn raw_distribution_becomes_flat_runtime() {
        let dir = tmp();
        let dist = dir.path().join("dist");
        fs::create_dir_all(dist.join("Release")).unwrap();
        fs::create_dir_all(dist.join("Resources").join("locales")).unwrap();
        fs::write(dist.join("Release").join("libcef.so"), "lib").unwrap();
        fs::write(dist.join("Release").join("chrome-sandbox"), "sb").unwrap();
        fs::write(dist.join("Resources").join("icudtl.dat"), "icu").unwrap();
        fs::write(dist.join("Resources").join("v8_context_snapshot.bin"), "v8").unwrap();
        fs::write(dist.join("Resources").join("locales").join("en-US.pak"), "pak").unwrap();

        let dest = dir.path().join("runtime");
        let out = materialize_cef_runtime(&dist, &dest).unwrap();

        assert_eq!(out, dest);
        assert!(dest.join("libcef.so").exists());
        assert!(dest.join("chrome-sandbox").exists());
        assert!(dest.join("icudtl.dat").exists());
        assert!(dest.join("locales/en-US.pak").exists());
        assert!(validate_cef_runtime(&dest).is_ok());
    }

    #[test]
    fn flat_distribution_strips_development_material() {
        let dir = tmp();
        let dist = write_runtime_fixture(&dir.path().join("managed"));
        fs::create_dir_all(dist.join("include").join("cef")).unwrap();
        fs::write(dist.join("include").join("cef").join("cef_app.h"), "h").unwrap();
        fs::create_dir_all(dist.join("cmake")).unwrap();
        fs::create_dir_all(dist.join("libcef_dll")).unwrap();
        fs::write(dist.join("CMakeLists.txt"), "cmake").unwrap();
        fs::write(dist.join("CREDITS.html"), "credits").unwrap();

        let dest = dir.path().join("runtime");
        materialize_cef_runtime(&dist, &dest).unwrap();

        assert!(dest.join(libcef_name()).exists());
        assert!(!dest.join("include").exists());
        assert!(!dest.join("cmake").exists());
        assert!(!dest.join("libcef_dll").exists());
        assert!(!dest.join("CMakeLists.txt").exists());
        assert!(!dest.join("CREDITS.html").exists());
    }

    #[test]
    fn flat_distribution_strips_download_cache_residue() {
        let dir = tmp();
        let dist = write_runtime_fixture(&dir.path().join("managed"));
        fs::write(
            dist.join("archive.json"),
            r#"{"type":"minimal","name":"x.tar.bz2","sha1":"0"}"#,
        )
        .unwrap();
        fs::write(
            dist.join("cef_binary_150.0.10_linux64_minimal.tar.bz2"),
            "100MB of archive",
        )
        .unwrap();

        let dest = dir.path().join("runtime");
        materialize_cef_runtime(&dist, &dest).unwrap();

        assert!(!dest.join("archive.json").exists());
        assert!(!dest.join("cef_binary_150.0.10_linux64_minimal.tar.bz2").exists());
        assert!(dest.join(libcef_name()).exists(), "runtime files unaffected");
    }

    #[test]
    fn materialization_is_cached_when_valid() {
        let dir = tmp();
        let dist = write_runtime_fixture(&dir.path().join("managed"));
        let dest = dir.path().join("runtime");
        materialize_cef_runtime(&dist, &dest).unwrap();

        // Corrupt the source; cache must still be returned untouched
        fs::remove_file(dist.join(libcef_name())).unwrap();
        let marker = dest.join("cache-marker");
        fs::write(&marker, "hit").unwrap();

        materialize_cef_runtime(&dist, &dest).unwrap();
        assert!(marker.exists(), "valid destination must be reused");
    }

    #[test]
    fn invalid_cached_destination_is_rebuilt() {
        let dir = tmp();
        let dist = write_runtime_fixture(&dir.path().join("managed"));
        let dest = dir.path().join("runtime");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("garbage"), "").unwrap();

        materialize_cef_runtime(&dist, &dest).unwrap();
        assert!(dest.join(libcef_name()).exists());
        assert!(!dest.join("garbage").exists());
    }

    // Runtime validation

    #[test]
    fn complete_runtime_passes_validation() {
        let dir = tmp();
        let runtime = write_runtime_fixture(&dir.path().join("rt"));
        assert!(validate_cef_runtime(&runtime).is_ok());
    }

    #[test]
    fn missing_required_files_are_reported_together() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        fs::create_dir_all(&runtime).unwrap();

        match validate_cef_runtime(&runtime) {
            Err(CefError::InvalidRuntime { missing, .. }) => {
                assert!(missing.contains(libcef_name()));
                assert!(missing.contains("icudtl.dat"));
                assert!(missing.contains("locales"));
            }
            other => panic!("expected InvalidRuntime, got {other:?}"),
        }
    }

    #[test]
    fn either_v8_snapshot_satisfies_requirement() {
        let dir = tmp();
        let runtime = dir.path().join("rt");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join(libcef_name()), "").unwrap();
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

    // Resolution policy

    // Environment mutation must be serialized across tests
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn resolution_fails_without_managed_install_or_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var("CEF_PATH").ok();
        unsafe {
            std::env::remove_var("CEF_PATH");
        }

        let err = resolve_cef_for_bundle("0.0.0-nonexistent").unwrap_err();
        assert!(matches!(err, CefError::NotFound { .. }));

        if let Some(v) = original {
            unsafe {
                std::env::set_var("CEF_PATH", v);
            }
        }
    }

    #[test]
    fn unverifiable_override_is_rejected() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp();
        let fake = dir.path().join("dev-cef");
        write_runtime_fixture(&fake); // looks like CEF but has no archive.json

        let original = std::env::var("CEF_PATH").ok();
        unsafe {
            std::env::set_var("CEF_PATH", &fake);
        }

        let err = resolve_cef_for_bundle("131.3.5").unwrap_err();
        assert!(matches!(err, CefError::UnverifiableOverride(_)));

        match original {
            Some(v) => unsafe {
                std::env::set_var("CEF_PATH", v);
            },
            None => unsafe {
                std::env::remove_var("CEF_PATH");
            },
        }
    }

    #[test]
    fn version_mismatched_override_is_rejected() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp();
        let fake = write_runtime_fixture(&dir.path().join("dev-cef"));
        fs::write(
            fake.join("archive.json"),
            r#"{"type":"minimal","name":"cef_binary_127.1.1+gabcdef+chromium-127.0.1.2_linux64_minimal.tar.bz2","sha1":"x"}"#,
        )
        .unwrap();

        let original = std::env::var("CEF_PATH").ok();
        unsafe {
            std::env::set_var("CEF_PATH", &fake);
        }

        let err = resolve_cef_for_bundle("131.3.5").unwrap_err();
        assert!(matches!(err, CefError::VersionMismatch { .. }));

        match original {
            Some(v) => unsafe {
                std::env::set_var("CEF_PATH", v);
            },
            None => unsafe {
                std::env::remove_var("CEF_PATH");
            },
        }
    }

    #[test]
    fn verified_override_with_matching_provenance_is_accepted() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp();
        let fake = write_runtime_fixture(&dir.path().join("dev-cef"));
        let platform = current_platform_name().unwrap_or("linux64");
        let archive_name = format!(
            "cef_binary_131.3.5+g6a8d2b7+chromium-131.0.6778.204_{platform}_minimal.tar.bz2"
        );
        fs::write(
            fake.join("archive.json"),
            serde_json::json!({ "type": "minimal", "name": archive_name, "sha1": "x" }).to_string(),
        )
        .unwrap();

        let original = std::env::var("CEF_PATH").ok();
        unsafe {
            std::env::set_var("CEF_PATH", &fake);
        }

        let resolved = resolve_cef_for_bundle("131.3.5").unwrap();
        assert_eq!(resolved.source, CefSource::EnvironmentOverride);
        let prov = resolved.provenance.expect("provenance present");
        assert_eq!(prov.chromium_version.as_deref(), Some("131.0.6778.204"));

        match original {
            Some(v) => unsafe {
                std::env::set_var("CEF_PATH", v);
            },
            None => unsafe {
                std::env::remove_var("CEF_PATH");
            },
        }
    }

    // Managed-install resolution (injected root; no environment mutation)

    fn managed_provenance_fixture(dir: &Path) -> PathBuf {
        let managed = write_runtime_fixture(&dir.join("managed"));
        let platform = current_platform_name().unwrap_or("linux64");
        let archive_name = format!(
            "cef_binary_131.3.5+g6a8d2b7+chromium-131.0.6778.204_{platform}_minimal.tar.bz2"
        );
        fs::write(
            managed.join("archive.json"),
            serde_json::json!({ "type": "minimal", "name": archive_name, "sha1": "x" }).to_string(),
        )
        .unwrap();
        managed
    }

    #[test]
    fn valid_managed_install_is_accepted_with_provenance() {
        let dir = tmp();
        let managed = managed_provenance_fixture(dir.path());

        let resolved = resolve_cef("131.3.5", |_| Some(managed.clone())).unwrap();

        assert_eq!(resolved.source, CefSource::ManagedCache);
        assert_eq!(resolved.root, managed);
        assert!(resolved.provenance.is_some());
    }

    #[test]
    fn managed_install_without_provenance_is_rejected() {
        let dir = tmp();
        let managed = write_runtime_fixture(&dir.path().join("managed"));

        let err = resolve_cef("131.3.5", |_| Some(managed.clone())).unwrap_err();

        assert!(
            matches!(err, CefError::UnverifiableManaged(ref p) if p == &managed),
            "expected UnverifiableManaged, got: {err}"
        );
    }

    #[test]
    fn version_mismatched_managed_install_is_rejected() {
        let dir = tmp();
        let managed = managed_provenance_fixture(dir.path());

        let err = resolve_cef("127.1.1", |_| Some(managed.clone())).unwrap_err();

        assert!(
            matches!(err, CefError::VersionMismatch { .. }),
            "expected VersionMismatch, got: {err}"
        );
    }

    #[test]
    fn platform_mismatched_managed_install_is_rejected() {
        let dir = tmp();
        let managed = write_runtime_fixture(&dir.path().join("managed"));
        let wrong_platform = if current_platform_name() == Some("linux64") {
            "windowsarm64"
        } else {
            "linux64"
        };
        let archive_name = format!(
            "cef_binary_131.3.5+g6a8d2b7+chromium-131.0.6778.204_{wrong_platform}_minimal.tar.bz2"
        );
        fs::write(
            managed.join("archive.json"),
            serde_json::json!({ "type": "minimal", "name": archive_name, "sha1": "x" }).to_string(),
        )
        .unwrap();

        let err = resolve_cef("131.3.5", |_| Some(managed.clone())).unwrap_err();

        assert!(
            matches!(err, CefError::PlatformMismatch { .. }),
            "expected PlatformMismatch, got: {err}"
        );
    }

    #[test]
    fn override_takes_precedence_over_managed_install() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp();
        let override_root = managed_provenance_fixture(&dir.path().join("ovr"));
        let managed_root = managed_provenance_fixture(&dir.path().join("mgr"));

        let original = std::env::var("CEF_PATH").ok();
        unsafe {
            std::env::set_var("CEF_PATH", &override_root);
        }

        let resolved = resolve_cef("131.3.5", |_| Some(managed_root.clone())).unwrap();

        match original {
            Some(v) => unsafe {
                std::env::set_var("CEF_PATH", v);
            },
            None => unsafe {
                std::env::remove_var("CEF_PATH");
            },
        }

        assert_eq!(resolved.source, CefSource::EnvironmentOverride);
        assert_eq!(resolved.root, override_root);
    }

    #[test]
    fn missing_override_path_errors_even_with_managed_install() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp();
        let managed_root = managed_provenance_fixture(&dir.path().join("mgr"));
        let missing = dir.path().join("does-not-exist");

        let original = std::env::var("CEF_PATH").ok();
        unsafe {
            std::env::set_var("CEF_PATH", &missing);
        }

        let err = resolve_cef("131.3.5", |_| Some(managed_root.clone())).unwrap_err();

        match original {
            Some(v) => unsafe {
                std::env::set_var("CEF_PATH", v);
            },
            None => unsafe {
                std::env::remove_var("CEF_PATH");
            },
        }

        assert!(
            matches!(err, CefError::OverrideMissing(ref p) if p == &missing),
            "expected OverrideMissing, got: {err}"
        );
    }
}
