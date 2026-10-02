//! Resolved application distribution model.
//!
//! This module defines the platform-independent description of the files
//! required to distribute an application and validates that all declared
//! inputs are usable.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Application identity and distribution metadata.
#[derive(Debug, Clone, Default)]
pub struct AppMetadata {
    pub name: String,
    pub version: String,
    /// File name the executable is installed under, in every package format.
    pub exe_name: String,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub copyright: Option<String>,
    pub icon: Option<PathBuf>,
}

/// Resolved resource source and bundle destination.
#[derive(Debug, Clone)]
pub struct ResolvedResource {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// The resolved contents of an application distribution.
///
/// Describes what must be distributed without prescribing how it is
/// packaged or laid out on disk.
///
/// Platform-specific layout is the responsibility of the materializer.
#[derive(Debug, Clone)]
pub struct ResolvedDistribution {
    pub metadata: AppMetadata,
    /// The binary the user starts, installed as [`AppMetadata::exe_name`].
    pub executable: Executable,
    pub frontend: Option<PathBuf>,
    /// Materialized CEF runtime.
    pub cef_runtime: PathBuf,
    pub extra_resources: Vec<ResolvedResource>,
}

/// The binary a distribution starts, and what it loads.
#[derive(Debug, Clone)]
pub enum Executable {
    /// The application's own executable.
    Application(PathBuf),

    /// CEF's sandbox bootstrap, which loads the application from a library.
    ///
    /// Chromium's Windows sandbox is brokered by whichever process starts the
    /// browser, and CEF keeps that broker in its bootstrap. The bootstrap is
    /// installed under the application's name and finds the library beside
    /// it under the same name, see [`crate::client_library_path`].
    Bootstrap {
        bootstrap: PathBuf,
        library: PathBuf,
    },
}

impl Executable {
    /// Returns the binary installed as the application's executable.
    pub fn binary(&self) -> &Path {
        match self {
            Executable::Application(binary)
            | Executable::Bootstrap {
                bootstrap: binary, ..
            } => binary,
        }
    }

    /// Returns the library the bootstrap loads, if the application is one.
    pub fn library(&self) -> Option<&Path> {
        match self {
            Executable::Application(_) => None,
            Executable::Bootstrap { library, .. } => Some(library),
        }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DistributionError {
    #[error("executable not found: {0}")]
    MissingExecutable(PathBuf),

    #[error("executable path is not a file: {0}")]
    ExecutableNotFile(PathBuf),

    #[error("client library not found: {0}")]
    MissingClientLibrary(PathBuf),

    #[error("client library path is not a file: {0}")]
    ClientLibraryNotFile(PathBuf),

    #[error("the application has no executable name to install under")]
    MissingExeName,

    /// A name every package format turns into a file or folder name, and
    /// some into a script, that cannot be one on every platform.
    #[error("the {what} {name:?} cannot name a file: {problem}")]
    InvalidName {
        what: &'static str,
        name: String,
        problem: NameProblem,
    },

    #[error("frontend directory not found: {0}")]
    MissingFrontend(PathBuf),

    #[error("frontend path is not a directory: {0}")]
    FrontendNotDir(PathBuf),

    #[error("frontend missing index.html at {0}")]
    MissingIndex(PathBuf),

    #[error("CEF runtime not found: {0}")]
    MissingCefRoot(PathBuf),

    #[error("CEF runtime is not a directory: {0}")]
    CefRootNotDir(PathBuf),

    #[error("extra resource not found: {0}")]
    MissingResource(PathBuf),

    #[error("resource destination must be a relative path without '..' components: {0}")]
    InvalidResourceDestination(PathBuf),

    #[error(transparent)]
    InvalidCefRuntime(#[from] crate::cef::CefError),
}

impl ResolvedDistribution {
    /// Validates the resolved distribution.
    pub fn validate(&self) -> Result<(), DistributionError> {
        let binary = self.executable.binary();

        if !binary.exists() {
            return Err(DistributionError::MissingExecutable(binary.to_path_buf()));
        }

        if !binary.is_file() {
            return Err(DistributionError::ExecutableNotFile(binary.to_path_buf()));
        }

        if let Some(library) = self.executable.library() {
            if !library.exists() {
                return Err(DistributionError::MissingClientLibrary(
                    library.to_path_buf(),
                ));
            }

            if !library.is_file() {
                return Err(DistributionError::ClientLibraryNotFile(
                    library.to_path_buf(),
                ));
            }
        }

        check_app_name(&self.metadata.name)?;

        if self.metadata.exe_name.is_empty() {
            return Err(DistributionError::MissingExeName);
        }
        check_name("executable name", &self.metadata.exe_name)?;

        if let Some(frontend) = &self.frontend {
            if !frontend.exists() {
                return Err(DistributionError::MissingFrontend(frontend.clone()));
            }

            if !frontend.is_dir() {
                return Err(DistributionError::FrontendNotDir(frontend.clone()));
            }

            let index = frontend.join("index.html");
            if !index.exists() {
                return Err(DistributionError::MissingIndex(index));
            }
        }

        if !self.cef_runtime.exists() {
            return Err(DistributionError::MissingCefRoot(self.cef_runtime.clone()));
        }

        if !self.cef_runtime.is_dir() {
            return Err(DistributionError::CefRootNotDir(self.cef_runtime.clone()));
        }

        self.validate_cef()?;

        for resource in &self.extra_resources {
            if !resource.source.exists() {
                return Err(DistributionError::MissingResource(resource.source.clone()));
            }

            validate_resource_destination(&resource.destination)?;
        }

        Ok(())
    }

    fn validate_cef(&self) -> Result<(), DistributionError> {
        crate::cef::validate_cef_runtime(&self.cef_runtime)?;
        Ok(())
    }
}

/// Why a name cannot name a file on every platform Kurogane bundles for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum NameProblem {
    #[error("it is empty")]
    Empty,

    #[error("it is a relative path")]
    Relative,

    #[error("it starts or ends with whitespace")]
    OuterWhitespace,

    #[error("it ends with a dot, which Windows drops")]
    TrailingDot,

    #[error("it holds a control character")]
    Control,

    #[error("it holds {0:?}, which a file name cannot hold on every platform")]
    Reserved(char),

    #[error("Windows reserves it for a device")]
    Device,
}

/// Characters a file name cannot hold on Windows; `/` also separates paths
/// everywhere else.
const RESERVED: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Names Windows keeps for devices, whatever their extension or case.
const DEVICES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether `name` names one file or folder on Windows, macOS and Linux
/// alike, so a project bundles under the same name everywhere.
///
/// Every character else is allowed: `$`, quotes and backticks reach scripts
/// only quoted for them.
pub fn portable_file_name(name: &str) -> Result<(), NameProblem> {
    if name.is_empty() {
        return Err(NameProblem::Empty);
    }
    if name == "." || name == ".." {
        return Err(NameProblem::Relative);
    }
    if name.starts_with(char::is_whitespace) || name.ends_with(char::is_whitespace) {
        return Err(NameProblem::OuterWhitespace);
    }
    if name.ends_with('.') {
        return Err(NameProblem::TrailingDot);
    }
    if name.chars().any(char::is_control) {
        return Err(NameProblem::Control);
    }
    if let Some(reserved) = name.chars().find(|c| RESERVED.contains(c)) {
        return Err(NameProblem::Reserved(reserved));
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if DEVICES
        .iter()
        .any(|device| device.eq_ignore_ascii_case(stem))
    {
        return Err(NameProblem::Device);
    }
    Ok(())
}

/// Checks the application's display name, `[app].name` or the package name,
/// before anything is built under it.
pub fn check_app_name(name: &str) -> Result<(), DistributionError> {
    check_name("application name", name)
}

fn check_name(what: &'static str, name: &str) -> Result<(), DistributionError> {
    portable_file_name(name).map_err(|problem| DistributionError::InvalidName {
        what,
        name: name.to_string(),
        problem,
    })
}

/// Validates a resource destination within the bundle.
///
/// Fail-fast guard against authoring mistakes, not a security boundary.
fn validate_resource_destination(destination: &Path) -> Result<(), DistributionError> {
    let escapes = destination.has_root()
        || destination
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));

    if escapes {
        return Err(DistributionError::InvalidResourceDestination(
            destination.to_path_buf(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn valid_distribution_passes_validation() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn missing_executable_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        let binary = dist.executable.binary();
        fs::remove_file(binary).unwrap();

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingExecutable(ref p) if p == binary),
            "expected MissingExecutable, got: {err}"
        );
    }

    #[test]
    fn executable_is_directory_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());
        fs::remove_file(dist.executable.binary()).unwrap();
        fs::create_dir(dist.executable.binary()).unwrap();

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::ExecutableNotFile(_)),
            "expected ExecutableNotFile, got: {err}"
        );
    }

    #[test]
    fn an_executable_without_a_name_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.metadata.exe_name.clear();

        assert!(matches!(
            dist.validate(),
            Err(DistributionError::MissingExeName)
        ));
    }

    #[test]
    fn a_display_name_must_name_one_file_on_every_platform() {
        for (name, problem) in [
            ("", NameProblem::Empty),
            ("..", NameProblem::Relative),
            (" My App", NameProblem::OuterWhitespace),
            ("My App\t", NameProblem::OuterWhitespace),
            ("My App.", NameProblem::TrailingDot),
            ("My\nApp", NameProblem::Control),
            ("My\u{7f}App", NameProblem::Control),
            ("../../outside", NameProblem::Reserved('/')),
            ("/tmp/app", NameProblem::Reserved('/')),
            (r"..\outside", NameProblem::Reserved('\\')),
            ("C:App", NameProblem::Reserved(':')),
            ("My \"Best\" App", NameProblem::Reserved('"')),
            ("Why?", NameProblem::Reserved('?')),
            ("Ben & Jerry <Ltd>", NameProblem::Reserved('<')),
            ("a|b", NameProblem::Reserved('|')),
            ("CON", NameProblem::Device),
            ("nul.tar.gz", NameProblem::Device),
            ("Com1 .app", NameProblem::Device),
        ] {
            assert_eq!(portable_file_name(name), Err(problem), "{name:?}");
        }
    }

    #[test]
    fn shell_and_markup_characters_are_ordinary_in_a_display_name() {
        // Scripts and plists quote or escape these where they write them
        for name in [
            "My App",
            "Tom's App",
            "Ca$h",
            "$(touch mark)",
            "`touch mark`",
            "Ben & Jerry",
            "Price 5$",
            "Café",
            "Console",
            "COM10",
            "My.App",
        ] {
            assert_eq!(portable_file_name(name), Ok(()), "{name:?}");
        }
    }

    #[test]
    fn a_distribution_whose_names_cannot_name_files_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.metadata.name = "/etc".to_string();
        assert!(matches!(
            dist.validate(),
            Err(DistributionError::InvalidName {
                what: "application name",
                problem: NameProblem::Reserved('/'),
                ..
            })
        ));

        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.metadata.exe_name = "a\nb".to_string();
        let err = dist.validate().unwrap_err();
        assert!(
            matches!(
                err,
                DistributionError::InvalidName {
                    what: "executable name",
                    problem: NameProblem::Control,
                    ..
                }
            ),
            "{err}"
        );
        assert_eq!(
            err.to_string(),
            "the executable name \"a\\nb\" cannot name a file: it holds a control character"
        );
    }

    #[test]
    fn a_bootstrap_distribution_passes_validation() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sandboxed_distribution(dir.path());

        assert!(dist.validate().is_ok());
    }

    #[test]
    fn a_bootstrap_without_its_library_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sandboxed_distribution(dir.path());
        let library = dist.executable.library().unwrap();
        fs::remove_file(library).unwrap();

        assert!(matches!(
            dist.validate(),
            Err(DistributionError::MissingClientLibrary(ref p)) if p == library
        ));
    }

    #[test]
    fn missing_frontend_directory_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.frontend = Some(dir.path().join("nonexistent"));

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingFrontend(_)),
            "expected MissingFrontend, got: {err}"
        );
    }

    #[test]
    fn frontend_not_a_directory_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        let file_path = dir.path().join("not_a_dir");
        fs::write(&file_path, "content").unwrap();
        dist.frontend = Some(file_path);

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::FrontendNotDir(_)),
            "expected FrontendNotDir, got: {err}"
        );
    }

    #[test]
    fn missing_index_html_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        let empty_frontend = dir.path().join("empty_frontend");
        fs::create_dir(&empty_frontend).unwrap();
        dist.frontend = Some(empty_frontend);

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingIndex(_)),
            "expected MissingIndex, got: {err}"
        );
    }

    #[test]
    fn frontend_none_does_not_require_index() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.frontend = None;

        assert!(
            dist.validate().is_ok(),
            "frontend=None should pass validation"
        );
    }

    #[test]
    fn missing_cef_root_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.cef_runtime = dir.path().join("nonexistent_cef");

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingCefRoot(_)),
            "expected MissingCefRoot, got: {err}"
        );
    }

    #[test]
    fn cef_root_not_a_directory_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        let file_path = dir.path().join("not_a_cef_dir");
        fs::write(&file_path, "").unwrap();
        dist.cef_runtime = file_path;

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::CefRootNotDir(_)),
            "expected CefRootNotDir, got: {err}"
        );
    }

    #[test]
    fn incomplete_cef_runtime_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        let empty_cef = dir.path().join("empty_cef");
        fs::create_dir(&empty_cef).unwrap();
        dist.cef_runtime = empty_cef;

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::InvalidCefRuntime(_)),
            "expected InvalidCefRuntime, got: {err}"
        );
    }

    #[test]
    fn missing_extra_resource_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        let missing = dir.path().join("nonexistent_resource");
        dist.extra_resources = vec![ResolvedResource {
            source: missing.clone(),
            destination: "nonexistent_resource".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingResource(ref p) if p == &missing),
            "expected MissingResource, got: {err}"
        );
    }

    #[test]
    fn missing_resource_not_confused_with_cef_or_frontend_error() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("missing_res"),
            destination: "missing_res".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(!matches!(
            err,
            DistributionError::MissingCefRoot(_)
                | DistributionError::InvalidCefRuntime(_)
                | DistributionError::MissingFrontend(_)
                | DistributionError::MissingIndex(_)
        ));
    }

    #[test]
    fn raw_distribution_root_is_not_a_valid_runtime() {
        let dir = crate::test_fixtures::tmp_dir();
        let raw = dir.path().join("raw_dist");
        fs::create_dir_all(raw.join("Release")).unwrap();
        fs::create_dir_all(raw.join("Resources")).unwrap();

        let dist = ResolvedDistribution {
            metadata: AppMetadata {
                name: "test".to_string(),
                version: "0.1.0".to_string(),
                exe_name: "test".to_string(),
                ..Default::default()
            },
            executable: {
                let e = dir.path().join("test");
                fs::write(&e, "").unwrap();
                Executable::Application(e)
            },
            frontend: None,
            cef_runtime: raw,
            extra_resources: Vec::new(),
        };

        assert!(
            dist.validate().is_err(),
            "raw distribution root must fail runtime validation"
        );
    }

    #[test]
    fn extra_resources_dirs_are_checked() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        let missing_dir = dir.path().join("missing_dir");
        dist.extra_resources = vec![ResolvedResource {
            source: missing_dir,
            destination: "missing_dir".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::MissingResource(_)),
            "expected MissingResource for missing directory, got: {err}"
        );
    }

    #[test]
    fn absolute_resource_destination_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("extra.txt"),
            destination: "/etc/passwd".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::InvalidResourceDestination(_)),
            "expected InvalidResourceDestination for absolute path, got: {err}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn drive_letter_destination_is_rejected_on_windows() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("extra.txt"),
            destination: r"C:\Windows\evil.dll".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(matches!(
            err,
            DistributionError::InvalidResourceDestination(_)
        ));
    }

    #[test]
    fn parent_dir_resource_destination_is_rejected() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("extra.txt"),
            destination: "../escape.txt".into(),
        }];

        let err = dist.validate().unwrap_err();
        assert!(
            matches!(err, DistributionError::InvalidResourceDestination(ref p) if p == &PathBuf::from("../escape.txt")),
            "expected InvalidResourceDestination for '..' path, got: {err}"
        );
    }

    #[test]
    fn nested_relative_resource_destination_is_accepted() {
        let dir = crate::test_fixtures::tmp_dir();
        let mut dist = crate::test_fixtures::sample_distribution(dir.path());
        dist.extra_resources = vec![ResolvedResource {
            source: dir.path().join("extra.txt"),
            destination: "share/data/extra.txt".into(),
        }];

        dist.validate().unwrap();
    }

    #[test]
    fn exe_name_matches_executable_filename() {
        let dir = crate::test_fixtures::tmp_dir();
        let dist = crate::test_fixtures::sample_distribution(dir.path());

        let actual_filename = dist
            .executable
            .binary()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();

        assert_eq!(
            dist.metadata.exe_name, actual_filename,
            "exe_name should match the actual executable filename"
        );
    }
}
