//! What a MIDI part is for, as the MIDI page sorts them (the C# `MidiRoles`).

/// The kind of part: chords, a melody, a bassline, an arp, drums, or something else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    Chords,
    Melody,
    Bass,
    Arp,
    Drums,
    Other,
}

impl Role {
    /// Every kind, in the order the page's chips show them.
    pub const ALL: [Role; 6] = [Role::Chords, Role::Melody, Role::Bass, Role::Arp, Role::Drums, Role::Other];

    /// The name as `midi.db` stores it: "Chords", "Melody" …
    pub fn name(self) -> &'static str {
        match self {
            Role::Chords => "Chords",
            Role::Melody => "Melody",
            Role::Bass => "Bass",
            Role::Arp => "Arp",
            Role::Drums => "Drums",
            Role::Other => "Other",
        }
    }

    /// How the page's chips name a kind of part: "Basslines", "Melodies".
    pub fn plural(self) -> &'static str {
        match self {
            Role::Chords => "Chords",
            Role::Melody => "Melodies",
            Role::Bass => "Basslines",
            Role::Arp => "Arps",
            Role::Drums => "Drums",
            Role::Other => "Other",
        }
    }

    /// The kind a stored name stands for, as written ("Bass") or as a chip says it ("Basslines"). Anything else (an
    /// empty role, which the C# app gives a part with no notes) is `Other`, as the C# page counts it.
    pub fn from_name(name: &str) -> Role {
        Role::ALL.into_iter().find(|r| r.name() == name || r.plural() == name).unwrap_or(Role::Other)
    }
}

/// Why a part was taken for its kind: one of the analyser's few sentences ([`crate::analyser::role`]), kept as a small
/// number and made into words only when a card shows it. Every part holds one, so it's stored and held once per
/// sentence, not once per part (rule 23, the 19 Nov review's MP-6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RoleWhy(u8);

impl RoleWhy {
    /// No reason (a part with no notes, or a reason this app no longer gives).
    pub const NONE: RoleWhy = RoleWhy(0);
    pub const DRUMS_BY_PLACE: RoleWhy = RoleWhy(1);
    pub const GM_DRUMS: RoleWhy = RoleWhy(2);
    pub const MOSTLY_CHORDS: RoleWhy = RoleWhy(3);
    pub const ONE_LOW_NOTE: RoleWhy = RoleWhy(4);
    pub const ONE_NOTE: RoleWhy = RoleWhy(5);
    pub const SINGLE_LOW: RoleWhy = RoleWhy(6);
    pub const FAST_EVEN: RoleWhy = RoleWhy(7);
    pub const ONE_AFTER_ANOTHER: RoleWhy = RoleWhy(8);
    pub const NAMED_DRUMS: RoleWhy = RoleWhy(9);
    pub const NAMED_DRUM_SOUND: RoleWhy = RoleWhy(10);
    pub const NAMED_ARP: RoleWhy = RoleWhy(11);
    pub const NAMED_CHORDS: RoleWhy = RoleWhy(12);
    pub const NAMED_BASS: RoleWhy = RoleWhy(13);
    pub const NAMED_MELODY: RoleWhy = RoleWhy(14);

    /// Added to a reason from the notes when the part's name says the same kind.
    const AGREES: u8 = 0x40;

    /// Each reason's words, by its number. Numbers are stored (`parts.why` in midi-index.db): never reuse or reorder
    /// one; a new reason takes the next.
    const WORDS: [&'static str; 15] = [
        "",
        "Drums by where it's played (MIDI channel 10 or a drum plugin)",
        "Uses the General MIDI kick, snare and hat notes, played like drums",
        "Mostly three or more notes at once",
        "One low note, repeated",
        "One note, repeated",
        "Single notes, low down",
        "Fast, evenly spaced notes of one chord at a time",
        "Single notes, one after another",
        "Named like a drum part",
        "Named after a drum sound",
        "Named as an arp",
        "Named as chords",
        "Named as a bass",
        "Named as a melody",
    ];
    const AGREES_WORDS: &'static str = ", and its name agrees";

    /// The same reason, with the part's name agreeing.
    pub fn and_name_agrees(self) -> RoleWhy {
        if self == RoleWhy::NONE { self } else { RoleWhy(self.0 | RoleWhy::AGREES) }
    }

    /// The words a card shows: "Single notes, low down, and its name agrees".
    pub fn words(self) -> String {
        let words = RoleWhy::WORDS.get(usize::from(self.0 & !RoleWhy::AGREES)).copied().unwrap_or("");
        if self.0 & RoleWhy::AGREES != 0 && !words.is_empty() {
            format!("{words}{}", RoleWhy::AGREES_WORDS)
        } else {
            words.to_owned()
        }
    }

    /// The number midi-index.db stores.
    pub fn code(self) -> i64 {
        i64::from(self.0)
    }

    /// The reason a stored number stands for; one this app doesn't know is [`RoleWhy::NONE`].
    pub fn from_code(code: i64) -> RoleWhy {
        u8::try_from(code).ok().map(RoleWhy).filter(|w| !w.words().is_empty()).unwrap_or(RoleWhy::NONE)
    }

    /// Every reason with words, each with and without the name agreeing: what an older index (or the C# app's) stored
    /// as words is matched against these.
    pub fn all() -> impl Iterator<Item = RoleWhy> {
        (1..RoleWhy::WORDS.len() as u8).flat_map(|n| [RoleWhy(n), RoleWhy(n).and_name_agrees()])
    }

    /// The reason stored as `words`; words this app doesn't give are [`RoleWhy::NONE`].
    pub fn from_words(words: &str) -> RoleWhy {
        RoleWhy::all().find(|w| w.words() == words).unwrap_or(RoleWhy::NONE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_go_both_ways() {
        for r in Role::ALL {
            assert_eq!(Role::from_name(r.name()), r);
            assert_eq!(Role::from_name(r.plural()), r);
        }
        assert_eq!(Role::from_name(""), Role::Other);
        assert_eq!(Role::Bass.plural(), "Basslines");
    }

    #[test]
    fn reasons_go_both_ways() {
        for why in RoleWhy::all() {
            assert_eq!(RoleWhy::from_code(why.code()), why);
            assert_eq!(RoleWhy::from_words(&why.words()), why);
        }
        assert_eq!(RoleWhy::all().count(), 28);
        let agrees = RoleWhy::SINGLE_LOW.and_name_agrees();
        assert_eq!(agrees.words(), "Single notes, low down, and its name agrees");
        assert_eq!(RoleWhy::NONE.and_name_agrees(), RoleWhy::NONE);
        assert_eq!(RoleWhy::from_code(99), RoleWhy::NONE, "a number this app doesn't know");
        assert_eq!(RoleWhy::from_code(-1), RoleWhy::NONE);
        assert_eq!(RoleWhy::from_words("Something an older app said"), RoleWhy::NONE);
    }
}
