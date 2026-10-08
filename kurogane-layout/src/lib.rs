//! The contract between a Kurogane bundle and the runtime that runs it.
//!
//! Defines where a bundle keeps its Chromium runtime, resources and macOS
//! helper, what makes a runtime complete and where application profiles
//! live. `kurogane bundle` writes this layout; the runtime reads it.

mod cef;
mod layout;
mod platform;
mod profile;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_fixtures;

pub use cef::{IncompleteRuntime, validate_cef_runtime};
pub use layout::{BUNDLE_MARKER, bundle_cef_root, bundle_cef_root_for, bundled_resource_root};
#[cfg(target_os = "macos")]
pub use layout::{bundled_helper_path, bundled_helper_path_for};
pub use profile::{profile_dir, profiles_root};
/// The name of CEF's framework inside a macOS runtime or bundle.
#[cfg(target_os = "macos")]
pub use platform::MACOS_FRAMEWORK;
