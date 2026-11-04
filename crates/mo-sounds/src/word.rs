//! Short words many sounds share: types ("Kick"), where a type or tempo came from ("name", "learned"), character
//! words ("punchy", "dark"). Each different word is held once for the whole app and every sound points at it, so half
//! a million sounds don't each keep their own copies: about 120 MB less (each copy was its own small piece of memory,
//! with the system's bookkeeping on top).
//!
//! A [`Word`] reads like text: it compares with `"Kick"` or a `String`, sorts and hashes as its text, prints as its
//! text, and lends `&str` wherever text is wanted.

use std::borrow::Borrow;
use std::collections::HashSet;
use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A shared short word. Empty costs nothing.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Word(Option<Arc<str>>);

/// Every different word met so far. Only ever grows, and only by words not seen before: types, sources and character
/// words come from a short list, plus the names you give your own types. (Odd text in a damaged database's type
/// columns would stay here until the app closes: a few bytes a word.) Many threads can look words up at once; only a
/// word never seen before waits its turn to be added.
static WORDS: LazyLock<RwLock<HashSet<Arc<str>>>> = LazyLock::new(|| RwLock::new(HashSet::new()));

impl Word {
    /// `text` as a shared word: the copy already held when the word has been seen before.
    pub fn new(text: &str) -> Word {
        if text.is_empty() {
            return Word(None);
        }
        // A panic elsewhere while the list was held can't have left it wrong (it's only ever added to), so go on.
        if let Some(held) = WORDS.read().unwrap_or_else(PoisonError::into_inner).get(text) {
            return Word(Some(held.clone()));
        }
        let mut words = WORDS.write().unwrap_or_else(PoisonError::into_inner);
        // Another thread may have added it between the two looks.
        if let Some(held) = words.get(text) {
            return Word(Some(held.clone()));
        }
        let held: Arc<str> = Arc::from(text);
        words.insert(held.clone());
        Word(Some(held))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_deref().unwrap_or("")
    }
}

impl Deref for Word {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

/// Hashed as its text, so a set or map of words can be looked up by text.
impl std::hash::Hash for Word {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.as_str().hash(h);
    }
}

impl AsRef<str> for Word {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Word {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for Word {
    fn from(text: &str) -> Word {
        Word::new(text)
    }
}

impl From<String> for Word {
    fn from(text: String) -> Word {
        Word::new(&text)
    }
}

impl From<&String> for Word {
    fn from(text: &String) -> Word {
        Word::new(text)
    }
}

impl From<Word> for String {
    fn from(w: Word) -> String {
        w.as_str().to_owned()
    }
}

impl PartialEq<str> for Word {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Word {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for Word {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<Word> for str {
    fn eq(&self, other: &Word) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<Word> for &str {
    fn eq(&self, other: &Word) -> bool {
        *self == other.as_str()
    }
}

impl PartialEq<Word> for String {
    fn eq(&self, other: &Word) -> bool {
        self == other.as_str()
    }
}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl Serialize for Word {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Word {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Word, D::Error> {
        // Looked at where it lies in the text being read, so a word seen before costs no copy at all.
        struct Text;
        impl serde::de::Visitor<'_> for Text {
            type Value = Word;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a word")
            }
            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Word, E> {
                Ok(Word::new(text))
            }
        }
        d.deserialize_str(Text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_word_is_held_once_and_reads_as_its_text() {
        let (a, b) = (Word::new("Kick"), Word::from("Kick".to_owned()));
        assert!(Arc::ptr_eq(a.0.as_ref().unwrap(), b.0.as_ref().unwrap()));
        let owned = String::from("Kick");
        assert!(a == "Kick" && a == *"Kick" && a == owned && owned == a && "Kick" == a);
        assert_eq!(a.to_lowercase(), "kick");
        assert_eq!(format!("{a} {a:?}"), "Kick \"Kick\"");
        assert!(Word::new("Kick") < Word::new("Snare"));
        assert_eq!(Word::new(""), Word::default());
        assert!(Word::default().is_empty() && Word::default().0.is_none());
        let set: HashSet<Word> = [Word::new("Kick")].into();
        assert!(set.contains("Kick"));
        let json = serde_json::to_string(&vec![Word::new("dark"), Word::new("")]).unwrap();
        assert_eq!(json, r#"["dark",""]"#);
        assert_eq!(serde_json::from_str::<Vec<Word>>(&json).unwrap(), [Word::new("dark"), Word::default()]);
    }
}
