//! One clip inside a phrase joined from several clips that play back to back (the C# `PhraseMember`).

use std::fmt;

/// A clip in a joined phrase: the hash its notes have on their own (as the page lists that clip), and where it sits
/// in the phrase, in ticks at [`crate::PPQ`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhraseMember {
    pub hash: String,
    pub start: i64,
    pub length: i64,
}

impl PhraseMember {
    /// The members as `midi.db` keeps them: "HASH@start+length", with spaces between.
    pub fn encode(members: &[PhraseMember]) -> String {
        members.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")
    }

    /// The members from `midi.db`'s text; a piece that doesn't read is left out, as the C# app does.
    pub fn decode(text: &str) -> Vec<PhraseMember> {
        text.split(' ').filter(|p| !p.is_empty()).filter_map(PhraseMember::parse).collect()
    }

    fn parse(part: &str) -> Option<PhraseMember> {
        let at = part.find('@').filter(|at| *at > 0)?;
        let (hash, rest) = (part.get(..at)?, part.get(at + 1..)?);
        let (start, length) = rest.split_once('+')?;
        Some(PhraseMember { hash: hash.to_owned(), start: start.parse().ok()?, length: length.parse().ok()? })
    }
}

impl fmt::Display for PhraseMember {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}+{}", self.hash, self.start, self.length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_as_the_csharp_app() {
        let members = vec![
            PhraseMember { hash: "0A1B2C3D4E5F6071".into(), start: 0, length: 1536 },
            PhraseMember { hash: "FFEEDDCCBBAA9988".into(), start: 1536, length: 768 },
        ];
        let text = PhraseMember::encode(&members);
        assert_eq!(text, "0A1B2C3D4E5F6071@0+1536 FFEEDDCCBBAA9988@1536+768");
        assert_eq!(PhraseMember::decode(&text), members);
        assert_eq!(PhraseMember::decode("@1+2 X@1 X@a+2 X@-384+2").len(), 1);
        assert!(PhraseMember::decode("").is_empty());
    }
}
