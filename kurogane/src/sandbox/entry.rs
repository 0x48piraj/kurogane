//! Entry points for CEF's Windows sandbox bootstrap.
//!
//! Chromium's sandbox is brokered by CEF's `bootstrap.exe`/`bootstrapc.exe`
//! which loads the application DLL and calls one of the exported
//! entry points defined by [`crate::sandbox_entry`].

/// Declares the entry points CEF's sandbox bootstrap calls.
///
/// Chromium's Windows sandbox is brokered by CEF's bootstrap executable, which
/// loads the application as a DLL and calls `RunWinMain` (windowed) or
/// `RunConsoleMain` (console). This macro writes both, handing the broker
/// state to the runtime before running `entry`.
///
/// It expands to nothing but a signature check off Windows, so the same source
/// builds everywhere.
///
/// # Layout
///
/// The exports have to come from the application's own crate, because a
/// `cdylib` does not re-export symbols from its dependencies. So the
/// application lives in `src/lib.rs` and `src/main.rs` becomes a shim, which
/// keeps `cargo run` and unsandboxed builds working unchanged:
///
/// ```toml
/// [lib]
/// name = "myapp_lib"
/// crate-type = ["cdylib", "rlib"]
///
/// [[bin]]
/// name = "myapp"
/// ```
///
/// The library is named apart from the binary because on Windows the two
/// would otherwise write the same `.pdb`, which Cargo warns about. The
/// bootstrap keeps the binary's name.
///
/// ```no_run
/// // src/lib.rs
/// pub fn run() {
///     kurogane::App::new("content")
///         .sandbox_mode(kurogane::SandboxMode::Chromium)
///         .run_or_exit();
/// }
///
/// kurogane::sandbox_entry!(run);
/// # fn main() {}
/// ```
///
/// ```ignore
/// // src/main.rs
/// fn main() {
///     myapp_lib::run()
/// }
/// ```
///
/// Then set `sandbox = true` under `[app]` in `kurogane.toml`: `kurogane run`
/// and `kurogane dev` start the application through the bootstrap, and
/// `kurogane bundle` ships the bootstrap under the application's name.
///
/// Invoke the macro once, at the crate root. A second invocation is a
/// duplicate symbol.
#[macro_export]
macro_rules! sandbox_entry {
    ($entry:path) => {
        // Check the entry point on every platform
        const _: fn() = $entry;

        /// Entry point for the windowed CEF bootstrap.
        ///
        /// # Safety
        ///
        /// CEF must call this function with valid sandbox state and version
        /// information supplied by the bootstrap.
        #[cfg(target_os = "windows")]
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn RunWinMain(
            _instance: *mut ::core::ffi::c_void,
            _command_line: *mut u16,
            _show_command: ::core::ffi::c_int,
            sandbox_info: *mut ::core::ffi::c_void,
            version_info: *mut ::core::ffi::c_void,
        ) -> ::core::ffi::c_int {
            // SAFETY: the pointers come from CEF and are passed unchanged to the
            // runtime entry point.
            unsafe { $crate::__private::enter(sandbox_info, version_info, $entry) }
        }

        /// Entry point for the console CEF bootstrap.
        ///
        /// # Safety
        ///
        /// CEF must call this function with valid sandbox state and version
        /// information supplied by the bootstrap.
        #[cfg(target_os = "windows")]
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn RunConsoleMain(
            _argc: ::core::ffi::c_int,
            _argv: *mut *mut ::core::ffi::c_char,
            sandbox_info: *mut ::core::ffi::c_void,
            version_info: *mut ::core::ffi::c_void,
        ) -> ::core::ffi::c_int {
            // SAFETY: the pointers come from CEF and are passed unchanged to the
            // runtime entry point.
            unsafe { $crate::__private::enter(sandbox_info, version_info, $entry) }
        }
    };
}
