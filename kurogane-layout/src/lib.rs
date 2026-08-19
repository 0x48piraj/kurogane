mod discover;
mod layout;
mod platform;
mod profile;
mod validate;
mod package;
mod distribution;
mod bundle;

pub use bundle::BundleLayout;
pub use discover::{DetectError, DetectedCef, DiscoveryMode, detect_cef_root};
pub use distribution::{AppMetadata, DistributionError, ResolvedDistribution};
pub use layout::{bundled_cef_root, cef_install_dir, install_root, installed_cef_root};
pub use package::{PackageError, package_directory};
pub use profile::{cache_root, profile_dir};
pub use validate::{CefValidationError, validate_cef_root};
