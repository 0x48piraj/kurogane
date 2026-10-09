//! Reads the installer's receipt and identifies installer-managed installations.
//!
//! The receipt is the contract between the installer and `kurogane self
//! uninstall`. It records what the installer owns. An installation is
//! installer-managed only when its receipt names the running binary.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Receipt format understood by this version of Kurogane.
const SCHEMA: u32 = 1;

/// Receipt file name within the installer's home.
const FILE_NAME: &str = "receipt.json";

/// What the installer recorded for an installation.
#[derive(Debug, Deserialize)]
pub(crate) struct Receipt {
    /// Installed CLI version.
    pub(crate) version: String,

    /// Installed executable.
    pub(crate) binary: PathBuf,

    /// Unix PATH and shell setup.
    #[cfg(any(unix, test))]
    pub(crate) env: Option<PathBuf>,

    #[cfg(any(unix, test))]
    #[serde(default)]
    pub(crate) startup_files: Vec<PathBuf>,

    #[cfg(any(unix, test))]
    pub(crate) fish: Option<PathBuf>,

    /// Windows user PATH entry.
    #[cfg(any(windows, test))]
    pub(crate) user_path: Option<PathBuf>,
}

/// An installer-managed installation.
#[derive(Debug)]
pub(crate) struct Installed {
    pub(crate) receipt: Receipt,
    /// Directory containing the receipt.
    pub(crate) home: PathBuf,
}

impl Installed {
    pub(crate) fn receipt_path(&self) -> PathBuf {
        self.home.join(FILE_NAME)
    }
}

/// Finds the installer-managed installation for `exe`.
pub(crate) fn find(exe: &Path) -> Result<Option<Installed>> {
    find_in(&homes(exe), exe)
}

fn find_in(homes: &[PathBuf], exe: &Path) -> Result<Option<Installed>> {
    for home in homes {
        let path = home.join(FILE_NAME);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("cannot read {}", path.display()));
            }
        };

        let receipt = parse(&text).with_context(|| format!("cannot use {}", path.display()))?;

        if same_file(&receipt.binary, exe) {
            let home = fs::canonicalize(home).unwrap_or_else(|_| home.clone());
            return Ok(Some(Installed { receipt, home }));
        }
    }

    Ok(None)
}

fn parse(text: &str) -> Result<Receipt> {
    #[derive(Deserialize)]
    struct Header {
        schema: u32,
    }

    let header: Header = serde_json::from_str(text)?;

    if header.schema > SCHEMA {
        bail!(
            "it was written by a newer installer (receipt format {}, this kurogane reads {SCHEMA});\n  \
             install the latest Kurogane, then run `kurogane self uninstall` again",
            header.schema
        );
    }

    if header.schema != SCHEMA {
        bail!("unknown receipt format {}", header.schema);
    }

    Ok(serde_json::from_str(text)?)
}

/// Returns the installer's possible home directories for `exe` in
/// precedence order.
///
/// The installer keeps the binary in its home's `bin` folder unless told
/// otherwise, so a home chosen at install time is found from the binary.
fn homes(exe: &Path) -> Vec<PathBuf> {
    let mut homes = Vec::new();

    #[cfg(unix)]
    if let Some(home) = std::env::var_os("KUROGANE_HOME").filter(|v| !v.is_empty()) {
        homes.push(PathBuf::from(home));
    }

    if let Some(home) = exe
        .parent()
        .filter(|bin| bin.ends_with("bin"))
        .and_then(Path::parent)
    {
        homes.push(home.to_path_buf());
    }

    #[cfg(unix)]
    if let Some(home) = dirs::home_dir() {
        homes.push(home.join(".kurogane"));
    }

    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            homes.push(PathBuf::from(local).join("kurogane"));
        }
        if let Some(local) = dirs::data_local_dir() {
            homes.push(local.join("kurogane"));
        }
    }

    let mut unique: Vec<PathBuf> = Vec::new();
    for home in homes {
        if !unique.contains(&home) {
            unique.push(home);
        }
    }
    unique
}

/// Returns whether `a` and `b` name the same existing file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a receipt as `install.sh` writes it.
    fn unix_receipt(binary: &Path) -> String {
        format!(
            r#"{{
  "schema": 1,
  "version": "0.0.7",
  "binary": "{binary}",
  "env": "/home/me/.kurogane/env",
  "startup_files": [
    "/home/me/.profile",
    "/home/me/.zshenv"
  ],
  "fish": "/home/me/.config/fish/conf.d/kurogane.fish"
}}
"#,
            binary = json_path(binary)
        )
    }

    /// Returns `path` escaped for a JSON string.
    fn json_path(path: &Path) -> String {
        path.display().to_string().replace('\\', "\\\\")
    }

    fn installed_binary(dir: &Path) -> PathBuf {
        let binary = dir.join("bin").join("kurogane");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, "").unwrap();
        binary
    }

    #[test]
    fn reads_what_install_sh_writes() {
        let receipt = parse(&unix_receipt(Path::new("/home/me/.kurogane/bin/kurogane"))).unwrap();

        assert_eq!(receipt.version, "0.0.7");
        assert_eq!(receipt.binary, Path::new("/home/me/.kurogane/bin/kurogane"));
        assert_eq!(
            receipt.env.as_deref(),
            Some(Path::new("/home/me/.kurogane/env"))
        );
        assert_eq!(
            receipt.startup_files,
            [
                Path::new("/home/me/.profile"),
                Path::new("/home/me/.zshenv")
            ]
        );
        assert!(receipt.fish.is_some());
        assert_eq!(receipt.user_path, None);
    }

    #[test]
    fn reads_what_install_ps1_writes() {
        // Windows PowerShell 5.1's ConvertTo-Json layout
        let text = r#"{
    "schema":  1,
    "version":  "0.0.7",
    "binary":  "C:\\Users\\me\\AppData\\Local\\kurogane\\bin\\kurogane.exe",
    "user_path":  "C:\\Users\\me\\AppData\\Local\\kurogane\\bin"
}"#;
        let receipt = parse(text).unwrap();

        assert_eq!(
            receipt.user_path.as_deref(),
            Some(Path::new(r"C:\Users\me\AppData\Local\kurogane\bin"))
        );
        assert_eq!(receipt.env, None);
        assert!(receipt.startup_files.is_empty());
    }

    #[test]
    fn fields_a_newer_installer_adds_are_ignored() {
        let text = unix_receipt(Path::new("/k")).replacen("{", r#"{ "arch": "x86_64","#, 1);

        assert!(parse(&text).is_ok());
    }

    #[test]
    fn a_newer_receipt_format_is_refused() {
        let text = unix_receipt(Path::new("/k")).replace(r#""schema": 1"#, r#""schema": 2"#);

        let error = format!("{:#}", parse(&text).unwrap_err());
        assert!(error.contains("newer installer"), "{error}");
    }

    #[test]
    fn finds_the_receipt_that_names_the_running_binary() {
        let home = tempfile::tempdir().unwrap();
        let binary = installed_binary(home.path());
        fs::write(home.path().join(FILE_NAME), unix_receipt(&binary)).unwrap();

        let installed = find_in(&[home.path().to_path_buf()], &binary)
            .unwrap()
            .expect("the receipt names this binary");

        assert_eq!(installed.home, fs::canonicalize(home.path()).unwrap());
        assert_eq!(installed.receipt_path(), installed.home.join(FILE_NAME));
    }

    #[test]
    fn the_home_a_binary_was_installed_into_is_looked_at() {
        let home = tempfile::tempdir().unwrap();
        let binary = installed_binary(home.path());

        assert!(homes(&binary).contains(&home.path().to_path_buf()));
    }

    #[test]
    fn a_binary_outside_a_bin_folder_names_no_home() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("kurogane");

        assert!(!homes(&binary).contains(&dir.path().parent().unwrap().to_path_buf()));
    }

    #[test]
    fn a_receipt_for_another_binary_is_not_this_install() {
        let home = tempfile::tempdir().unwrap();
        let installed = installed_binary(home.path());
        fs::write(home.path().join(FILE_NAME), unix_receipt(&installed)).unwrap();
        let elsewhere = home.path().join("kurogane-copy");
        fs::write(&elsewhere, "").unwrap();

        assert!(
            find_in(&[home.path().to_path_buf()], &elsewhere)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn no_receipt_is_no_install() {
        let home = tempfile::tempdir().unwrap();
        let binary = installed_binary(home.path());

        assert!(
            find_in(&[home.path().to_path_buf()], &binary)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn an_unreadable_receipt_is_an_error_not_a_missing_one() {
        let home = tempfile::tempdir().unwrap();
        let binary = installed_binary(home.path());
        fs::write(home.path().join(FILE_NAME), "{ not json").unwrap();

        let error = find_in(&[home.path().to_path_buf()], &binary).unwrap_err();
        assert!(format!("{error:#}").contains(FILE_NAME), "{error:#}");
    }
}
