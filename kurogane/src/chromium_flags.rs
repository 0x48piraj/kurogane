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
