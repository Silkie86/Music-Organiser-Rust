//! Your favourite sounds (the heart in the app) as the plugin's presets in the DAW's own browser (CLAP's preset
//! discovery): Bitwig lists them under the plugin, with each sound's type and tags to filter by, and picking one plays
//! it through the plugin (CLAP's preset load).
//!
//! The DAW asks for the list on its own, now and then (Bitwig when it starts, and when it looks for new presets). The
//! list is read from the app's sound library the way the panel reads it: its copy, read-only, opened and let go at
//! once, leaving out a copy you've hidden as a duplicate. A preset's "load key" is the sound's id in the library.

use crate::audition::Player;
use crate::library;
use clack_extensions::preset_discovery::prelude::*;
use clack_plugin::prelude::PluginError;
use mo_sounds::Sound;
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The provider's own id in the DAW: never changes, or the DAW forgets what it listed.
pub const PROVIDER_ID: &str = "com.silkie86.music-organiser.favourites";
/// The plugin the presets load in.
pub const PLUGIN_ID: &CStr = c"com.silkie86.music-organiser.test";
/// The place the presets are in, as the DAW's browser names it.
pub const PLACE: &CStr = c"Favourite sounds";

/// One favourite as the DAW's browser lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    /// The sound's file name, without its ending ("Kick 04").
    pub name: CString,
    /// The sound's id in the library: what the DAW hands back to load it.
    pub key: CString,
    /// Its type and tags, for the browser's filters.
    pub features: Vec<CString>,
    /// Its last two folders and what the app knows of it ("Drums\Kicks  ·  Kick  ·  0.4 s").
    pub description: CString,
}

/// A text the DAW can take: a nul inside it is a space.
fn text(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

/// How the browser lists the favourites `sounds`.
pub fn presets(sounds: &[Sound]) -> Vec<Preset> {
    sounds
        .iter()
        .map(|sound| {
            let file = library::name(&sound.path);
            let name = file.rfind('.').filter(|&dot| dot > 0).map_or(file, |dot| &file[..dot]);
            let mut features = Vec::new();
            let kind = sound.sound_type.as_str();
            if !kind.is_empty() && !kind.eq_ignore_ascii_case(mo_sounds::types::UNKNOWN) {
                features.push(text(kind));
            }
            for tag in &sound.tags {
                let tag = text(tag);
                if !tag.is_empty() && !features.contains(&tag) {
                    features.push(tag);
                }
            }
            let about: Vec<String> = [library::folder(&sound.path).to_owned(), library::facts(sound)]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            Preset {
                name: text(name),
                key: text(&sound.id.to_string()),
                features,
                description: text(&about.join("  \u{b7}  ")),
            }
        })
        .collect()
}

/// Your favourite sounds in the app's own sound library beside `sounds_db` (its copy), none when there's no library
/// yet. Slow for a big library: never on the DAW's audio thread.
pub fn read_from(sounds_db: &Path) -> Result<Vec<Sound>, String> {
    read_some(sounds_db, None)
}

/// The sounds with these `ids` (all your favourites for None), read as [`read_from`] does.
fn read_some(sounds_db: &Path, ids: Option<&[i64]>) -> Result<Vec<Sound>, String> {
    let copy = mo_sounds::compact::copy_of(sounds_db);
    if !copy.is_file() {
        return Ok(Vec::new());
    }
    let db = match mo_sounds::SoundsDb::open_read_only(&copy) {
        Ok(db) if db.is_copy() => db,
        Ok(_) => return Err("the sound library isn't one this plugin can read".into()),
        Err(e) => return Err(format!("couldn't open the sound library: {e}")),
    };
    let now = chrono::Local::now().naive_local();
    let read = match ids {
        Some(ids) => db.some(ids, now),
        None => db.favourite_ids().and_then(|ids| db.some(&ids, now)),
    };
    read.map_err(|e| format!("couldn't read the sound library: {e}"))
}

/// The app's sound library, or none in the tests' stand-in DAW (tests never read a real folder, rule 11).
pub fn sounds_db(vendor: Option<&CStr>) -> Option<PathBuf> {
    let tests = vendor.map(CStr::to_bytes) == Some(crate::TESTS_VENDOR.as_bytes());
    (!tests).then(|| mo_app::paths::data_dir().join("sounds.db"))
}

/// How the DAW names the favourites' provider.
pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor::new(PROVIDER_ID, "Music Organiser favourites").with_vendor("Music Organiser")
}

/// The favourites' provider, made when the DAW asks for it.
pub fn provider<'a>(indexer: IndexerInfo<'a>, descriptor: &'a ProviderDescriptor) -> ProviderInstance<'a> {
    let sounds_db = sounds_db(indexer.vendor());
    ProviderInstance::new(indexer, descriptor, move |mut indexer| {
        indexer
            .declare_location(LocationInfo {
                name: PLACE,
                flags: Flags::IS_USER_CONTENT | Flags::IS_FAVORITE,
                location: Location::Plugin,
            })
            .map_err(|_| PluginError::Message("The DAW didn't take the favourites' place"))?;
        Ok(Provider { sounds_db })
    })
}

/// Lists the favourites when the DAW asks.
struct Provider {
    sounds_db: Option<PathBuf>,
}

impl ProviderImpl<'_> for Provider {
    fn get_metadata(&mut self, location: Location, receiver: &mut MetadataReceiver) -> Result<(), PluginError> {
        if location != Location::Plugin {
            return Ok(());
        }
        let started = std::time::Instant::now();
        let sounds = match &self.sounds_db {
            Some(db) => read_from(db).map_err(|why| {
                crate::log::took("Presets", &format!("Couldn't list your favourites for the DAW: {why}"), started);
                PluginError::Message("Couldn't read the app's sound library")
            })?,
            None => Vec::new(),
        };
        let presets = presets(&sounds);
        for preset in &presets {
            let Ok(receiver) = receiver.begin_preset(Some(&preset.name), Some(&preset.key)) else {
                return Err(PluginError::Message("The DAW stopped taking presets"));
            };
            receiver
                .add_plugin_id(UniversalPluginId::clap(PLUGIN_ID))
                .set_flags(Flags::IS_USER_CONTENT | Flags::IS_FAVORITE)
                .set_description(&preset.description);
            for feature in &preset.features {
                receiver.add_feature(feature);
            }
        }
        crate::log::took("Presets", &format!("Listed {} favourites for the DAW's browser", presets.len()), started);
        Ok(())
    }
}

/// The sound id in a preset's load key, when it's one of these presets.
pub fn sound_of(location: Location, key: Option<&CStr>) -> Option<i64> {
    match location {
        Location::Plugin => key?.to_str().ok()?.trim().parse().ok(),
        Location::File { .. } => None,
    }
}

/// Plays the sound with this id from the app's library beside `sounds_db`, read on a background thread.
pub fn play(player: &Arc<Player>, sounds_db: PathBuf, id: i64) {
    let player = Arc::clone(player);
    crate::threads::background("Preset load", move || {
        let started = std::time::Instant::now();
        match read_some(&sounds_db, Some(&[id])).map(|mut found| found.pop()) {
            Ok(Some(sound)) => {
                crate::log::took("Presets", &format!("The DAW loaded favourite {id}: playing it"), started);
                player.play(sound.path.clone().into(), 0.0, library::own_tempo(&sound), |_| {});
            }
            Ok(None) => {
                crate::log::took("Presets", &format!("The DAW loaded sound {id}: not in the library now"), started)
            }
            Err(why) => crate::log::took("Presets", &format!("The DAW loaded sound {id}: {why}"), started),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(id: i64, path: &str, kind: &str, tags: &[&str]) -> Sound {
        let mut s = Sound::new(id, path);
        s.sound_type = mo_sounds::Word::new(kind);
        s.tags = tags.iter().map(|t| (*t).to_owned()).collect();
        s.favourite = true;
        s
    }

    #[test]
    fn a_favourite_is_listed_by_its_name_type_and_tags() {
        let listed = presets(&[
            sound(7, r"J:\Samples\Drums\Kicks\Kick 04.wav", "Kick", &["punchy", "Kick"]),
            sound(12, "/samples/.hidden", mo_sounds::types::UNKNOWN, &[]),
        ]);
        assert_eq!(listed[0].name.as_c_str(), c"Kick 04");
        assert_eq!(listed[0].key.as_c_str(), c"7");
        assert_eq!(listed[0].features, [c"Kick".to_owned(), c"punchy".to_owned()]);
        assert_eq!(listed[0].description.to_str().unwrap(), "Drums\\Kicks  \u{b7}  Kick");
        // A name that is all ending keeps it; a type the app doesn't know isn't a filter.
        assert_eq!(listed[1].name.as_c_str(), c".hidden");
        assert!(listed[1].features.is_empty());
    }

    #[test]
    fn a_load_key_is_a_sound_id() {
        assert_eq!(sound_of(Location::Plugin, Some(c"42")), Some(42));
        assert_eq!(sound_of(Location::Plugin, Some(c"kick")), None);
        assert_eq!(sound_of(Location::Plugin, None), None);
        assert_eq!(sound_of(Location::File { path: c"/a.wav" }, Some(c"42")), None);
    }

    #[test]
    fn no_library_yet_lists_nothing() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-folder");
        assert_eq!(read_from(&dir.join("sounds.db")), Ok(Vec::new()));
    }
}
