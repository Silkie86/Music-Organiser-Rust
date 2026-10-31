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
}
