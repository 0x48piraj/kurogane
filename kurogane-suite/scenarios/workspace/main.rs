//! Capability-controlled filesystem workspace.
//!
//! One origin is granted six capabilities over one directory i.e. browse the tree,
//! read/write files, create folders, rename, delete goes through `window.kurogane.fs`;
//! the grant is the authorization boundary.
//!
//! Workspace under `std::env::temp_dir()/kurogane-workspace`.
//!
//! Try editing a file, restarting the app, opening the workspace in the system
//! file manager and probing the denied `vault/` subtree.
//! Try editing a file, restarting the app, opening the workspace in the system
//! file manager, probing the denied `vault/` subtree, or removing `RENAME` from
//! `access` to see the corresponding operation start failing.

use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

use kurogane::capability::{Filesystem, FsAccess};
use kurogane::{App, AppHandle, IpcError, Origin};
use serde_json::Value;

const INDEX_HTML: &str = include_str!("frontend/index.html");
const ORIGIN: &str = "app://app";

/// Subtree excluded from the grant.
const DENIED: &str = "vault";

fn main() -> Result<(), Box<dyn Error>> {
    let home = std::env::temp_dir().join("kurogane-workspace");
    let root = home.join("workspace");
    let dist = home.join("dist");

    // Keep the demo grant minimal; METADATA is intentionally absent
    let access = FsAccess::READ
        | FsAccess::LIST
        | FsAccess::CREATE
        | FsAccess::WRITE
        | FsAccess::RENAME
        | FsAccess::DELETE;

    // Seed only in the browser process. Helper processes also reach `App::run`
    let browser = kurogane::is_browser_process();
    if browser {
        seed(&root, &dist, access)?;
        println!(
            "kurogane workspace\n  ui:        {ORIGIN}\n  workspace: {}",
            root.display()
        );
    }

    let mut builder = Filesystem::builder();
    let scope = builder.scope("workspace", |scope| {
        scope.allow_directory_recursive(&root);
        // Denied paths are omitted from listings as well as reads
        scope.deny_path(root.join(DENIED));
    });
    builder.grant(Origin::parse(ORIGIN)?, scope, access);

    // Deliberate host action
    // Reveal the granted root, never a page-supplied path
    let reveal_root = root.clone();
    App::new(&dist)
        .filesystem(builder.build()?)
        .command("workspace.reveal", move |_: Value, _: &AppHandle| {
            reveal(&reveal_root).map_err(|e| IpcError::new(format!("reveal failed: {e}")))?;
            Ok(Value::Null)
        })
        .run()?;
    Ok(())
}

/// Opens the granted root in the system file manager, so the files in the
/// tree can be seen outside the app for transparency.
fn reveal(root: &Path) -> io::Result<()> {
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";

    // Explorer may return a non-zero status after opening the folder
    // Child is launched without waiting for its exit status
    std::process::Command::new(opener)
        .arg(root)
        .spawn()
        .map(drop)
}

/// Lays down the demo workspace without overwriting existing files.
fn seed(root: &Path, dist: &Path, access: FsAccess) -> io::Result<()> {
    std::fs::create_dir_all(root.join("notes"))?;
    std::fs::create_dir_all(root.join("projects").join("hello"))?;
    std::fs::create_dir_all(root.join(DENIED))?;
    std::fs::create_dir_all(dist)?;

    std::fs::write(dist.join("index.html"), INDEX_HTML)?;
    std::fs::write(dist.join("grant.js"), grant_js(root, access))?;

    keep(
        root.join("README.md"),
        "# Workspace\n\
         \n\
         A small workspace backed by a real directory on disk.\n\
         Edit something, make a mess, restart the app. It'll still be here.\n\
         \n\
         Press Open in system ↗ to see the same files in your file manager.\n",
    )?;
    keep(
        root.join("notes").join("ideas.txt"),
        "A few things to try:\n\
        \n\
        - edit this file\n\
        - make another one\n\
        - rename it\n\
        - open the workspace in your file manager\n",
    )?;
    keep(
        root.join("notes").join("todo.txt"),
        "[ ] read the filesystem docs\n\
         [ ] try the new filesystem API\n\
         [ ] clean up this example\n\
         [ ] test the edge cases\n\
         [ ] pretend this is finished\n",
    )?;
    keep(
        root.join("projects").join("hello").join("README.md"),
        "# hello\n\
         \n\
         A project directory, two levels down. Nested files are covered by\n\
         the same grant as files at the workspace root.\n",
    )?;
    keep(
        root.join("projects").join("hello").join("config.json"),
        "{\n\
        \x20 \"name\": \"hello\",\n\
        \x20 \"version\": 1,\n\
        \x20 \"note\": \"Edit this and save. It is a real file on disk.\"\n\
         }\n",
    )?;
    // A real denied file for the policy demo
    keep(
        root.join(DENIED).join("recovery-codes.txt"),
        "The app should not be able to read this.\n\
         \n\
         If you're seeing this inside the app, either you changed the grant, or we fucked up spectacularly.\n",
    )
}

/// Writes `body` only when `path` does not already exist.
fn keep(path: PathBuf, body: &str) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    std::fs::write(path, body)
}

/// The policy panel needs three host-side facts that `fs.*` never exposes;
/// the granted root, the denied subtree and this origin's capabilities. The
/// page cannot query its own authority; it either holds a capability or it
/// does not. The denied subtree is likewise undiscoverable: `readDir` omits it.
/// These values are passed to the UI explicitly for transparency in the demo,
/// not as part of the filesystem API.
///
/// The panel reflects the capabilities held by the origin's [`FsAccess`] grant.
fn grant_js(root: &Path, access: FsAccess) -> String {
    let names = format!("{access:?}");
    let held: Vec<&str> = names.split('|').collect();
    format!(
        "window.GRANT = {{ root: {:?}, denied: {DENIED:?}, held: {held:?} }};\n",
        root.display().to_string(),
    )
}
