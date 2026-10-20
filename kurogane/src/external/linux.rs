//! Opens links on Linux through `xdg-open`.
//!
//! `xdg-open` picks the desktop's browser. It gets the URL as one argument,
//! none of the application's input or output and no `LD_LIBRARY_PATH`. A
//! bundle launcher's `KUROGANE_LD_LIBRARY_PATH` may point it at CEF's
//! directory, whose libraries (`libEGL`, `libvulkan`) the browser must not
//! load. A worker thread waits for it and leaves no finished `xdg-open`
//! unreaped.

use std::process::{Command, Stdio};

use tracing::warn;

pub(super) fn open(link: String) {
    let spawned = std::thread::Builder::new()
        .name("kurogane-open".into())
        .spawn(move || {
            let child = Command::new("xdg-open")
                .arg(&link)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .env_remove("LD_LIBRARY_PATH")
                .spawn();
            match child.and_then(|mut child| child.wait()) {
                Ok(status) if !status.success() => {
                    warn!("xdg-open did not open the link ({status})")
                }
                Ok(_) => {}
                Err(error) => warn!("cannot run xdg-open: {error}"),
            }
        });
    if let Err(error) = spawned {
        warn!("cannot start a thread to open the link: {error}");
    }
}
