//! Chromium command-line construction.
//!
//! This module provides a normalized intermediate representation for
//! Chromium command-line switches. Each switch is keyed as Chromium keys
//! it, so every spelling of one switch is one entry. The feature lists are
//! added to, never replaced: CEF's own entries, the launch's and every
//! setting's stay.

use cef::*;
use std::collections::BTreeMap;

/// User supplied Chromium standalone switches and switches with values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChromiumFlag {
    Present(String),
    WithValue(String, String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SwitchValue {
    Present,
    Value(String),
}

/// The switches Chromium reads as comma-separated lists of features.
const LIST_SWITCHES: [&str; 2] = ["disable-features", "enable-features"];

/// Chromium switch plan with last-write-wins precedence model, but for the
/// list switches, which add.
#[derive(Default, Debug)]
pub(crate) struct ChromiumFlags {
    switches: BTreeMap<String, SwitchValue>,
    // The list switches' entries, each once, in the order added
    lists: BTreeMap<String, Vec<String>>,
}

fn is_list(key: &str) -> bool {
    LIST_SWITCHES.contains(&key)
}

/// Adds the entries of `value`, a comma-separated list, to `entries`, each
/// once. Chromium trims each entry and skips empty ones.
fn add_entries(entries: &mut Vec<String>, value: &str) {
    for entry in value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if !entries.iter().any(|known| known == entry) {
            entries.push(entry.to_owned());
        }
    }
}

/// The list `existing` (the command line's value) with `added`'s entries
/// after its own, each once.
fn merged(existing: &str, added: &[String]) -> String {
    let mut entries = Vec::new();
    add_entries(&mut entries, existing);
    for entry in added {
        add_entries(&mut entries, entry);
    }
    entries.join(",")
}

/// The name Chromium files a switch under. base::CommandLine drops one
/// leading `--` or `-` (or `/` on Windows) and lowercases the name on
/// Windows only.
fn key(name: &str) -> String {
    let stripped = name.strip_prefix("--").or_else(|| name.strip_prefix('-'));

    #[cfg(target_os = "windows")]
    let stripped = stripped.or_else(|| name.strip_prefix('/'));

    let key = stripped.unwrap_or(name);

    if cfg!(target_os = "windows") {
        key.to_ascii_lowercase()
    } else {
        key.to_owned()
    }
}

impl ChromiumFlags {
    /// Insert a standalone switch. A list switch without a value adds no
    /// entry.
    pub(crate) fn set(&mut self, name: impl AsRef<str>) {
        let name = key(name.as_ref());
        if is_list(&name) {
            self.lists.entry(name).or_default();
        } else {
            self.switches.insert(name, SwitchValue::Present);
        }
    }

    /// Insert a switch with a value. A list switch's value adds its
    /// entries to the list.
    pub(crate) fn set_with_value(&mut self, name: impl AsRef<str>, value: impl Into<String>) {
        let name = key(name.as_ref());
        if is_list(&name) {
            add_entries(self.lists.entry(name).or_default(), &value.into());
        } else {
            self.switches.insert(name, SwitchValue::Value(value.into()));
        }
    }

    /// Returns whether a switch is present, with or without a value.
    pub(crate) fn contains(&self, name: &str) -> bool {
        let name = key(name);
        self.switches.contains_key(&name) || self.lists.contains_key(&name)
    }

    /// Apply user-supplied Chromium flags.
    ///
    /// User flags are applied after runtime policies and therefore
    /// override runtime defaults for the same switch, however it is spelled;
    /// a list switch's entries are added to the runtime's.
    pub(crate) fn extend_user_flags(&mut self, user_flags: &[ChromiumFlag]) {
        for flag in user_flags {
            match flag {
                ChromiumFlag::Present(name) => self.set(name),
                ChromiumFlag::WithValue(name, value) => {
                    self.set_with_value(name, value.clone());
                }
            }
        }
    }

    /// Emit the finalized switch set into CEF.
    ///
    /// This is the only place where ChromiumFlags interacts with
    /// CommandLine directly. A list switch's entries go after those the
    /// command line holds already (CEF's own and the launch's), since
    /// Chromium keeps only the last value of a switch.
    pub(crate) fn apply(self, cmd: &mut CommandLine) {
        for (name, added) in self.lists {
            if added.is_empty() {
                continue;
            }
            let name = CefString::from(name.as_str());
            let existing = if cmd.has_switch(Some(&name)) != 0 {
                CefString::from(&cmd.switch_value(Some(&name))).to_string()
            } else {
                String::new()
            };
            let value = CefString::from(merged(&existing, &added).as_str());
            cmd.append_switch_with_value(Some(&name), Some(&value));
        }

        for (name, value) in self.switches {
            let name = CefString::from(name.as_str());

            match value {
                SwitchValue::Present => {
                    cmd.append_switch(Some(&name));
                }
                SwitchValue::Value(value) => {
                    let value = CefString::from(value.as_str());
                    cmd.append_switch_with_value(Some(&name), Some(&value));
                }
            }
        }
    }
}

impl std::fmt::Display for ChromiumFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, value) in &self.switches {
            match value {
                SwitchValue::Present => {
                    writeln!(f, "--{name}")?;
                }

                SwitchValue::Value(v) => {
                    writeln!(f, "--{name}={v}")?;
                }
            }
        }

        for (name, entries) in &self.lists {
            writeln!(f, "--{name}=+{}", entries.join(","))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Normalization and precedence tests

    #[test]
    fn duplicate_set_keeps_single_effective_switch() {
        let mut flags = ChromiumFlags::default();

        flags.set("disable-gpu");
        flags.set("disable-gpu");

        assert_eq!(
            flags.switches.get("disable-gpu"),
            Some(&SwitchValue::Present)
        );
    }

    #[test]
    fn last_assignment_wins() {
        let mut flags = ChromiumFlags::default();

        flags.set_with_value("use-gl", "angle");
        flags.set_with_value("use-gl", "egl");

        assert_eq!(
            flags.switches.get("use-gl"),
            Some(&SwitchValue::Value("egl".into()))
        );
    }

    #[test]
    fn value_replaces_flag() {
        let mut flags = ChromiumFlags::default();

        flags.set("disable-gpu");

        flags.set_with_value("disable-gpu", "ignored");

        assert_eq!(
            flags.switches.get("disable-gpu"),
            Some(&SwitchValue::Value("ignored".into()))
        );
    }

    #[test]
    fn flag_replaces_existing_value() {
        let mut flags = ChromiumFlags::default();

        flags.set_with_value("foo", "bar");
        flags.set("foo");

        assert_eq!(flags.switches.get("foo"), Some(&SwitchValue::Present));
    }

    #[test]
    fn contains_reports_switches_with_and_without_values() {
        let mut flags = ChromiumFlags::default();

        flags.set("no-sandbox");
        flags.set_with_value("use-gl", "egl");

        assert!(flags.contains("no-sandbox"));
        assert!(flags.contains("use-gl"));
        assert!(!flags.contains("disable-gpu"));
    }

    #[test]
    fn a_prefix_does_not_make_another_switch() {
        let mut flags = ChromiumFlags::default();

        flags.set("no-sandbox");
        flags.set("--no-sandbox");
        flags.set("-no-sandbox");

        assert_eq!(flags.switches.len(), 1);
        assert!(flags.contains("no-sandbox"));
        assert!(flags.contains("--no-sandbox"));
    }

    #[test]
    fn a_prefixed_user_flag_overrides_the_runtime_value() {
        let mut flags = ChromiumFlags::default();

        flags.set_with_value("use-gl", "angle");
        flags.extend_user_flags(&[ChromiumFlag::WithValue("--use-gl".into(), "egl".into())]);

        assert_eq!(flags.switches.len(), 1);
        assert_eq!(
            flags.switches.get("use-gl"),
            Some(&SwitchValue::Value("egl".into()))
        );
    }

    #[test]
    fn a_switch_is_printed_with_one_prefix() {
        let mut flags = ChromiumFlags::default();

        flags.set("--no-sandbox");
        flags.set_with_value("-use-gl", "egl");

        assert_eq!(flags.to_string(), "--no-sandbox\n--use-gl=egl\n");
    }

    #[test]
    fn a_feature_list_adds_entries_however_the_switch_is_spelled() {
        let mut flags = ChromiumFlags::default();

        flags.set_with_value("disable-features", "A,B");
        flags.set_with_value("--disable-features", " B , C,,");
        flags.extend_user_flags(&[ChromiumFlag::WithValue(
            "disable-features".into(),
            "D".into(),
        )]);

        assert_eq!(flags.lists["disable-features"], ["A", "B", "C", "D"]);
        assert!(flags.switches.is_empty());
    }

    #[test]
    fn a_feature_list_without_a_value_adds_nothing_and_removes_nothing() {
        let mut flags = ChromiumFlags::default();

        flags.set("enable-features");
        assert!(flags.contains("enable-features"));
        assert!(flags.lists["enable-features"].is_empty());

        flags.set_with_value("enable-features", "A");
        flags.extend_user_flags(&[ChromiumFlag::Present("--enable-features".into())]);
        assert_eq!(flags.lists["enable-features"], ["A"]);
    }

    #[test]
    fn the_command_line_s_entries_stay_first_and_each_entry_is_once() {
        let cef = "GlicActorUi,AutofillActorMode,LensOverlay,KillOnInvalidNavigationHeaders";

        assert_eq!(
            merged(cef, &["LcApp".into(), "LensOverlay".into()]),
            format!("{cef},LcApp")
        );
        assert_eq!(merged("", &["A".into()]), "A");
        assert_eq!(merged("A,B", &[]), "A,B");
    }

    #[test]
    fn a_feature_list_is_printed_as_an_addition() {
        let mut flags = ChromiumFlags::default();

        flags.set_with_value("enable-features", "A,B");

        assert_eq!(flags.to_string(), "--enable-features=+A,B\n");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_switch_names_ignore_case_and_take_a_slash() {
        let mut flags = ChromiumFlags::default();

        flags.set("/NO-SANDBOX");
        flags.set_with_value("use-gl", "angle");
        flags.set_with_value("--Use-GL", "egl");

        assert!(flags.contains("no-sandbox"));
        assert_eq!(flags.switches.len(), 2);
        assert_eq!(
            flags.switches.get("use-gl"),
            Some(&SwitchValue::Value("egl".into()))
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn other_platforms_keep_the_case_and_no_slash_prefix() {
        let mut flags = ChromiumFlags::default();

        // Chromium keeps both as written, so neither is no-sandbox
        flags.set("--No-Sandbox");
        flags.set("/no-sandbox");

        assert!(!flags.contains("no-sandbox"));
        assert!(flags.contains("No-Sandbox"));
        assert!(flags.contains("/no-sandbox"));
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // Generated names never start with '-', which Chromium strips as a
    // prefix, and are never a list switch, which adds instead

    proptest! {
        #[test]
        fn last_write_wins(
            key in "[a-z0-9][a-z0-9\\-]{0,31}".prop_filter("adds", |k| !is_list(k)),
            first in ".*",
            second in ".*",
        ) {
            let mut flags = ChromiumFlags::default();

            flags.set_with_value(
                key.clone(),
                first,
            );

            flags.set_with_value(
                key.clone(),
                second.clone(),
            );

            prop_assert_eq!(
                flags.switches.get(&key),
                Some(&SwitchValue::Value(second))
            );
        }
    }

    proptest! {
        #[test]
        fn user_flags_always_override_runtime_values(
            key in "[a-z0-9][a-z0-9\\-]{0,31}".prop_filter("adds", |k| !is_list(k)),
            runtime in ".*",
            user in ".*",
        ) {
            let mut flags = ChromiumFlags::default();

            flags.set_with_value(key.clone(), runtime);

            flags.extend_user_flags(&[
                ChromiumFlag::WithValue(
                    key.clone(),
                    user.clone(),
                )
            ]);

            prop_assert_eq!(
                flags.switches.get(&key),
                Some(&SwitchValue::Value(user))
            );
        }
    }

    proptest! {
        #[test]
        fn intermediate_assignments_do_not_affect_final_state(
            key in "[a-z0-9][a-z0-9\\-]{0,31}".prop_filter("adds", |k| !is_list(k)),
            a in ".*",
            b in ".*",
            c in ".*",
        ) {
            let mut flags = ChromiumFlags::default();

            flags.set_with_value(key.clone(), a);
            flags.set_with_value(key.clone(), b);
            flags.set_with_value(key.clone(), c.clone());

            prop_assert_eq!(
                flags.switches.get(&key),
                Some(&SwitchValue::Value(c))
            );
        }
    }

    proptest! {
        #[test]
        fn number_of_switches_equals_number_of_unique_keys(
            keys in prop::collection::vec(
                "[a-z0-9][a-z0-9\\-]{0,15}".prop_filter("adds", |k| !is_list(k)),
                0..50
            )
        ) {
            let mut flags = ChromiumFlags::default();

            for key in &keys {
                flags.set(key.clone());
            }

            let unique: std::collections::HashSet<_> =
                keys.iter().collect();

            prop_assert_eq!(
                flags.switches.len(),
                unique.len()
            );
        }
    }
}
