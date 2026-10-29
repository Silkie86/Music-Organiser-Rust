//! What a plugin's file path and name say about it.

/// The file endings plugins have.
pub const PLUGIN_EXTENSIONS: [&str; 6] = [".vst3", ".dll", ".clap", ".component", ".vst", ".aaxplugin"];

fn ends_with_ignore_case(s: &str, end: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

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
