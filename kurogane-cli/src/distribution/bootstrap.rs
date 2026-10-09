//! CEF's sandbox bootstrap and the layout around it.
//!
//! Chromium's Windows sandbox is brokered by the process that starts the
//! browser, and CEF ships that broker as two prebuilt executables rather than
//! exporting it from `libcef.dll`. A sandboxed application installs one of
//! them under its own name; the bootstrap then loads `<app>.dll` from its own
//! directory and calls into it.
//!
//! Two rules follow from CEF's loader and shape every layout here:
//!
//! - The client library is looked up beside the bootstrap, never on a search
//!   path.
//! - `chrome_elf.dll` must resolve to the bootstrap's own directory, so the
//!   whole Chromium runtime sits there too.
//!
//! Both are rules about paths, so they are compiled and tested on every
//! platform, although only Windows builds this shape.

use std::path::{Path, PathBuf};

/// CEF's sandbox bootstraps, one per Windows subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bootstrap {
    /// `bootstrap.exe`: no console, for a packaged application.
    Windowed,

    /// `bootstrapc.exe`: keeps stdio attached, for development.
    Console,
}

impl Bootstrap {
    /// Every bootstrap a CEF distribution ships.
    pub const ALL: [Bootstrap; 2] = [Bootstrap::Windowed, Bootstrap::Console];

    /// Returns the bootstrap's name inside a CEF runtime.
    pub fn file_name(self) -> &'static str {
        match self {
            Bootstrap::Windowed => "bootstrap.exe",
            Bootstrap::Console => "bootstrapc.exe",
        }
    }

    /// Returns the bootstrap's path inside `cef_root`.
    pub fn path_in(self, cef_root: &Path) -> PathBuf {
        cef_root.join(self.file_name())
    }
}

/// Returns where a bootstrap installed at `bootstrap` looks for the
/// application's library.
///
/// Mirrors CEF's own derivation: the bootstrap's path with its last extension
/// replaced by `.dll`.
pub fn client_library_path(bootstrap: &Path) -> PathBuf {
    bootstrap.with_extension("dll")
}

/// Stages a CEF runtime beside a bootstrap and marks the directory as a
/// bundle's, which runs that runtime and the resources staged with it.
///
/// The bootstrap resolves `chrome_elf.dll` in its own directory and refuses
/// to start when it finds it anywhere else, so a development run needs the
/// whole runtime there rather than on `PATH`. The files are shared with the
/// installation where the filesystem allows it.
pub fn stage_runtime(cef_root: &Path, dst: &Path) -> std::io::Result<()> {
    crate::distribution::files::link_dir(
        cef_root,
        dst,
        &crate::distribution::cef::is_runtime_artifact,
    )?;
    std::fs::write(dst.join(kurogane_layout::BUNDLE_MARKER), b"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_subsystem_names_its_bootstrap() {
        assert_eq!(Bootstrap::Windowed.file_name(), "bootstrap.exe");
        assert_eq!(Bootstrap::Console.file_name(), "bootstrapc.exe");
    }

    #[test]
    fn the_client_library_takes_the_bootstrap_s_name() {
        assert_eq!(
            client_library_path(Path::new("dist/myapp.exe")),
            Path::new("dist/myapp.dll")
        );
    }

    #[test]
    fn only_the_last_extension_is_replaced() {
        // CEF removes one extension, so a dotted name keeps the rest
        assert_eq!(
            client_library_path(Path::new("my.app.exe")),
            Path::new("my.app.dll")
        );
    }

    #[test]
    fn a_name_without_an_extension_still_gains_one() {
        assert_eq!(
            client_library_path(Path::new("myapp")),
            Path::new("myapp.dll")
        );
    }

    #[test]
    fn staging_leaves_cef_s_own_bootstraps_behind() {
        let dir = crate::distribution::test_fixtures::tmp_dir();
        let cef = crate::distribution::test_fixtures::cef_runtime(&dir.path().join("cef"));
        for bootstrap in Bootstrap::ALL {
            std::fs::write(bootstrap.path_in(&cef), "bootstrap").unwrap();
        }

        let staged = dir.path().join("staged");
        stage_runtime(&cef, &staged).unwrap();

        for bootstrap in Bootstrap::ALL {
            assert!(
                !bootstrap.path_in(&staged).exists(),
                "the application installs its bootstrap under its own name"
            );
        }
        assert!(
            staged
                .join(crate::distribution::cef::cef_binary_name())
                .exists()
        );
    }

    #[test]
    fn a_staged_run_is_a_bundle() {
        let dir = crate::distribution::test_fixtures::tmp_dir();
        let cef = crate::distribution::test_fixtures::cef_runtime(&dir.path().join("cef"));

        let staged = dir.path().join("staged");
        stage_runtime(&cef, &staged).unwrap();

        assert!(staged.join(kurogane_layout::BUNDLE_MARKER).is_file());
        #[cfg(target_os = "windows")]
        assert_eq!(
            kurogane_layout::bundle_cef_root_for(&staged.join("myapp.exe")),
            Some(staged)
        );
    }
}
