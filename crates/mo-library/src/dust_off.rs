//! The songs you said "Not now" to on the Songs page's Dust off card: each song's key with the day it may come back,
//! and the id of its newest save then, kept in one setting (`dust_off_later`, a JSON list of `[key, until, save]`; a
//! list kept before the save was has `[key, until]`). One that's come back is let go the next time the list is
//! changed, so it never grows past the songs put off now.

use std::path::Path;

use serde_json::{Value, json};

use crate::{Library, LibraryError};

/// The setting the list is kept in.
pub const SETTING: &str = "dust_off_later";

/// A song put off the card: its key then, the id of its newest save then, and when it may come back (Unix seconds).
/// The key changes when the song's folder is moved or renamed, or it's joined to another song; its save keeps its id,
/// so the song is found by either (the 26 Nov review's SZ-4). One kept before the save was has its key alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutOff {
    pub key: String,
    pub save: Option<i64>,
    pub until: i64,
}

/// The songs put off, as `library` keeps them. A list that can't be read is none put off.
pub fn read(library: &Library) -> Result<Vec<PutOff>, LibraryError> {
    Ok(library.setting(SETTING)?.map(|text| parse(&text)).unwrap_or_default())
}

fn parse(text: &str) -> Vec<PutOff> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    list.iter()
        .filter_map(|v| {
            let key = v.get(0)?.as_str()?.to_owned();
            Some(PutOff { key, save: v.get(2).and_then(Value::as_i64), until: v.get(1)?.as_i64()? })
        })
        .collect()
}

/// Puts a song off (`off`) in the library at `library` (its `settings`), in place of any put-off of the same key or
/// save, letting go of any that came back before `now`. Says whether it changed.
pub fn put_off(library: &Path, off: &PutOff, now: i64) -> Result<bool, LibraryError> {
    crate::setting_list::change(library, SETTING, |list| {
        let before = list.clone();
        list.retain(|v| {
            let same_key = v.get(0).and_then(Value::as_str) == Some(off.key.as_str());
            let same_save = off.save.is_some() && v.get(2).and_then(Value::as_i64) == off.save;
            !same_key && !same_save && v.get(1).and_then(Value::as_i64).is_some_and(|t| t > now)
        });
        list.push(match off.save {
            Some(save) => json!([off.key, off.until, save]),
            None => json!([off.key, off.until]),
        });
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
        let a = PutOff { key: "a".into(), save: None, until: 5 };
        assert_eq!(parse(r#"[["a", 5], ["b"], [1, 2]]"#), std::slice::from_ref(&a));
        // A save that isn't a number is none: the key still finds it.
        let c = PutOff { key: "c".into(), save: Some(7), until: 6 };
        assert_eq!(parse(r#"[["a", 5, "x"], ["c", 6, 7]]"#), [a, c]);
    }
}
