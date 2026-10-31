//! Checking a key someone gave against what the notes say ([`KeyAnswer::check_hint`]): a key in a file's name, a key
//! signature saved in a MIDI file, or a key typed on the Songs page. A hint is never heard as evidence: the notes alone
//! decide, and the hint is shown beside the answer with how it stands to it.

use crate::key::{Key, KeyRelation, Mode};
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

/// How a hint stands to the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HintCheck {
    /// It's the key given.
    Agrees,
    /// It's one of the keys the answer gives: one of a pair, the home note in either mode, or the key an unclear answer
    /// leans to.
    InAnswer,
    /// It's the relative of the key given (or leant to).
    Relative,
    /// It's the parallel: the same tonic, the other mode.
    Parallel,
    /// It's a fifth above or below, in the same mode.
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
        match self {
            KeyAnswer::Key(k) if k == hint => return HintCheck::Agrees,
            KeyAnswer::OrRelative(_) | KeyAnswer::HomeNote { .. } if self.names(hint) => return HintCheck::InAnswer,
            KeyAnswer::Unclear { leaning: Some(k), .. } if k == hint => return HintCheck::InAnswer,
            _ => {}
        }
        // A home note leaning to neither mode is checked against its major key: the tonic is what counts there.
        let against = match self {
            KeyAnswer::HomeNote { tonic, leaning: None } => Some(Key { tonic, mode: Mode::Major }),
            _ => self.leaning_key(),
        };
        let Some(key) = against else { return HintCheck::Ignored };
        match hint.relation_to(key) {
            KeyRelation::Same => HintCheck::Agrees,
            KeyRelation::Relative => HintCheck::Relative,
            KeyRelation::Parallel => HintCheck::Parallel,
            KeyRelation::FifthAbove | KeyRelation::FifthBelow => HintCheck::Fifth,
            KeyRelation::Other => HintCheck::Disagrees,
        }
    }
}
