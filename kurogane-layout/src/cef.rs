//! CEF distribution and runtime handling.
//!
//! This module owns CEF resolution, validation and runtime preparation.
//! Package formats consume the resulting runtime and do not define CEF
//! packaging policy.

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
    let archive: ArchiveJson = serde_json::from_reader(file)
        .map_err(|e| CefError::InvalidDistribution { root: root.to_path_buf(), reason: format!("unreadable archive.json: {e}") })?;

    Ok(parse_archive_name(&archive.name).map(|(cef_version, chromium_version, platform)| CefProvenance {
        cef_version,
        chromium_version,
        platform,
        distribution: archive.file_type,
        artifact: archive.name,
    }))
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

/// Resolves the CEF distribution to use for release packaging.
pub fn resolve_cef_for_bundle(version: &str) -> Result<ResolvedCef, CefError> {
    if let Some(root) = crate::layout::installed_cef_root(version) {
        validate_distribution(&root)?;
        let provenance = read_provenance(&root)?;
        return Ok(ResolvedCef {
            root,
            source: CefSource::ManagedCache,
            provenance,
        });
    }

    if let Ok(path) = std::env::var("CEF_PATH") {
        let root = PathBuf::from(path);
        if !root.exists() {
            return Err(CefError::OverrideMissing(root));
        }

        let provenance = read_provenance(&root)?
            .ok_or_else(|| CefError::UnverifiableOverride(root.clone()))?;

        if !provenance.matches_version(version) {
            return Err(CefError::VersionMismatch {
                expected: version.to_string(),
                found: provenance.cef_version.clone(),
                path: root.clone(),
            });
        }

        if !provenance.matches_current_platform() {
            return Err(CefError::PlatformMismatch {
                expected: current_platform_name()
                    .unwrap_or("unknown")
                    .to_string(),
                found: provenance.platform.clone().unwrap_or_else(|| "unknown".into()),
                path: root.clone(),
            });
        }

        validate_distribution(&root)?;
        return Ok(ResolvedCef {
            root,
            source: CefSource::EnvironmentOverride,
            provenance: Some(provenance),
        });
    }

    Err(CefError::NotFound {
        expected: version.to_string(),
        path: crate::layout::cef_install_dir(version),
    })
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
            reason: format!(
                "neither Release/+Resources/ nor {} found",
                libcef_name()
            ),
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

            if DEV_ARTIFACTS.iter().any(|d| d == &name_str)
                || is_download_cache_artifact(&name_str)
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
            reason: format!(
                "neither Release/+Resources/ nor {} found",
                libcef_name()
            ),
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
