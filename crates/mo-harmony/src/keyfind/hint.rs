//! Checking a key someone gave against what the notes say ([`KeyAnswer::check_hint`]): a key in a file's name, a key
//! signature saved in a MIDI file, or a key typed on the Songs page. A hint is never heard as evidence: the notes alone
//! decide, and the hint is shown beside the answer with how it stands to it.

use crate::key::{Key, KeyRelation};
use crate::pitch::PitchClass;

use super::KeyAnswer;

/// Where a hint came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HintSource {
    /// A key signature saved in a MIDI file. Many programs save C major whatever the music is, so C major says nothing.
    KeySignatureEvent,
    /// A key in a file's or a part's name ("Dark Keys Cm 90bpm").
    Name,
    /// A key someone typed (the Songs page's key field).
    Typed,
}

/// How a hint stands to the answer. The relations are to any key the answer gives ([`KeyAnswer::keys`]): a pair's
/// two keys, a home note's two modes, or the key no clear key leans to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HintCheck {
    /// It's the key given.
    Agrees,
    /// It's one of the keys the answer gives: one of a pair, the home note in either mode, or the key an unclear answer
    /// leans to.
    InAnswer,
    /// It's the relative of a key the answer gives.
    Relative,
    /// It's the parallel of a key the answer gives: the same tonic, the other mode.
    Parallel,
    /// It's a fifth above or below a key the answer gives, in the same mode.
    Fifth,
    /// It's some other key.
    Disagrees,
    /// Not checked: a C major key signature (often only a default), or an answer with no key to check against.
    Ignored,
}

impl KeyAnswer {
    /// How a hint stands to this answer.
    pub fn check_hint(self, hint: Key, from: HintSource) -> HintCheck {
        if from == HintSource::KeySignatureEvent && hint == Key::major(PitchClass::C) {
            return HintCheck::Ignored;
        }
        let keys = self.keys();
        if keys.iter().all(Option::is_none) {
            return HintCheck::Ignored;
        }
        if self == KeyAnswer::Key(hint) {
            return HintCheck::Agrees;
        }
        if keys.contains(&Some(hint)) {
            return HintCheck::InAnswer;
        }
        // The keys an answer gives are never a relative, parallel or fifth of the same hint in two ways, so the first
        // key the hint stands near says how it stands.
        keys.iter()
            .flatten()
            .find_map(|key| match hint.relation_to(*key) {
                KeyRelation::Relative => Some(HintCheck::Relative),
                KeyRelation::Parallel => Some(HintCheck::Parallel),
                KeyRelation::FifthAbove | KeyRelation::FifthBelow => Some(HintCheck::Fifth),
                KeyRelation::Same | KeyRelation::Other => None,
            })
            .unwrap_or(HintCheck::Disagrees)
    }
}
