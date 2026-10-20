//! Chromium command-line construction.
//!
//! This module provides a normalized intermediate representation for
//! Chromium command-line switches. Each switch is keyed as Chromium keys
//! it. Every spelling of one switch is one entry. The feature lists are
//! added to, never replaced. CEF's own entries, the launch's and every
//! setting's stay.

use tanso::*;
use std::collections::BTreeMap;
use tracing::warn;

/// User supplied Chromium switches, standalone, with a value or removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChromiumFlag {
    Present(String),
    WithValue(String, String),
    Removed(String),
}

impl ChromiumFlag {
    /// Switch name as the application wrote it.
    fn name(&self) -> &str {
        match self {
            Self::Present(name) | Self::WithValue(name, _) | Self::Removed(name) => name,
        }
    }
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
    /// Features the runtime turns off unless an `enable-features` entry
    /// turns them on, the application's, the launch's or CEF's.
    disabled_by_default: Vec<String>,
    /// Switches the application removed.
    removed: Vec<String>,
}

fn is_list(key: &str) -> bool {
    LIST_SWITCHES.contains(&key)
}

/// The feature a feature list's entry names. Chromium reads `*Name`,
/// `Name<Study.Group` and `Name:param/value` as `Name`.
fn feature_name(entry: &str) -> &str {
    let entry = entry.strip_prefix('*').unwrap_or(entry);
    entry.split(['<', '.', ':']).next().unwrap_or(entry)
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

    /// Turns `feature` off unless an `enable-features` entry turns it on.
    /// Chromium applies `disable-features` over `enable-features`. A default
    /// that stayed in the list would win over the application.
    pub(crate) fn disable_by_default(&mut self, feature: &str) {
        add_entries(&mut self.disabled_by_default, feature);
    }

    /// Removes the switch `name` with a list switch's entries. Removing
    /// `disable-features` also turns the runtime's default features back on.
    pub(crate) fn remove(&mut self, name: impl AsRef<str>) {
        let name = key(name.as_ref());
        self.switches.remove(&name);
        self.lists.remove(&name);
        if name == "disable-features" {
            self.disabled_by_default.clear();
        }
        if !self.removed.contains(&name) {
            self.removed.push(name);
        }
    }

    /// Returns whether a switch is present, with or without a value.
    pub(crate) fn contains(&self, name: &str) -> bool {
        let name = key(name);
        self.switches.contains_key(&name) || self.lists.contains_key(&name)
    }

    /// Apply user-supplied Chromium flags.
    ///
    /// User flags are applied after runtime policies and in order and therefore
    /// override or remove the runtime's switches however they are
    /// spelled; a list switch's entries are added to the runtime's. A name
    /// holding `=` is skipped, as no switch name holds one.
    pub(crate) fn extend_user_flags(&mut self, user_flags: &[ChromiumFlag]) {
        for flag in user_flags {
            if flag.name().contains('=') {
                warn!(
                    "skipped Chromium flag {}: a switch name holds no '=' (chromium_flag_with_value takes a value)",
                    flag.name()
                );
                continue;
            }
            match flag {
                ChromiumFlag::Present(name) => self.set(name),
                ChromiumFlag::WithValue(name, value) => {
                    self.set_with_value(name, value.clone());
                }
                ChromiumFlag::Removed(name) => self.remove(name),
            }
        }
    }

    /// Emit the finalized switch set into CEF.
    ///
    /// Each switch is then read back through CEF's command line, Chromium's
    /// own parse; one it holds otherwise than passed is logged.
    ///
    /// This is the only place where ChromiumFlags interacts with
    /// CommandLine directly. A list switch's entries go after those the
    /// command line holds already (CEF's own and the launch's), since
    /// Chromium keeps only the last value of a switch.
    pub(crate) fn apply(self, cmd: &mut CommandLine) {
        for name in self.removed_switches() {
            if cmd.has_switch(Some(&CefString::from(name))) != 0 {
                warn!("--{name} stays: the application was launched with it or CEF passes it");
            }
        }

        let existing = |name: &str| {
            let name = CefString::from(name);
            if cmd.has_switch(Some(&name)) != 0 {
                CefString::from(&cmd.switch_value(Some(&name))).to_string()
            } else {
                String::new()
            }
        };
        let mut passed = self.list_values(existing);
        passed.extend(self.switches.into_iter().map(|(name, value)| match value {
            SwitchValue::Present => (name, String::new()),
            SwitchValue::Value(value) => (name, value),
        }));

        for (name, value) in &passed {
            let name = CefString::from(name.as_str());
            if value.is_empty() {
                cmd.append_switch(Some(&name));
            } else {
                cmd.append_switch_with_value(Some(&name), Some(&CefString::from(value.as_str())));
            }
        }

        let held = |name: &str| {
            let name = CefString::from(name);
            (cmd.has_switch(Some(&name)) != 0)
                .then(|| CefString::from(&cmd.switch_value(Some(&name))).to_string())
        };
        for (name, value, held) in unheld(&passed, held) {
            let held = held.map_or_else(|| "no such switch".to_owned(), |held| format!("{held:?}"));
            warn!("Chromium does not hold --{name} as passed ({value:?}); it holds {held}");
        }
    }
}

impl ChromiumFlags {
    /// Returns the switches the application removed and set no more. The
    /// list switches are left out; CEF's and the launch's entries stay.
    fn removed_switches(&self) -> impl Iterator<Item = &str> {
        self.removed
            .iter()
            .filter(|name| !is_list(name) && !self.switches.contains_key(*name))
            .map(String::as_str)
    }

    /// Returns each list switch that gains entries with its whole value,
    /// `existing` giving the value the command line holds. The runtime's
    /// defaults go first in `disable-features`, but for any feature an
    /// `enable-features` entry names, in whatever form Chromium reads.
    fn list_values(&self, existing: impl Fn(&str) -> String) -> Vec<(String, String)> {
        let mut lists = self.lists.clone();

        let mut enabled = Vec::new();
        add_entries(&mut enabled, &existing("enable-features"));
        for entry in lists.get("enable-features").into_iter().flatten() {
            add_entries(&mut enabled, entry);
        }
        let mut disabled: Vec<String> = self
            .disabled_by_default
            .iter()
            .filter(|feature| !enabled.iter().any(|entry| feature_name(entry) == *feature))
            .cloned()
            .collect();
        if !disabled.is_empty() {
            let own = lists.remove("disable-features").unwrap_or_default();
            for entry in &own {
                add_entries(&mut disabled, entry);
            }
            lists.insert("disable-features".to_owned(), disabled);
        }

        lists
            .into_iter()
            .filter(|(_, added)| !added.is_empty())
            .map(|(name, added)| {
                let value = merged(&existing(&name), &added);
                (name, value)
            })
            .collect()
    }
}

/// Returns each switch of `passed` that `held`, the command line read back,
/// holds otherwise, with what it holds.
fn unheld(
    passed: &[(String, String)],
    held: impl Fn(&str) -> Option<String>,
) -> Vec<(&str, &str, Option<String>)> {
    passed
        .iter()
        .filter_map(|(name, value)| {
            let found = held(name);
            (found.as_deref() != Some(value.as_str())).then_some((
                name.as_str(),
                value.as_str(),
                found,
            ))
        })
        .collect()
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

        if !self.disabled_by_default.is_empty() {
            writeln!(
                f,
                "disabled unless enabled: {}",
                self.disabled_by_default.join(",")
            )?;
        }

        for name in self.removed_switches() {
            writeln!(f, "without --{name}")?;
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

    /// The list values `flags` gives a command line that holds `existing`.
    fn lists_over(flags: &ChromiumFlags, existing: &[(&str, &str)]) -> Vec<(String, String)> {
        flags.list_values(|name| {
            existing
                .iter()
                .find(|(key, _)| *key == name)
                .map_or_else(String::new, |(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn defaults_follow_the_command_line_s_disabled_features() {
        let mut flags = ChromiumFlags::default();
        flags.disable_by_default("Spare");
        flags.disable_by_default("Cast");

        assert_eq!(
            lists_over(&flags, &[("disable-features", "CefOwn")]),
            [(
                "disable-features".to_owned(),
                "CefOwn,Spare,Cast".to_owned()
            )]
        );
    }

    #[test]
    fn the_application_turns_a_default_back_on() {
        let mut flags = ChromiumFlags::default();
        flags.disable_by_default("Spare");
        flags.disable_by_default("Cast");
        flags.extend_user_flags(&[
            ChromiumFlag::WithValue("enable-features".into(), "Spare".into()),
            ChromiumFlag::WithValue("disable-features".into(), "Mine".into()),
        ]);

        assert_eq!(
            lists_over(&flags, &[]),
            [
                ("disable-features".to_owned(), "Cast,Mine".to_owned()),
                ("enable-features".to_owned(), "Spare".to_owned()),
            ]
        );
    }

    #[test]
    fn the_launch_turns_a_default_back_on() {
        let mut flags = ChromiumFlags::default();
        flags.disable_by_default("Spare");

        assert!(
            lists_over(&flags, &[("enable-features", "Other,Spare")]).is_empty(),
            "nothing is added once the launch enables the only default"
        );
    }

    #[test]
    fn a_feature_entry_in_any_form_turns_a_default_back_on() {
        let mut flags = ChromiumFlags::default();
        for feature in ["Spare", "Aim", "Cast", "Kept"] {
            flags.disable_by_default(feature);
        }
        flags.extend_user_flags(&[ChromiumFlag::WithValue(
            "enable-features".into(),
            "Spare:lc/1,Aim<Study.Group,*Cast".into(),
        )]);

        assert_eq!(
            lists_over(&flags, &[])[0],
            ("disable-features".to_owned(), "Kept".to_owned())
        );
    }

    #[test]
    fn a_removed_switch_is_gone_until_set_again() {
        let mut flags = ChromiumFlags::default();
        flags.set("disable-stack-profiler");

        flags.extend_user_flags(&[ChromiumFlag::Removed("--disable-stack-profiler".into())]);
        assert!(!flags.contains("disable-stack-profiler"));
        assert_eq!(flags.to_string(), "without --disable-stack-profiler\n");

        flags.extend_user_flags(&[ChromiumFlag::Present("disable-stack-profiler".into())]);
        assert!(flags.contains("disable-stack-profiler"));
    }

    #[test]
    fn removing_disable_features_turns_every_default_back_on() {
        let mut flags = ChromiumFlags::default();
        flags.disable_by_default("Spare");
        flags.set_with_value("disable-features", "Runtime");

        flags.extend_user_flags(&[ChromiumFlag::Removed("disable-features".into())]);

        assert!(lists_over(&flags, &[("disable-features", "CefOwn")]).is_empty());
    }

    #[test]
    fn a_switch_read_back_otherwise_than_passed_is_reported() {
        let passed = [
            ("kept".to_owned(), String::new()),
            ("changed".to_owned(), "a".to_owned()),
            ("lost".to_owned(), "b".to_owned()),
        ];
        let held = |name: &str| match name {
            "kept" => Some(String::new()),
            "changed" => Some("other".to_owned()),
            _ => None,
        };

        assert_eq!(
            unheld(&passed, held),
            [
                ("changed", "a", Some("other".to_owned())),
                ("lost", "b", None),
            ]
        );
    }

    #[test]
    fn a_flag_name_holding_a_value_is_skipped() {
        let mut flags = ChromiumFlags::default();

        flags.extend_user_flags(&[
            ChromiumFlag::Present("--enable-features=Spare".into()),
            ChromiumFlag::WithValue("use-gl=egl".into(), "egl".into()),
            ChromiumFlag::Removed("use-gl=egl".into()),
        ]);

        assert_eq!(flags.to_string(), "");
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
