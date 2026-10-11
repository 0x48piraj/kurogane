//! Removes a Kurogane installation using its installation receipt.
//!
//! The installer's receipt defines the uninstall boundary. An installation
//! without a matching receipt is left intact with manual removal guidance.
//! Installed runtimes and caches are removed; application profiles are kept.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use kurogane_layout::profiles_root;

use crate::cache::cache_root;

use crate::clean;
use crate::receipt::{self, Installed};
use crate::tui;

#[cfg(any(unix, test))]
mod startup;
#[cfg(any(windows, test))]
mod user_path;

/// Manual uninstall instructions for installations without a matching receipt.
const MANUAL_STEPS: &str = concat!(
    env!("CARGO_PKG_REPOSITORY"),
    "/blob/master/docs/install.md#uninstalling"
);

pub fn run(confirmed: bool, keep_data: bool, non_interactive: bool) -> Result<()> {
    tui::section("Kurogane Uninstall");

    let shown = std::env::current_exe().context("cannot locate the running kurogane")?;
    let exe = fs::canonicalize(&shown)
        .with_context(|| format!("cannot locate the running kurogane at {}", shown.display()))?;

    let confirm = Confirm {
        confirmed,
        non_interactive,
    };
    match receipt::find(&exe)? {
        Some(installed) => uninstall(&exe, &installed, keep_data, confirm),
        None => remove_data_only(
            &shown,
            Owner::of(&exe, cargo_bin().as_deref()),
            keep_data,
            confirm,
        ),
    }
}

/// How the run was confirmed.
struct Confirm {
    /// Whether `--yes` was given.
    confirmed: bool,
    non_interactive: bool,
}

impl Confirm {
    /// Returns whether the uninstall has been confirmed.
    fn ask(&self) -> Result<bool> {
        if self.confirmed {
            return Ok(true);
        }
        // Never prompt when running unattended
        if self.non_interactive {
            bail!(
                "`self uninstall` needs confirmation and cannot prompt here.\n\n  \
                 Re-run with --yes to confirm."
            );
        }
        let accepted = tui::confirm("Continue?")?;
        tui::blank();
        if !accepted {
            tui::info("Aborted");
        }
        Ok(accepted)
    }
}

/// Removes everything recorded for the installation and its Kurogane data,
/// unless `keep_data` is set.
fn uninstall(exe: &Path, installed: &Installed, keep_data: bool, confirm: Confirm) -> Result<()> {
    let receipt = &installed.receipt;
    let bin = exe
        .parent()
        .context("the running kurogane has no parent folder")?;

    tui::step("Installed by the Kurogane installer");
    tui::field("version", &receipt.version);
    tui::field("binary", tui::format_path(&receipt.binary));
    tui::blank();

    tui::warn("This removes the kurogane binary and the PATH setup the installer added.");
    if !keep_data {
        tui::warn(
            "Including Kurogane's caches and tanso's shared Chromium runtimes, which other tanso projects use too.",
        );
    }
    note_profiles();
    if !confirm.ask()? {
        return Ok(());
    }

    // Leave the receipt and binary until all other cleanup succeeds
    let mut failed = Vec::new();
    if !keep_data {
        remove_data(&mut failed);
        tui::blank();
    }
    tui::step("Removing PATH setup");
    #[cfg(unix)]
    startup::remove(receipt, &mut failed);
    #[cfg(windows)]
    if let Some(entry) = &receipt.user_path {
        user_path::remove(entry, &mut failed);
    }
    remove_leftovers(bin, &mut failed);
    tui::blank();
    if !failed.is_empty() {
        bail!(
            "Uninstall incomplete; could not remove: {}\n\n  \
             The kurogane binary is still in place. Close running Kurogane \
             applications, then run `kurogane self uninstall` again.",
            failed.join(", ")
        );
    }

    tui::step("Removing kurogane");
    remove_binary(exe, &installed.home).context("failed to remove the kurogane binary")?;
    tui::field("binary", "removed");
    let receipt_path = installed.receipt_path();
    fs::remove_file(&receipt_path)
        .with_context(|| format!("failed to remove {}", receipt_path.display()))?;
    prune(&installed.home, bin);
    prune_data_roots();
    tui::blank();

    tui::success("Kurogane uninstalled");
    if keep_data {
        tui::info("Chromium runtimes and caches kept (--keep-data)");
    }
    note_profiles();
    if let Some(other) = on_path() {
        tui::info(&format!(
            "Another kurogane is still on PATH: {}",
            tui::format_path(&other)
        ));
    }
    Ok(())
}

/// Removes Kurogane's runtimes and caches for an unmanaged installation and
/// reports how its binary should be removed.
fn remove_data_only(exe: &Path, owner: Owner, keep_data: bool, confirm: Confirm) -> Result<()> {
    tui::step(owner.describe());
    tui::field("binary", tui::format_path(exe));
    tui::field("remove it with", owner.removal());
    tui::blank();

    if keep_data {
        tui::info("Nothing else to remove (--keep-data)");
        return Ok(());
    }

    tui::warn(
        "This removes Kurogane's caches and tanso's shared Chromium runtimes, which other tanso projects use too.",
    );
    tui::warn("The kurogane binary stays.");
    note_profiles();
    if !confirm.ask()? {
        return Ok(());
    }

    let mut failed = Vec::new();
    remove_data(&mut failed);
    tui::blank();
    if !failed.is_empty() {
        bail!(
            "Cleanup incomplete; could not remove: {}",
            failed.join(", ")
        );
    }
    prune_data_roots();

    tui::success("Chromium runtimes and caches removed");
    note_profiles();
    tui::info(&format!("Remove the binary with: {}", owner.removal()));
    Ok(())
}

/// How an unmanaged installation should be removed.
#[derive(Debug, PartialEq, Eq)]
enum Owner {
    Nix,
    Cargo,
    /// A build tree, standalone copy or installation with a missing receipt.
    Unknown,
}

impl Owner {
    /// Identifies who manages `exe` from its installation location.
    fn of(exe: &Path, cargo_bin: Option<&Path>) -> Self {
        if exe.starts_with("/nix/store") {
            Self::Nix
        } else if cargo_bin.is_some() && exe.parent() == cargo_bin {
            Self::Cargo
        } else {
            Self::Unknown
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            Self::Nix => "Installed with Nix, which owns the binary",
            Self::Cargo => "Installed with Cargo, which owns the binary",
            Self::Unknown => {
                "Not installed by the Kurogane installer: no install receipt names this binary"
            }
        }
    }

    fn removal(&self) -> String {
        match self {
            Self::Nix => {
                "nix profile remove kurogane (or drop it from your Nix configuration)".into()
            }
            Self::Cargo => "cargo uninstall kurogane-cli".into(),
            Self::Unknown => format!("delete it yourself; manual steps: {MANUAL_STEPS}"),
        }
    }
}

/// Returns Cargo's `bin` folder, where `cargo install` puts binaries.
fn cargo_bin() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))?;
    fs::canonicalize(home.join("bin")).ok()
}

/// Removes Kurogane's installed runtimes and caches.
fn remove_data(failed: &mut Vec<String>) {
    tui::step("Removing Chromium runtimes and caches");
    let mut labels = Vec::new();
    for data in clean::runtimes().into_iter().chain(clean::caches()) {
        clean::remove(data.label, data.what, &data.path, &mut labels);
    }
    failed.extend(labels.into_iter().map(String::from));
}

/// Reports where application profiles are kept, if any exist.
fn note_profiles() {
    let profiles = profiles_root();
    if profiles.exists() {
        tui::info(&format!(
            "Application profiles stay in {}; delete it if no Kurogane application needs them",
            tui::format_path(&profiles)
        ));
    }
}

/// Removes staged copies and the previous Windows binary that an installer
/// run can leave beside the binary.
fn remove_leftovers(bin: &Path, failed: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(bin) else {
        return;
    };
    for entry in entries.flatten() {
        if !is_leftover(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let path = entry.path();
        match fs::remove_file(&path) {
            Ok(()) => tui::field(&tui::format_path(&path), "removed"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                tui::warn(&format!(
                    "Failed to remove {}: {e}",
                    tui::format_path(&path)
                ));
                failed.push(tui::format_path(&path));
            }
        }
    }
}

fn is_leftover(name: &str) -> bool {
    name.starts_with(".kurogane.new.") || name == "kurogane.exe.old"
}

#[cfg(unix)]
fn remove_binary(exe: &Path, _home: &Path) -> io::Result<()> {
    fs::remove_file(exe)
}

#[cfg(windows)]
fn remove_binary(exe: &Path, home: &Path) -> io::Result<()> {
    // A running executable cannot be deleted on Windows. Move it out of
    // `home` now so the folders can be removed; a helper process deletes it
    // after this process exits
    let outside = if exe.starts_with(home) {
        home
    } else {
        exe.parent().unwrap_or(home)
    };
    self_replace::self_delete_outside_path(outside)
}

/// Removes the default `bin` folder and the installer's home once empty.
/// A folder chosen with `--install-dir` is kept.
fn prune(home: &Path, bin: &Path) {
    if bin == home.join("bin") {
        remove_if_empty(bin);
    }
    remove_if_empty(home);
}

/// Removes Kurogane's data folder, its cache folder and tanso's data folder
/// when nothing is left in them.
fn prune_data_roots() {
    if let Some(data) = profiles_root().parent() {
        remove_if_empty(data);
    }
    if let Some(tanso) = tanso_download::cef_install_root()
        .as_deref()
        .and_then(Path::parent)
    {
        remove_if_empty(tanso);
    }
    remove_if_empty(&cache_root());
}

fn remove_if_empty(dir: &Path) {
    // A folder that is not empty is kept, so the error is ignored
    let _ = fs::remove_dir(dir);
}

/// Returns another `kurogane` on PATH after this one is removed.
fn on_path() -> Option<PathBuf> {
    let name = format!("kurogane{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nix_store_binary_is_nixs() {
        let exe = Path::new("/nix/store/abc-kurogane-0.0.7/bin/.kurogane-wrapped");

        assert_eq!(Owner::of(exe, None), Owner::Nix);
    }

    #[test]
    fn a_binary_in_cargos_bin_is_cargos() {
        let cargo_bin = Path::new("/home/me/.cargo/bin");

        assert_eq!(
            Owner::of(&cargo_bin.join("kurogane"), Some(cargo_bin)),
            Owner::Cargo
        );
        assert_eq!(
            Owner::of(
                Path::new("/home/me/.cargo/bin/sub/kurogane"),
                Some(cargo_bin)
            ),
            Owner::Unknown
        );
    }

    #[test]
    fn anything_else_is_unknown() {
        let exe = Path::new("/work/kurogane/target/debug/kurogane");

        assert_eq!(Owner::of(exe, None), Owner::Unknown);
        assert!(Owner::Unknown.removal().contains("#uninstalling"));
    }

    #[test]
    fn leftovers_are_only_the_installers_temporary_names() {
        assert!(is_leftover(".kurogane.new.4242"));
        assert!(is_leftover(".kurogane.new.4242.exe"));
        assert!(is_leftover("kurogane.exe.old"));
        assert!(!is_leftover("kurogane"));
        assert!(!is_leftover("kurogane.exe"));
        assert!(!is_leftover("other-tool"));
    }

    #[test]
    fn the_default_bin_folder_goes_once_empty() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join(".kurogane");
        let bin = home.join("bin");
        fs::create_dir_all(&bin).unwrap();

        prune(&home, &bin);

        assert!(!home.exists());
    }

    #[test]
    fn a_bin_folder_chosen_at_install_stays() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join(".kurogane");
        let bin = root.path().join("tools").join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();

        prune(&home, &bin);

        assert!(bin.exists());
        assert!(!home.exists());
    }

    #[test]
    fn a_home_that_holds_something_else_stays() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join(".kurogane");
        let bin = home.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(home.join("notes.txt"), "mine").unwrap();

        prune(&home, &bin);

        assert!(!bin.exists());
        assert!(home.join("notes.txt").exists());
    }
}
