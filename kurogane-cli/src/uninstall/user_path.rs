//! Removes the installer's entry from the Windows user `Path`.
//!
//! The user's `Path` value is preserved with all other entries unchanged.
//! Values are edited in place and kept unexpanded.

/// Returns `path` without the installer's `dir` entry.
fn without_entry(path: &str, dir: &str) -> Option<String> {
    let dir = normalize(dir);
    if dir.is_empty() {
        return None;
    }

    let mut found = false;
    let kept: Vec<&str> = path
        .split(';')
        .filter(|entry| {
            let ours = normalize(entry) == dir;
            found |= ours;
            !ours
        })
        .collect();

    found.then(|| kept.join(";"))
}

fn normalize(entry: &str) -> String {
    entry.trim_end_matches('\\').to_lowercase()
}

#[cfg(windows)]
pub(super) use registry::remove;

#[cfg(windows)]
mod registry {
    use std::io;
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, WIN32_ERROR,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_VALUE_TYPE, RRF_NOEXPAND,
        RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteValueW, RegGetValueW,
        RegOpenKeyExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    use crate::tui;

    const ENVIRONMENT: &str = "Environment";

    /// Removes `dir` from the user's `Path`.
    pub(in crate::uninstall) fn remove(dir: &Path, failed: &mut Vec<String>) {
        match remove_entry(dir) {
            Ok(true) => tui::field("user PATH", format!("{} removed", dir.display())),
            Ok(false) => {}
            Err(e) => {
                tui::warn(&format!(
                    "Failed to remove {} from the user PATH: {e}",
                    dir.display()
                ));
                failed.push("user PATH".into());
            }
        }
    }

    fn remove_entry(dir: &Path) -> io::Result<bool> {
        let (subkey, test) = environment_key()?;
        let key = Key::open(&subkey)?;
        let Some((kind, path)) = key.string("Path")? else {
            return Ok(false);
        };
        let Some(updated) = super::without_entry(&path, &dir.to_string_lossy()) else {
            return Ok(false);
        };
        if updated.is_empty() {
            key.delete("Path")?;
        } else {
            key.set_string("Path", kind, &updated)?;
        }
        if !test {
            broadcast_environment_change();
        }
        Ok(true)
    }

    /// Returns the user environment key, or the test override.
    fn environment_key() -> io::Result<(String, bool)> {
        let Some(test) = std::env::var_os("KUROGANE_TEST_ENV_KEY").filter(|v| !v.is_empty()) else {
            return Ok((ENVIRONMENT.into(), false));
        };
        let test = test.to_string_lossy();
        match test.strip_prefix(r"HKCU:\") {
            Some(subkey) if !subkey.is_empty() => Ok((subkey.into(), true)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("KUROGANE_TEST_ENV_KEY must name a key under HKCU:\\, not {test}"),
            )),
        }
    }

    /// Notifies running programs that the user environment changed.
    fn broadcast_environment_change() {
        let area = wide(ENVIRONMENT);
        // SAFETY: `area` is a NUL-terminated UTF-16 string that outlives the
        // call. The result pointer may be null
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                area.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                5000,
                ptr::null_mut(),
            );
        }
    }

    /// An open registry key, closed on drop.
    struct Key(HKEY);

    impl Key {
        fn open(subkey: &str) -> io::Result<Self> {
            let subkey = wide(subkey);
            let mut key: HKEY = ptr::null_mut();
            // SAFETY: `subkey` is a NUL-terminated UTF-16 string that
            // outlives the call. `key` is a valid place for the handle
            let status = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    subkey.as_ptr(),
                    0,
                    KEY_QUERY_VALUE | KEY_SET_VALUE,
                    &mut key,
                )
            };
            check(status)?;
            Ok(Self(key))
        }

        /// Reads a string value without expanding environment variables.
        fn string(&self, name: &str) -> io::Result<Option<(REG_VALUE_TYPE, String)>> {
            let name = wide(name);
            let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
            loop {
                let mut size = 0u32;
                // SAFETY: `self.0` is an open key and `name` a NUL-terminated
                // UTF-16 string; with no data buffer the call only writes the
                // size `size` points to
                let status = unsafe {
                    RegGetValueW(
                        self.0,
                        ptr::null(),
                        name.as_ptr(),
                        flags,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        &mut size,
                    )
                };
                if status == ERROR_FILE_NOT_FOUND {
                    return Ok(None);
                }
                check(status)?;

                let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
                let mut size = (buffer.len() * 2) as u32;
                let mut kind: REG_VALUE_TYPE = 0;
                // SAFETY: `buffer` is the `size` bytes the call is told about.
                // `kind` and `size` are valid places for its results
                let status = unsafe {
                    RegGetValueW(
                        self.0,
                        ptr::null(),
                        name.as_ptr(),
                        flags,
                        &mut kind,
                        buffer.as_mut_ptr().cast(),
                        &mut size,
                    )
                };
                // The value grew between the two calls
                if status == ERROR_MORE_DATA {
                    continue;
                }
                check(status)?;

                buffer.truncate(size as usize / 2);
                while buffer.last() == Some(&0) {
                    buffer.pop();
                }
                let value = String::from_utf16(&buffer).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "the user PATH is not valid text",
                    )
                })?;
                return Ok(Some((kind, value)));
            }
        }

        fn set_string(&self, name: &str, kind: REG_VALUE_TYPE, value: &str) -> io::Result<()> {
            let name = wide(name);
            let data = wide(value);
            // SAFETY: `self.0` is an open key and `name` a NUL-terminated UTF-16
            // string. `data` is `data.len() * 2` bytes of UTF-16 ending in the
            // NUL a string value needs
            let status = unsafe {
                RegSetValueExW(
                    self.0,
                    name.as_ptr(),
                    0,
                    kind,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            };
            check(status)
        }

        fn delete(&self, name: &str) -> io::Result<()> {
            let name = wide(name);
            // SAFETY: `self.0` is an open key and `name` a NUL-terminated
            // UTF-16 string that outlives the call
            check(unsafe { RegDeleteValueW(self.0, name.as_ptr()) })
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: `self.0` was opened by `Key::open` and is closed only
            // here
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn check(status: WIN32_ERROR) -> io::Result<()> {
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status as i32))
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIN: &str = r"C:\Users\me\AppData\Local\kurogane\bin";

    #[test]
    fn removes_the_entry_and_keeps_the_rest_as_stored() {
        let path = format!(r"{BIN};%USERPROFILE%\tools;C:\other");

        assert_eq!(
            without_entry(&path, BIN).as_deref(),
            Some(r"%USERPROFILE%\tools;C:\other")
        );
    }

    #[test]
    fn entries_compare_ignoring_case_and_a_trailing_backslash() {
        let path = format!(r"C:\a;{}\;C:\b", BIN.to_uppercase());

        assert_eq!(without_entry(&path, BIN).as_deref(), Some(r"C:\a;C:\b"));
    }

    #[test]
    fn a_path_without_the_entry_is_left_alone() {
        assert_eq!(without_entry(r"C:\a;C:\b", BIN), None);
        assert_eq!(without_entry("", BIN), None);
    }

    #[test]
    fn the_only_entry_leaves_an_empty_path() {
        assert_eq!(without_entry(BIN, BIN).as_deref(), Some(""));
    }

    #[test]
    fn an_empty_dir_matches_nothing() {
        assert_eq!(without_entry(r"C:\a;;C:\b", ""), None);
        assert_eq!(without_entry(r"C:\a;;C:\b", r"\"), None);
    }
}
