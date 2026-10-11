//! The CEF the project's application loads.
//!
//! Finds it where the application does, in the runtime `CEF_PATH` names or
//! else in tanso's shared installation, verifies it and installs the
//! project's CEF version when the installation is missing or unverified.

use std::fmt::{self, Display};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cargo_metadata::Metadata;
use kurogane_layout::{IncompleteRuntime, validate_cef_runtime};
use tanso_download::{Archive, DEFAULT_TARGET, OsAndArch};
use thiserror::Error;

use crate::tui;

/// CEF's licence and the credits of the code it bundles, which every bundle
/// passes on; a complete installation has them.
pub(crate) const NOTICES: &[&str] = &["LICENSE.txt", "CREDITS.html"];

/// Where the CEF the application loads comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CefSource {
    /// The runtime `CEF_PATH` names.
    CefPath,

    /// tanso's shared installation of the CEF version.
    Installed,
}

impl CefSource {
    /// Returns what makes an unusable runtime from here usable.
    fn remedy(self) -> &'static str {
        match self {
            Self::CefPath => {
                "Point CEF_PATH at a distribution `export-cef-dir` wrote, or unset it to use \
                 the installed one."
            }
            Self::Installed => "Run `kurogane install` to reinstall it.",
        }
    }
}

impl Display for CefSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CefPath => "CEF_PATH",
            Self::Installed => "installed",
        })
    }
}

/// The CEF the project's application loads.
#[derive(Debug, Clone)]
pub(crate) enum ProjectCef {
    /// The runtime `CEF_PATH` names, used as it is.
    CefPath(PathBuf),

    /// tanso's shared installation, verified against the CEF build its
    /// `archive.json` names.
    Installed { root: PathBuf, archive: Archive },
}

impl ProjectCef {
    /// Returns the runtime directory.
    pub(crate) fn root(&self) -> &Path {
        match self {
            Self::CefPath(root) | Self::Installed { root, .. } => root,
        }
    }

    /// Returns where the runtime comes from.
    pub(crate) fn source(&self) -> CefSource {
        match self {
            Self::CefPath(_) => CefSource::CefPath,
            Self::Installed { .. } => CefSource::Installed,
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum CefError {
    #[error("CEF {version} is not installed at {}; run `kurogane install`", .path.display())]
    NotInstalled { version: String, path: PathBuf },

    #[error("CEF_PATH names {}, which is not a directory", .0.display())]
    CefPathMissing(PathBuf),

    #[error("this user has no local data directory to install Chromium into")]
    NoInstallDir,

    #[error("CEF publishes no build for this host")]
    UnsupportedHost(#[source] tanso_download::Error),

    #[error("the {from} runtime is incomplete. {}", .from.remedy())]
    InvalidRuntime {
        from: CefSource,
        #[source]
        missing: IncompleteRuntime,
    },

    #[error("the {from} runtime cannot be verified. {}", .from.remedy())]
    Unverified {
        from: CefSource,
        #[source]
        error: tanso_download::Error,
    },

    #[error("{} has no {notice}, which a bundle passes on. {}", .path.display(), .from.remedy())]
    MissingNotice {
        path: PathBuf,
        notice: &'static str,
        from: CefSource,
    },
}

impl CefError {
    /// Returns whether the failure concerns the runtime `CEF_PATH` names.
    pub(crate) fn concerns_cef_path(&self) -> bool {
        match self {
            Self::CefPathMissing(_) => true,
            Self::InvalidRuntime { from, .. }
            | Self::Unverified { from, .. }
            | Self::MissingNotice { from, .. } => *from == CefSource::CefPath,
            Self::NotInstalled { .. } | Self::NoInstallDir | Self::UnsupportedHost(_) => false,
        }
    }
}

/// Returns the CEF distribution `CEF_PATH` names, when it is set and not
/// empty.
pub(crate) fn cef_path() -> Option<PathBuf> {
    std::env::var_os("CEF_PATH")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Returns the CEF version the current project's application loads, the
/// CLI's own outside a project.
pub(crate) fn project_cef_version() -> String {
    match cargo_metadata::MetadataCommand::new().exec() {
        Ok(metadata) => cef_version_of(&metadata),
        Err(_) => env!("KUROGANE_CEF_VERSION").to_owned(),
    }
}

/// Returns the CEF version a resolved project loads, the one its tanso-sys
/// was generated for (`154.0.33` in `154.4.0+154.0.33`), else the CLI's own.
pub(crate) fn cef_version_of(metadata: &Metadata) -> String {
    metadata
        .packages
        .iter()
        .find(|package| package.name == "tanso-sys")
        .map(|package| package.version.build.as_str())
        .filter(|version| !version.is_empty())
        .unwrap_or(env!("KUROGANE_CEF_VERSION"))
        .to_owned()
}

/// Returns the installation of CEF `version` for this host.
pub(crate) fn installed_cef_dir(version: &str) -> Result<PathBuf, CefError> {
    let host = OsAndArch::try_from(DEFAULT_TARGET).map_err(CefError::UnsupportedHost)?;

    tanso_download::cef_install_dir(version, &host).ok_or(CefError::NoInstallDir)
}

/// Finds the CEF the application loads without installing anything, the
/// runtime `CEF_PATH` names or else the verified installation of `version`.
pub(crate) fn find_cef(version: &str) -> Result<ProjectCef, CefError> {
    find_cef_in(cef_path(), installed_cef_dir(version), version)
}

/// [`find_cef`] with `CEF_PATH` and the installation passed in.
///
/// A set `CEF_PATH` naming no usable runtime is an error, never replaced by
/// the installation.
fn find_cef_in(
    cef_path: Option<PathBuf>,
    installed: Result<PathBuf, CefError>,
    version: &str,
) -> Result<ProjectCef, CefError> {
    if let Some(root) = cef_path {
        if !root.is_dir() {
            return Err(CefError::CefPathMissing(root));
        }
        validate_cef_runtime(&root).map_err(|missing| CefError::InvalidRuntime {
            from: CefSource::CefPath,
            missing,
        })?;

        return Ok(ProjectCef::CefPath(root));
    }

    let root = installed?;
    let archive = verify_installation(&root, version)?;

    Ok(ProjectCef::Installed { root, archive })
}

/// Checks that `root` is a complete installation of CEF `version` whose
/// `archive.json` names that version and this platform.
pub(crate) fn verify_installation(root: &Path, version: &str) -> Result<Archive, CefError> {
    if !root.is_dir() {
        return Err(CefError::NotInstalled {
            version: version.to_owned(),
            path: root.to_path_buf(),
        });
    }

    let archive =
        tanso_download::check_archive_json(root, version, DEFAULT_TARGET).map_err(|error| {
            CefError::Unverified {
                from: CefSource::Installed,
                error,
            }
        })?;
    validate_cef_runtime(root).map_err(|missing| CefError::InvalidRuntime {
        from: CefSource::Installed,
        missing,
    })?;
    check_notices(root, CefSource::Installed)?;

    Ok(archive)
}

/// Checks that the CEF at `root` carries its licence and credits.
fn check_notices(root: &Path, from: CefSource) -> Result<(), CefError> {
    match NOTICES.iter().find(|notice| !root.join(notice).is_file()) {
        Some(notice) => Err(CefError::MissingNotice {
            path: root.to_path_buf(),
            notice,
            from,
        }),
        None => Ok(()),
    }
}

/// Returns the CEF build a bundle of `cef` packages, which its `archive.json`
/// must name as CEF `version` for this platform, wherever it comes from.
pub(crate) fn packaged_archive(cef: &ProjectCef, version: &str) -> Result<Archive, CefError> {
    match cef {
        ProjectCef::Installed { archive, .. } => Ok(archive.clone()),
        ProjectCef::CefPath(root) => {
            let archive = tanso_download::check_archive_json(root, version, DEFAULT_TARGET)
                .map_err(|error| CefError::Unverified {
                    from: CefSource::CefPath,
                    error,
                })?;
            check_notices(root, CefSource::CefPath)?;

            Ok(archive)
        }
    }
}

/// Returns the CEF the application will load, installing CEF `version` when
/// the installation is missing or unverified.
///
/// The application finds the same one itself, so nothing is passed to it.
pub(crate) fn ensure_cef_runtime(version: &str) -> Result<ProjectCef> {
    tui::step("Checking Chromium engine");

    let cef = match find_cef(version) {
        Ok(cef) => cef,

        // A CEF_PATH the CLI cannot use is an error, never replaced by the
        // installation
        Err(err) if err.concerns_cef_path() => {
            return Err(err).context("CEF_PATH names no usable Chromium runtime");
        }

        Err(err) => {
            if matches!(err, CefError::NotInstalled { .. }) {
                tui::warn("Chromium runtime not installed");
            } else {
                tui::warn("Chromium runtime incomplete or unverified");
                tui::error_fields(&err);
            }
            tui::info("Initiating install process...");

            install(version)?
        }
    };

    tui::success("Chromium engine ready");
    tui::field("path", tui::format_path(cef.root()));
    if let ProjectCef::CefPath(_) = cef {
        tui::field("source", CefSource::CefPath);
    }

    Ok(cef)
}

pub fn run() -> Result<()> {
    install(&project_cef_version()).map(drop)
}

/// Installs CEF `version` unless a verified installation is in place;
/// returns it.
pub(crate) fn install(cef_version: &str) -> Result<ProjectCef> {
    tui::section("Kurogane installer");

    let install_dir = installed_cef_dir(cef_version)?;

    if install_dir.exists() {
        match verify_installation(&install_dir, cef_version) {
            Ok(archive) => {
                tui::success("Chromium engine already installed");
                tui::field("version", cef_version);
                tui::field("path", tui::format_path(&install_dir));
                return Ok(ProjectCef::Installed {
                    root: install_dir,
                    archive,
                });
            }
            Err(err) => {
                tui::warn("Existing Chromium runtime is incomplete or unverified; reinstalling");
                tui::error_fields(&err);
                std::fs::remove_dir_all(&install_dir).with_context(|| {
                    format!("failed to remove directory {}", install_dir.display())
                })?;
            }
        }
    }

    tui::step("Downloading Chromium engine...");
    tui::field("chromium", cef_version);
    tui::field("path", tui::format_path(&install_dir));

    let installed = tanso_download::install(
        DEFAULT_TARGET,
        cef_version,
        &tanso_download::default_download_url(),
        true,
    )
    .context("failed to install Chromium")?;

    // Fail rather than leaving an unusable tree for the next run to trip on
    let archive = verify_installation(&installed, cef_version)
        .context("installed Chromium runtime is invalid")?;

    tui::blank();

    tui::success("Chromium engine installed");
    tui::field("path", tui::format_path(&installed));

    Ok(ProjectCef::Installed {
        root: installed,
        archive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use kurogane_layout::test_fixtures::{cef_runtime, tmp_dir};

    const VERSION: &str = "154.0.33";

    /// A runtime with CEF's notices whose `archive.json` names CEF `version`
    /// for `platform`.
    fn installation(dir: &Path, version: &str, platform: &str) -> PathBuf {
        let root = cef_runtime(dir);
        for notice in NOTICES {
            std::fs::write(root.join(notice), "notice").unwrap();
        }
        let name = format!(
            "cef_binary_{version}+ga03e714+chromium-154.0.8037.94_{platform}_minimal.tar.bz2"
        );
        std::fs::write(
            root.join("archive.json"),
            format!(r#"{{"type":"minimal","name":"{name}","sha1":""}}"#),
        )
        .unwrap();
        root
    }

    fn host_platform() -> &'static str {
        tanso_download::cef_platform_name(DEFAULT_TARGET).unwrap()
    }

    #[test]
    fn the_cli_s_own_workspace_loads_the_cli_s_cef() {
        // The tests run in Kurogane's workspace, whose tanso-sys the CLI was
        // built against
        assert_eq!(project_cef_version(), env!("KUROGANE_CEF_VERSION"));
    }

    #[test]
    fn a_verified_installation_is_found_with_its_archive() {
        let dir = tmp_dir();
        let root = installation(&dir.path().join("installed"), VERSION, host_platform());

        match find_cef_in(None, Ok(root.clone()), VERSION).unwrap() {
            ProjectCef::Installed {
                root: found,
                archive,
            } => {
                assert_eq!(found, root);
                assert_eq!(archive.cef_version, VERSION);
            }
            other => panic!("expected the installation, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_installation_is_not_installed() {
        let dir = tmp_dir();
        let err = find_cef_in(None, Ok(dir.path().join("absent")), VERSION).unwrap_err();

        assert!(matches!(err, CefError::NotInstalled { .. }), "got: {err}");
    }

    #[test]
    fn an_installation_counts_only_with_its_archive_naming_this_cef_here() {
        let other_platform = if host_platform() == "linux64" {
            "windows64"
        } else {
            "linux64"
        };
        for (case, version, platform) in [
            ("another version", "154.0.34", host_platform()),
            ("another platform", VERSION, other_platform),
        ] {
            let dir = tmp_dir();
            let root = installation(&dir.path().join("installed"), version, platform);

            let err = find_cef_in(None, Ok(root), VERSION).unwrap_err();
            assert!(
                matches!(
                    err,
                    CefError::Unverified {
                        from: CefSource::Installed,
                        ..
                    }
                ),
                "{case}: {err}"
            );
            assert!(
                err.to_string().contains("kurogane install"),
                "{case}: {err}"
            );
            assert!(!err.concerns_cef_path(), "{case}: {err}");
        }
    }

    #[test]
    fn an_installation_without_archive_json_is_unverified() {
        let dir = tmp_dir();
        let root = cef_runtime(&dir.path().join("installed"));

        let err = find_cef_in(None, Ok(root), VERSION).unwrap_err();
        assert!(matches!(err, CefError::Unverified { .. }), "got: {err}");
    }

    #[test]
    fn an_incomplete_installation_is_not_verified_despite_its_archive_json() {
        let dir = tmp_dir();
        let root = installation(&dir.path().join("installed"), VERSION, host_platform());
        std::fs::remove_dir_all(root.join("locales")).ok();
        let libcef = if cfg!(target_os = "windows") {
            "libcef.dll"
        } else if cfg!(target_os = "macos") {
            "Chromium Embedded Framework.framework"
        } else {
            "libcef.so"
        };
        let path = root.join(libcef);
        if path.is_dir() {
            std::fs::remove_dir_all(path).unwrap();
        } else {
            std::fs::remove_file(path).unwrap();
        }

        let err = find_cef_in(None, Ok(root), VERSION).unwrap_err();
        assert!(
            matches!(
                err,
                CefError::InvalidRuntime {
                    from: CefSource::Installed,
                    ..
                }
            ),
            "got: {err}"
        );
    }

    #[test]
    fn an_installation_without_cef_s_notices_is_reinstalled() {
        for notice in NOTICES {
            let dir = tmp_dir();
            let root = installation(&dir.path().join("installed"), VERSION, host_platform());
            std::fs::remove_file(root.join(notice)).unwrap();

            let err = find_cef_in(None, Ok(root), VERSION).unwrap_err();
            assert!(
                matches!(
                    err,
                    CefError::MissingNotice {
                        from: CefSource::Installed,
                        ..
                    }
                ),
                "{notice}: {err}"
            );
        }
    }

    #[test]
    fn cef_path_comes_before_the_installation() {
        let dir = tmp_dir();
        let chosen = cef_runtime(&dir.path().join("chosen"));
        let installed = installation(&dir.path().join("installed"), VERSION, host_platform());

        let cef = find_cef_in(Some(chosen.clone()), Ok(installed), VERSION).unwrap();

        assert!(
            matches!(cef, ProjectCef::CefPath(ref root) if *root == chosen),
            "got {cef:?}"
        );
    }

    #[test]
    fn a_cef_path_problem_is_one_of_cef_path() {
        let dir = tmp_dir();
        let missing = dir.path().join("missing");
        let installed = installation(&dir.path().join("installed"), VERSION, host_platform());

        let err = find_cef_in(Some(missing.clone()), Ok(installed.clone()), VERSION).unwrap_err();
        assert!(
            matches!(err, CefError::CefPathMissing(ref p) if p == &missing),
            "expected CefPathMissing, got: {err}"
        );
        assert!(err.concerns_cef_path());

        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        let err = find_cef_in(Some(empty), Ok(installed), VERSION).unwrap_err();
        assert!(
            matches!(
                err,
                CefError::InvalidRuntime {
                    from: CefSource::CefPath,
                    ..
                }
            ),
            "got: {err}"
        );
        assert!(err.concerns_cef_path());
    }

    #[test]
    fn a_bundle_needs_an_archive_naming_this_cef_even_from_cef_path() {
        let dir = tmp_dir();
        let unverified = cef_runtime(&dir.path().join("dev-cef"));
        let cef = find_cef_in(Some(unverified), Ok(dir.path().join("absent")), VERSION).unwrap();

        let err = packaged_archive(&cef, VERSION).unwrap_err();
        assert!(
            matches!(
                err,
                CefError::Unverified {
                    from: CefSource::CefPath,
                    ..
                }
            ),
            "got: {err}"
        );
        assert!(
            err.to_string().contains("CEF_PATH"),
            "the remedy names CEF_PATH: {err}"
        );

        let verified = installation(&dir.path().join("exported"), VERSION, host_platform());
        let cef = find_cef_in(Some(verified), Ok(dir.path().join("absent")), VERSION).unwrap();
        assert_eq!(
            packaged_archive(&cef, VERSION).unwrap().cef_version,
            VERSION
        );
    }
}
