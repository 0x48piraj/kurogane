//! Declarative packaging configuration for Kurogane projects.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use kurogane_layout::{AppMetadata, ResolvedResource};

/// Name of the project packaging configuration file.
pub const CONFIG_FILE_NAME: &str = "kurogane.toml";

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("failed to read {}", .0.display())]
    Io(PathBuf, #[source] std::io::Error),

    #[error("failed to parse {}", .0.display())]
    Parse(PathBuf, #[source] Box<toml::de::Error>),

    #[error("resource source has no file name: {}", .0.display())]
    InvalidResourceSource(PathBuf),
}

/// Project packaging configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PackagingConfig {
    pub app: AppConfig,
    pub bundle: BundleConfig,
    pub linux: LinuxPackagingConfig,
    pub macos: MacosPackagingConfig,
    pub windows: WindowsPackagingConfig,
    pub signing: SigningFileConfig,
}

impl PackagingConfig {
    /// Loads the project packaging configuration.
    pub fn load(project_root: &Path) -> Result<PackagingConfig, ConfigError> {
        let path = project_root.join(CONFIG_FILE_NAME);

        if !path.exists() {
            return Ok(PackagingConfig::default());
        }

        let raw = fs::read_to_string(&path).map_err(|e| ConfigError::Io(path.clone(), e))?;
        toml::from_str(&raw).map_err(|e| ConfigError::Parse(path.clone(), Box::new(e)))
    }
}

/// Resolve relative paths against the project root so
/// bundling works from any working directory.
///
/// Absolute paths pass through unchanged.
pub fn anchor_path(project_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

/// Application identity and presentation configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct AppConfig {
    pub name: Option<String>,
    pub identifier: Option<String>,
    pub frontend: Option<PathBuf>,
    pub frontend_dist: Option<PathBuf>,
    pub frontend_build: Option<String>,
    pub frontend_install: Option<String>,
    pub frontend_run: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub copyright: Option<String>,
    pub icon: Option<PathBuf>,
    /// Whether the application enters Chromium's sandbox.
    ///
    /// When enabled, the CLI builds and starts the application with
    /// platform-specific setup required by the sandbox. The runtime
    /// policy is selected separately with `App::sandbox_mode`.
    pub sandbox: bool,
}

impl AppConfig {
    /// Applies configured application identity to distribution metadata.
    /// The icon is a project path, anchored to `project_root`.
    pub fn apply_to(&self, metadata: &mut AppMetadata, project_root: &Path) {
        if let Some(name) = &self.name {
            metadata.name = name.clone();
        }
        if let Some(identifier) = &self.identifier {
            metadata.identifier = Some(identifier.clone());
        }
        if let Some(publisher) = &self.publisher {
            metadata.publisher = Some(publisher.clone());
        }
        if let Some(description) = &self.description {
            metadata.description = Some(description.clone());
        }
        if let Some(copyright) = &self.copyright {
            metadata.copyright = Some(copyright.clone());
        }
        if let Some(icon) = &self.icon {
            metadata.icon = Some(anchor_path(project_root, icon));
        }
    }
}

/// Canonical bundle configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct BundleConfig {
    pub resources: Vec<ResourceConfig>,
}

/// Additional bundle resource configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ResourceConfig {
    pub source: PathBuf,
    pub destination: Option<String>,
}

impl ResourceConfig {
    /// Resolves the resource declaration into a bundle resource.
    pub fn to_resolved(&self) -> Result<ResolvedResource, ConfigError> {
        let destination = match &self.destination {
            Some(dest) => PathBuf::from(dest),
            None => self
                .source
                .file_name()
                .map(PathBuf::from)
                .ok_or_else(|| ConfigError::InvalidResourceSource(self.source.clone()))?,
        };

        Ok(ResolvedResource {
            source: self.source.clone(),
            destination,
        })
    }
}

/// Linux packaging configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LinuxPackagingConfig {
    pub categories: Option<Vec<String>>,
    pub terminal: Option<bool>,
}

/// `[macos]`: what a macOS `.app` declares in its `Info.plist`.
///
/// Values are read as written: the macOS bundler checks them when it
/// builds, so a mistake here never stops `kurogane run` elsewhere. A key the
/// table does not take is an error naming it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct MacosPackagingConfig {
    /// `LSMinimumSystemVersion`, such as `"13.0"`; CEF's own floor when
    /// unset, and never lower.
    pub minimum_system_version: Option<String>,
    /// `LSApplicationCategoryType`, one of Apple's
    /// `public.app-category.*` values.
    pub category: Option<String>,
    /// `[macos.privacy]`: why the application uses each device.
    pub privacy: MacosPrivacyConfig,
}

/// `[macos.privacy]`: what macOS shows when a page asks for a device, in
/// place of Kurogane's generic wording. Every device a page can ask for is
/// declared, configured or not.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MacosPrivacyConfig {
    /// `NSCameraUsageDescription`.
    pub camera: Option<String>,
    /// `NSMicrophoneUsageDescription`.
    pub microphone: Option<String>,
}

/// Windows installer configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct WindowsPackagingConfig {
    pub start_menu_shortcut: bool,
    pub desktop_shortcut: bool,
}

impl Default for WindowsPackagingConfig {
    fn default() -> Self {
        Self {
            start_menu_shortcut: true,
            desktop_shortcut: true,
        }
    }
}

/// Code signing, one table per platform that signs: each bundler reads its
/// own, and `--sign` checks both wherever it runs.
///
/// A key a table does not take, such as the older flat `[signing]` keys, is
/// an error naming it, never ignored.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SigningFileConfig {
    /// `[signing.windows]`: the PE files of a Windows bundle and installer.
    pub windows: WindowsSigningConfig,
    /// `[signing.macos]`: a macOS `.app`.
    pub macos: MacosSigningConfig,
}

/// `[signing.windows]`: a certificate, as a file (`certificate`) or a Windows
/// certificate store thumbprint (`certificate-thumbprint`), or instead a
/// signing program of the application's own (`custom-command`).
///
/// Passwords are never stored here: `certificate-password-env` names the
/// environment variable the password is read from, so CI keeps it in secrets
/// and it stays out of version control naturally.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct WindowsSigningConfig {
    pub certificate: Option<PathBuf>,
    pub certificate_thumbprint: Option<String>,
    pub certificate_password_env: Option<String>,
    pub timestamp_url: Option<String>,
    pub digest_algorithm: Option<String>,
    /// A program and its arguments; `%1` in an argument is the file to sign.
    pub custom_command: Option<Vec<String>>,
}

/// `[signing.macos]`: the codesign identity a `.app` is signed with, from the
/// keychain; `-` signs ad hoc.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MacosSigningConfig {
    pub identity: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &Path, contents: &str) {
        fs::write(dir.join(CONFIG_FILE_NAME), contents).unwrap();
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();

        let config = PackagingConfig::load(dir.path()).unwrap();

        assert_eq!(config.app.name, None);
        assert!(!config.app.sandbox, "the sandbox is opt-in");
        assert!(config.bundle.resources.is_empty());
        assert!(config.windows.start_menu_shortcut);
        assert!(config.windows.desktop_shortcut);
    }

    #[test]
    fn empty_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path(), "");

        let config = PackagingConfig::load(dir.path()).unwrap();

        assert_eq!(config.app.name, None);
        assert!(config.linux.categories.is_none());
        assert!(config.macos.minimum_system_version.is_none());
        assert!(config.macos.privacy.camera.is_none());
        assert!(config.signing.windows.certificate.is_none());
        assert!(config.signing.macos.identity.is_none());
    }

    #[test]
    fn full_example_parses() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            r#"
[app]
name = "My App"
frontend = "web"
frontend-dist = "web/dist"
frontend-build = "npm --prefix web run build"
frontend-install = "npm --prefix web install"
frontend-run = "npm --prefix web run dev"
publisher = "Example Corp"
description = "A demo application"
copyright = "(c) 2026 Example Corp"
icon = "assets/icon.png"
sandbox = true

[[bundle.resources]]
source = "assets/data"
destination = "share/data"

[[bundle.resources]]
source = "README.md"

[linux]
categories = ["Development", "IDE"]
terminal = true

[macos]
minimum-system-version = "13.0"
category = "public.app-category.video"

[macos.privacy]
camera = "Calls use your camera."
microphone = "Calls use your microphone."

[windows]
start-menu-shortcut = false
desktop-shortcut = false

[signing.windows]
certificate = "certs/codesign.pfx"
timestamp-url = "http://timestamp.digicert.com"
digest-algorithm = "sha256"

[signing.macos]
identity = "Developer ID Application: Example Corp (TEAMID1234)"
"#,
        );

        let config = PackagingConfig::load(dir.path()).unwrap();

        assert_eq!(config.app.name.as_deref(), Some("My App"));
        assert_eq!(config.app.frontend.as_deref(), Some(Path::new("web")));
        assert_eq!(
            config.app.frontend_dist.as_deref(),
            Some(Path::new("web/dist"))
        );
        assert_eq!(
            config.app.frontend_build.as_deref(),
            Some("npm --prefix web run build")
        );
        assert_eq!(
            config.app.frontend_install.as_deref(),
            Some("npm --prefix web install")
        );
        assert_eq!(
            config.app.frontend_run.as_deref(),
            Some("npm --prefix web run dev")
        );
        assert_eq!(config.app.publisher.as_deref(), Some("Example Corp"));
        assert_eq!(
            config.app.description.as_deref(),
            Some("A demo application")
        );
        assert_eq!(
            config.app.copyright.as_deref(),
            Some("(c) 2026 Example Corp")
        );
        assert_eq!(
            config.app.icon.as_deref(),
            Some(Path::new("assets/icon.png"))
        );
        assert!(config.app.sandbox);

        assert_eq!(config.bundle.resources.len(), 2);
        let resolved: Vec<_> = config
            .bundle
            .resources
            .iter()
            .map(|r| r.to_resolved().unwrap())
            .collect();
        assert_eq!(resolved[0].destination, PathBuf::from("share/data"));
        assert_eq!(resolved[1].destination, PathBuf::from("README.md"));

        assert_eq!(
            config.linux.categories,
            Some(vec!["Development".into(), "IDE".into()])
        );
        assert_eq!(config.linux.terminal, Some(true));

        assert_eq!(config.macos.minimum_system_version.as_deref(), Some("13.0"));
        assert_eq!(
            config.macos.category.as_deref(),
            Some("public.app-category.video")
        );
        assert_eq!(
            config.macos.privacy.camera.as_deref(),
            Some("Calls use your camera.")
        );
        assert_eq!(
            config.macos.privacy.microphone.as_deref(),
            Some("Calls use your microphone.")
        );

        assert!(!config.windows.start_menu_shortcut);
        assert!(!config.windows.desktop_shortcut);

        let windows = &config.signing.windows;
        assert_eq!(
            windows.certificate,
            Some(PathBuf::from("certs/codesign.pfx"))
        );
        assert_eq!(
            windows.timestamp_url.as_deref(),
            Some("http://timestamp.digicert.com")
        );
        assert_eq!(windows.digest_algorithm.as_deref(), Some("sha256"));
        assert_eq!(
            config.signing.macos.identity.as_deref(),
            Some("Developer ID Application: Example Corp (TEAMID1234)")
        );
    }

    #[test]
    fn signing_takes_a_table_per_platform() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            r#"
[signing.windows]
certificate-thumbprint = "AB12CD34"
certificate-password-env = "KUROGANE_CERT_PASSWORD"
timestamp-url = "http://timestamp.digicert.com"

[signing.macos]
identity = "Developer ID Application: Acme (TEAMID1234)"
"#,
        );

        let config = PackagingConfig::load(dir.path()).unwrap();

        let windows = &config.signing.windows;
        assert_eq!(windows.certificate_thumbprint.as_deref(), Some("AB12CD34"));
        assert_eq!(
            windows.certificate_password_env.as_deref(),
            Some("KUROGANE_CERT_PASSWORD")
        );
        assert_eq!(
            windows.timestamp_url.as_deref(),
            Some("http://timestamp.digicert.com")
        );
        assert!(windows.certificate.is_none(), "a thumbprint is not a file");
        assert_eq!(
            config.signing.macos.identity.as_deref(),
            Some("Developer ID Application: Acme (TEAMID1234)")
        );
    }

    #[test]
    fn a_custom_command_is_an_argument_list() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            r#"
[signing.windows]
custom-command = ['C:\Program Files\Signer\sign.exe', "sign", "%1"]
"#,
        );

        let config = PackagingConfig::load(dir.path()).unwrap();

        assert_eq!(
            config.signing.windows.custom_command,
            Some(vec![
                r"C:\Program Files\Signer\sign.exe".to_string(),
                "sign".to_string(),
                "%1".to_string(),
            ])
        );
    }

    #[test]
    fn signing_keys_outside_their_table_are_refused_where_they_are_written() {
        for (written, key, line) in [
            // The flat table before per-platform tables
            (
                "[signing]\ncertificate = \"c.pfx\"\n",
                "certificate",
                "line 2",
            ),
            // A key of the other platform
            (
                "[signing.windows]\nidentity = \"Developer ID Application\"\n",
                "identity",
                "line 2",
            ),
            (
                "[signing.macos]\ncertificate = \"c.pfx\"\n",
                "certificate",
                "line 2",
            ),
            // A command line where an argument list belongs
            (
                "[signing.windows]\ncustom-command = \"signtool sign %1\"\n",
                "custom-command",
                "line 2",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            write_config(dir.path(), written);

            let err = PackagingConfig::load(dir.path()).unwrap_err();

            let ConfigError::Parse(_, cause) = &err else {
                panic!("{written:?} is a parse error, got: {err}");
            };
            let cause = cause.to_string();
            assert!(
                cause.contains(key) && cause.contains(line),
                "{written:?}: the error names the key and its line: {cause}"
            );
        }
    }

    #[test]
    fn macos_keys_the_tables_do_not_take_are_refused_where_they_are_written() {
        for (written, key, line) in [
            // A misspelt key
            (
                "[macos]\nminimum-version = \"13.0\"\n",
                "minimum-version",
                "line 2",
            ),
            // A device the bundler declares nothing for
            (
                "[macos.privacy]\nlocation = \"Maps\"\n",
                "location",
                "line 2",
            ),
            // The Info.plist key itself, not the device
            (
                "[macos.privacy]\nNSCameraUsageDescription = \"Calls\"\n",
                "NSCameraUsageDescription",
                "line 2",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            write_config(dir.path(), written);

            let err = PackagingConfig::load(dir.path()).unwrap_err();

            let ConfigError::Parse(_, cause) = &err else {
                panic!("{written:?} is a parse error, got: {err}");
            };
            let cause = cause.to_string();
            assert!(
                cause.contains(key) && cause.contains(line),
                "{written:?}: the error names the key and its line: {cause}"
            );
        }
    }

    #[test]
    fn template_schema_unknown_keys_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            r#"
[app]
name = "kurogane-vanilla-template"
frontend = "web"
frontend-dist = "content"
dev_url = "http://localhost:3000"

[bundle]
future-option = 42
"#,
        );

        let config = PackagingConfig::load(dir.path()).unwrap();

        assert_eq!(
            config.app.name.as_deref(),
            Some("kurogane-vanilla-template")
        );
        assert_eq!(config.app.frontend.as_deref(), Some(Path::new("web")));
        assert_eq!(
            config.app.frontend_dist.as_deref(),
            Some(Path::new("content"))
        );
        assert!(config.linux.terminal.is_none());
    }

    #[test]
    fn windows_section_defaults_to_enabled_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path(), "[app]\nname = \"x\"\n");

        let config = PackagingConfig::load(dir.path()).unwrap();

        assert!(config.windows.start_menu_shortcut);
        assert!(config.windows.desktop_shortcut);
    }

    #[test]
    fn resource_without_filename_is_rejected() {
        let config = ResourceConfig {
            source: PathBuf::from(".."),
            destination: None,
        };

        let err = config.to_resolved().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidResourceSource(_)));
    }

    #[test]
    fn invalid_toml_reports_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path(), "[app\nname = ");

        let err = PackagingConfig::load(dir.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_, _)));
    }

    #[test]
    fn apply_to_overrides_only_set_fields() {
        let mut metadata = kurogane_layout::AppMetadata {
            name: "cargo-name".into(),
            ..Default::default()
        };

        // Unset configuration leaves metadata untouched
        AppConfig::default().apply_to(&mut metadata, Path::new("/project"));
        assert_eq!(metadata.name, "cargo-name");
        assert!(metadata.publisher.is_none());

        let app = AppConfig {
            name: Some("Config Name".into()),
            publisher: Some("Example Corp".into()),
            icon: Some(PathBuf::from("assets/icon.png")),
            ..Default::default()
        };
        app.apply_to(&mut metadata, Path::new("/project"));

        assert_eq!(metadata.name, "Config Name");
        assert_eq!(metadata.publisher.as_deref(), Some("Example Corp"));
        assert_eq!(
            metadata.icon,
            Some(Path::new("/project").join("assets/icon.png")),
            "the icon is a project path"
        );
        assert!(metadata.description.is_none());
        assert!(metadata.copyright.is_none());
    }

    #[test]
    fn anchor_path_joins_relative_paths() {
        let anchored = anchor_path(Path::new("/workspace"), Path::new("content"));

        assert_eq!(anchored, PathBuf::from("/workspace/content"));
    }

    #[test]
    fn anchor_path_keeps_absolute_paths() {
        let absolute = if cfg!(windows) {
            PathBuf::from(r"C:\shared\assets\icon.png")
        } else {
            PathBuf::from("/usr/share/icon.png")
        };

        let anchored = anchor_path(Path::new("/workspace"), &absolute);

        assert_eq!(anchored, absolute);
    }
}
