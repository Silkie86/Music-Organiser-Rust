//! The songs you said "Not now" to on the Songs page's Dust off card: each song's key with the day it may come back,
//! kept in one setting (`dust_off_later`, a JSON list of `[key, until]`). One that's come back is let go the next time
//! the list is changed, so it never grows past the songs put off now.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value, json};

use crate::{Library, LibraryError};

/// The setting the list is kept in.
pub const SETTING: &str = "dust_off_later";

/// The songs put off, by key, each with when it may come back (Unix seconds), as `library` keeps them. A list that
/// can't be read is none put off.
pub fn read(library: &Library) -> Result<HashMap<String, i64>, LibraryError> {
    Ok(library.setting(SETTING)?.map(|text| parse(&text)).unwrap_or_default())
}

fn parse(text: &str) -> HashMap<String, i64> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(text) else { return HashMap::new() };
    list.iter().filter_map(|v| Some((v.get(0)?.as_str()?.to_owned(), v.get(1)?.as_i64()?))).collect()
}

/// Puts the song `key` off until `until` (Unix seconds) in the library at `library` (its `settings`), letting go of
/// any that came back before `now`. Says whether it changed.
pub fn put_off(library: &Path, key: &str, until: i64, now: i64) -> Result<bool, LibraryError> {
    crate::setting_list::change(library, SETTING, |list| {
        let before = list.clone();
        list.retain(|v| {
            v.get(0).and_then(Value::as_str) != Some(key) && v.get(1).and_then(Value::as_i64).is_some_and(|t| t > now)
        });
        list.push(json!([key, until]));
        *list != before
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_that_cant_be_read_is_none_put_off() {
        assert!(parse("oops").is_empty());
        assert!(parse("{}").is_empty());
        assert_eq!(parse(r#"[["a", 5], ["b"], [1, 2]]"#), HashMap::from([("a".to_owned(), 5)]));
    }
}
