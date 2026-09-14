//! Fixtures shared by the capability test suites.

use std::path::Path;

/// Creates a directory link at `link` pointing to `target`; a symlink on
/// Unix, a junction on Windows (junctions need no privilege).
pub(crate) fn link_dir(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("create a directory symlink");
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("run mklink");
        assert!(status.success(), "mklink /J failed for {}", link.display());
    }
}

/// Creates a file symlink when this process may; Windows requires Developer
/// Mode or elevation.
pub(crate) fn link_file(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link).is_ok();
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link).is_ok();
}
