//! What Kurogane turns off in Chromium for every application.
//!
//! A default is a preference where one gates the work, a feature turned off
//! where none does and a switch where neither exists. An application turns a
//! preference back on with [`App::chromium_pref`](crate::App::chromium_pref),
//! a feature with an `enable-features` entry and a switch with
//! [`App::without_chromium_flag`](crate::App::without_chromium_flag).
//! Preferences are declarative. One set at an earlier start and set no more
//! returns to Chromium's default.

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

/// Switches Kurogane sets for work neither a preference nor a feature gates.
const SWITCHES: [&str; 1] = [
    // Chrome's stack-sampling profiler which runs in CEF's builds as in a
    // development build and keeps the browser, GPU and network processes
    // busy at rest; CEF uploads none of its samples
    "disable-stack-profiler",
];

/// Global preference recording the preferences Kurogane set at the previous start
const SET_LAST_START: &str = "kurogane.chromium_preferences";

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
/// `enable-features` entry turns it on, and sets its switches.
pub(crate) fn apply_flag_defaults(flags: &mut ChromiumFlags) {
    for feature in DISABLED_FEATURES {
        flags.disable_by_default(feature);
    }
    for name in SWITCHES {
        flags.set(name);
    }
}

/// Registers global preference [`SET_LAST_START`].
pub(crate) fn register_preferences(
    kind: PreferencesType,
    registrar: Option<&mut PreferenceRegistrar>,
) {
    if kind != PreferencesType::GLOBAL {
        return;
    }
    let (Some(registrar), Some(mut empty), Some(mut list)) =
        (registrar, value_create(), list_value_create())
    else {
        return;
    };
    empty.set_list(Some(&mut list));
    if registrar.add_preference(Some(&CefString::from(SET_LAST_START)), Some(&mut empty)) == 0 {
        warn!("Chromium refused Kurogane's preference {SET_LAST_START}");
    }
}

/// Sets Kurogane's preferences and the application's (`chosen`), each once
/// and the application's in place of Kurogane's. One set at the last start
/// and set no more returns to Chromium's default. One Chromium reads only as
/// the profile starts takes effect at the next start.
pub(crate) fn apply_preferences(chosen: &[(String, Json)]) {
    let wanted = resolved(default_preferences(), chosen);

    for name in set_last_start() {
        if !wanted.iter().any(|(wanted, _)| *wanted == name) {
            set_preference(&name, &Json::Null);
        }
    }
    for (name, value) in &wanted {
        set_preference(name, value);
    }

    let set: Vec<Json> = wanted
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(name, _)| Json::from(name.as_str()))
        .collect();
    set_preference(SET_LAST_START, &Json::Array(set));
}

/// Returns `defaults` with `chosen` in their place, each name once with the
/// last value given for it.
fn resolved(
    defaults: impl IntoIterator<Item = (&'static str, Json)>,
    chosen: &[(String, Json)],
) -> Vec<(String, Json)> {
    let mut wanted: Vec<(String, Json)> = defaults
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
    for (name, value) in chosen {
        match wanted.iter_mut().find(|(wanted, _)| wanted == name) {
            Some((_, held)) => *held = value.clone(),
            None => wanted.push((name.clone(), value.clone())),
        }
    }
    wanted
}

/// Returns the names of the preferences Kurogane set at the last start.
fn set_last_start() -> Vec<String> {
    let Some(list) = preference_manager_get_global()
        .and_then(|global| global.preference(Some(&CefString::from(SET_LAST_START))))
        .and_then(|value| value.list())
    else {
        return Vec::new();
    };
    (0..list.size())
        .map(|index| CefString::from(&list.string(index)).to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

/// Sets a Chromium preference, the profile's when the profile has one of
/// that name, else Chromium's global one ("Local State"). A null returns it
/// to Chromium's default.
fn set_preference(name: &str, value: &Json) {
    let mut cef = cef_value(value);
    if cef.is_none() && !value.is_null() {
        warn!("Chromium's preference {name} takes no {value}");
        return;
    }
    let key = CefString::from(name);
    let mut error = CefString::default();

    let set = if let Some(context) = request_context_get_global_context()
        && context.has_preference(Some(&key)) != 0
    {
        context.set_preference(Some(&key), cef.as_mut(), Some(&mut error))
    } else if let Some(global) = preference_manager_get_global()
        && global.has_preference(Some(&key)) != 0
    {
        global.set_preference(Some(&key), cef.as_mut(), Some(&mut error))
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
    fn every_default_flag_is_set() {
        let mut flags = ChromiumFlags::default();
        apply_flag_defaults(&mut flags);
        let shown = flags.to_string();
        for feature in DISABLED_FEATURES {
            assert!(shown.contains(feature), "{feature} is not off: {shown}");
        }
        for name in SWITCHES {
            assert!(flags.contains(name), "{name} is not set: {shown}");
        }
    }

    #[test]
    fn the_application_s_preferences_take_the_defaults_place() {
        let chosen = [
            ("b".to_owned(), Json::from(false)),
            ("c".to_owned(), Json::from(1)),
            ("b".to_owned(), Json::Null),
        ];

        assert_eq!(
            resolved([("a", Json::from(1)), ("b", Json::from(true))], &chosen),
            [
                ("a".to_owned(), Json::from(1)),
                ("b".to_owned(), Json::Null),
                ("c".to_owned(), Json::from(1)),
            ]
        );
    }
}
