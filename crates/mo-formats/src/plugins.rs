//! What a plugin's file path and name say about it.

use crate::model::PluginKind;

/// The file endings plugins have.
pub const PLUGIN_EXTENSIONS: [&str; 6] = [".vst3", ".dll", ".clap", ".component", ".vst", ".aaxplugin"];

pub(crate) fn ends_with_ignore_case(s: &str, end: &str) -> bool {
    s.len() >= end.len()
        && s.is_char_boundary(s.len() - end.len())
        && s[s.len() - end.len()..].eq_ignore_ascii_case(end)
}

pub(crate) fn starts_with_ignore_case(s: &str, start: &str) -> bool {
    s.len() >= start.len() && s.is_char_boundary(start.len()) && s[..start.len()].eq_ignore_ascii_case(start)
}

/// The plugin's name from its file: `C:\VstPlugins\Serum_x64.dll` is "Serum_x64".
pub fn name_from_path(path: &str) -> String {
    let name = path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or("");
    for e in PLUGIN_EXTENSIONS {
        if ends_with_ignore_case(name, e) {
            return name[..name.len() - e.len()].to_owned();
        }
    }
    name.to_owned()
}

/// The plugin's format from its file's ending, or "" when the ending doesn't say.
pub fn format_from_path(path: &str) -> &'static str {
    let p = path.to_lowercase();
    if p.ends_with(".vst3") {
        "VST3"
    } else if p.ends_with(".clap") {
        "CLAP"
    } else if p.ends_with(".component") {
        "AU"
    } else if p.ends_with(".aaxplugin") {
        "AAX"
    } else if p.ends_with(".dll") || p.ends_with(".vst") {
        "VST2"
    } else {
        ""
    }
}

/// Plugins that come with FL Studio. Old projects often load FL's own VST builds of them
/// (`C:\Program Files\FL Studio 10\Plugins\VST\Fruity Balance.dll`), which aren't in the usual plugin folders but are
/// in every FL install.
const IMAGE_LINE: [&str; 59] = [
    "3x Osc",
    "Autogun",
    "BassDrum",
    "BeepMap",
    "BooBass",
    "DirectWave",
    "Distructor",
    "Drumaxx",
    "Drumpad",
    "Edison",
    "Effector",
    "Emphasis",
    "Equo",
    "FL Keys",
    "FLEX",
    "FPC",
    "Frequency Shifter",
    "Frequency Splitter",
    "Gross Beat",
    "Groove Machine",
    "Hardcore",
    "Harmless",
    "Harmor",
    "Kepler",
    "Kepler Exo",
    "Love Philter",
    "Luxeverb",
    "Maximus",
    "MiniSynth",
    "Morphine",
    "Multiband Delay",
    "Newtime",
    "Newtone",
    "Ogun",
    "Parametric EQ",
    "Parametric EQ 2",
    "Patcher",
    "Pitch Resonator",
    "Pitch Shifter",
    "Pitcher",
    "Plucked!",
    "PoiZone",
    "Sakura",
    "Sawer",
    "SimSynth",
    "Slicex",
    "Soundgoodizer",
    "Sytrus",
    "Toxic Biohazard",
    "Transient Processor",
    "Transistor Bass",
    "Vintage Bass",
    "Vintage Chorus",
    "Vintage Phaser",
    "Vocodex",
    "Wasp",
    "Wasp XT",
    "Wave Candy",
    "Wave Traveller",
];

fn is_image_line(name: &str) -> bool {
    IMAGE_LINE.iter().any(|n| n.eq_ignore_ascii_case(name.trim()))
}

/// Whether a plugin ships with FL Studio, so it's never "missing" on a PC with FL installed.
pub fn is_fl_studio_bundled(name: &str, vendor: &str, path: &str) -> bool {
    let path = path.replace('/', "\\").to_lowercase();
    starts_with_ignore_case(name, "Fruity ")
        || is_image_line(name)
        || (starts_with_ignore_case(name, "IL ") && is_image_line(&name[3..]))
        || starts_with_ignore_case(vendor, "Image-Line")
        || path.contains(r"\image-line\")
        || in_fl_studio_plugin_folder(&path)
}

/// `\FL Studio<anything but a backslash>\Plugins\` somewhere in a lower-case path with backslashes.
fn in_fl_studio_plugin_folder(path: &str) -> bool {
    path.match_indices(r"\fl studio").any(|(at, m)| {
        let rest = &path[at + m.len()..];
        rest.find('\\').is_some_and(|slash| rest[slash..].starts_with(r"\plugins\"))
    })
}

/// Elektron's Overbridge plugins, by how the boxes show up inside projects, and the box each one is. Longest first,
/// so a name holding "Digitakt II" is never taken for "Digitakt" (and in the C# app's order where they're as long).
const ELEKTRON_OVERBRIDGE: [(&str, &str); 9] = [
    ("Analog Rytm MKII", "Analog Rytm MKII"),
    ("Analog Four MKII", "Analog Four MKII"),
    ("Analog Heat MKII", "Analog Heat MKII"),
    ("Analog Rytm", "Analog Rytm MKII"),
    ("Digitakt II", "Digitakt II"),
    ("Digitone II", "Digitone II"),
    ("Digitakt", "Digitakt"),
    ("Digitone", "Digitone"),
    ("Syntakt", "Syntakt"),
];

/// Plugin names known on every PC, installed or not: the Overbridge ones, which a text scan can trust as plugins.
pub fn is_always_known(name: &str) -> bool {
    ELEKTRON_OVERBRIDGE.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
}

/// The Elektron box a plugin's name refers to, if any.
pub fn elektron_device(name: &str) -> Option<&'static str> {
    let holds = |n: &str| name.as_bytes().windows(n.len()).any(|w| w.eq_ignore_ascii_case(n.as_bytes()));
    // Most names hold none of them: those are done with here, without making any text.
    if !ELEKTRON_OVERBRIDGE.iter().any(|(n, _)| holds(n)) {
        return None;
    }
    let bare = name.to_lowercase().replace("overbridge", "");
    let bare = bare.trim_matches([' ', '-', '_']);
    let exact = ELEKTRON_OVERBRIDGE.iter().find(|(n, _)| n.eq_ignore_ascii_case(bare));
    exact.or_else(|| ELEKTRON_OVERBRIDGE.iter().find(|(n, _)| holds(n))).map(|(_, device)| *device)
}

/// What a plugin is from its name alone: an instrument when it's an Elektron box, otherwise not known.
pub fn kind_of(name: &str) -> PluginKind {
    if elektron_device(name).is_some() { PluginKind::Instrument } else { PluginKind::Unknown }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_elektron_box_a_plugin_name_means() {
        assert_eq!(elektron_device("Digitakt II Overbridge"), Some("Digitakt II"));
        assert_eq!(elektron_device("Overbridge - analog rytm"), Some("Analog Rytm MKII"));
        assert_eq!(elektron_device("My Digitone II rig"), Some("Digitone II"));
        assert_eq!(elektron_device("Serum"), None);
        assert_eq!(kind_of("Syntakt"), PluginKind::Instrument);
        assert!(is_always_known("analog rytm mkii") && !is_always_known("Serum"));
    }

    #[test]
    fn a_plugins_name_and_format_come_from_its_file() {
        assert_eq!(name_from_path(r"C:\VstPlugins\Serum_x64.dll"), "Serum_x64");
        assert_eq!(name_from_path("/Library/Audio/Plug-Ins/VST3/Arcade.VST3/"), "Arcade");
        assert_eq!(name_from_path("Odd name"), "Odd name");
        assert_eq!(format_from_path(r"C:\x\Arcade.vst3"), "VST3");
        assert_eq!(format_from_path(r"C:\x\Serum_x64.DLL"), "VST2");
        assert_eq!(format_from_path(r"C:\x\Thing.clap"), "CLAP");
        assert_eq!(format_from_path(r"C:\x\Thing"), "");
    }

    #[test]
    fn knows_the_plugins_that_come_with_fl_studio() {
        assert!(is_fl_studio_bundled("Fruity Compressor", "", ""));
        assert!(is_fl_studio_bundled("sytrus", "", ""));
        assert!(is_fl_studio_bundled("IL Harmor", "", ""));
        assert!(is_fl_studio_bundled("Anything", "Image-Line Software", ""));
        assert!(is_fl_studio_bundled("Balance", "", r"C:\Program Files\FL Studio 10\Plugins\VST\Balance.dll"));
        assert!(is_fl_studio_bundled("Balance", "", "C:/Program Files/Image-Line/Balance.dll"));
        assert!(!is_fl_studio_bundled("Serum", "Xfer Records", r"C:\Program Files\VstPlugins\Serum_x64.dll"));
        assert!(
            !is_fl_studio_bundled("Odd", "", r"C:\FL Studio\x\y\Plugins\Odd.dll"),
            "Plugins must follow the FL folder"
        );
    }
}
