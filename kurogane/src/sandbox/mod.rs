use cef::*;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
mod macos;

pub(crate) fn apply_sandbox_flags(cmd: &mut CommandLine) {
    #[cfg(target_os = "linux")]
    linux::apply_sandbox_flags(cmd);

    #[cfg(target_os = "windows")]
    windows::apply_sandbox_flags(cmd);

    #[cfg(target_os = "macos")]
    macos::apply_sandbox_flags(cmd);
}
