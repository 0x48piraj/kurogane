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
    if let Some(path) = override_path {
        if path.exists() {
            return Some(path.to_path_buf());
        }
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
