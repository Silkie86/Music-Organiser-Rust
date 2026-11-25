//! Short words many sounds share: types ("Kick"), where a type or tempo came from ("name", "learned"), character
//! words ("punchy", "dark"). Each different word is held once for the whole app and every sound points at it, so half
//! a million sounds don't each keep their own copies: about 120 MB less (each copy was its own small piece of memory,
//! with the system's bookkeeping on top).
//!
//! A [`Word`] reads like text: it compares with `"Kick"` or a `String`, sorts and hashes as its text, prints as its
//! text, and lends `&str` wherever text is wanted.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::{LazyLock, OnceLock, PoisonError, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A shared short word. Empty costs nothing.
///
/// Held as the word's number in the app's list of words (0 for the empty word), 4 bytes where a pointer to the text
/// and its length took 16: every sound has six of these and a few more for its character, which for half a million
/// sounds is some 50 MB. The same text always has the same number, so two words are the same word when their numbers
/// are.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Word(u32);

/// Every different word met so far, with its number. Only ever grows, and only by words not seen before: types,
/// sources and character words come from a short list, plus the names you give your own types. (Odd text in a damaged
/// database's type columns would stay here until the app closes: a few bytes a word.) Many threads can look words up
/// at once; only a word never seen before waits its turn to be added.
static WORDS: LazyLock<RwLock<HashMap<&'static str, u32>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Each word's text by its number, read without waiting for anything (sorting and filtering half a million sounds on
/// every core reads words all the time). In blocks, each twice the size of the one before, made when first needed
/// and never moved: block 0 holds numbers 1 to [`FIRST_BLOCK`], block 1 the next `2 * FIRST_BLOCK`, and so on.
static TEXTS: [OnceLock<Box<[OnceLock<&'static str>]>>; BLOCKS] = [const { OnceLock::new() }; BLOCKS];

const FIRST_BLOCK: u32 = 1024;
/// Enough blocks for every number a `u32` can hold.
const BLOCKS: usize = 23;

/// Which block a word's number is in, and where in it.
fn place(number: u32) -> (usize, usize) {
    // Numbers start at 1. With them counted from 0 and FIRST_BLOCK added, each block starts at a power of two.
    let n = u64::from(number - 1) + u64::from(FIRST_BLOCK);
    let block = (63 - n.leading_zeros()) - FIRST_BLOCK.trailing_zeros();
    (block as usize, (n - (u64::from(FIRST_BLOCK) << block)) as usize)
}

impl Word {
    /// `text` as a shared word: the number it already has when the word has been seen before.
    pub fn new(text: &str) -> Word {
        if text.is_empty() {
            return Word(0);
        }
        // A panic elsewhere while the list was held can't have left it wrong (it's only ever added to), so go on.
        if let Some(&number) = WORDS.read().unwrap_or_else(PoisonError::into_inner).get(text) {
            return Word(number);
        }
        let mut words = WORDS.write().unwrap_or_else(PoisonError::into_inner);
        // Another thread may have added it between the two looks.
        if let Some(&number) = words.get(text) {
            return Word(number);
        }
        // More different words than a number can count (4 thousand million) can't be: they'd not fit in memory. If
        // they ever were, the word reads as empty rather than as another word's text.
        let Ok(number) = u32::try_from(words.len() + 1) else { return Word(0) };
        // Kept for as long as the app runs, as every word always was.
        let held: &'static str = Box::leak(Box::from(text));
        let (block, at) = place(number);
        let texts = TEXTS[block].get_or_init(|| {
            let size = (FIRST_BLOCK as usize) << block;
            (0..size).map(|_| OnceLock::new()).collect()
        });
        // Set before the number is given out, and only here, under the list's lock: a number anyone holds has its
        // text.
        let _ = texts[at].set(held);
        words.insert(held, number);
        Word(number)
    }

    pub fn as_str(&self) -> &'static str {
        if self.0 == 0 {
            return "";
        }
        let (block, at) = place(self.0);
        TEXTS[block].get().and_then(|texts| texts.get(at)).and_then(|text| text.get()).copied().unwrap_or("")
    }
}

/// Sorted as its text, the empty word first.
impl Ord for Word {
    fn cmp(&self, other: &Word) -> std::cmp::Ordering {
        if self.0 == other.0 { std::cmp::Ordering::Equal } else { self.as_str().cmp(other.as_str()) }
    }
}

impl PartialOrd for Word {
    fn partial_cmp(&self, other: &Word) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
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

    use std::collections::HashSet;

    #[test]
    fn the_same_word_is_held_once_and_reads_as_its_text() {
        let (a, b) = (Word::new("Kick"), Word::from("Kick".to_owned()));
        assert_eq!(a.0, b.0);
        assert!(std::ptr::eq(a.as_str(), b.as_str()));
        let owned = String::from("Kick");
        assert!(a == "Kick" && a == *"Kick" && a == owned && owned == a && "Kick" == a);
        assert_eq!(a.to_lowercase(), "kick");
        assert_eq!(format!("{a} {a:?}"), "Kick \"Kick\"");
        assert!(Word::new("Kick") < Word::new("Snare"));
        // By text, not by which was met first.
        let (later, earlier) = (Word::new("zz met first"), Word::new("aa met second"));
        assert!(earlier < later && Word::default() < earlier);
        assert_eq!(Word::new(""), Word::default());
        assert!(Word::default().is_empty() && Word::default().0 == 0);
        let set: HashSet<Word> = [Word::new("Kick")].into();
        assert!(set.contains("Kick"));
        let json = serde_json::to_string(&vec![Word::new("dark"), Word::new("")]).unwrap();
        assert_eq!(json, r#"["dark",""]"#);
        assert_eq!(serde_json::from_str::<Vec<Word>>(&json).unwrap(), [Word::new("dark"), Word::default()]);
        assert_eq!(std::mem::size_of::<Word>(), 4);
        assert_eq!(std::mem::size_of::<Option<Word>>(), 8);
        // What it's for: a sound in the list was 496 bytes with its six words as pointers.
        #[cfg(target_pointer_width = "64")]
        assert_eq!(std::mem::size_of::<crate::Sound>(), 416);
    }

    #[test]
    fn every_number_has_its_place_and_blocks_follow_on() {
        assert_eq!(place(1), (0, 0));
        assert_eq!(place(FIRST_BLOCK), (0, FIRST_BLOCK as usize - 1));
        assert_eq!(place(FIRST_BLOCK + 1), (1, 0));
        assert_eq!(place(3 * FIRST_BLOCK), (1, 2 * FIRST_BLOCK as usize - 1));
        assert_eq!(place(3 * FIRST_BLOCK + 1), (2, 0));
        let (block, at) = place(u32::MAX);
        assert!(block < BLOCKS && at < (FIRST_BLOCK as usize) << block, "{block} {at}");
    }

    #[test]
    fn many_new_words_from_many_threads_each_read_as_their_own_text() {
        // Past the first block, and the same words from every thread at once.
        let words: Vec<String> = (0..3 * FIRST_BLOCK).map(|i| format!("word-test-{i}")).collect();
        let made: Vec<Vec<Word>> = std::thread::scope(|scope| {
            let threads: Vec<_> =
                (0..4).map(|_| scope.spawn(|| words.iter().map(|w| Word::new(w)).collect::<Vec<_>>())).collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        for list in &made {
            for (word, text) in list.iter().zip(&words) {
                assert_eq!(word.as_str(), text);
            }
            assert_eq!(list, &made[0]);
        }
        let numbers: HashSet<u32> = made[0].iter().map(|w| w.0).collect();
        assert_eq!(numbers.len(), words.len());
    }
}
