//! Boots a page under a sandbox policy and checks whether its renderer is
//! really sandboxed.
//!
//! `KUROGANE_SMOKE_SANDBOX=disabled` selects `SandboxMode::Disabled`; anything
//! else selects `SandboxMode::Chromium`. Exits 0 when the renderer's sandbox
//! state matches the policy, 1 when it does not or no renderer appears and 2
//! when the runtime refuses to start.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use kurogane::{App, SandboxMode};

/// How long a renderer must stay observable before its state is trusted.
const SETTLE: Duration = Duration::from_secs(2);

const TIMEOUT: Duration = Duration::from_secs(60);

fn main() -> ExitCode {
    let mode = match std::env::var("KUROGANE_SMOKE_SANDBOX").as_deref() {
        Ok("disabled") => SandboxMode::Disabled,
        _ => SandboxMode::Chromium,
    };

    let runtime = match App::url("about:blank").sandbox_mode(mode).start() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    let started = Instant::now();
    let mut first_seen: Option<Instant> = None;

    let verdict = loop {
        runtime.pump();
        std::thread::sleep(Duration::from_millis(16));

        match probe::renderer_sandboxed() {
            Some(sandboxed) => {
                if first_seen.get_or_insert_with(Instant::now).elapsed() >= SETTLE {
                    break Some(sandboxed);
                }
            }
            None => first_seen = None,
        }

        if started.elapsed() > TIMEOUT || runtime.should_shutdown() {
            break None;
        }
    };

    runtime.close_all_browsers(true);

    let closing = Instant::now();
    while !runtime.should_shutdown() && closing.elapsed() < Duration::from_secs(10) {
        runtime.pump();
        std::thread::sleep(Duration::from_millis(16));
    }

    runtime.shutdown();

    let Some(sandboxed) = verdict else {
        eprintln!("sandbox-smoke: no renderer process observed");
        return ExitCode::FAILURE;
    };

    println!("sandbox-smoke: policy={mode:?} renderer_sandboxed={sandboxed}");

    if sandboxed == (mode == SandboxMode::Chromium) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(target_os = "linux")]
mod probe {
    use std::path::Path;

    /// Returns whether this app's renderer runs under seccomp-bpf, if one is
    /// running.
    ///
    /// Matches on the command line: sandboxed renderers are not dumpable, so
    /// their `/proc/<pid>/exe` link is unreadable.
    pub fn renderer_sandboxed() -> Option<bool> {
        let exe = std::env::current_exe().ok()?;
        let exe_name = exe.file_name()?;

        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let dir = entry.path();

            let Ok(cmdline) = std::fs::read(dir.join("cmdline")) else {
                continue;
            };

            let mut args = cmdline.split(|b| *b == 0);

            let program = args.next().and_then(|arg| std::str::from_utf8(arg).ok());
            if program.and_then(|p| Path::new(p).file_name()) != Some(exe_name) {
                continue;
            }

            if !args.any(|arg| arg == b"--type=renderer") {
                continue;
            }

            let Ok(status) = std::fs::read_to_string(dir.join("status")) else {
                continue;
            };

            let seccomp = status.lines().find_map(|line| line.strip_prefix("Seccomp:"))?;

            return Some(seccomp.trim() == "2");
        }

        None
    }
}

#[cfg(target_os = "macos")]
mod probe {
    /// Returns whether this app's renderer runs under seatbelt, if one is
    /// running.
    ///
    /// Chromium launches seatbelt-sandboxed helpers with `--seatbelt-client`.
    pub fn renderer_sandboxed() -> Option<bool> {
        let exe = std::env::current_exe().ok()?;
        let bundle = exe
            .ancestors()
            .find(|path| path.extension().is_some_and(|ext| ext == "app"))?
            .to_string_lossy()
            .into_owned();

        let output = std::process::Command::new("ps")
            .args(["-axww", "-o", "args="])
            .output()
            .ok()?;

        let listing = String::from_utf8_lossy(&output.stdout);

        let renderer = listing
            .lines()
            .find(|line| line.starts_with(&bundle) && line.contains("--type=renderer"))?;

        Some(renderer.contains("--seatbelt-client"))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod probe {
    pub fn renderer_sandboxed() -> Option<bool> {
        None
    }
}
