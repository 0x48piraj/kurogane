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
    use std::collections::HashMap;

    /// A process's parent pid and command line.
    type Process = (u32, String);

    /// Returns whether this app's renderers run under seccomp-bpf, if any
    /// renderer is running.
    ///
    /// Renderers are found among this process's descendants. Their program
    /// name is unreliable and sandboxed ones are not dumpable; so
    /// `/proc/<pid>/exe` is unreadable.
    pub fn renderer_sandboxed() -> Option<bool> {
        let processes: HashMap<u32, Process> = std::fs::read_dir("/proc")
            .ok()?
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
            .filter_map(|pid| Some((pid, (parent_of(pid)?, cmdline_of(pid)?))))
            .collect();

        let me = std::process::id();
        let mut found = None;

        for (&pid, (parent, cmdline)) in &processes {
            if !descends_from(pid, me, &processes) || !is_renderer(cmdline, processes.get(parent)) {
                continue;
            }

            let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
                continue;
            };

            let Some(seccomp) = status
                .lines()
                .find_map(|line| line.strip_prefix("Seccomp:"))
            else {
                continue;
            };

            if seccomp.trim() == "2" {
                return Some(true);
            }

            found = Some(false);
        }

        found
    }

    /// Returns whether a process is a renderer.
    ///
    /// Chrome retitles renderers `--type=renderer` but CEF's keep the command
    /// line of the zygote they were forked from. The zygote marked
    /// `--no-zygote-sandbox` serves helpers other than renderers.
    fn is_renderer(cmdline: &str, parent: Option<&Process>) -> bool {
        has_arg(cmdline, "--type=renderer")
            || (has_arg(cmdline, "--type=zygote")
                && !has_arg(cmdline, "--no-zygote-sandbox")
                && parent.is_some_and(|(_, parent)| has_arg(parent, "--type=zygote")))
    }

    fn has_arg(cmdline: &str, arg: &str) -> bool {
        cmdline.split_whitespace().any(|a| a == arg)
    }

    /// Reads a command line with arguments separated by spaces.
    fn cmdline_of(pid: u32) -> Option<String> {
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;

        Some(String::from_utf8_lossy(&cmdline).replace('\0', " "))
    }

    /// Reads a process's parent pid from `/proc/<pid>/stat`.
    fn parent_of(pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;

        // The command name may contain spaces; the fields after it are
        // state, then parent pid
        let fields = &stat[stat.rfind(')')? + 1..];

        fields.split_whitespace().nth(1)?.parse().ok()
    }

    fn descends_from(mut pid: u32, ancestor: u32, processes: &HashMap<u32, Process>) -> bool {
        for _ in 0..64 {
            match processes.get(&pid) {
                Some(&(parent, _)) if parent == ancestor => return true,
                Some(&(parent, _)) if parent > 1 => pid = parent,
                _ => return false,
            }
        }

        false
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
