//! Property lists, as a macOS bundle's `Info.plist` files and its signing
//! entitlements are written.
//!
//! A plist is built as data and written by one serializer, so every value a
//! bundle declares is escaped the same way, and a new key is one more entry.

use thiserror::Error;

/// A property-list value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    String(String),
    Bool(bool),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

/// A dictionary, written in the order its keys were first set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dict(Vec<(String, Value)>);

/// A key or value XML cannot carry: XML 1.0 has no form, escaped or not, for
/// most control characters.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("the property list key {key:?} holds {character:?}, which a property list cannot")]
pub struct PlistError {
    pub key: String,
    pub character: char,
}

impl Dict {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `key`; a key set before keeps its place and takes the new value.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) -> &mut Self {
        let value = value.into();
        match self.0.iter_mut().find(|(existing, _)| existing == key) {
            Some((_, existing)) => *existing = value,
            None => self.0.push((key.to_string(), value)),
        }
        self
    }

    /// The value of `key`, if set.
    #[cfg(test)]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    /// The XML property list, with the DOCTYPE Apple's tools write.
    pub fn to_xml(&self) -> Result<String, PlistError> {
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n",
        );
        for (key, value) in &self.0 {
            xml.push_str("    <key>");
            xml.push_str(&escape(key, key)?);
            xml.push_str("</key>\n");
            match value {
                Value::String(text) => {
                    xml.push_str("    <string>");
                    xml.push_str(&escape(key, text)?);
                    xml.push_str("</string>\n");
                }
                Value::Bool(true) => xml.push_str("    <true/>\n"),
                Value::Bool(false) => xml.push_str("    <false/>\n"),
            }
        }
        xml.push_str("</dict>\n</plist>\n");
        Ok(xml)
    }
}

/// Escapes `text`, the value of `key` or the key itself, for XML character
/// data, refusing a character XML 1.0 cannot carry.
fn escape(key: &str, text: &str) -> Result<String, PlistError> {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\t' | '\n' | '\r' => escaped.push(c),
            c if c.is_control() || c == '\u{fffe}' || c == '\u{ffff}' => {
                return Err(PlistError {
                    key: key.to_string(),
                    character: c,
                });
            }
            c => escaped.push(c),
        }
    }
    Ok(escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_keep_their_first_place_and_take_the_last_value() {
        let mut plist = Dict::new();
        plist
            .set("CFBundleName", "first")
            .set("LSUIElement", true)
            .set("CFBundleName", "second");

        assert_eq!(plist.get("CFBundleName"), Some(&Value::from("second")));
        assert_eq!(
            plist.to_xml().unwrap(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \x20   <key>CFBundleName</key>\n    <string>second</string>\n\
             \x20   <key>LSUIElement</key>\n    <true/>\n\
             </dict>\n</plist>\n"
        );
    }

    #[test]
    fn markup_in_a_value_is_escaped_and_cannot_close_the_element() {
        let mut plist = Dict::new();
        plist.set("CFBundleName", "Ben & Jerry <Ltd></string><key>x</key>");

        let xml = plist.to_xml().unwrap();
        assert!(xml.contains(
            "<string>Ben &amp; Jerry &lt;Ltd&gt;&lt;/string&gt;&lt;key&gt;x&lt;/key&gt;</string>"
        ));
        assert_eq!(xml.matches("<key>").count(), 1);
    }

    #[test]
    fn a_character_xml_cannot_carry_is_refused_naming_its_key() {
        let mut plist = Dict::new();
        plist.set("NSCameraUsageDescription", "calls\u{1}");

        assert_eq!(
            plist.to_xml(),
            Err(PlistError {
                key: "NSCameraUsageDescription".to_string(),
                character: '\u{1}',
            })
        );

        // A line break is text
        let mut plist = Dict::new();
        plist.set("NSCameraUsageDescription", "calls\nand meetings");
        assert!(plist.to_xml().is_ok());
    }
}
