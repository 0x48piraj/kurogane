//! Keys by their W3C names, one vocabulary for every browser.
//!
//! A key is two things. [`PhysicalCode`] names its place on the keyboard,
//! the same in every layout. [`LogicalKey`] names what it means in the
//! layout, the character it types or a named key such as `Enter`. Both
//! follow the W3C `KeyboardEvent.code` and `KeyboardEvent.key` values, every
//! one the specification lists and every one Chromium knows.

mod logical_key;
mod physical_code;

pub use logical_key::LogicalKey;
pub use physical_code::PhysicalCode;

#[cfg(target_os = "macos")]
pub(crate) use logical_key::mac_character;
pub(crate) use logical_key::us_characters;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_its_w3c_value() {
        for code in PhysicalCode::ALL {
            assert_eq!(PhysicalCode::from_w3c(code.as_str()), Some(code));
        }
        assert!(PhysicalCode::ALL.is_sorted_by_key(|code| code.as_str()));
        assert_eq!(PhysicalCode::KeyW.to_string(), "KeyW");
        assert_eq!(PhysicalCode::from_w3c("w"), None);
    }

    #[test]
    fn hid_usages_name_keyboard_keys() {
        assert_eq!(
            PhysicalCode::from_hid_usage(0x07_0004),
            Some(PhysicalCode::KeyA)
        );
        assert_eq!(
            PhysicalCode::from_hid_usage(0x07_002c),
            Some(PhysicalCode::Space)
        );
        assert_eq!(
            PhysicalCode::from_hid_usage(0x07_00e1),
            Some(PhysicalCode::ShiftLeft)
        );
        assert_eq!(PhysicalCode::from_hid_usage(0), None);
    }

    #[test]
    fn a_named_key_is_its_name_and_other_text_a_character() {
        for (value, key) in [
            ("Enter", LogicalKey::Enter),
            ("F24", LogicalKey::F24),
            ("AltGraph", LogicalKey::AltGraph),
            ("a", LogicalKey::Character("a".into())),
            (" ", LogicalKey::Character(" ".into())),
            ("é", LogicalKey::Character("é".into())),
        ] {
            assert_eq!(LogicalKey::from_w3c(value), key);
            assert_eq!(key.as_str(), value);
        }
        assert_eq!(LogicalKey::from_w3c(""), LogicalKey::Unidentified);
        assert_eq!(LogicalKey::Unidentified.to_string(), "Unidentified");
    }
}
