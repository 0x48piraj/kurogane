//! Removes Unix PATH setup recorded by the installer.
//!
//! Startup files are preserved; only the installer's source line is removed.
//! Installer-owned scripts are removed only when marked as installer-created.

use std::fs;
use std::io;
use std::path::Path;

use crate::receipt::Receipt;
use crate::tui;

/// Marker used by installer-owned scripts.
const MARKER: &str = "# Added by the Kurogane installer";

/// Result of checking an installer-owned script.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Removed,
    Absent,
    /// The file exists, but is not installer-owned.
    Foreign,
}

/// Removes the PATH setup recorded in `receipt`.
pub(super) fn remove(receipt: &Receipt, failed: &mut Vec<String>) {
    if let Some(env) = &receipt.env {
        let line = source_line(env);

        for file in &receipt.startup_files {
            match strip_line(file, &line) {
                Ok(true) => tui::field(&tui::format_path(file), "line removed"),
                Ok(false) => {}
                Err(e) => fail(file, &e, failed),
            }

            if mentions(file, &env.display().to_string()) {
                tui::warn(&format!(
                    "{} still mentions {}; remove that line yourself",
                    tui::format_path(file),
                    tui::format_path(env)
                ));
            }
        }

        remove_script(env, failed);
    }

    if let Some(fish) = &receipt.fish {
        remove_script(fish, failed);
    }
}

fn source_line(env: &Path) -> String {
    format!(". \"{}\"", env.display())
}

fn remove_script(file: &Path, failed: &mut Vec<String>) {
    match remove_marked(file) {
        Ok(Outcome::Removed) => tui::field(&tui::format_path(file), "removed"),
        Ok(Outcome::Absent) => {}
        Ok(Outcome::Foreign) => tui::field(
            &tui::format_path(file),
            "left in place: not written by the installer",
        ),
        Err(e) => fail(file, &e, failed),
    }
}

fn fail(file: &Path, error: &io::Error, failed: &mut Vec<String>) {
    tui::warn(&format!(
        "Failed to update {}: {error}",
        tui::format_path(file)
    ));
    failed.push(tui::format_path(file));
}

/// Removes exact `line` entries in place, preserving the file itself.
fn strip_line(file: &Path, line: &str) -> io::Result<bool> {
    let text = match fs::read(file) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };

    let kept = without_line(&text, line.as_bytes());
    if kept.len() == text.len() {
        return Ok(false);
    }

    // Write in place so symlinks and file metadata are preserved
    fs::write(file, kept)?;
    Ok(true)
}

fn without_line(text: &[u8], line: &[u8]) -> Vec<u8> {
    text.split_inclusive(|&b| b == b'\n')
        .filter(|l| {
            let l = l.strip_suffix(b"\n").unwrap_or(l);
            l.strip_suffix(b"\r").unwrap_or(l) != line
        })
        .flatten()
        .copied()
        .collect()
}

fn mentions(file: &Path, text: &str) -> bool {
    !text.is_empty()
        && fs::read(file).is_ok_and(|content| {
            content
                .windows(text.len())
                .any(|window| window == text.as_bytes())
        })
}

/// Removes a script only when it carries the installer marker.
fn remove_marked(file: &Path) -> io::Result<Outcome> {
    let text = match fs::read(file) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Outcome::Absent),
        Err(e) => return Err(e),
    };

    if !text.starts_with(MARKER.as_bytes()) {
        return Ok(Outcome::Foreign);
    }

    fs::remove_file(file)?;
    Ok(Outcome::Removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ENV_SCRIPT: &str = "# Added by the Kurogane installer: puts kurogane on PATH.\n\
                              case \":${PATH}:\" in\n    *) ;;\nesac\n";

    struct Home {
        dir: tempfile::TempDir,
    }

    impl Home {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.path(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }

        fn read(&self, name: &str) -> String {
            fs::read_to_string(self.path(name)).unwrap()
        }

        /// Returns the receipt `install.sh` writes for this home.
        fn receipt(&self) -> Receipt {
            Receipt {
                version: "0.0.7".into(),
                binary: self.path(".kurogane/bin/kurogane"),
                env: Some(self.path(".kurogane/env")),
                startup_files: [
                    ".profile",
                    ".bashrc",
                    ".bash_profile",
                    ".bash_login",
                    ".zshenv",
                ]
                .map(|name| self.path(name))
                .to_vec(),
                fish: Some(self.path(".config/fish/conf.d/kurogane.fish")),
                user_path: None,
            }
        }

        fn line(&self) -> String {
            source_line(&self.path(".kurogane/env"))
        }
    }

    #[test]
    fn removes_the_installers_setup_and_nothing_else() {
        let home = Home::new();
        let line = home.line();
        home.write(".kurogane/env", ENV_SCRIPT);
        home.write(".profile", &format!("{line}\n"));
        home.write(
            ".bashrc",
            &format!("export FOO=1\n{line}\nalias ll='ls -l'\n"),
        );
        home.write(
            ".config/fish/conf.d/kurogane.fish",
            &format!("{MARKER}: puts kurogane on PATH.\n"),
        );
        let mut failed = Vec::new();

        remove(&home.receipt(), &mut failed);

        assert!(failed.is_empty(), "{failed:?}");
        assert!(!home.path(".kurogane/env").exists());
        assert!(!home.path(".config/fish/conf.d/kurogane.fish").exists());
        assert_eq!(home.read(".bashrc"), "export FOO=1\nalias ll='ls -l'\n");
        // A startup file stays even when the installer created it
        assert_eq!(home.read(".profile"), "");
        assert!(!home.path(".zshenv").exists());
    }

    #[test]
    fn only_the_exact_line_goes() {
        let home = Home::new();
        let line = home.line();
        let env = home.path(".kurogane/env");
        let near = format!(
            "source \"{env}\"\n{line} # mine\n  {line}\n",
            env = env.display()
        );
        let file = home.write(".bashrc", &format!("{near}{line}\n"));

        assert!(strip_line(&file, &line).unwrap());
        assert_eq!(home.read(".bashrc"), near);
        assert!(mentions(&file, &env.display().to_string()));
    }

    #[test]
    fn a_crlf_line_goes_too() {
        let home = Home::new();
        let line = home.line();
        let file = home.write(".profile", &format!("a\r\n{line}\r\nb\r\n"));

        assert!(strip_line(&file, &line).unwrap());
        assert_eq!(home.read(".profile"), "a\r\nb\r\n");
    }

    #[test]
    fn a_file_without_the_line_is_not_rewritten() {
        let home = Home::new();
        let file = home.write(".bashrc", "export FOO=1");

        assert!(!strip_line(&file, &home.line()).unwrap());
        assert!(!strip_line(&home.path(".zshenv"), &home.line()).unwrap());
        assert_eq!(home.read(".bashrc"), "export FOO=1");
    }

    #[test]
    fn a_script_the_installer_did_not_write_stays() {
        let home = Home::new();
        let env = home.write(".kurogane/env", "export PATH=\"$HOME/bin:$PATH\"\n");

        assert_eq!(remove_marked(&env).unwrap(), Outcome::Foreign);
        assert!(env.exists());
        assert_eq!(
            remove_marked(&home.path("absent")).unwrap(),
            Outcome::Absent
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_startup_file_is_edited_through_its_link() {
        let home = Home::new();
        let line = home.line();
        let target = home.write("dotfiles/bashrc", &format!("x\n{line}\n"));
        let link = home.path(".bashrc");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(strip_line(&link, &line).unwrap());
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(home.read("dotfiles/bashrc"), "x\n");
    }
}
