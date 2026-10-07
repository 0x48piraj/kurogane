mod bootstrap;
mod cef;
mod layout;
mod platform;
mod profile;
mod package;
mod distribution;
mod bundle;
mod shell;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_fixtures;

pub use bootstrap::{Bootstrap, client_library_path, stage_runtime};
pub use bundle::{BundleError, BundleLayout};
pub use cef::{
    cef_path, read_provenance, resolve_cef_for_bundle, validate_cef_runtime, verify_installation,
    CefError, CefProvenance, CefSource, ResolvedCef,
};
pub use distribution::{
    AppMetadata, DistributionError, Executable, LinkProblem, NameProblem, ResolvedDistribution,
    ResolvedResource, check_app_name, portable_file_name,
};
pub use shell::sh_quote;
pub use layout::{bundle_cef_root, bundled_helper_path, bundled_resource_root, copy_dir, link_dir};
#[cfg(target_os = "macos")]
pub use layout::bundled_helper_path_for;
pub use package::{PackageError, package_directory};
pub use profile::{cache_root, profile_dir, profiles_root};
/// The name of CEF's framework inside a macOS runtime or bundle.
#[cfg(target_os = "macos")]
pub use platform::MACOS_FRAMEWORK;
