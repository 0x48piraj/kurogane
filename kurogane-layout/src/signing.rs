//! Code signing operations for packaged artifacts.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

use crate::SigningFileConfig;

/// Code signing configuration.
#[derive(Debug, Clone)]
pub struct SignConfig {
    /// Signing tool override.
    pub tool: Option<PathBuf>,

    /// Certificate thumbprint or path to certificate file.
    pub certificate: Option<String>,

    /// RFC-3161 timestamp authority URL.
    pub timestamp_url: Option<String>,

    /// Signing digest algorithm.
    pub digest_algorithm: String,

    /// Custom signing command.
    /// When set, overrides default tool invocation.
    pub custom_command: Option<String>,

    /// Arguments for the custom signing command.
    pub custom_args: Vec<String>,
}

impl Default for SignConfig {
    fn default() -> Self {
        Self {
            tool: None,
            certificate: None,
            timestamp_url: None,
            digest_algorithm: "sha256".to_string(),
            custom_command: None,
            custom_args: Vec::new(),
        }
    }
}

impl SignConfig {
    /// Returns whether signing is configured.
    pub fn is_configured(&self) -> bool {
        self.certificate.is_some() || self.custom_command.is_some()
    }

    /// Resolves file configuration into signing settings.
    pub fn from_file_config(file: &SigningFileConfig) -> Option<SignConfig> {
        let mut config = SignConfig {
            certificate: file.certificate.as_ref().map(|p| p.display().to_string()),
            timestamp_url: file.timestamp_url.clone(),
            digest_algorithm: file
                .digest_algorithm
                .clone()
                .unwrap_or_else(|| "sha256".to_string()),
            custom_command: None,
            custom_args: Vec::new(),
            tool: None,
        };

        if let Some(command) = &file.custom_command {
            let mut parts = command.split_whitespace();
            config.custom_command = parts.next().map(String::from);
            config.custom_args = parts.map(String::from).collect();
        }

        if config.is_configured() {
            Some(config)
        } else {
            None
        }
    }
}

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("no signing tool found; install signtool.exe (Windows SDK) or osslsigncode")]
    NoSigningTool,

    #[error("custom sign command failed: {command}")]
    CustomCommandFailed { command: String },

    #[error("{tool} failed ({status})")]
    ToolFailed {
        tool: String,
        status: std::process::ExitStatus,
    },

    #[error("signed output was not produced for {}", .0.display())]
    MissingSignedOutput(PathBuf),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Builds signtool `sign` arguments, excluding the tool path and target file.
///
/// The digest algorithm defaults to SHA-256 and timestamps use the RFC-3161
/// `/tr` + `/td` pair.
pub fn signtool_sign_args(config: &SignConfig) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("sign"),
        OsString::from("/fd"),
        OsString::from(&config.digest_algorithm),
    ];

    if let Some(cert) = &config.certificate {
        args.push(OsString::from("/sha1"));
        args.push(OsString::from(cert));
    }

    if let Some(url) = &config.timestamp_url {
        args.push(OsString::from("/tr"));
        args.push(OsString::from(url));
        args.push(OsString::from("/td"));
        args.push(OsString::from(&config.digest_algorithm));
    }

    args
}

/// Builds the certificate input arguments for `osslsigncode`.
///
/// Uses `-pkcs12` for PKCS#12 certificates and `-certs` for PEM/DER certificates.
fn osslsigncode_cert_args(certificate: &str) -> Vec<OsString> {
    let is_pkcs12 = Path::new(certificate)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "pfx" | "p12" | "pkcs12"));

    if is_pkcs12 {
        vec![OsString::from("-pkcs12"), OsString::from(certificate)]
    } else {
        vec![OsString::from("-certs"), OsString::from(certificate)]
    }
}

/// Builds arguments for `osslsigncode sign` using the configured certificate,
/// timestamp URL and digest algorithm.
pub fn osslsigncode_sign_args(config: &SignConfig, input: &Path, output: &Path) -> Vec<OsString> {
    let mut args = vec![OsString::from("sign")];

    if let Some(cert) = &config.certificate {
        args.extend(osslsigncode_cert_args(cert));
    }

    if let Some(url) = &config.timestamp_url {
        args.push(OsString::from("-ts"));
        args.push(OsString::from(url));
    }

    args.push(OsString::from("-h"));
    args.push(OsString::from(&config.digest_algorithm));
    args.push(OsString::from("-in"));
    args.push(OsString::from(input));
    args.push(OsString::from("-out"));
    args.push(OsString::from(output));

    args
}

/// Builds arguments for `signtool verify` using the default Authenticode
/// policy and checking all signatures.
pub fn signtool_verify_args(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("verify"),
        OsString::from("/pa"),
        OsString::from("/all"),
        OsString::from(path),
    ]
}

/// Builds arguments for `osslsigncode verify`.
pub fn osslsigncode_verify_args(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("verify"),
        OsString::from("-in"),
        OsString::from(path),
    ]
}

/// Resolves custom signing arguments for a target artifact.
fn expand_custom_args(args: &[String], path: &Path) -> Vec<OsString> {
    args.iter()
        .map(|arg| {
            if arg == "%1" {
                OsString::from(path)
            } else {
                OsString::from(arg)
            }
        })
        .collect()
}

fn run_custom(path: &Path, config: &SignConfig) -> Result<(), SigningError> {
    let command = config.custom_command.as_deref().unwrap_or_default();

    let status = Command::new(command)
        .args(expand_custom_args(&config.custom_args, path))
        .status()?;

    if !status.success() {
        return Err(SigningError::CustomCommandFailed {
            command: command.to_string(),
        });
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn find_signtool(override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = override_path
        && path.exists()
    {
        return Some(path.to_path_buf());
    }

    // Check KUROGANE_SIGNTOOL_PATH env var
    if let Ok(path) = std::env::var("KUROGANE_SIGNTOOL_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Try common Windows SDK locations
    let program_files = std::env::var("ProgramFiles(x86)")
        .or_else(|_| std::env::var("ProgramFiles"))
        .unwrap_or_default();

    let kits_root = Path::new(&program_files)
        .join("Windows Kits")
        .join("10")
        .join("bin");

    if let Ok(entries) = std::fs::read_dir(&kits_root) {
        let mut kits: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().map(String::from))
            .filter(|s| s.starts_with("10."))
            .collect();
        kits.sort();

        let arch = if cfg!(target_arch = "x86_64") {
            "x64"
        } else {
            "x86"
        };

        for kit in kits.iter().rev() {
            let signtool = kits_root.join(kit).join(arch).join("signtool.exe");
            if signtool.exists() {
                return Some(signtool);
            }
        }
    }

    None
}

fn find_osslsigncode() -> Option<PathBuf> {
    // Check override path
    if let Ok(path) = std::env::var("KUROGANE_OSSLSIGNCODE_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Try to find osslsigncode in PATH
    #[cfg(unix)]
    {
        if let Ok(output) = Command::new("which").arg("osslsigncode").output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Some(PathBuf::from(path));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = Command::new("where").arg("osslsigncode").output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            return Some(PathBuf::from(path));
        }
    }

    None
}

/// Signs a file using the configured signing strategy.
pub fn sign_file(path: &Path, config: &SignConfig) -> Result<(), SigningError> {
    if !config.is_configured() {
        return Ok(());
    }

    if config.custom_command.is_some() {
        return run_custom(path, config);
    }

    #[cfg(target_os = "windows")]
    if let Some(signtool) = find_signtool(config.tool.as_deref()) {
        return sign_with_signtool(path, &signtool, config);
    }

    if let Some(osslsigncode) = find_osslsigncode() {
        return sign_with_osslsigncode(path, &osslsigncode, config);
    }

    Err(SigningError::NoSigningTool)
}

#[cfg(target_os = "windows")]
fn sign_with_signtool(
    path: &Path,
    signtool: &Path,
    config: &SignConfig,
) -> Result<(), SigningError> {
    let status = Command::new(signtool)
        .args(signtool_sign_args(config))
        .arg(path)
        .status()?;

    if !status.success() {
        return Err(SigningError::ToolFailed {
            tool: "signtool".to_string(),
            status,
        });
    }

    Ok(())
}

fn sign_with_osslsigncode(
    path: &Path,
    osslsigncode: &Path,
    config: &SignConfig,
) -> Result<(), SigningError> {
    // Preserve the original until signing succeeds.
    let mut output = path.as_os_str().to_os_string();
    output.push(".kurogane-sign-tmp");
    let output = PathBuf::from(output);

    let result = Command::new(osslsigncode)
        .args(osslsigncode_sign_args(config, path, &output))
        .status();

    match result {
        Ok(status) if status.success() => {
            if !output.exists() {
                return Err(SigningError::MissingSignedOutput(path.to_path_buf()));
            }
            if output != path {
                fs::rename(&output, path)?;
            }
            Ok(())
        }
        Ok(status) => {
            let _ = fs::remove_file(&output);
            Err(SigningError::ToolFailed {
                tool: "osslsigncode".to_string(),
                status,
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&output);
            Err(e.into())
        }
    }
}

/// Determines whether a bundle entry is a signable PE artifact.
pub fn should_sign(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "exe" || ext == "dll")
}

/// Signs signable artifacts within a bundle.
pub fn sign_tree(root: &Path, config: &SignConfig) -> Result<usize, SigningError> {
    if !config.is_configured() {
        return Ok(0);
    }

    let mut signed = 0;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                stack.push(path);
            } else if should_sign(&path) {
                sign_file(&path, config)?;
                signed += 1;
            }
        }
    }

    Ok(signed)
}

/// Signs a packaged artifact.
pub fn sign_artifact(path: &Path, config: &SignConfig) -> Result<(), SigningError> {
    sign_file(path, config)
}

/// Verifies a packaged artifact using the configured signing strategy.
pub fn verify_signature(path: &Path, config: &SignConfig) -> Result<(), SigningError> {
    if !config.is_configured() {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    if let Some(signtool) = find_signtool(config.tool.as_deref()) {
        let status = Command::new(signtool)
            .args(signtool_verify_args(path))
            .status()?;
        return if status.success() {
            Ok(())
        } else {
            Err(SigningError::ToolFailed {
                tool: "signtool verify".to_string(),
                status,
            })
        };
    }

    if let Some(osslsigncode) = find_osslsigncode() {
        let status = Command::new(osslsigncode)
            .args(osslsigncode_verify_args(path))
            .status()?;
        return if status.success() {
            Ok(())
        } else {
            Err(SigningError::ToolFailed {
                tool: "osslsigncode verify".to_string(),
                status,
            })
        };
    }

    Err(SigningError::NoSigningTool)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        crate::test_fixtures::tmp_dir()
    }

    fn os(input: &[&str]) -> Vec<OsString> {
        input.iter().map(OsString::from).collect()
    }

    #[test]
    fn default_config_is_not_configured() {
        assert!(!SignConfig::default().is_configured());
    }

    #[test]
    fn certificate_config_enables_signing() {
        let config = SignConfig {
            certificate: Some("thumbprint".to_string()),
            ..Default::default()
        };
        assert!(config.is_configured());
    }

    #[test]
    fn custom_command_config_enables_signing() {
        let config = SignConfig {
            custom_command: Some("my-sign-tool".to_string()),
            ..Default::default()
        };
        assert!(config.is_configured());
    }

    #[test]
    fn timestamp_only_does_not_enable_signing() {
        let config = SignConfig {
            timestamp_url: Some("http://timestamp".to_string()),
            ..Default::default()
        };
        assert!(!config.is_configured());
    }

    #[test]
    fn should_sign_exe() {
        assert!(should_sign(Path::new("/some/path/app.exe")));
    }

    #[test]
    fn should_sign_dll() {
        assert!(should_sign(Path::new("/some/path/lib.dll")));
    }

    #[test]
    fn should_not_sign_other_files() {
        assert!(!should_sign(Path::new("/some/readme.txt")));
        assert!(!should_sign(Path::new("/some/app.AppImage")));
        assert!(!should_sign(Path::new("/some/binary")));
    }

    #[test]
    fn sign_returns_ok_when_not_configured() {
        let dir = tmp();
        let path = dir.path().join("app.exe");
        assert!(sign_file(&path, &SignConfig::default()).is_ok());
    }

    #[test]
    fn sign_custom_command_expands_target_path() {
        let dir = tmp();
        let target = dir.path().join("app.exe");
        fs::write(&target, "").unwrap();

        let config = SignConfig {
            custom_command: Some("echo".to_string()),
            custom_args: vec!["%1".to_string(), "--flag".to_string()],
            ..Default::default()
        };

        assert!(sign_file(&target, &config).is_ok());
    }

    #[test]
    fn sign_custom_command_failure_is_propagated() {
        let dir = tmp();
        let target = dir.path().join("app.exe");
        fs::write(&target, "").unwrap();

        let config = SignConfig {
            custom_command: Some("false".to_string()),
            ..Default::default()
        };

        let err = sign_file(&target, &config).unwrap_err();
        assert!(matches!(err, SigningError::CustomCommandFailed { .. }));
    }

    #[test]
    fn expand_custom_args_replaces_placeholder() {
        let expanded = expand_custom_args(
            &["%1".into(), "--flag".into(), "/literal %1".into()],
            Path::new("/bin/app.exe"),
        );

        assert_eq!(expanded, os(&["/bin/app.exe", "--flag", "/literal %1"]));
    }

    #[test]
    fn signtool_args_default_to_sha256_without_timestamp() {
        let config = SignConfig {
            certificate: Some("abc123".to_string()),
            ..Default::default()
        };

        assert_eq!(
            signtool_sign_args(&config),
            os(&["sign", "/fd", "sha256", "/sha1", "abc123"])
        );
    }

    #[test]
    fn signtool_args_pair_rfc3161_directives() {
        let config = SignConfig {
            certificate: Some("abc123".to_string()),
            timestamp_url: Some("http://ts.example".to_string()),
            digest_algorithm: "sha1".to_string(),
            ..Default::default()
        };

        assert_eq!(
            signtool_sign_args(&config),
            os(&[
                "sign",
                "/fd",
                "sha1",
                "/sha1",
                "abc123",
                "/tr",
                "http://ts.example",
                "/td",
                "sha1",
            ])
        );
    }

    #[test]
    fn osslsigncode_uses_in_out_form() {
        let config = SignConfig {
            certificate: Some("/certs/cert.pfx".to_string()),
            timestamp_url: Some("http://ts.example".to_string()),
            ..Default::default()
        };

        assert_eq!(
            osslsigncode_sign_args(
                &config,
                Path::new("/dist/app.exe"),
                Path::new("/dist/app.exe.kurogane-sign-tmp"),
            ),
            os(&[
                "sign",
                "-pkcs12",
                "/certs/cert.pfx",
                "-ts",
                "http://ts.example",
                "-h",
                "sha256",
                "-in",
                "/dist/app.exe",
                "-out",
                "/dist/app.exe.kurogane-sign-tmp",
            ])
        );
    }

    #[test]
    fn osslsigncode_selects_certs_flag_for_pem_chains() {
        let config = SignConfig {
            certificate: Some("/certs/chain.pem".to_string()),
            ..Default::default()
        };

        let args = osslsigncode_sign_args(&config, Path::new("/a.exe"), Path::new("/b.tmp"));

        assert!(
            args.windows(2).any(|w| w[0] == "-certs"),
            "PEM chain must use -certs"
        );
        assert!(
            !args.contains(&OsString::from("-pkcs12")),
            "PEM chain must not use -pkcs12"
        );
    }

    #[test]
    fn osslsigncode_p12_extension_also_uses_pkcs12() {
        assert_eq!(
            osslsigncode_cert_args("/certs/store.P12"),
            os(&["-pkcs12", "/certs/store.P12"])
        );
    }

    #[test]
    fn verify_args_are_conservative() {
        assert_eq!(
            signtool_verify_args(Path::new("/app.exe")),
            os(&["verify", "/pa", "/all", "/app.exe"])
        );
        assert_eq!(
            osslsigncode_verify_args(Path::new("/app.exe")),
            os(&["verify", "-in", "/app.exe"])
        );
    }

    #[test]
    fn from_file_config_maps_certificate_and_digest() {
        let file = SigningFileConfig {
            certificate: Some("/certs/codesign.pfx".into()),
            timestamp_url: Some("http://ts.example".into()),
            digest_algorithm: Some("sha512".into()),
            custom_command: None,
        };

        let config = SignConfig::from_file_config(&file).unwrap();

        assert_eq!(config.certificate.as_deref(), Some("/certs/codesign.pfx"));
        assert_eq!(config.timestamp_url.as_deref(), Some("http://ts.example"));
        assert_eq!(config.digest_algorithm, "sha512");
        assert!(config.is_configured());
    }

    #[test]
    fn from_file_config_splits_whitespace_command() {
        let file = SigningFileConfig {
            custom_command: Some("signtool sign /fd sha256 extra.bin".into()),
            ..Default::default()
        };

        let config = SignConfig::from_file_config(&file).unwrap();

        assert_eq!(config.custom_command.as_deref(), Some("signtool"));
        assert_eq!(
            config.custom_args,
            vec![
                "sign".to_string(),
                "/fd".to_string(),
                "sha256".to_string(),
                "extra.bin".to_string(),
            ]
        );
    }

    #[test]
    fn from_file_config_defaults_digest_and_none_when_unconfigured() {
        assert!(SignConfig::from_file_config(&SigningFileConfig::default()).is_none());

        let file = SigningFileConfig {
            certificate: Some("/c.pfx".into()),
            ..Default::default()
        };
        let config = SignConfig::from_file_config(&file).unwrap();
        assert_eq!(config.digest_algorithm, "sha256");
    }

    #[test]
    fn sign_tree_counts_only_pe_files() {
        let dir = tmp();
        fs::write(dir.path().join("app.exe"), "").unwrap();
        fs::write(dir.path().join("notes.txt"), "").unwrap();
        let nested = dir.path().join("runtime");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("lib.dll"), "").unwrap();

        let count = sign_tree(dir.path(), &SignConfig::default()).unwrap();
        assert_eq!(count, 0, "unconfigured signing signs nothing");

        let config = SignConfig {
            custom_command: Some("true".to_string()),
            ..Default::default()
        };
        let count = sign_tree(dir.path(), &config).unwrap();
        assert_eq!(count, 2, "only app.exe and runtime/lib.dll are signed");
    }
}
