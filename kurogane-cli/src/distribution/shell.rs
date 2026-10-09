//! Quoting for the shell scripts a bundle starts through.

/// Quotes `value` as one `sh` word that expands nothing: inside single
/// quotes only a `'` is special, and it is written as `'\''`.
pub fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_single_quoted_and_a_quote_closes_and_reopens() {
        assert_eq!(sh_quote("My App"), "'My App'");
        assert_eq!(sh_quote("Tom's App"), r"'Tom'\''s App'");
        assert_eq!(sh_quote(""), "''");
    }

    /// What `sh` makes of the quoted word: the value itself, and nothing run.
    #[cfg(unix)]
    #[test]
    fn sh_reads_the_word_back_as_written() {
        let dir = crate::distribution::test_fixtures::tmp_dir();
        let mark = dir.path().join("mark");
        let mark = mark.to_string_lossy();
        for value in [
            "My \"Best\" App".to_string(),
            "Tom's App".to_string(),
            "Ca$h $HOME ${HOME}".to_string(),
            format!("$(touch {mark})"),
            format!("`touch {mark}`"),
            r"a\b\\c\$d".to_string(),
            "x\" | touch mark | \"y".to_string(),
            "line\nbreak".to_string(),
        ] {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf '%s' {}", sh_quote(&value)))
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{value:?}");
            assert_eq!(String::from_utf8_lossy(&output.stdout), value);
        }
        assert!(!dir.path().join("mark").exists(), "nothing ran");
    }
}
