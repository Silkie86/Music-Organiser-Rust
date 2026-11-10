//! The DAW check: which CLAP extensions the DAW (the "host") has, asked for one by one when the plugin starts. The
//! answer is shown on the panel's DAW check view, so a look at it in Bitwig says which of the plan's extensions can be
//! used there (the plan: "use every CLAP extension the current Bitwig supports", Solomon 2026-11-07).
//!
//! Only extensions the DAW itself offers to a plugin can be asked about this way. Some extensions go the other way
//! (the plugin offers them and the DAW calls them, such as param-indication or preset discovery): those show up only
//! once the plugin offers them and the DAW uses them, in later steps.

use clack_plugin::host::HostInfo;
use std::ffi::CStr;

/// One extension to ask the DAW about: what it's for, in plain words, and the ids it may go by (the final id first,
/// then the older "draft" id some DAWs still use).
pub struct Asked {
    pub name: &'static str,
    pub ids: &'static [&'static CStr],
}

/// What the DAW said about one extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offered {
    pub name: &'static str,
    /// The id the DAW has it by, or the first id asked when it has none of them.
    pub id: String,
    pub offered: bool,
}

/// Every extension a DAW can offer a plugin, as the CLAP headers name them (checked against free-audio/clap on
/// 2026-11-10). Asking about an id a DAW doesn't know is harmless: it just says no.
pub const ASKED: &[Asked] = &[
    Asked { name: "Log lines in the DAW's log", ids: &[c"clap.log"] },
    Asked { name: "Which thread is which", ids: &[c"clap.thread-check"] },
    Asked { name: "The DAW's own worker threads", ids: &[c"clap.thread-pool"] },
    Asked { name: "Timers on the DAW's main thread", ids: &[c"clap.timer-support"] },
    Asked { name: "Watching files (Linux and Mac)", ids: &[c"clap.posix-fd-support"] },
    Asked { name: "The plugin's window", ids: &[c"clap.gui"] },
    Asked { name: "Audio in and out", ids: &[c"clap.audio-ports"] },
    Asked { name: "Audio port layouts", ids: &[c"clap.audio-ports-config"] },
    Asked { name: "Notes in and out", ids: &[c"clap.note-ports"] },
    Asked { name: "Note names (drum pads)", ids: &[c"clap.note-name"] },
    Asked { name: "Knobs the DAW can automate", ids: &[c"clap.params"] },
    Asked {
        name: "Knobs on the DAW's controller pages",
        ids: &[c"clap.remote-controls/2", c"clap.remote-controls.draft/2"],
    },
    Asked { name: "Voices for per-note modulation", ids: &[c"clap.voice-info"] },
    Asked { name: "Saving settings in the project", ids: &[c"clap.state"] },
    Asked { name: "Delay the plugin adds", ids: &[c"clap.latency"] },
    Asked { name: "Sound after the notes stop", ids: &[c"clap.tail"] },
    Asked { name: "The track's name and colour", ids: &[c"clap.track-info/1", c"clap.track-info.draft/1"] },
    Asked {
        name: "Right-click menus shared with the DAW",
        ids: &[c"clap.context-menu/1", c"clap.context-menu.draft/0"],
    },
    Asked { name: "Loading the plugin's presets", ids: &[c"clap.preset-load/2", c"clap.preset-load.draft/2"] },
    Asked { name: "The DAW's own undo", ids: &[c"clap.undo/4"] },
    Asked { name: "Starting and stopping the DAW", ids: &[c"clap.transport-control/2", c"clap.transport-control/1"] },
    Asked { name: "A folder for the plugin's files in the project", ids: &[c"clap.resource-directory/1"] },
    Asked { name: "Note tuning", ids: &[c"clap.tuning/2"] },
    Asked { name: "Custom events", ids: &[c"clap.event-registry"] },
    Asked { name: "Surround sound", ids: &[c"clap.surround/4", c"clap.surround.draft/4"] },
    Asked { name: "Ambisonic sound", ids: &[c"clap.ambisonic/3", c"clap.ambisonic.draft/3"] },
    Asked { name: "Small curve pictures (EQ, envelope)", ids: &[c"clap.mini-curve-display/3"] },
    Asked { name: "Gain meters", ids: &[c"clap.gain-adjustment-metering/0"] },
    Asked { name: "Where the plugin sits in the project", ids: &[c"clap.project-location/2"] },
    Asked { name: "Scratch memory for the audio thread", ids: &[c"clap.scratch-memory/1"] },
    Asked { name: "Web-page windows (not used: no web UI)", ids: &[c"clap.webview/3"] },
    Asked { name: "Getting ready in the background", ids: &[c"clap.background-activation/1"] },
    Asked { name: "Progress of background work", ids: &[c"clap.background-progress/1"] },
    Asked { name: "Sending knob changes while stopped", ids: &[c"clap.flush-events/1"] },
    Asked { name: "Which octave the DAW calls middle C", ids: &[c"clap.octave-number/1"] },
    Asked { name: "Where a knob change came from", ids: &[c"clap.params-origin/1"] },
    Asked { name: "Which knob the mouse is over", ids: &[c"clap.param-hovered/1"] },
];

/// Asks the DAW about every extension in [`ASKED`]. CLAP lets a plugin ask from `init` on, from any thread.
pub fn ask(host: &HostInfo<'_>) -> Vec<Offered> {
    ASKED
        .iter()
        .map(|asked| {
            let found = asked.ids.iter().find(|id| offers(host, id));
            let id = found.or(asked.ids.first()).map(|id| id.to_string_lossy().into_owned()).unwrap_or_default();
            Offered { name: asked.name, id, offered: found.is_some() }
        })
        .collect()
}

/// Whether the DAW has the extension `id`.
#[allow(unsafe_code)]
fn offers(host: &HostInfo<'_>, id: &CStr) -> bool {
    let raw = host.as_raw();
    let Some(get) = raw.get_extension else { return false };
    // SAFETY: `get` is the DAW's own function for this `clap_host`, which `HostInfo` keeps valid while the plugin
    // lives; CLAP says it may be called from any thread with any id (a null pointer back means "not here"), and `id` is
    // a whole C string that outlives the call.
    !unsafe { get(raw, id.as_ptr()) }.is_null()
}

/// The DAW's name and version, for the panel's top line: "Bitwig Studio 6.1.6".
pub fn host_line(host: &HostInfo<'_>) -> String {
    let name = host.name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let version = host.version().map(|v| v.to_string_lossy().into_owned()).unwrap_or_default();
    let line = format!("{} {}", name.trim(), version.trim());
    if line.trim().is_empty() { "An unnamed DAW".to_owned() } else { line.trim().to_owned() }
}

/// The DAW check view's top line: how many it has.
pub fn summary(offered: &[Offered]) -> String {
    let yes = offered.iter().filter(|o| o.offered).count();
    format!(
        "The DAW has {yes} of the {} CLAP extensions a DAW can offer a plugin. Extensions the plugin offers the DAW \
         (presets in its browser, knob colours) aren't in this list: they're tried as each is built.",
        offered.len()
    )
}

/// The whole DAW check as plain text, for Copy list: the DAW's name, the count, then one line per extension.
pub fn as_text(host: &str, offered: &[Offered]) -> String {
    let yes = offered.iter().filter(|o| o.offered).count();
    let mut text = format!("{host}: {yes} of {} CLAP extensions\n", offered.len());
    for o in offered {
        text.push_str(&format!("{}  {}  {}\n", if o.offered { "Yes" } else { "No " }, o.id, o.name));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_text_has_the_daw_and_every_line() {
        let offered = [
            Offered { name: "Log lines in the DAW's log", id: "clap.log".into(), offered: true },
            Offered { name: "Note tuning", id: "clap.tuning/2".into(), offered: false },
        ];
        assert_eq!(
            as_text("Bitwig Studio 6.1.7", &offered),
            "Bitwig Studio 6.1.7: 1 of 2 CLAP extensions\n\
             Yes  clap.log  Log lines in the DAW's log\n\
             No   clap.tuning/2  Note tuning\n"
        );
    }

    #[test]
    fn every_extension_is_asked_by_a_clap_id() {
        for asked in ASKED {
            assert!(!asked.ids.is_empty(), "{} has no id", asked.name);
            for id in asked.ids {
                let id = id.to_str().unwrap();
                assert!(id.starts_with("clap.") && id.is_ascii(), "{id}");
            }
        }
        // One line per extension.
        let mut ids: Vec<_> = ASKED.iter().map(|a| a.ids[0]).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ASKED.len());
    }

    #[test]
    fn summary_counts_what_the_daw_has() {
        let offered = vec![
            Offered { name: "a", id: "clap.a".into(), offered: true },
            Offered { name: "b", id: "clap.b".into(), offered: false },
        ];
        assert!(summary(&offered).starts_with("The DAW has 1 of the 2 "));
    }
}
