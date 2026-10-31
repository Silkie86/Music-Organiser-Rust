//! The page's choices, as the C# Presets window had them: who made the plugin (all, third-party, the DAW's own) and
//! what it is (all, synths, effects), each with how many presets it would show; plus the part guessed, the plugin and
//! words to search for.

use mo_formats::PluginKind;
use mo_formats::presets::PresetBlob;

use crate::role::Role;

/// Who made the plugin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Maker {
    #[default]
    All,
    ThirdParty,
    /// The DAW's own plugins (FL Studio's).
    Native,
}

/// What it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Type {
    #[default]
    All,
    Synths,
    Effects,
}

/// The maker and type choices. A preset whose kind isn't known (a Bitwig plugin's, as the project doesn't say) passes
/// the type choice, as in the C# app's `PresetFilter`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub maker: Maker,
    pub kind: Type,
}

impl Choice {
    pub fn matches(&self, native: bool, kind: PluginKind) -> bool {
        let maker = match self.maker {
            Maker::All => true,
            Maker::ThirdParty => !native,
            Maker::Native => native,
        };
        let kind = match (self.kind, kind) {
            (Type::All, _) | (_, PluginKind::Unknown) => true,
            (Type::Synths, k) => k == PluginKind::Instrument,
            (Type::Effects, k) => matches!(k, PluginKind::Effect | PluginKind::NoteEffect),
        };
        maker && kind
    }

    pub fn matches_blob(&self, p: &PresetBlob) -> bool {
        self.matches(p.native, p.kind)
    }
}

/// How many presets each choice would show, with the other choice as it is (the counts beside each choice).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub all_makers: usize,
    pub third_party: usize,
    pub native: usize,
    pub all_types: usize,
    pub synths: usize,
    pub effects: usize,
}

/// Counts the choices over presets given as (native, kind), in one pass.
pub fn counts(choice: Choice, presets: impl IntoIterator<Item = (bool, PluginKind)>) -> Counts {
    let mut c = Counts::default();
    let with_maker = |maker| Choice { maker, ..choice };
    let with_type = |kind| Choice { kind, ..choice };
    for (native, kind) in presets {
        let hit = |ch: Choice| usize::from(ch.matches(native, kind));
        c.all_makers += hit(with_maker(Maker::All));
        c.third_party += hit(with_maker(Maker::ThirdParty));
        c.native += hit(with_maker(Maker::Native));
        c.all_types += hit(with_type(Type::All));
        c.synths += hit(with_type(Type::Synths));
        c.effects += hit(with_type(Type::Effects));
    }
    c
}

/// Where each kind of file can be loaded: the line under the list.
pub fn load_hint(extension: &str) -> &'static str {
    match extension.to_lowercase().as_str() {
        ".bwpreset" => "open it from Bitwig's browser or drag it onto a track",
        ".fxp" | ".fxb" => "load into the VST2 version of the plugin",
        ".vstpreset" => "load into the VST3 version of the plugin",
        ".flstate" => "only FL Studio can load these",
        ".vst3state" => "raw VST3 state, most DAWs can't load it directly",
        _ => "",
    }
}

/// The details line under a preset's name: where it sits, synth or effect, an FL plugin, the file type (the C#
/// `PresetRow.Details`).
pub fn details(p: &PresetBlob) -> String {
    let kind = match p.kind {
        PluginKind::Instrument => "Synth",
        PluginKind::Effect | PluginKind::NoteEffect => "Effect",
        PluginKind::Unknown => "",
    };
    let native = if p.native { "FL Studio plugin" } else { "" };
    [p.place.as_str(), kind, native, p.extension.trim_start_matches('.')]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Every word of a search is somewhere in the texts (letter case aside), as the Songs page searches.
pub fn search_matches(search: &str, texts: &[&str]) -> bool {
    let haystack = texts.iter().map(|t| t.to_lowercase()).collect::<Vec<_>>().join("\u{1f}");
    search.split_whitespace().all(|w| haystack.contains(&w.to_lowercase()))
}

/// The part filter: none means any.
pub fn role_matches(wanted: Option<Role>, guessed: Option<Role>) -> bool {
    wanted.is_none() || wanted == guessed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_match_as_the_c_sharp_filter() {
        let any = Choice::default();
        let synths = Choice { kind: Type::Synths, ..any };
        let fl_effects = Choice { maker: Maker::Native, kind: Type::Effects };
        assert!(any.matches(true, PluginKind::Effect));
        assert!(synths.matches(false, PluginKind::Instrument) && !synths.matches(false, PluginKind::Effect));
        assert!(synths.matches(false, PluginKind::Unknown), "an unknown kind passes the type choice");
        assert!(fl_effects.matches(true, PluginKind::NoteEffect) && !fl_effects.matches(false, PluginKind::Effect));
    }

    #[test]
    fn counts_each_choice_with_the_other_as_it_is() {
        let list = [(false, PluginKind::Instrument), (true, PluginKind::Effect), (true, PluginKind::Instrument)];
        let c = counts(Choice { maker: Maker::Native, kind: Type::All }, list);
        assert_eq!((c.all_makers, c.third_party, c.native), (3, 1, 2));
        assert_eq!((c.all_types, c.synths, c.effects), (2, 1, 1));
    }

    #[test]
    fn searches_every_word() {
        assert!(search_matches("serum bass", &["Serum", "Sub Bass", "Song"]));
        assert!(!search_matches("serum lead", &["Serum", "Sub Bass"]));
        assert!(search_matches("", &[]));
        assert_eq!(load_hint(".FXB"), "load into the VST2 version of the plugin");
    }
}
