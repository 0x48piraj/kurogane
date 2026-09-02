mod cef;
mod config;
mod discover;
mod layout;
mod platform;
mod profile;
mod package;
mod distribution;
mod bundle;
mod signing;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_fixtures;

pub use bundle::{BundleError, BundleLayout};
pub use cef::{
    materialize_cef_runtime, read_provenance, resolve_cef_for_bundle, validate_cef_runtime,
    CefError, CefProvenance, CefSource, ResolvedCef,
};
pub use discover::{DetectError, DetectedCef, DiscoveryMode, detect_cef_root_with_version};
pub use config::{
    anchor_path, AppConfig, BundleConfig, ConfigError, LinuxPackagingConfig, PackagingConfig,
    ResourceConfig, SigningFileConfig, WindowsPackagingConfig, CONFIG_FILE_NAME,
};
pub use distribution::{AppMetadata, DistributionError, ResolvedDistribution, ResolvedResource};
pub use layout::{bundled_cef_root, cef_install_dir, copy_dir, install_root, installed_cef_root};
pub use package::{PackageError, package_directory};
pub use profile::{cache_root, profile_dir, PROFILE_HASH_HEX_DIGITS};
#[cfg(target_os = "macos")]
pub use platform::link_unbundled_angle_libraries;
pub use signing::{
    SignConfig, SigningError, osslsigncode_sign_args, sign_artifact, sign_file, sign_tree,
    signtool_sign_args, signtool_verify_args, verify_signature,
};
