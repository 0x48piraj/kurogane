mod discover;
mod layout;
mod platform;
mod profile;
mod validate;
mod bundle;

pub use discover::{DetectError, DetectedCef, DiscoveryMode, detect_cef_root};

pub use layout::{bundled_cef_root, cef_install_dir, install_root, installed_cef_root};

pub use profile::{cache_root, profile_dir};

pub use validate::{CefValidationError, validate_cef_root};

pub use bundle::BundleLayout;
