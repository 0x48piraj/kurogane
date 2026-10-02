//! What a macOS bundle declares, resolved from `[macos]` once, when bundling
//! starts: the oldest macOS it runs on, its category, and why it uses each
//! device a page can ask for.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::config::{CONFIG_FILE_NAME, MacosPackagingConfig, MacosPrivacyConfig};
use crate::plist::Dict;

/// The oldest macOS CEF runs on: `CEF_TARGET_SDK` in CEF's
/// `cmake/cef_variables.cmake`. Raise it with a CEF update that raises CEF's.
pub const CEF_MINIMUM_MACOS: MacosVersion = MacosVersion([12, 0, 0]);

/// The CEF whose `CEF_TARGET_SDK` [`CEF_MINIMUM_MACOS`] was read from; a
/// test fails once the CEF Kurogane builds with is another.
#[cfg(test)]
const CEF_MINIMUM_MACOS_CHECKED_FOR: &str = "150.0.10";

/// The category of an application whose `[macos]` names none.
const DEFAULT_CATEGORY: &str = "public.app-category.utilities";

/// Apple's `LSApplicationCategoryType` values.
const CATEGORIES: [&str; 40] = [
    "public.app-category.business",
    "public.app-category.developer-tools",
    "public.app-category.education",
    "public.app-category.entertainment",
    "public.app-category.finance",
    "public.app-category.games",
    "public.app-category.graphics-design",
    "public.app-category.healthcare-fitness",
    "public.app-category.lifestyle",
    "public.app-category.medical",
    "public.app-category.music",
    "public.app-category.news",
    "public.app-category.photography",
    "public.app-category.productivity",
    "public.app-category.reference",
    "public.app-category.social-networking",
    "public.app-category.sports",
    "public.app-category.travel",
    "public.app-category.utilities",
    "public.app-category.video",
    "public.app-category.weather",
    "public.app-category.action-games",
    "public.app-category.adventure-games",
    "public.app-category.arcade-games",
    "public.app-category.board-games",
    "public.app-category.card-games",
    "public.app-category.casino-games",
    "public.app-category.dice-games",
    "public.app-category.educational-games",
    "public.app-category.family-games",
    "public.app-category.kids-games",
    "public.app-category.music-games",
    "public.app-category.puzzle-games",
    "public.app-category.racing-games",
    "public.app-category.role-playing-games",
    "public.app-category.simulation-games",
    "public.app-category.sports-games",
    "public.app-category.strategy-games",
    "public.app-category.trivia-games",
    "public.app-category.word-games",
];

/// A device a page can ask for, which macOS gives only to an application
/// that says why, and under the hardened runtime only with an entitlement.
/// A device more is one entry here and one key in `[macos.privacy]`.
pub struct PrivacyUse {
    /// Its key in `[macos.privacy]`.
    pub key: &'static str,
    /// The `Info.plist` key whose text macOS shows when it asks the user.
    pub usage_key: &'static str,
    /// The entitlement a hardened-runtime signature needs for it.
    pub entitlement: &'static str,
    /// What macOS shows when the application names no reason of its own.
    pub default_reason: &'static str,
}

/// Every device Kurogane declares. A page's `getUserMedia` reaches them, and
/// macOS is reported to end an application that asks without the reason.
pub const PRIVACY_USES: [PrivacyUse; 2] = [
    PrivacyUse {
        key: "camera",
        usage_key: "NSCameraUsageDescription",
        entitlement: "com.apple.security.device.camera",
        default_reason: "A page in this application asked to use the camera.",
    },
    PrivacyUse {
        key: "microphone",
        usage_key: "NSMicrophoneUsageDescription",
        entitlement: "com.apple.security.device.audio-input",
        default_reason: "A page in this application asked to use the microphone.",
    },
];

/// Code-signing entitlements CEF needs under the hardened runtime: V8's JIT,
/// executable memory for generated code, and loading the ANGLE and
/// SwiftShader libraries, which Apple did not sign.
const CEF_ENTITLEMENTS: [&str; 3] = [
    "com.apple.security.cs.allow-jit",
    "com.apple.security.cs.allow-unsigned-executable-memory",
    "com.apple.security.cs.disable-library-validation",
];

/// A macOS version, as `LSMinimumSystemVersion` writes it: one to three
/// numbers, missing ones zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacosVersion([u32; 3]);

impl FromStr for MacosVersion {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let mut parts = [0; 3];
        for (index, part) in text.split('.').enumerate() {
            let slot = parts.get_mut(index).ok_or(())?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(());
            }
            *slot = part.parse().map_err(drop)?;
        }
        Ok(MacosVersion(parts))
    }
}

impl fmt::Display for MacosVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.0;
        if patch == 0 {
            write!(f, "{major}.{minor}")
        } else {
            write!(f, "{major}.{minor}.{patch}")
        }
    }
}

/// A `[macos]` value the bundle cannot declare.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum MacosConfigError {
    #[error(
        "[macos] minimum-system-version = {0:?} in {CONFIG_FILE_NAME} is not a version such as \"13.0\""
    )]
    InvalidVersion(String),

    #[error(
        "[macos] minimum-system-version = {0:?} in {CONFIG_FILE_NAME} is older than macOS {CEF_MINIMUM_MACOS}, the oldest CEF runs on"
    )]
    BelowCefMinimum(String),

    #[error(
        "[macos] category = {0:?} in {CONFIG_FILE_NAME} is not one of Apple's application categories, such as \"public.app-category.productivity\""
    )]
    UnknownCategory(String),

    #[error("[macos.privacy] {0} in {CONFIG_FILE_NAME} is empty")]
    EmptyReason(&'static str),

    #[error("[macos.privacy] {0} in {CONFIG_FILE_NAME} holds a control character")]
    ControlInReason(&'static str),
}

/// What a macOS bundle declares, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosSettings {
    pub minimum_system_version: MacosVersion,
    pub category: String,
    /// Each device in [`PRIVACY_USES`] order, with the reason shown for it.
    pub reasons: Vec<(&'static PrivacyUse, String)>,
}

impl fmt::Debug for PrivacyUse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key)
    }
}

impl PartialEq for PrivacyUse {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for PrivacyUse {}

impl MacosSettings {
    /// Checks `[macos]` and fills in what it leaves out.
    pub fn resolve(config: &MacosPackagingConfig) -> Result<Self, MacosConfigError> {
        let minimum_system_version = match &config.minimum_system_version {
            None => CEF_MINIMUM_MACOS,
            Some(text) => {
                let version: MacosVersion = text
                    .parse()
                    .map_err(|()| MacosConfigError::InvalidVersion(text.clone()))?;
                if version < CEF_MINIMUM_MACOS {
                    return Err(MacosConfigError::BelowCefMinimum(text.clone()));
                }
                version
            }
        };

        let category = match &config.category {
            None => DEFAULT_CATEGORY.to_string(),
            Some(category) if CATEGORIES.contains(&category.as_str()) => category.clone(),
            Some(category) => return Err(MacosConfigError::UnknownCategory(category.clone())),
        };

        let mut reasons = Vec::with_capacity(PRIVACY_USES.len());
        for privacy in &PRIVACY_USES {
            let reason = match configured_reason(&config.privacy, privacy) {
                None => privacy.default_reason.to_string(),
                Some(reason) if reason.trim().is_empty() => {
                    return Err(MacosConfigError::EmptyReason(privacy.key));
                }
                Some(reason) if reason.chars().any(char::is_control) => {
                    return Err(MacosConfigError::ControlInReason(privacy.key));
                }
                Some(reason) => reason.clone(),
            };
            reasons.push((privacy, reason));
        }

        Ok(Self {
            minimum_system_version,
            category,
            reasons,
        })
    }

    /// Sets the keys the application's `Info.plist` and every helper's
    /// share: the macOS it needs and why it uses each device. A device is
    /// charged to whichever process asks for it, a helper too.
    pub fn declare(&self, plist: &mut Dict) {
        plist.set(
            "LSMinimumSystemVersion",
            self.minimum_system_version.to_string(),
        );
        for (privacy, reason) in &self.reasons {
            plist.set(privacy.usage_key, reason.as_str());
        }
    }

    /// The entitlements a hardened-runtime signature of the application and
    /// its helpers carries: CEF's, and every declared device's.
    pub fn entitlements(&self) -> Dict {
        let mut entitlements = Dict::new();
        for entitlement in CEF_ENTITLEMENTS {
            entitlements.set(entitlement, true);
        }
        for (privacy, _) in &self.reasons {
            entitlements.set(privacy.entitlement, true);
        }
        entitlements
    }
}

/// The reason `[macos.privacy]` gives for `privacy`, read by its key. A
/// device in [`PRIVACY_USES`] without an arm here fails the tests; it never
/// takes another device's reason.
fn configured_reason<'a>(
    config: &'a MacosPrivacyConfig,
    privacy: &PrivacyUse,
) -> Option<&'a String> {
    match privacy.key {
        "camera" => config.camera.as_ref(),
        "microphone" => config.microphone.as_ref(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plist::Value;

    fn config(version: Option<&str>, category: Option<&str>) -> MacosPackagingConfig {
        MacosPackagingConfig {
            minimum_system_version: version.map(str::to_string),
            category: category.map(str::to_string),
            privacy: MacosPrivacyConfig::default(),
        }
    }

    #[test]
    fn an_empty_table_declares_cef_s_floor_utilities_and_every_device() {
        let settings = MacosSettings::resolve(&MacosPackagingConfig::default()).unwrap();

        assert_eq!(settings.minimum_system_version.to_string(), "12.0");
        assert_eq!(settings.category, "public.app-category.utilities");
        let mut plist = Dict::new();
        settings.declare(&mut plist);
        assert_eq!(
            plist.get("LSMinimumSystemVersion"),
            Some(&Value::from("12.0"))
        );
        assert_eq!(
            plist.get("NSCameraUsageDescription"),
            Some(&Value::from(
                "A page in this application asked to use the camera."
            ))
        );
        assert_eq!(
            plist.get("NSMicrophoneUsageDescription"),
            Some(&Value::from(
                "A page in this application asked to use the microphone."
            ))
        );
    }

    #[test]
    fn configured_values_replace_the_defaults() {
        let mut macos = config(Some("13.4.1"), Some("public.app-category.video"));
        macos.privacy.camera = Some("Calls use your camera.".into());

        let settings = MacosSettings::resolve(&macos).unwrap();

        assert_eq!(settings.minimum_system_version.to_string(), "13.4.1");
        assert_eq!(settings.category, "public.app-category.video");
        let mut plist = Dict::new();
        settings.declare(&mut plist);
        assert_eq!(
            plist.get("NSCameraUsageDescription"),
            Some(&Value::from("Calls use your camera."))
        );
        assert!(matches!(
            plist.get("NSMicrophoneUsageDescription"),
            Some(Value::String(reason)) if reason.contains("microphone")
        ));
    }

    #[test]
    fn the_macos_floor_was_read_from_this_cef() {
        assert_eq!(
            env!("KUROGANE_CEF_VERSION"),
            CEF_MINIMUM_MACOS_CHECKED_FOR,
            "CEF changed: read CEF_TARGET_SDK in the new CEF's cmake/cef_variables.cmake, \
             set CEF_MINIMUM_MACOS to it, then CEF_MINIMUM_MACOS_CHECKED_FOR to the new CEF"
        );
    }

    #[test]
    fn versions_compare_by_number_and_never_go_below_cef_s() {
        for (text, expected) in [("12", "12.0"), ("12.0.0", "12.0"), ("15.10", "15.10")] {
            let settings = MacosSettings::resolve(&config(Some(text), None)).unwrap();
            assert_eq!(settings.minimum_system_version.to_string(), expected);
        }
        // 12.10 is newer than 12.9, not older as text would have it
        assert!("12.10".parse::<MacosVersion>() > "12.9".parse::<MacosVersion>());

        for text in ["11.7", "10.15", "11"] {
            assert_eq!(
                MacosSettings::resolve(&config(Some(text), None)),
                Err(MacosConfigError::BelowCefMinimum(text.into())),
                "{text}"
            );
        }
        for text in [
            "",
            "thirteen",
            "13..0",
            "13.0.",
            "1.2.3.4",
            "13.0-beta",
            "+13",
            " 13",
        ] {
            assert_eq!(
                MacosSettings::resolve(&config(Some(text), None)),
                Err(MacosConfigError::InvalidVersion(text.into())),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_category_must_be_one_of_apple_s() {
        for category in CATEGORIES {
            assert!(MacosSettings::resolve(&config(None, Some(category))).is_ok());
        }
        for category in [
            "photography",
            "public.app-category.Photography",
            "public.app-category.photo-editing",
            "public.app-category.",
        ] {
            assert_eq!(
                MacosSettings::resolve(&config(None, Some(category))),
                Err(MacosConfigError::UnknownCategory(category.into()))
            );
        }
    }

    #[test]
    fn a_reason_must_say_something_on_one_line() {
        let mut macos = MacosPackagingConfig::default();
        macos.privacy.microphone = Some("  ".into());
        assert_eq!(
            MacosSettings::resolve(&macos),
            Err(MacosConfigError::EmptyReason("microphone"))
        );

        macos.privacy.microphone = Some("Calls\nand meetings".into());
        assert_eq!(
            MacosSettings::resolve(&macos),
            Err(MacosConfigError::ControlInReason("microphone"))
        );
    }

    #[test]
    fn signed_bundles_carry_cef_s_entitlements_and_every_device_s() {
        let settings = MacosSettings::resolve(&MacosPackagingConfig::default()).unwrap();
        let entitlements = settings.entitlements();

        for key in CEF_ENTITLEMENTS.into_iter().chain([
            "com.apple.security.device.camera",
            "com.apple.security.device.audio-input",
        ]) {
            assert_eq!(entitlements.get(key), Some(&Value::Bool(true)), "{key}");
        }
    }

    #[test]
    fn every_device_reads_its_own_key_in_the_privacy_table() {
        // A device added to PRIVACY_USES needs a [macos.privacy] key of the
        // same name, and an arm in configured_reason
        let table = toml::to_string(&toml::Table::from_iter(PRIVACY_USES.iter().map(
            |privacy| {
                let reason = format!("why the {}", privacy.key);
                (privacy.key.to_string(), toml::Value::from(reason))
            },
        )))
        .unwrap();
        let privacy: MacosPrivacyConfig = toml::from_str(&table).unwrap();
        let macos = MacosPackagingConfig {
            privacy,
            ..MacosPackagingConfig::default()
        };

        let settings = MacosSettings::resolve(&macos).unwrap();
        for (privacy, reason) in &settings.reasons {
            assert_eq!(*reason, format!("why the {}", privacy.key));
        }
        assert_eq!(settings.reasons.len(), PRIVACY_USES.len());
    }
}
