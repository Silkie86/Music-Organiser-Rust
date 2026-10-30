//! Letter case aside, as the C# app compares names: .NET's `OrdinalIgnoreCase` and `ToLowerInvariant`, letter by
//! letter by Unicode's simple (one-letter) mapping, where Rust's `to_uppercase` and `to_lowercase` give the full one.

/// A letter's one-letter capital, as `OrdinalIgnoreCase` compares it.
pub fn upper(c: char) -> char {
    match c {
        // .NET keeps these two as they are, where Unicode makes them I and S.
        '\u{131}' | '\u{17f}' => c,
        // Greek small letters with an iota below: their one-letter capital is 8 on (9 for the three on their own),
        // where the full capital is two letters.
        '\u{1f80}'..='\u{1faf}' if u32::from(c) & 8 == 0 => char::from_u32(u32::from(c) + 8).unwrap_or(c),
        '\u{1fb3}' | '\u{1fc3}' | '\u{1ff3}' => char::from_u32(u32::from(c) + 9).unwrap_or(c),
        _ => one(c.to_uppercase()).unwrap_or(c),
    }
}

/// A letter's one-letter small form, as `ToLowerInvariant` gives it (no final-sigma rule).
pub fn lower(c: char) -> char {
    match c {
        // A dotted capital I: its one-letter small form is i, where the full one adds a dot above.
        '\u{130}' => 'i',
        _ => one(c.to_lowercase()).unwrap_or(c),
    }
}

fn one(mut letters: impl Iterator<Item = char>) -> Option<char> {
    match (letters.next(), letters.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// Equal letter case aside, as `OrdinalIgnoreCase` compares.
pub fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.chars().map(upper).eq(b.chars().map(upper))
}

/// Text that two names equal by `OrdinalIgnoreCase` share, to look them up by.
pub fn key(s: &str) -> String {
    s.chars().map(upper).collect()
}

/// Text in small letters, as `ToLowerInvariant` gives it.
pub fn to_lower(s: &str) -> String {
    s.chars().map(lower).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letter_case_goes_by_one_letter_at_a_time() {
        // The Kelvin sign's capital is itself, so it isn't k.
        assert_ne!(key("\u{212a}ick.wav"), key("kick.wav"));
        assert_eq!(key("Kick.WAV"), key("kick.wav"));
        // No final sigma: the same small sigma wherever it is.
        assert_eq!(to_lower("ΟΔΟΣ"), "οδοσ");
        assert_eq!(to_lower("\u{130}"), "i");
        assert!(eq_ignore_case("stra\u{df}e", "STRA\u{df}E"));
    }
}
