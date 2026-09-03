//! Code signing operations for packaged artifacts.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

use crate::SigningFileConfig;

/// Source of the signing certificate.
///
/// Represents either a certificate file or a Windows certificate-store thumbprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertificateSource {
    /// A certificate file with an optional password environment variable.
    File {
        path: PathBuf,
        password_env: Option<String>,
    },

    /// A SHA-1 thumbprint identifying a certificate in the Windows store.
    Thumbprint(String),
}

/// Code signing configuration.
#[derive(Debug, Clone)]
pub struct SignConfig {
    /// Signing tool override.
    pub tool: Option<PathBuf>,

    /// How the signing certificate is supplied.
    pub certificate: Option<CertificateSource>,

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
    ///
    /// Returns `Ok(None)` when nothing is configured.
    pub fn from_file_config(file: &SigningFileConfig) -> Result<Option<SignConfig>, SigningError> {
        let certificate = match (&file.certificate, &file.certificate_thumbprint) {
            (Some(_), Some(_)) => return Err(SigningError::AmbiguousCertificate),
            (Some(path), None) => Some(CertificateSource::File {
                path: path.clone(),
                password_env: file.certificate_password_env.clone(),
            }),
            (None, Some(thumbprint)) => Some(CertificateSource::Thumbprint(thumbprint.clone())),
            (None, None) => None,
        };

        let mut config = SignConfig {
            certificate,
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

        Ok(config.is_configured().then_some(config))
    }
}

/// Resolves the certificate password from its configured environment variable.
fn resolve_password(
    certificate: Option<&CertificateSource>,
) -> Result<Option<String>, SigningError> {
    let Some(CertificateSource::File {
        password_env: Some(name),
        ..
    }) = certificate
    else {
        return Ok(None);
    };

    std::env::var(name)
        .map(Some)
        .map_err(|_| SigningError::MissingCertificatePassword { env: name.clone() })
}

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("no signing tool found; install signtool.exe (Windows SDK) or osslsigncode")]
    NoSigningTool,

    #[error(
        "[signing] sets both `certificate` and `certificate-thumbprint`; \
         choose one (a certificate file or a Windows certificate store thumbprint)"
    )]
    AmbiguousCertificate,

    #[error(
        "certificate password environment variable `{env}` is not set; \
         export it or remove `certificate-password-env` from [signing]"
    )]
    MissingCertificatePassword { env: String },

    #[error(
        "osslsigncode cannot use a Windows certificate store thumbprint; \
         set `certificate` to a PKCS#12 or PEM file instead"
    )]
    ThumbprintUnsupported,

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

/// Builds `signtool sign` arguments, excluding the tool path and target file.
pub fn signtool_sign_args(config: &SignConfig, password: Option<&str>) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("sign"),
        OsString::from("/fd"),
        OsString::from(&config.digest_algorithm),
    ];

    match &config.certificate {
        // Certificate files use `/f`; store certificates use `/sha1`
        Some(CertificateSource::File { path, .. }) => {
            args.push(OsString::from("/f"));
            args.push(path.into());

            if let Some(password) = password {
                args.push(OsString::from("/p"));
                args.push(OsString::from(password));
            }
        }
        Some(CertificateSource::Thumbprint(thumbprint)) => {
            args.push(OsString::from("/sha1"));
            args.push(OsString::from(thumbprint));
        }
        None => {}
    }

    if let Some(url) = &config.timestamp_url {
        args.push(OsString::from("/tr"));
        args.push(OsString::from(url));
        args.push(OsString::from("/td"));
        args.push(OsString::from(&config.digest_algorithm));
    }

    args
}

/// Builds certificate arguments for `osslsigncode`.
///
/// Uses `-pkcs12` for PKCS#12 certificates and `-certs` for PEM/DER certificates.
fn osslsigncode_cert_args(
    certificate: &CertificateSource,
    password: Option<&str>,
) -> Result<Vec<OsString>, SigningError> {
    let CertificateSource::File { path, .. } = certificate else {
        // osslsigncode has no equivalent of the Windows certificate store
        return Err(SigningError::ThumbprintUnsupported);
    };

    let is_pkcs12 = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "pfx" | "p12" | "pkcs12"));

    let mut args = if is_pkcs12 {
        vec![OsString::from("-pkcs12"), path.into()]
    } else {
        vec![OsString::from("-certs"), path.into()]
    };

    if is_pkcs12 && let Some(password) = password {
        args.push(OsString::from("-pass"));
        args.push(OsString::from(password));
    }

    Ok(args)
}

/// Builds arguments for `osslsigncode sign`.
pub fn osslsigncode_sign_args(
    config: &SignConfig,
    password: Option<&str>,
    input: &Path,
    output: &Path,
) -> Result<Vec<OsString>, SigningError> {
    let mut args = vec![OsString::from("sign")];

    if let Some(cert) = &config.certificate {
        args.extend(osslsigncode_cert_args(cert, password)?);
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

    Ok(args)
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
    let password = resolve_password(config.certificate.as_ref())?;

    let status = Command::new(signtool)
        .args(signtool_sign_args(config, password.as_deref()))
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

    let password = resolve_password(config.certificate.as_ref())?;
    let args = osslsigncode_sign_args(config, password.as_deref(), path, &output)?;

    let result = Command::new(osslsigncode).args(args).status();

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

/// Verifies every signable artifact within a bundle.
///
/// The counterpart to [`sign_tree`]. Returns the number of verified artifacts.
pub fn verify_tree(root: &Path, config: &SignConfig) -> Result<usize, SigningError> {
    if !config.is_configured() {
        return Ok(0);
    }

    let mut verified = 0;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                stack.push(path);
            } else if should_sign(&path) {
                verify_signature(&path, config)?;
                verified += 1;
            }
        }
    }

    Ok(verified)
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
            certificate: Some(CertificateSource::Thumbprint("thumbprint".into())),
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

    fn file_cert(path: &str) -> Option<CertificateSource> {
        Some(CertificateSource::File {
            path: PathBuf::from(path),
            password_env: None,
        })
    }

    #[test]
    fn signtool_args_default_to_sha256_without_timestamp() {
        let config = SignConfig {
            certificate: Some(CertificateSource::Thumbprint("abc123".into())),
            ..Default::default()
        };

        assert_eq!(
            signtool_sign_args(&config, None),
            os(&["sign", "/fd", "sha256", "/sha1", "abc123"])
        );
    }

    #[test]
    fn signtool_uses_the_file_flag_for_a_certificate_file() {
        let config = SignConfig {
            certificate: file_cert("/certs/codesign.pfx"),
            ..Default::default()
        };

        let args = signtool_sign_args(&config, None);

        assert_eq!(
            args,
            os(&["sign", "/fd", "sha256", "/f", "/certs/codesign.pfx"])
        );
        assert!(
            !args.contains(&OsString::from("/sha1")),
            "/sha1 selects a store certificate by thumbprint; a file needs /f"
        );
    }

    #[test]
    fn signtool_passes_the_password_when_one_is_resolved() {
        let config = SignConfig {
            certificate: file_cert("/certs/codesign.pfx"),
            ..Default::default()
        };

        assert_eq!(
            signtool_sign_args(&config, Some("hunter2")),
            os(&[
                "sign",
                "/fd",
                "sha256",
                "/f",
                "/certs/codesign.pfx",
                "/p",
                "hunter2"
            ])
        );
    }

    #[test]
    fn signtool_omits_the_password_flag_when_there_is_none() {
        let config = SignConfig {
            certificate: file_cert("/certs/codesign.pfx"),
            ..Default::default()
        };

        assert!(!signtool_sign_args(&config, None).contains(&OsString::from("/p")));
    }

    #[test]
    fn signtool_args_pair_rfc3161_directives() {
        let config = SignConfig {
            certificate: Some(CertificateSource::Thumbprint("abc123".into())),
            timestamp_url: Some("http://ts.example".to_string()),
            digest_algorithm: "sha1".to_string(),
            ..Default::default()
        };

        assert_eq!(
            signtool_sign_args(&config, None),
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
            certificate: file_cert("/certs/cert.pfx"),
            timestamp_url: Some("http://ts.example".to_string()),
            ..Default::default()
        };

        assert_eq!(
            osslsigncode_sign_args(
                &config,
                None,
                Path::new("/dist/app.exe"),
                Path::new("/dist/app.exe.kurogane-sign-tmp"),
            )
            .unwrap(),
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
            certificate: file_cert("/certs/chain.pem"),
            ..Default::default()
        };

        let args = osslsigncode_sign_args(&config, None, Path::new("/a.exe"), Path::new("/b.tmp"))
            .unwrap();

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
            osslsigncode_cert_args(&file_cert("/certs/store.P12").unwrap(), None).unwrap(),
            os(&["-pkcs12", "/certs/store.P12"])
        );
    }

    #[test]
    fn osslsigncode_passes_the_password_for_pkcs12() {
        assert_eq!(
            osslsigncode_cert_args(&file_cert("/certs/store.pfx").unwrap(), Some("hunter2"))
                .unwrap(),
            os(&["-pkcs12", "/certs/store.pfx", "-pass", "hunter2"])
        );
    }

    #[test]
    fn osslsigncode_rejects_a_store_thumbprint() {
        let err = osslsigncode_cert_args(&CertificateSource::Thumbprint("ABCD".into()), None)
            .unwrap_err();

        assert!(
            matches!(err, SigningError::ThumbprintUnsupported),
            "osslsigncode has no certificate store, got: {err}"
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
            ..Default::default()
        };

        let config = SignConfig::from_file_config(&file).unwrap().unwrap();

        assert_eq!(config.certificate, file_cert("/certs/codesign.pfx"));
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

        let config = SignConfig::from_file_config(&file).unwrap().unwrap();

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
        assert!(
            SignConfig::from_file_config(&SigningFileConfig::default())
                .unwrap()
                .is_none()
        );

        let file = SigningFileConfig {
            certificate: Some("/c.pfx".into()),
            ..Default::default()
        };
        let config = SignConfig::from_file_config(&file).unwrap().unwrap();
        assert_eq!(config.digest_algorithm, "sha256");
    }

    #[test]
    fn from_file_config_carries_the_password_variable_name_not_the_secret() {
        let file = SigningFileConfig {
            certificate: Some("/c.pfx".into()),
            certificate_password_env: Some("KUROGANE_CERT_PASSWORD".into()),
            ..Default::default()
        };

        let config = SignConfig::from_file_config(&file).unwrap().unwrap();

        assert_eq!(
            config.certificate,
            Some(CertificateSource::File {
                path: PathBuf::from("/c.pfx"),
                password_env: Some("KUROGANE_CERT_PASSWORD".into()),
            })
        );
        assert!(
            !format!("{config:?}").contains("hunter2"),
            "only the variable name is stored, never the secret"
        );
    }

    #[test]
    fn from_file_config_rejects_both_certificate_forms() {
        let file = SigningFileConfig {
            certificate: Some("/c.pfx".into()),
            certificate_thumbprint: Some("ABCD".into()),
            ..Default::default()
        };

        let err = SignConfig::from_file_config(&file).unwrap_err();

        assert!(
            matches!(err, SigningError::AmbiguousCertificate),
            "a file and a thumbprint are different signing identities, got: {err}"
        );
    }

    #[test]
    fn from_file_config_maps_a_thumbprint() {
        let file = SigningFileConfig {
            certificate_thumbprint: Some("ABCD1234".into()),
            ..Default::default()
        };

        let config = SignConfig::from_file_config(&file).unwrap().unwrap();

        assert_eq!(
            config.certificate,
            Some(CertificateSource::Thumbprint("ABCD1234".into()))
        );
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
