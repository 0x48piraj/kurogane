//! What Kurogane turns off in Chromium for every application.
//!
//! A default is a preference where one gates the work and a feature turned
//! off where none does. An application turns a preference back on with
//! [`App::chromium_pref`](crate::App::chromium_pref) and a feature with an
//! `enable-features` entry.

use serde_json::Value as Json;
use tanso::*;
use tracing::warn;

use crate::chromium_flags::ChromiumFlags;

/// Features Kurogane turns off, none of which a preference gates.
const DISABLED_FEATURES: [&str; 3] = [
    // The omnibox's AI Mode check, a request to Google with its cookies at every start
    "AimEnabled",
    // Cast, which searches the local network for devices at every start
    "MediaRouter",
    // A renderer kept ready for the next site, a process the application never uses
    "SpareRendererForSitePerProcess",
];

/// Chrome's `session.restore_on_startup` value for starting without the
/// last session ([`SessionStartupPref::kPrefValueNewTab`](https://source.chromium.org/chromium/chromium/src/+/main:chrome/browser/prefs/session_startup_pref.h)).
const START_WITHOUT_LAST_SESSION: i32 = 5;

/// Preferences Kurogane sets on every start, before the application's.
fn default_preferences() -> [(&'static str, Json); 5] {
    [
        // Kurogane opens its own windows; a restored session would reopen
        // stale ones after an unclean exit
        (
            "session.restore_on_startup",
            Json::from(START_WITHOUT_LAST_SESSION),
        ),
        // The bubble of finished downloads would open over the page, even
        // for a file the application saved itself
        ("download_bubble.partial_view_enabled", Json::from(false)),
        // Extensions other programs register for Google Chrome, which
        // Chromium would fetch into the profile and keep updated
        ("extensions.block_external_extensions", Json::from(true)),
        // Component downloads; an application that needs Widevine turns
        // them back on
        (
            "component_updates.component_updates_enabled",
            Json::from(false),
        ),
        // Queries to Google's time server
        (
            "network_time.network_time_queries_enabled",
            Json::from(false),
        ),
    ]
}

/// Turns Kurogane's features off in `flags`, each unless an
/// `enable-features` entry turns it on.
pub(crate) fn apply_feature_defaults(flags: &mut ChromiumFlags) {
    for feature in DISABLED_FEATURES {
        flags.disable_by_default(feature);
    }
}

/// Sets Kurogane's preferences, then `chosen`, the application's, which
/// override them. Preferences apply once the profile exists, so one that
/// Chromium reads only as the profile starts takes effect at the next start.
pub(crate) fn apply_preferences(chosen: &[(String, Json)]) {
    for (name, value) in default_preferences() {
        set_preference(name, &value);
    }
    for (name, value) in chosen {
        set_preference(name, value);
    }
}

/// Sets a Chromium preference, the profile's when the profile has one of
/// that name, else Chromium's global one ("Local State").
fn set_preference(name: &str, value: &Json) {
    let Some(mut cef) = cef_value(value) else {
        warn!("Chromium's preference {name} takes no {value}");
        return;
    };
    let key = CefString::from(name);
    let mut error = CefString::default();

    let set = if let Some(context) = request_context_get_global_context()
        && context.has_preference(Some(&key)) != 0
    {
        context.set_preference(Some(&key), Some(&mut cef), Some(&mut error))
    } else if let Some(global) = preference_manager_get_global()
        && global.has_preference(Some(&key)) != 0
    {
        global.set_preference(Some(&key), Some(&mut cef), Some(&mut error))
    } else {
        warn!("Chromium has no preference {name}");
        return;
    };
    if set == 0 {
        warn!("failed to set Chromium's {name}: {error}");
    }
}

/// Returns `json` as a CEF value. `None` for a null, which no preference
/// holds, or a value CEF cannot create.
fn cef_value(json: &Json) -> Option<Value> {
    let value = value_create()?;
    match json {
        Json::Null => return None,
        Json::Bool(b) => value.set_bool(i32::from(*b)),
        Json::Number(n) => match n.as_i64().and_then(|n| i32::try_from(n).ok()) {
            Some(int) => value.set_int(int),
            None => value.set_double(n.as_f64()?),
        },
        Json::String(s) => value.set_string(Some(&CefString::from(s.as_str()))),
        Json::Array(items) => {
            let mut list = list_value_create()?;
            for (index, item) in items.iter().enumerate() {
                list.set_value(index, Some(&mut cef_value(item)?));
            }
            value.set_list(Some(&mut list))
        }
        Json::Object(entries) => {
            let mut dictionary = dictionary_value_create()?;
            for (key, item) in entries {
                dictionary.set_value(
                    Some(&CefString::from(key.as_str())),
                    Some(&mut cef_value(item)?),
                );
            }
            value.set_dictionary(Some(&mut dictionary))
        }
    };
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_feature_is_off_until_an_entry_turns_it_on() {
        let mut flags = ChromiumFlags::default();
        apply_feature_defaults(&mut flags);
        let shown = flags.to_string();
        for feature in DISABLED_FEATURES {
            assert!(shown.contains(feature), "{feature} is not off: {shown}");
        }
    }
}
