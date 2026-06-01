//! Chromium command-line construction.
//!
//! This module provides a normalized intermediate representation for
//! Chromium command-line switches.

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

/// Chromium switch plan with last-write-wins precedence model.
#[derive(Default, Debug)]
pub(crate) struct ChromiumFlags {
    switches: BTreeMap<String, SwitchValue>,
}

impl ChromiumFlags {
    /// Insert a standalone switch.
    pub(crate) fn set(&mut self, name: impl Into<String>) {
        self.switches.insert(name.into(), SwitchValue::Present);
    }

    /// Insert a switch with a value.
    pub(crate) fn set_with_value(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) {
        self.switches.insert(name.into(), SwitchValue::Value(value.into()));
    }

    /// Apply user-supplied Chromium flags.
    ///
    /// User flags are appended after runtime policies and therefore
    /// override runtime defaults when the same switch name is used.
    pub(crate) fn extend_user_flags(&mut self, user_flags: &[ChromiumFlag]) {
        for flag in user_flags {
            match flag {
                ChromiumFlag::Present(name) => self.set(name.clone()),
                ChromiumFlag::WithValue(name, value) => {
                    self.set_with_value(name.clone(), value.clone());
                }
            }
        }
    }

    /// Emit the finalized switch set into CEF.
    ///
    /// This is the only place where ChromiumFlags interacts with
    /// CommandLine directly.
    pub(crate) fn apply(self, cmd: &mut CommandLine) {
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
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
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

        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    // Normalization and precedence tests

    #[test]
    fn set_replaces_existing_switch() {
        let mut flags = ChromiumFlags::default();

        flags.set("disable-gpu");
        flags.set("disable-gpu");

        assert_eq!(flags.switches.len(), 1);
    }

    #[test]
    fn last_value_wins() {
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

        flags.set_with_value(
            "disable-gpu",
            "ignored",
        );

        assert_eq!(
            flags.switches.get("disable-gpu"),
            Some(&SwitchValue::Value(
                "ignored".into()
            ))
        );
    }

    #[test]
    fn user_flags_override_defaults() {
        let mut flags = ChromiumFlags::default();

        flags.set_with_value(
            "use-gl",
            "angle",
        );

        flags.extend_user_flags(&[
            ChromiumFlag::WithValue(
                "use-gl".into(),
                "egl".into(),
            )
        ]);

        assert_eq!(
            flags.switches.get("use-gl"),
            Some(&SwitchValue::Value(
                "egl".into()
            ))
        );
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn last_write_wins(
            key in "[a-z0-9\\-]{1,32}",
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
}
