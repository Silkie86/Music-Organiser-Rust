//! Your preset files (VST3 and VST2 presets) in the DAW's own browser, with the part the app reads in each (bass,
//! lead, pad...) beside the file's own category and tags (CLAP's preset discovery). A try, as Solomon asked
//! (2026-11-13): whether Bitwig lists presets of other plugins that a CLAP plugin points it to, and loads them.
//!
//! The provider tells the DAW the folders the app's Presets page looks in ([`crate::presets::folders`]) and the file
//! types it reads; the DAW walks them itself and hands each file back to be described. Each is read as the app reads
//! it (`mo_presets`), read-only. A preset names the plugin it loads in (its VST3 class id or VST2 id), so the DAW
//! opens that plugin, not this one. Bitwig's own presets (`.bwpreset`) are left to Bitwig, which lists them already.

use clack_extensions::preset_discovery::prelude::*;
use clack_plugin::prelude::PluginError;
use mo_presets::files::{FileFormat, PresetFile};
use std::ffi::{CStr, CString};
use std::path::Path;

/// The provider's own id in the DAW: never changes, or the DAW forgets what it listed.
pub const PROVIDER_ID: &str = "com.silkie86.music-organiser.preset-files";

/// The file types the DAW is asked to hand back.
const TYPES: [(&CStr, &CStr, &CStr); 3] = [
    (c"VST3 preset", c"A VST3 plugin's preset", c"vstpreset"),
    (c"VST2 preset", c"A VST2 plugin's program", c"fxp"),
    (c"VST2 bank", c"A VST2 plugin's bank of programs", c"fxb"),
];

/// A preset file as the DAW's browser lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub name: CString,
    /// The plugin it loads in: the kind ("vst3", "vst2") and the id as CLAP writes it.
    pub abi: &'static CStr,
    pub plugin_id: CString,
    pub creator: Option<CString>,
    /// The part the app reads in it first, then the file's own category and tags.
    pub features: Vec<CString>,
    /// The plugin's name, where the file says it.
    pub description: Option<CString>,
}

/// A text the DAW can take: a nul inside it is a space.
fn text(s: &str) -> CString {
    CString::new(s.trim().replace('\0', " ")).unwrap_or_default()
}

/// A VST3 class id (32 hex digits) as CLAP writes it: "12345678-9ABC-DEF0-1234-56789ABCDEF0".
pub fn vst3_id(class_id: &str) -> Option<String> {
    let hex = mo_formats::plugins::class_id(class_id);
    if hex.is_empty() {
        return None;
    }
    Some(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

/// A VST2 id (its four letters, or the number when they aren't all letters, as `mo_formats::plugins::vst2_id`
/// writes it) as CLAP writes it: the signed number.
pub fn vst2_id(id: &str) -> Option<String> {
    let bytes = id.as_bytes();
    if let Ok(four) = <[u8; 4]>::try_from(bytes)
        && four.iter().all(|b| (0x20..=0x7E).contains(b))
    {
        return Some(i32::from_be_bytes(four).to_string());
    }
    id.trim().parse::<i32>().ok().map(|n| n.to_string())
}

/// How the browser lists a preset file: None for one it shouldn't (Bitwig's own, or one whose plugin isn't known).
pub fn listed(file: &PresetFile) -> Option<Listed> {
    let (abi, plugin_id) = match file.format? {
        FileFormat::VstPreset => (c"vst3", vst3_id(&file.plugin_id)?),
        FileFormat::Fxp | FileFormat::Fxb => (c"vst2", vst2_id(&file.plugin_id)?),
        FileFormat::BwPreset => return None,
    };
    let mut features: Vec<CString> = Vec::new();
    let mut add = |word: &str| {
        let word = text(word);
        if !word.is_empty() && !features.iter().any(|f| f.as_bytes().eq_ignore_ascii_case(word.as_bytes())) {
            features.push(word);
        }
    };
    // The part, as the app's Presets page reads it from the file's category or name.
    if let Some((role, _)) =
        [file.category.as_str(), file.name.as_str()].into_iter().find_map(mo_presets::role::name_clue)
    {
        add(role.label());
    }
    for word in file.category.split(['|', '/']) {
        add(word);
    }
    for tag in &file.tags {
        add(tag);
    }
    let some = |s: &str| Some(text(s)).filter(|t| !t.is_empty());
    Some(Listed {
        name: text(&file.name),
        abi,
        plugin_id: text(&plugin_id),
        creator: some(&file.creator),
        features,
        description: some(&file.plugin),
    })
}

/// How the DAW names the preset files' provider.
pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor::new(PROVIDER_ID, "Music Organiser preset files").with_vendor("Music Organiser")
}

/// The preset files' provider, made when the DAW asks for it. It tells the DAW where to look: the app's preset
/// folders (none in the tests' stand-in DAW: tests never read a real folder, rule 11).
pub fn provider<'a>(indexer: IndexerInfo<'a>, descriptor: &'a ProviderDescriptor) -> ProviderInstance<'a> {
    let tests = indexer.vendor().map(CStr::to_bytes) == Some(crate::TESTS_VENDOR.as_bytes());
    ProviderInstance::new(indexer, descriptor, move |mut indexer| {
        for (name, description, extension) in TYPES {
            indexer
                .declare_filetype(FileType { name, description: Some(description), file_extension: Some(extension) })
                .map_err(|_| PluginError::Message("The DAW didn't take the preset file types"))?;
        }
        let folders = if tests { Vec::new() } else { crate::presets::folders() };
        let mut declared = 0;
        for folder in folders.iter().filter(|f| f.is_dir()) {
            let (Ok(path), Ok(name)) = (
                CString::new(folder.to_string_lossy().as_bytes()),
                CString::new(format!(
                    "Music Organiser: {}",
                    folder.file_name().map_or_else(|| folder.to_string_lossy(), |n| n.to_string_lossy())
                )),
            ) else {
                continue;
            };
            let location =
                LocationInfo { name: &name, flags: Flags::empty(), location: Location::File { path: &path } };
            if indexer.declare_location(location).is_ok() {
                declared += 1;
            }
        }
        crate::log::line("Presets", &format!("Told the DAW's browser of {declared} preset folders"));
        Ok(Provider)
    })
}

/// Describes each preset file the DAW hands back.
struct Provider;

impl ProviderImpl<'_> for Provider {
    fn get_metadata(&mut self, location: Location, receiver: &mut MetadataReceiver) -> Result<(), PluginError> {
        let Location::File { path } = location else { return Ok(()) };
        let Ok(path) = path.to_str() else { return Ok(()) };
        let found = mo_presets::library::read_preset_file(Path::new(path));
        let Some(listed) = found.file.as_ref().ok().and_then(listed) else { return Ok(()) };
        let Ok(receiver) = receiver.begin_preset(Some(&listed.name), None) else {
            return Err(PluginError::Message("The DAW didn't take the preset"));
        };
        receiver.add_plugin_id(UniversalPluginId { abi: listed.abi, id: &listed.plugin_id });
        if let Some(creator) = &listed.creator {
            receiver.add_creator(creator);
        }
        if let Some(description) = &listed.description {
            receiver.set_description(description);
        }
        for feature in &listed.features {
            receiver.add_feature(feature);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_formats::PluginKind;

    fn file(format: FileFormat, plugin_id: &str, name: &str, category: &str) -> PresetFile {
        PresetFile {
            format: Some(format),
            name: name.into(),
            plugin: "Serum".into(),
            plugin_id: plugin_id.into(),
            kind: PluginKind::default(),
            category: category.into(),
            creator: "Someone".into(),
            tags: vec!["Dark".into(), "bass".into()],
            programs: 1,
            wraps_plugin: false,
        }
    }

    #[test]
    fn plugin_ids_as_clap_writes_them() {
        assert_eq!(vst3_id("56534558667350736572756D00000000").unwrap(), "56534558-6673-5073-6572-756D00000000");
        assert_eq!(vst3_id("{56534558-6673-5073-6572-756d00000000}").unwrap(), "56534558-6673-5073-6572-756D00000000");
        assert_eq!(vst3_id("not one"), None);
        assert_eq!(vst2_id("FQ2p").unwrap(), i32::from_be_bytes(*b"FQ2p").to_string());
        assert_eq!(vst2_id("-4382976").unwrap(), "-4382976");
        assert_eq!(vst2_id(""), None);
    }

    #[test]
    fn a_preset_file_is_listed_with_the_apps_part_first() {
        let listed =
            listed(&file(FileFormat::VstPreset, "56534558667350736572756D00000000", "BS Reese", "Bass|Synth")).unwrap();
        assert_eq!(listed.abi, c"vst3");
        assert_eq!(listed.name.as_c_str(), c"BS Reese");
        assert_eq!(listed.features[0].as_c_str(), c"Bass", "the part the app reads");
        // The category's words and the tags, each once, whatever its capitals.
        let words: Vec<&str> = listed.features.iter().map(|f| f.to_str().unwrap()).collect();
        assert_eq!(words, ["Bass", "Synth", "Dark"]);
        assert_eq!(listed.description.as_deref(), Some(c"Serum"));
        assert_eq!(listed.creator.as_deref(), Some(c"Someone"));
        let vst2 = super::listed(&file(FileFormat::Fxp, "FQ2p", "Pad 1", "")).unwrap();
        assert_eq!(vst2.abi, c"vst2");
    }

    #[test]
    fn bitwigs_own_and_unknown_plugins_are_left_out() {
        assert_eq!(listed(&file(FileFormat::BwPreset, "", "x", "")), None);
        assert_eq!(listed(&file(FileFormat::VstPreset, "", "x", "")), None);
        let mut none = file(FileFormat::Fxp, "FQ2p", "x", "");
        none.format = None;
        assert_eq!(listed(&none), None);
    }
}
