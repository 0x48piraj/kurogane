//! Code signing operations for packaged artifacts.
//!
//! `[signing]` has a table per platform that signs. `--sign` checks both
//! wherever it runs ([`SigningFileConfig::check`]), then resolves this
//! platform's into a `SignConfig`: a custom command or a certificate for the
//! PE files of a Windows bundle, signed through signtool or osslsigncode, or
//! a codesign identity for a macOS `.app`, signed inside-out. A Linux bundle
//! has nothing Kurogane signs, so Linux compiles the check alone.

#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::ffi::OsString;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::fs;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::process::Command;
use thiserror::Error;

use crate::config::{MacosSigningConfig, SigningFileConfig, WindowsSigningConfig};
#[cfg(target_os = "macos")]
use crate::platform::MACOS_FRAMEWORK;

/// What a custom command's arguments write for the file to sign.
const TARGET: &str = "%1";

impl SigningFileConfig {
    /// Checks every table, whichever platform reads it, so a table that
    /// would fail where it is used fails wherever `--sign` runs.
    pub fn check(&self) -> Result<(), SigningError> {
        self.windows.check()?;
        self.macos.check()
    }
}

impl WindowsSigningConfig {
    fn check(&self) -> Result<(), SigningError> {
        if self.certificate.is_some() && self.certificate_thumbprint.is_some() {
            return Err(SigningError::AmbiguousCertificate);
        }
        if let Some(command) = &self.custom_command {
            // A custom command signs on its own: certificate options beside
            // it would be ignored
            let certificate_options = self.certificate.is_some()
                || self.certificate_thumbprint.is_some()
                || self.certificate_password_env.is_some()
                || self.timestamp_url.is_some()
                || self.digest_algorithm.is_some();
            if certificate_options {
                return Err(SigningError::CommandAndCertificate);
            }
            if !command.iter().skip(1).any(|arg| arg.contains(TARGET)) {
                return Err(SigningError::CommandWithoutTarget);
            }
        }
        if self.certificate_password_env.is_some() && self.certificate.is_none() {
            return Err(SigningError::PasswordWithoutFile);
        }
        Ok(())
    }
}

impl MacosSigningConfig {
    fn check(&self) -> Result<(), SigningError> {
        match &self.identity {
            Some(identity) if identity.trim().is_empty() => Err(SigningError::EmptyIdentity),
            _ => Ok(()),
        }
    }
}

/// How a Windows bundle's PE files are signed: by a command of the
/// application's own, or with a certificate through signtool or
/// osslsigncode.
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignConfig {
    /// A signing program and its arguments, run once per file. Every `%1`
    /// in an argument is replaced by the path of the file to sign.
    Command(Vec<String>),

    /// A certificate Kurogane signs with.
    Certificate(CertificateConfig),
}

/// How a macOS `.app` is signed: with a codesign identity from the keychain.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignConfig {
    /// The identity codesign signs with; [`AD_HOC_IDENTITY`] signs ad hoc.
    pub identity: String,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl SignConfig {
    /// Checks every `[signing]` table, then resolves this platform's.
    /// `Ok(None)` when it configures no signing.
    #[cfg(target_os = "windows")]
    pub fn from_file_config(file: &SigningFileConfig) -> Result<Option<SignConfig>, SigningError> {
        file.check()?;
        let table = &file.windows;
        if let Some(command) = &table.custom_command {
            return Ok(Some(SignConfig::Command(command.clone())));
        }
        let source = match (&table.certificate, &table.certificate_thumbprint) {
            (Some(path), _) => CertificateSource::File {
                path: path.clone(),
                password_env: table.certificate_password_env.clone(),
            },
            (None, Some(thumbprint)) => CertificateSource::Thumbprint(thumbprint.clone()),
            (None, None) => return Ok(None),
        };
        Ok(Some(SignConfig::Certificate(CertificateConfig {
            source,
            timestamp_url: table.timestamp_url.clone(),
            digest: table
                .digest_algorithm
                .clone()
                .unwrap_or_else(|| DEFAULT_DIGEST.to_string()),
        })))
    }

    /// Checks every `[signing]` table, then resolves this platform's.
    /// `Ok(None)` when it configures no signing.
    #[cfg(target_os = "macos")]
    pub fn from_file_config(file: &SigningFileConfig) -> Result<Option<SignConfig>, SigningError> {
        file.check()?;
        Ok(file
            .macos
            .identity
            .clone()
            .map(|identity| SignConfig { identity }))
    }
}

/// Signing with a certificate: where it comes from, and how signtool or
/// osslsigncode stamp the signature.
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateConfig {
    pub source: CertificateSource,

    /// RFC-3161 timestamp authority URL.
    pub timestamp_url: Option<String>,

    /// Signing digest algorithm.
    pub digest: String,
}

/// Where a Windows signing certificate comes from.
#[cfg(target_os = "windows")]
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

/// The digest a certificate signs with unless `digest-algorithm` names one.
#[cfg(target_os = "windows")]
const DEFAULT_DIGEST: &str = "sha256";

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SigningError {
    #[error(
        "[signing.windows] sets both `certificate` and `certificate-thumbprint`; choose one \
         (a certificate file, or a certificate in the Windows certificate store)"
    )]
    AmbiguousCertificate,

    #[error(
        "[signing.windows] sets `custom-command` and certificate options (`certificate`, \
         `certificate-thumbprint`, `certificate-password-env`, `timestamp-url`, \
         `digest-algorithm`); a custom command signs on its own, so the options would be \
         ignored: keep one or the other"
    )]
    CommandAndCertificate,

    #[error(
        "[signing.windows] `custom-command` must name a program and pass `%1`, the file to \
         sign, in one of its arguments, e.g. [\"signer\", \"sign\", \"%1\"]"
    )]
    CommandWithoutTarget,

    #[error("[signing.windows] `certificate-password-env` applies only to a `certificate` file")]
    PasswordWithoutFile,

    #[error(
        "[signing.macos] `identity` is empty; name a keychain identity such as \
         \"Developer ID Application: Name (TEAMID)\", or `-` to sign ad hoc"
    )]
    EmptyIdentity,

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[cfg_attr(
        target_os = "windows",
        error("no signing tool found; install signtool.exe (Windows SDK) or osslsigncode")
    )]
    #[cfg_attr(
        target_os = "macos",
        error("codesign not found; install the Xcode Command Line Tools")
    )]
    NoSigningTool,

    #[cfg(target_os = "windows")]
    #[error(
        "certificate password environment variable `{env}` is not set; \
         export it or remove `certificate-password-env` from [signing.windows]"
    )]
    MissingCertificatePassword { env: String },

    #[cfg(target_os = "windows")]
    #[error(
        "osslsigncode cannot use a Windows certificate store thumbprint; \
         set `certificate` to a PKCS#12 or PEM file instead"
    )]
    ThumbprintUnsupported,

    #[cfg(target_os = "windows")]
    #[error("custom sign command `{command}` failed ({status})")]
    CustomCommandFailed {
        command: String,
        status: std::process::ExitStatus,
    },

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[error("{tool} failed ({status})")]
    ToolFailed {
        tool: String,
        status: std::process::ExitStatus,
    },

    #[cfg(target_os = "windows")]
    #[error("signed output was not produced for {}", .0.display())]
    MissingSignedOutput(PathBuf),

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Resolves the certificate password from its configured environment variable.
///
/// Returns `Ok(None)` when no password is needed: a store thumbprint, or a
/// file with no password variable.
#[cfg(target_os = "windows")]
fn resolve_password(certificate: &CertificateSource) -> Result<Option<String>, SigningError> {
    let CertificateSource::File {
        password_env: Some(name),
        ..
    } = certificate
    else {
        return Ok(None);
    };

    std::env::var(name)
        .map(Some)
        .map_err(|_| SigningError::MissingCertificatePassword { env: name.clone() })
}

/// Builds `signtool sign` arguments, excluding the tool path and target file.
#[cfg(target_os = "windows")]
pub fn signtool_sign_args(config: &CertificateConfig, password: Option<&str>) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("sign"),
        OsString::from("/fd"),
        OsString::from(&config.digest),
    ];

    match &config.source {
        // Certificate files use `/f`; store certificates use `/sha1`
        CertificateSource::File { path, .. } => {
            args.push(OsString::from("/f"));
            args.push(path.into());

            if let Some(password) = password {
                args.push(OsString::from("/p"));
                args.push(OsString::from(password));
            }
        }
        CertificateSource::Thumbprint(thumbprint) => {
            args.push(OsString::from("/sha1"));
            args.push(OsString::from(thumbprint));
        }
    }

    if let Some(url) = &config.timestamp_url {
        args.push(OsString::from("/tr"));
        args.push(OsString::from(url));
        args.push(OsString::from("/td"));
        args.push(OsString::from(&config.digest));
    }

    args
}

/// Builds certificate arguments for `osslsigncode`.
///
/// Uses `-pkcs12` for PKCS#12 certificates and `-certs` for PEM/DER certificates.
#[cfg(target_os = "windows")]
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
#[cfg(target_os = "windows")]
pub fn osslsigncode_sign_args(
    config: &CertificateConfig,
    password: Option<&str>,
    input: &Path,
    output: &Path,
) -> Result<Vec<OsString>, SigningError> {
    let mut args = vec![OsString::from("sign")];

    args.extend(osslsigncode_cert_args(&config.source, password)?);

    if let Some(url) = &config.timestamp_url {
        args.push(OsString::from("-ts"));
        args.push(OsString::from(url));
    }

    args.push(OsString::from("-h"));
    args.push(OsString::from(&config.digest));
    args.push(OsString::from("-in"));
    args.push(OsString::from(input));
    args.push(OsString::from("-out"));
    args.push(OsString::from(output));

    Ok(args)
}

/// Builds arguments for `signtool verify` using the default Authenticode
/// policy and checking all signatures.
#[cfg(target_os = "windows")]
pub fn signtool_verify_args(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("verify"),
        OsString::from("/pa"),
        OsString::from("/all"),
        OsString::from(path),
    ]
}

/// Builds arguments for `osslsigncode verify`.
#[cfg(target_os = "windows")]
pub fn osslsigncode_verify_args(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("verify"),
        OsString::from("-in"),
        OsString::from(path),
    ]
}

/// A custom command's arguments for one file: each `%1` becomes its path.
#[cfg(target_os = "windows")]
fn expand_custom_args(args: &[String], path: &Path) -> Vec<OsString> {
    let path = path.as_os_str();
    args.iter()
        .map(|arg| {
            let mut pieces = arg.split(TARGET);
            let mut expanded = OsString::from(pieces.next().unwrap_or_default());
            for piece in pieces {
                expanded.push(path);
                expanded.push(piece);
            }
            expanded
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn run_custom(path: &Path, command: &[String]) -> Result<(), SigningError> {
    let Some((program, args)) = command.split_first() else {
        return Err(SigningError::CommandWithoutTarget);
    };

    let status = Command::new(program)
        .args(expand_custom_args(args, path))
        .status()?;

    if !status.success() {
        return Err(SigningError::CustomCommandFailed {
            command: program.clone(),
            status,
        });
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn find_signtool() -> Option<PathBuf> {
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

#[cfg(target_os = "windows")]
fn find_osslsigncode() -> Option<PathBuf> {
    // Check override path
    if let Ok(path) = std::env::var("KUROGANE_OSSLSIGNCODE_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Try to find osslsigncode in PATH
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

    None
}

#[cfg(target_os = "macos")]
fn find_codesign() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("KUROGANE_CODESIGN_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(output) = Command::new("which").arg("codesign").output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Some(PathBuf::from(path));
    }

    None
}

/// The ad-hoc signing identity, which produces a signature bound to no
/// certificate. Useful locally; never valid for distribution.
#[cfg(target_os = "macos")]
pub const AD_HOC_IDENTITY: &str = "-";

/// Builds `codesign --sign` arguments for a single target (binary or bundle).
///
/// Never includes `--deep`: Apple deprecated it for signing because it applies
/// one set of options to every nested item. [`sign_app_bundle`] signs
/// inside-out instead.
#[cfg(target_os = "macos")]
pub fn codesign_sign_args(identity: &str, entitlements: Option<&Path>) -> Vec<OsString> {
    let mut args = vec![OsString::from("--sign"), OsString::from(identity)];

    if identity == AD_HOC_IDENTITY {
        args.push(OsString::from("--timestamp=none"));
    } else {
        args.push(OsString::from("--timestamp"));
        args.push(OsString::from("--options"));
        args.push(OsString::from("runtime"));
    }

    args.push(OsString::from("--force"));

    if let Some(entitlements) = entitlements {
        args.push(OsString::from("--entitlements"));
        args.push(OsString::from(entitlements));
    }

    args
}

/// Builds `codesign --verify` arguments for a signed target.
#[cfg(target_os = "macos")]
pub fn codesign_verify_args(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--verify"),
        OsString::from("--deep"),
        OsString::from("--strict"),
        OsString::from(path),
    ]
}

/// Signs a `.app` bundle inside-out.
#[cfg(target_os = "macos")]
pub fn sign_app_bundle(
    app_dir: &Path,
    config: &SignConfig,
    entitlements: Option<&Path>,
) -> Result<(), SigningError> {
    let Some(codesign) = find_codesign() else {
        return Err(SigningError::NoSigningTool);
    };
    let identity = config.identity.as_str();

    let run = |args: Vec<OsString>, tool: &str| -> Result<(), SigningError> {
        let status = Command::new(&codesign).args(&args).status()?;

        if status.success() {
            Ok(())
        } else {
            Err(SigningError::ToolFailed {
                tool: tool.to_string(),
                status,
            })
        }
    };

    let frameworks = app_dir.join("Contents").join("Frameworks");

    // Innermost first
    if frameworks.is_dir() {
        let mut helpers: Vec<PathBuf> = fs::read_dir(&frameworks)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
            .collect();

        // read_dir order is unspecified; sign in a stable order
        helpers.sort();

        for helper in helpers {
            let mut args = codesign_sign_args(identity, entitlements);
            args.push(OsString::from(&helper));
            run(args, "codesign (helper)")?;
        }
    }

    // Sign the framework after its nested helpers
    let framework = frameworks.join(MACOS_FRAMEWORK);

    if framework.exists() {
        let mut args = codesign_sign_args(identity, None);
        args.push(OsString::from(&framework));
        run(args, "codesign (framework)")?;
    }

    // Then the app itself, which is what the entitlements apply to
    let mut args = codesign_sign_args(identity, entitlements);
    args.push(OsString::from(app_dir));
    run(args, "codesign")?;

    run(codesign_verify_args(app_dir), "codesign verify")
}

/// Signs a PE file with the configured method.
#[cfg(target_os = "windows")]
pub fn sign_file(path: &Path, config: &SignConfig) -> Result<(), SigningError> {
    let certificate = match config {
        SignConfig::Command(command) => return run_custom(path, command),
        SignConfig::Certificate(certificate) => certificate,
    };

    if let Some(signtool) = find_signtool() {
        return sign_with_signtool(path, &signtool, certificate);
    }

    if let Some(osslsigncode) = find_osslsigncode() {
        return sign_with_osslsigncode(path, &osslsigncode, certificate);
    }

    Err(SigningError::NoSigningTool)
}

#[cfg(target_os = "windows")]
fn sign_with_signtool(
    path: &Path,
    signtool: &Path,
    config: &CertificateConfig,
) -> Result<(), SigningError> {
    let password = resolve_password(&config.source)?;

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

#[cfg(target_os = "windows")]
fn sign_with_osslsigncode(
    path: &Path,
    osslsigncode: &Path,
    config: &CertificateConfig,
) -> Result<(), SigningError> {
    // Preserve the original until signing succeeds.
    let mut output = path.as_os_str().to_os_string();
    output.push(".kurogane-sign-tmp");
    let output = PathBuf::from(output);

    let password = resolve_password(&config.source)?;
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

/// Returns whether a bundle entry is a signable PE artifact.
#[cfg(target_os = "windows")]
pub fn should_sign(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "exe" || ext == "dll")
}

/// Signs signable artifacts within a bundle.
#[cfg(target_os = "windows")]
pub fn sign_tree(root: &Path, config: &SignConfig) -> Result<usize, SigningError> {
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

/// Verifies every signable artifact within a bundle.
///
/// The counterpart to [`sign_tree`]. Returns the number of verified artifacts.
#[cfg(target_os = "windows")]
pub fn verify_tree(root: &Path) -> Result<usize, SigningError> {
    let mut verified = 0;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                stack.push(path);
            } else if should_sign(&path) {
                verify_signature(&path)?;
                verified += 1;
            }
        }
    }

    Ok(verified)
}

/// Verifies a signed PE file, however it was signed.
#[cfg(target_os = "windows")]
pub fn verify_signature(path: &Path) -> Result<(), SigningError> {
    if let Some(signtool) = find_signtool() {
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

    fn strings(input: &[&str]) -> Vec<String> {
        input.iter().map(|s| s.to_string()).collect()
    }

    fn check(file: SigningFileConfig) -> Result<(), SigningError> {
        file.check()
    }

    fn windows(table: WindowsSigningConfig) -> SigningFileConfig {
        SigningFileConfig {
            windows: table,
            ..Default::default()
        }
    }

    // --- every table is checked wherever --sign runs ----------------------

    #[test]
    fn an_empty_signing_section_checks() {
        assert!(check(SigningFileConfig::default()).is_ok());
    }

    #[test]
    fn a_certificate_file_and_a_thumbprint_are_refused_together() {
        let err = check(windows(WindowsSigningConfig {
            certificate: Some("/c.pfx".into()),
            certificate_thumbprint: Some("ABCD".into()),
            ..Default::default()
        }))
        .unwrap_err();

        assert!(
            matches!(err, SigningError::AmbiguousCertificate),
            "a file and a thumbprint are different signing identities, got: {err}"
        );
    }

    #[test]
    fn a_custom_command_beside_a_certificate_option_is_refused() {
        let command = Some(strings(&["signer", "%1"]));
        for table in [
            WindowsSigningConfig {
                certificate: Some("/c.pfx".into()),
                ..Default::default()
            },
            WindowsSigningConfig {
                certificate_thumbprint: Some("ABCD".into()),
                ..Default::default()
            },
            WindowsSigningConfig {
                certificate_password_env: Some("PASSWORD".into()),
                ..Default::default()
            },
            WindowsSigningConfig {
                timestamp_url: Some("http://ts.example".into()),
                ..Default::default()
            },
            WindowsSigningConfig {
                digest_algorithm: Some("sha512".into()),
                ..Default::default()
            },
        ] {
            let table = WindowsSigningConfig {
                custom_command: command.clone(),
                ..table
            };
            let err = check(windows(table.clone())).unwrap_err();
            assert!(
                matches!(err, SigningError::CommandAndCertificate),
                "the command would sign and the option be ignored: {table:?}: {err}"
            );
        }
    }

    #[test]
    fn a_custom_command_must_name_the_file_to_sign() {
        for command in [
            &[][..],
            &["signer"][..],
            &["%1"][..],
            &["signer", "--all"][..],
        ] {
            let err = check(windows(WindowsSigningConfig {
                custom_command: Some(strings(command)),
                ..Default::default()
            }))
            .unwrap_err();
            assert!(
                matches!(err, SigningError::CommandWithoutTarget),
                "{command:?}: {err}"
            );
        }
        let placed = check(windows(WindowsSigningConfig {
            custom_command: Some(strings(&[r"C:\Program Files\Signer\sign.exe", "--file=%1"])),
            ..Default::default()
        }));
        assert!(placed.is_ok(), "%1 may sit inside an argument: {placed:?}");
    }

    #[test]
    fn a_password_variable_needs_a_certificate_file() {
        for table in [
            WindowsSigningConfig::default(),
            WindowsSigningConfig {
                certificate_thumbprint: Some("ABCD".into()),
                ..Default::default()
            },
        ] {
            let table = WindowsSigningConfig {
                certificate_password_env: Some("PASSWORD".into()),
                ..table
            };
            let err = check(windows(table)).unwrap_err();
            assert!(
                matches!(err, SigningError::PasswordWithoutFile),
                "got: {err}"
            );
        }
    }

    #[test]
    fn an_empty_identity_is_refused() {
        let err = check(SigningFileConfig {
            macos: MacosSigningConfig {
                identity: Some(" ".into()),
            },
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, SigningError::EmptyIdentity), "got: {err}");
    }

    #[test]
    fn a_table_is_checked_on_every_platform() {
        // Each platform's resolution checks the other's table too
        let broken_macos = SigningFileConfig {
            macos: MacosSigningConfig {
                identity: Some(String::new()),
            },
            ..Default::default()
        };
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        assert!(SignConfig::from_file_config(&broken_macos).is_err());
        assert!(check(broken_macos).is_err());
    }

    // --- Windows: PE files ---------------------------------------------------

    #[cfg(target_os = "windows")]
    mod windows_signing {
        use super::*;

        fn tmp() -> tempfile::TempDir {
            crate::test_fixtures::tmp_dir()
        }

        fn os(input: &[&str]) -> Vec<OsString> {
            input.iter().map(OsString::from).collect()
        }

        fn certificate(source: CertificateSource) -> CertificateConfig {
            CertificateConfig {
                source,
                timestamp_url: None,
                digest: DEFAULT_DIGEST.to_string(),
            }
        }

        fn file_cert(path: &str) -> CertificateSource {
            CertificateSource::File {
                path: PathBuf::from(path),
                password_env: None,
            }
        }

        #[test]
        fn a_certificate_file_resolves_with_its_options() {
            let config = SignConfig::from_file_config(&windows(WindowsSigningConfig {
                certificate: Some("/certs/codesign.pfx".into()),
                certificate_password_env: Some("KUROGANE_CERT_PASSWORD".into()),
                timestamp_url: Some("http://ts.example".into()),
                digest_algorithm: Some("sha512".into()),
                ..Default::default()
            }));

            assert_eq!(
                config.unwrap(),
                Some(SignConfig::Certificate(CertificateConfig {
                    source: CertificateSource::File {
                        path: "/certs/codesign.pfx".into(),
                        password_env: Some("KUROGANE_CERT_PASSWORD".into()),
                    },
                    timestamp_url: Some("http://ts.example".into()),
                    digest: "sha512".into(),
                }))
            );
        }

        #[test]
        fn a_thumbprint_resolves_with_the_default_digest() {
            let config = SignConfig::from_file_config(&windows(WindowsSigningConfig {
                certificate_thumbprint: Some("ABCD1234".into()),
                ..Default::default()
            }));

            assert_eq!(
                config.unwrap(),
                Some(SignConfig::Certificate(certificate(
                    CertificateSource::Thumbprint("ABCD1234".into())
                )))
            );
        }

        #[test]
        fn a_custom_command_resolves_as_its_argument_list() {
            let command = strings(&[r"C:\Program Files\Signer\sign.exe", "sign", "--file=%1"]);
            let config = SignConfig::from_file_config(&windows(WindowsSigningConfig {
                custom_command: Some(command.clone()),
                ..Default::default()
            }));

            assert_eq!(
                config.unwrap(),
                Some(SignConfig::Command(command)),
                "a path with a space is one argument"
            );
        }

        #[test]
        fn nothing_configured_resolves_to_none() {
            assert_eq!(
                SignConfig::from_file_config(&SigningFileConfig::default()).unwrap(),
                None
            );
            let timestamp_only = windows(WindowsSigningConfig {
                timestamp_url: Some("http://timestamp".into()),
                ..Default::default()
            });
            assert_eq!(SignConfig::from_file_config(&timestamp_only).unwrap(), None);
        }

        #[test]
        fn only_pe_files_are_signed() {
            assert!(should_sign(Path::new("/some/path/app.exe")));
            assert!(should_sign(Path::new("/some/path/lib.dll")));
            assert!(!should_sign(Path::new("/some/readme.txt")));
            assert!(!should_sign(Path::new("/some/binary")));
        }

        #[test]
        fn sign_custom_command_expands_target_path() {
            let dir = tmp();
            let target = dir.path().join("app.exe");
            fs::write(&target, "").unwrap();

            let config = SignConfig::Command(strings(&["echo", "%1", "--flag"]));

            assert!(sign_file(&target, &config).is_ok());
        }

        #[test]
        fn sign_custom_command_failure_is_propagated() {
            let dir = tmp();
            let target = dir.path().join("app.exe");
            fs::write(&target, "").unwrap();

            let config = SignConfig::Command(strings(&["false", "%1"]));

            let err = sign_file(&target, &config).unwrap_err();
            assert!(
                matches!(err, SigningError::CustomCommandFailed { .. }),
                "got: {err}"
            );
        }

        #[test]
        fn expand_custom_args_replaces_every_placeholder() {
            let expanded = expand_custom_args(
                &strings(&["%1", "--flag", "/file:%1", "%1,%1", "plain"]),
                Path::new("/bin/app.exe"),
            );

            assert_eq!(
                expanded,
                os(&[
                    "/bin/app.exe",
                    "--flag",
                    "/file:/bin/app.exe",
                    "/bin/app.exe,/bin/app.exe",
                    "plain"
                ])
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

            let config = SignConfig::Command(strings(&["true", "%1"]));
            let count = sign_tree(dir.path(), &config).unwrap();
            assert_eq!(count, 2, "only app.exe and runtime/lib.dll are signed");
        }

        #[test]
        fn signtool_args_default_to_sha256_without_timestamp() {
            let config = certificate(CertificateSource::Thumbprint("abc123".into()));

            assert_eq!(
                signtool_sign_args(&config, None),
                os(&["sign", "/fd", "sha256", "/sha1", "abc123"])
            );
        }

        #[test]
        fn signtool_uses_the_file_flag_for_a_certificate_file() {
            let config = certificate(file_cert("/certs/codesign.pfx"));

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
            let config = certificate(file_cert("/certs/codesign.pfx"));

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
            let config = certificate(file_cert("/certs/codesign.pfx"));

            assert!(!signtool_sign_args(&config, None).contains(&OsString::from("/p")));
        }

        #[test]
        fn signtool_args_pair_rfc3161_directives() {
            let config = CertificateConfig {
                timestamp_url: Some("http://ts.example".to_string()),
                digest: "sha1".to_string(),
                ..certificate(CertificateSource::Thumbprint("abc123".into()))
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
            let config = CertificateConfig {
                timestamp_url: Some("http://ts.example".to_string()),
                ..certificate(file_cert("/certs/cert.pfx"))
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
            let config = certificate(file_cert("/certs/chain.pem"));

            let args =
                osslsigncode_sign_args(&config, None, Path::new("/a.exe"), Path::new("/b.tmp"))
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
                osslsigncode_cert_args(&file_cert("/certs/store.P12"), None).unwrap(),
                os(&["-pkcs12", "/certs/store.P12"])
            );
        }

        #[test]
        fn osslsigncode_passes_the_password_for_pkcs12() {
            assert_eq!(
                osslsigncode_cert_args(&file_cert("/certs/store.pfx"), Some("hunter2")).unwrap(),
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
    }

    // --- macOS: a .app -------------------------------------------------------

    #[cfg(target_os = "macos")]
    mod macos_signing {
        use super::*;

        const DEVELOPER_ID: &str = "Developer ID Application: Acme (TEAMID1234)";

        fn os(input: &[&str]) -> Vec<OsString> {
            input.iter().map(OsString::from).collect()
        }

        #[test]
        fn an_identity_resolves() {
            let config = SignConfig::from_file_config(&SigningFileConfig {
                macos: MacosSigningConfig {
                    identity: Some(DEVELOPER_ID.into()),
                },
                ..Default::default()
            });
            assert_eq!(
                config.unwrap(),
                Some(SignConfig {
                    identity: DEVELOPER_ID.into()
                })
            );
            assert_eq!(
                SignConfig::from_file_config(&SigningFileConfig::default()).unwrap(),
                None
            );
        }

        #[test]
        fn codesign_sign_args_include_identity_timestamp_and_options() {
            let args = codesign_sign_args(DEVELOPER_ID, None);

            assert!(args.contains(&OsString::from("--sign")));
            assert!(args.contains(&OsString::from(DEVELOPER_ID)));
            assert!(args.contains(&OsString::from("--timestamp")));
            assert!(args.contains(&OsString::from("--options")));
            assert!(args.contains(&OsString::from("runtime")));
            assert!(args.contains(&OsString::from("--force")));
        }

        #[test]
        fn codesign_sign_args_omit_entitlements_when_none_supplied() {
            let args = codesign_sign_args(DEVELOPER_ID, None);
            assert!(!args.contains(&OsString::from("--entitlements")));
        }

        #[test]
        fn codesign_sign_args_attach_entitlements_when_supplied() {
            let args = codesign_sign_args(DEVELOPER_ID, Some(Path::new("/tmp/ent.plist")));
            assert!(args.contains(&OsString::from("--entitlements")));
            assert!(args.contains(&OsString::from("/tmp/ent.plist")));
        }

        #[test]
        fn ad_hoc_signing_skips_timestamp_and_hardened_runtime() {
            let args = codesign_sign_args(AD_HOC_IDENTITY, None);

            // The timestamp authority rejects certificate-less signatures
            // Requesting one would fail the whole sign
            assert!(args.contains(&OsString::from("--timestamp=none")));
            assert!(!args.contains(&OsString::from("--timestamp")));
            assert!(
                !args.contains(&OsString::from("runtime")),
                "a hardened runtime on a dev signature reads as distributable"
            );
        }

        #[test]
        fn sign_args_never_request_deep_signing() {
            // Apple deprecated --deep for signing; nested code is signed first
            assert!(!codesign_sign_args(DEVELOPER_ID, None).contains(&OsString::from("--deep")));
        }

        #[test]
        fn codesign_verify_args_are_conservative() {
            assert_eq!(
                codesign_verify_args(Path::new("/MyApp.app")),
                os(&["--verify", "--deep", "--strict", "/MyApp.app"])
            );
        }
    }
}
