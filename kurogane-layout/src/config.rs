//! Declarative packaging configuration for Kurogane projects.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::{AppMetadata, ResolvedResource};

/// Name of the project packaging configuration file.
pub const CONFIG_FILE_NAME: &str = "kurogane.toml";

#[derive(Debug, Error)]
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
    pub frontend: Option<PathBuf>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub copyright: Option<String>,
    pub icon: Option<PathBuf>,
}

impl AppConfig {
    /// Applies configured application identity to distribution metadata.
    pub fn apply_to(&self, metadata: &mut AppMetadata) {
        if let Some(name) = &self.name {
            metadata.name = name.clone();
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
            metadata.icon = Some(icon.clone());
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

/// Code signing configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SigningFileConfig {
    pub certificate: Option<PathBuf>,
    pub timestamp_url: Option<String>,
    pub digest_algorithm: Option<String>,
    pub custom_command: Option<String>,
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
        assert!(config.signing.certificate.is_none());
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
publisher = "Example Corp"
description = "A demo application"
copyright = "(c) 2026 Example Corp"
icon = "assets/icon.png"

[[bundle.resources]]
source = "assets/data"
destination = "share/data"

[[bundle.resources]]
source = "README.md"

[linux]
categories = ["Development", "IDE"]
terminal = true

[windows]
start-menu-shortcut = false
desktop-shortcut = false

[signing]
certificate = "certs/codesign.pfx"
timestamp-url = "http://timestamp.digicert.com"
digest-algorithm = "sha256"
custom-command = "signtool sign /fd sha256"
"#,
        );

        let config = PackagingConfig::load(dir.path()).unwrap();

        assert_eq!(config.app.name.as_deref(), Some("My App"));
        assert_eq!(config.app.frontend.as_deref(), Some(Path::new("web")));
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

        assert!(!config.windows.start_menu_shortcut);
        assert!(!config.windows.desktop_shortcut);

        assert_eq!(
            config.signing.certificate,
            Some(PathBuf::from("certs/codesign.pfx"))
        );
        assert_eq!(
            config.signing.timestamp_url.as_deref(),
            Some("http://timestamp.digicert.com")
        );
        assert_eq!(config.signing.digest_algorithm.as_deref(), Some("sha256"));
        assert_eq!(
            config.signing.custom_command.as_deref(),
            Some("signtool sign /fd sha256")
        );
    }

    #[test]
    fn template_schema_unknown_keys_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            r#"
[app]
name = "kurogane-vanilla-template"
frontend = "content"
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
        assert_eq!(config.app.frontend.as_deref(), Some(Path::new("content")));
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
        let mut metadata = crate::AppMetadata {
            name: "cargo-name".into(),
            ..Default::default()
        };

        // Unset configuration leaves metadata untouched
        AppConfig::default().apply_to(&mut metadata);
        assert_eq!(metadata.name, "cargo-name");
        assert!(metadata.publisher.is_none());

        let app = AppConfig {
            name: Some("Config Name".into()),
            publisher: Some("Example Corp".into()),
            icon: Some(PathBuf::from("assets/icon.png")),
            ..Default::default()
        };
        app.apply_to(&mut metadata);

        assert_eq!(metadata.name, "Config Name");
        assert_eq!(metadata.publisher.as_deref(), Some("Example Corp"));
        assert_eq!(metadata.icon.as_deref(), Some(Path::new("assets/icon.png")));
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
