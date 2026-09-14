//! Access control: which origins may reach which IPC endpoints.
//!
//! The renderer installs `window.kurogane` in every V8 context, sub-frames
//! included. The router consults this table before a command is invoked or a
//! stream is opened and before an event subscription is accepted.
//!
//! - Policy is data: name to rule, for commands (commands and streams share a
//!   namespace) and, separately, for events. Unlisted names follow one
//!   default.
//! - With no rules and the default untouched, everything is allowed, exactly
//!   as before the ACL existed.
//! - Rules match [`Origin`] values (`scheme://host[:port]`); the opaque origin can
//!   never be listed.
//! - Commands owned by a native capability (the `fs.*` family) are
//!   authorized by that capability's grants, never by this table.
//!
//! Threat model: this is a "which origins are trusted" boundary. It does not
//! defend against compromised content inside a trusted origin (XSS).

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

use crate::error::ConfigError;

/// The security origin of a frame: `scheme://host[:port]`, or opaque.
///
/// Serialized like `location.origin`: lowercase scheme and host, default
/// ports elided. Frames without a host (`file:`, `data:`, `about:`) and
/// unparseable URLs are opaque and an opaque origin matches no rule and no
/// grant.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin(Repr);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Repr {
    Tuple {
        scheme: String,
        host: String,
        port: Option<u16>,
    },
    Opaque,
}

impl Origin {
    /// The opaque origin.
    pub const OPAQUE: Origin = Origin(Repr::Opaque);

    /// The origin of the document at `url` (browser side; never fails).
    pub fn from_url(url: &str) -> Origin {
        url::Url::parse(url)
            .ok()
            .and_then(|parsed| Origin::tuple(&parsed))
            .unwrap_or(Origin::OPAQUE)
    }

    /// Parses a configured origin such as `app://app` or
    /// `http://localhost:5173`.
    ///
    /// # Errors
    ///
    /// Returns [`OriginError`] when `text` is not a URL, has no host, is a
    /// `file:` URL, or carries a path, query, fragment or credentials.
    pub fn parse(text: &str) -> Result<Origin, OriginError> {
        let fail = |reason| OriginError {
            input: text.to_owned(),
            reason,
        };
        let parsed = url::Url::parse(text).map_err(|_| fail("not a URL"))?;
        if !matches!(parsed.path(), "" | "/") {
            return Err(fail("an origin has no path"));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(fail("an origin has no query or fragment"));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(fail("an origin has no credentials"));
        }
        Origin::tuple(&parsed).ok_or_else(|| fail("the URL has no host, so its origin is opaque"))
    }

    fn tuple(parsed: &url::Url) -> Option<Origin> {
        if parsed.scheme() == "file" {
            return None;
        }
        let host = parsed.host_str()?;
        Some(Origin(Repr::Tuple {
            scheme: parsed.scheme().to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            port: parsed.port(),
        }))
    }

    /// Returns true for the opaque origin.
    pub fn is_opaque(&self) -> bool {
        self.0 == Repr::Opaque
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Tuple {
                scheme,
                host,
                port: Some(port),
            } => write!(f, "{scheme}://{host}:{port}"),
            Repr::Tuple {
                scheme,
                host,
                port: None,
            } => write!(f, "{scheme}://{host}"),
            Repr::Opaque => f.write_str("null"),
        }
    }
}

impl FromStr for Origin {
    type Err = OriginError;

    fn from_str(text: &str) -> Result<Origin, OriginError> {
        Origin::parse(text)
    }
}

/// A configured origin that does not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginError {
    input: String,
    reason: &'static str,
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid origin '{}': {}", self.input, self.reason)
    }
}

impl std::error::Error for OriginError {}

/// How one name may be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AclRule {
    /// Only these origins; never the opaque origin.
    Allow(HashSet<Origin>),
    /// Any origin.
    AllowAll,
    /// A command owned by a native capability, which authorizes every call
    /// itself. Commands only.
    Capability,
}

/// Behavior for names without a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DefaultPolicy {
    /// Reachable from any origin (the behavior before the ACL existed).
    #[default]
    AllowAll,
    /// Reachable from no origin.
    Deny,
}

/// Immutable permission table for commands, streams and events, built
/// before startup.
#[derive(Debug, Clone, Default)]
pub(crate) struct CommandAcl {
    commands: HashMap<String, AclRule>,
    events: HashMap<String, AclRule>,
    default: DefaultPolicy,
}

impl CommandAcl {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Allows `origins` to invoke the command or open the stream `name`,
    /// merged with an existing rule.
    ///
    /// # Errors
    ///
    /// [`ConfigError::OpaqueOrigin`] if an origin is opaque;
    /// [`ConfigError::CapabilityCommand`] if `name` belongs to a native
    /// capability. The table is unchanged on error.
    pub(crate) fn allow(
        &mut self,
        name: impl Into<String>,
        origins: impl IntoIterator<Item = Origin>,
    ) -> Result<(), ConfigError> {
        allow_origins(&mut self.commands, name.into(), origins)
    }

    /// Makes the command or stream `name` reachable from any origin.
    ///
    /// # Errors
    ///
    /// [`ConfigError::CapabilityCommand`] if `name` belongs to a native
    /// capability.
    pub(crate) fn allow_all(&mut self, name: impl Into<String>) -> Result<(), ConfigError> {
        let name = name.into();
        if matches!(self.commands.get(&name), Some(AclRule::Capability)) {
            return Err(ConfigError::CapabilityCommand(name));
        }
        self.commands.insert(name, AclRule::AllowAll);
        Ok(())
    }

    /// Hands the command `name` to a native capability.
    ///
    /// # Errors
    ///
    /// [`ConfigError::CapabilityCommand`] if `name` already has a rule.
    pub(crate) fn capability(&mut self, name: &str) -> Result<(), ConfigError> {
        if self.commands.contains_key(name) {
            return Err(ConfigError::CapabilityCommand(name.to_owned()));
        }
        self.commands.insert(name.to_owned(), AclRule::Capability);
        Ok(())
    }

    /// Allows `origins` to subscribe to the event `name`, merged with an
    /// existing rule.
    ///
    /// # Errors
    ///
    /// [`ConfigError::OpaqueOrigin`] if an origin is opaque.
    pub(crate) fn allow_event(
        &mut self,
        name: impl Into<String>,
        origins: impl IntoIterator<Item = Origin>,
    ) -> Result<(), ConfigError> {
        allow_origins(&mut self.events, name.into(), origins)
    }

    /// Makes the event `name` subscribable from any origin.
    pub(crate) fn allow_event_all(&mut self, name: impl Into<String>) {
        self.events.insert(name.into(), AclRule::AllowAll);
    }

    /// Denies commands, streams and events without a rule.
    pub(crate) fn deny_unlisted(&mut self) {
        self.default = DefaultPolicy::Deny;
    }

    /// Whether `origin` may invoke the command or open the stream `name`.
    /// Capability-owned commands always pass here: their grants decide.
    pub(crate) fn allows(&self, name: &str, origin: &Origin) -> bool {
        self.decide(self.commands.get(name), origin)
    }

    /// Whether `origin` may subscribe to the event `name`.
    pub(crate) fn allows_event(&self, name: &str, origin: &Origin) -> bool {
        self.decide(self.events.get(name), origin)
    }

    fn decide(&self, rule: Option<&AclRule>, origin: &Origin) -> bool {
        match rule {
            Some(AclRule::Allow(origins)) => origins.contains(origin),
            Some(AclRule::AllowAll | AclRule::Capability) => true,
            None => self.default == DefaultPolicy::AllowAll,
        }
    }
}

/// Adds `origins` to the rule for `name` in `table`.
fn allow_origins(
    table: &mut HashMap<String, AclRule>,
    name: String,
    origins: impl IntoIterator<Item = Origin>,
) -> Result<(), ConfigError> {
    let origins: Vec<Origin> = origins.into_iter().collect();
    if origins.iter().any(Origin::is_opaque) {
        return Err(ConfigError::OpaqueOrigin(name));
    }
    match table.entry(name) {
        Entry::Vacant(slot) => {
            slot.insert(AclRule::Allow(origins.into_iter().collect()));
        }
        Entry::Occupied(mut slot) => match slot.get_mut() {
            AclRule::Allow(set) => set.extend(origins),
            AclRule::AllowAll => {}
            AclRule::Capability => return Err(ConfigError::CapabilityCommand(slot.key().clone())),
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(text: &str) -> Origin {
        Origin::parse(text).unwrap()
    }

    #[test]
    fn origins_serialize_like_location_origin() {
        let cases = [
            ("app://app/index.html", "app://app"),
            ("APP://App/deep/page?x#y", "app://app"),
            (
                "http://localhost:5173/index.html?x=1",
                "http://localhost:5173",
            ),
            ("https://Example.com:443/a", "https://example.com"),
            ("http://localhost/", "http://localhost"),
        ];
        for (url, expected) in cases {
            assert_eq!(Origin::from_url(url).to_string(), expected, "{url}");
        }
    }

    #[test]
    fn hostless_and_file_urls_are_opaque() {
        for url in [
            "file:///home/me/index.html",
            "data:text/html,hi",
            "about:blank",
            "not a url",
            "",
        ] {
            assert!(Origin::from_url(url).is_opaque(), "{url}");
        }
        assert_eq!(Origin::OPAQUE.to_string(), "null");
    }

    #[test]
    fn configured_origins_are_validated() {
        assert_eq!(
            origin("app://app/"),
            Origin::from_url("app://app/index.html")
        );
        for bad in [
            "app",
            "app://app/index.html",
            "app://app?x",
            "app://app#y",
            "file:///x",
            "https://u:p@x.example",
        ] {
            assert!(Origin::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn no_policy_allows_everything() {
        let acl = CommandAcl::new();
        assert!(acl.allows("anything", &origin("https://evil.example")));
        assert!(acl.allows("anything", &Origin::OPAQUE));
        assert!(acl.allows_event("anything", &Origin::OPAQUE));
    }

    #[test]
    fn allow_lists_origins_and_merges() {
        let mut acl = CommandAcl::new();
        acl.allow("cmd", [origin("app://app")]).unwrap();
        acl.allow("cmd", [origin("http://localhost:5173")]).unwrap();
        assert!(acl.allows("cmd", &origin("app://app")));
        assert!(acl.allows("cmd", &origin("http://localhost:5173")));
        assert!(!acl.allows("cmd", &origin("https://evil.example")));
        assert!(!acl.allows("cmd", &Origin::OPAQUE));
        assert!(
            acl.allows("other", &origin("https://evil.example")),
            "default still allows"
        );
    }

    #[test]
    fn deny_unlisted_blocks_unknown_names() {
        let mut acl = CommandAcl::new();
        acl.allow_all("ping").unwrap();
        acl.allow("cmd", [origin("app://app")]).unwrap();
        acl.deny_unlisted();
        assert!(acl.allows("ping", &origin("https://anywhere.example")));
        assert!(acl.allows("cmd", &origin("app://app")));
        assert!(!acl.allows("secret", &origin("app://app")));
        assert!(
            !acl.allows_event("tick", &origin("app://app")),
            "unlisted events too"
        );
    }

    #[test]
    fn events_have_their_own_rules() {
        let app = origin("app://app");
        let evil = origin("https://evil.example");
        let mut acl = CommandAcl::new();
        acl.allow_event("tick", [app.clone()]).unwrap();
        assert!(acl.allows_event("tick", &app));
        assert!(!acl.allows_event("tick", &evil));
        assert!(acl.allows_event("other", &evil), "default still allows");
        assert!(
            acl.allows("tick", &evil),
            "an event rule is not a command rule"
        );

        acl.deny_unlisted();
        acl.allow_event_all("public");
        assert!(acl.allows_event("public", &evil));
        assert!(!acl.allows_event("other", &app));
        assert!(!acl.allows("tick", &app));
    }

    #[test]
    fn capability_commands_defer_to_grants() {
        let mut acl = CommandAcl::new();
        acl.capability("fs.read_file").unwrap();
        acl.deny_unlisted();
        assert!(
            acl.allows("fs.read_file", &origin("https://evil.example")),
            "the grant decides"
        );
        assert!(!acl.allows("fs.other", &origin("https://evil.example")));
    }

    #[test]
    fn acl_rules_and_capabilities_cannot_share_a_command() {
        let conflict = Err(ConfigError::CapabilityCommand("fs.read_file".to_owned()));

        let mut acl = CommandAcl::new();
        acl.capability("fs.read_file").unwrap();
        assert_eq!(acl.allow("fs.read_file", [origin("app://app")]), conflict);
        assert_eq!(acl.allow_all("fs.read_file"), conflict);

        let mut acl = CommandAcl::new();
        acl.allow_all("fs.read_file").unwrap();
        assert_eq!(acl.capability("fs.read_file"), conflict);
    }

    #[test]
    fn the_opaque_origin_cannot_be_permitted() {
        let mut acl = CommandAcl::new();
        assert_eq!(
            acl.allow("cmd", [origin("app://app"), Origin::OPAQUE]),
            Err(ConfigError::OpaqueOrigin("cmd".to_owned()))
        );
        assert_eq!(
            acl.allow_event("tick", [Origin::OPAQUE]),
            Err(ConfigError::OpaqueOrigin("tick".to_owned()))
        );
        assert!(
            acl.allows("cmd", &origin("https://evil.example")),
            "nothing was recorded"
        );
    }
}
