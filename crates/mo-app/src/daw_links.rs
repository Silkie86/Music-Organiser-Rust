//! What the app does by itself, a little after each start, about its links in the DAWs and its CLAP plugin (Solomon,
//! 2026-11-25: "both", to "the Bitwig extension and the FL script go in by themselves" and "the installer brings the
//! CLAP plugin"):
//!
//! - Only the installed copy does this (`update::installed`): a launcher build, which may be older or newer than it,
//!   leaves the links to Settings › DAWs, so two copies of the app never take turns putting their own in.
//! - A link that is there but isn't this copy's own is updated, as Settings › DAWs' Install would, when this app is
//!   newer than the one that last wrote the links ([`WRITTEN_BY_KEY`], kept by every write, Install's too). So an
//!   update of the app updates the links, and a newer link put in by hand isn't swapped back for an older one. Only
//!   the link's own files are written (`mo_dawlink::scripts`), and FL's "Let the app make changes" line stays as the
//!   user set it.
//! - A link that isn't there goes in only at the first look ([`FIRST_LOOK_KEY`]), and only into a DAW whose user
//!   folder is there. After that it's the user's choice: one taken out with Take out stays out.
//! - The CLAP plugin the installer put beside the app goes into `Common Files\CLAP` the same way (Solomon picked that
//!   folder, with Windows' Yes): Windows asks once for each version, never while a DAW has the plugin open, and not
//!   again for a version you said no to. A test plugin put in by hand (the same plugin: a DAW would list it twice)
//!   keeps the app from putting it in.
//!
//! Nothing here asks: a copy the DAW has open (Bitwig on Windows) can't be replaced, so the update waits for a start
//! when the DAW is closed, and Settings › DAWs still offers Install.

use mo_dawlink::scripts::{BitwigInstalled, Installed, ScriptState};

use crate::update::Version;

/// The app's own setting kept once the first look at the DAWs has been made (so links only go in by themselves once).
pub const FIRST_LOOK_KEY: &str = "daw_links_first_look";

/// The app's own setting with the version whose CLAP plugin you said no to putting in (Windows asks for each version).
pub const PLUGIN_DECLINED_KEY: &str = "daw_links_plugin_declined";

/// The app's own setting with the version of the app that last put a link into a DAW.
pub const WRITTEN_BY_KEY: &str = "daw_links_written_by";

/// Whether this app (`this`, its version) is newer than the one that last wrote the links. True when none is kept,
/// or it can't be read.
pub fn newer_than_writer(this: &str, written_by: Option<&str>) -> bool {
    match written_by.and_then(Version::parse) {
        None => true,
        Some(writer) => Version::parse(this).is_some_and(|v| v > writer),
    }
}

/// What to do with one DAW's link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Leave,
    /// It's there, but older: the app's own copy goes in.
    Update,
    /// It isn't there, and this is the first look: it goes in.
    PutIn,
}

/// What to do with a DAW's link, from how it stands, whether this is the first look, whether the DAW's user folder
/// is there (an FL Studio user data folder, a `Bitwig Studio` folder), and whether this app is newer than the one that
/// last wrote the links ([`newer_than_writer`]).
pub fn step(state: ScriptState, first_look: bool, daw_there: bool, newer: bool) -> Step {
    match state {
        ScriptState::Outdated if newer => Step::Update,
        ScriptState::NotInstalled if first_look && daw_there => Step::PutIn,
        _ => Step::Leave,
    }
}

/// What was done at this start, for the card.
#[derive(Debug, Default)]
pub struct Done {
    pub fl: Option<(Step, Installed)>,
    pub bitwig: Option<(Step, BitwigInstalled)>,
    /// The CLAP plugin, into `Common Files\CLAP`.
    pub plugin: Option<(Step, Installed)>,
}

/// `a`, `a and b`, `a, b and c`.
fn listed(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The card's title and words, when a link went in or was updated. None when nothing went in: a copy that couldn't be
/// written (the DAW has it open) is only logged, as it's tried again at the next start and Settings › DAWs shows it.
pub fn card(done: &Done) -> Option<(String, String)> {
    let mut put_in: Vec<&str> = Vec::new();
    let mut updated: Vec<&str> = Vec::new();
    let mut words: Vec<&str> = Vec::new();
    if let Some((step, fl)) = &done.fl
        && !fl.went.is_empty()
    {
        if *step == Step::PutIn {
            put_in.push("FL Studio");
            words.push(
                "Restart FL Studio, then in Options \u{203a} MIDI settings pick DAWmate as the controller type of its \
                 port.",
            );
        } else {
            updated.push("FL Studio");
            words.push("Restart FL Studio to load the new script.");
        }
    }
    if let Some((step, bw)) = &done.bitwig
        && !bw.installed.went.is_empty()
    {
        if *step == Step::PutIn {
            put_in.push("Bitwig");
            words.push("Bitwig picks up the extension by itself (restart Bitwig if it doesn't).");
        } else {
            updated.push("Bitwig");
            words.push("Bitwig picks up the new extension by itself (restart Bitwig if it doesn't).");
        }
    }
    let links = !put_in.is_empty() || !updated.is_empty();
    if let Some((step, plugin)) = &done.plugin
        && !plugin.went.is_empty()
    {
        if *step == Step::PutIn {
            put_in.push("your CLAP plug-ins");
            words.push(
                "Add the DAWmate plugin to a track in your DAW (in Bitwig, rescan the plug-ins in Settings \u{203a} \
                 Locations if it isn't listed).",
            );
        } else {
            updated.push("your CLAP plug-ins");
            words.push("Restart your DAW for the new DAWmate plugin.");
        }
    }
    let title = match (put_in.is_empty(), updated.is_empty()) {
        (true, true) => return None,
        (false, true) => format!("DAWmate is now in {}", listed(&put_in)),
        (true, false) => format!("DAWmate is updated in {}", listed(&updated)),
        (false, false) => format!("DAWmate is now in {}, and updated in {}", listed(&put_in), listed(&updated)),
    };
    let mut detail = words.join(" ");
    if links {
        detail.push_str(" Settings \u{203a} DAWs can take the links out again.");
    }
    Some((title, detail))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn an_older_link_is_updated_only_by_a_newer_app() {
        assert_eq!(step(ScriptState::Outdated, false, true, true), Step::Update);
        assert_eq!(step(ScriptState::Outdated, true, true, true), Step::Update);
        // The same version or an older one wrote it last (a launcher build's Install, say): left as it is.
        assert_eq!(step(ScriptState::Outdated, false, true, false), Step::Leave);
    }

    #[test]
    fn newer_than_writer_goes_by_the_version() {
        assert!(newer_than_writer("0.9.0-beta.2", None));
        assert!(newer_than_writer("0.9.0-beta.2", Some("not a version")));
        assert!(newer_than_writer("0.9.0-beta.2", Some("0.9.0-beta.1")));
        assert!(newer_than_writer("0.9.0", Some("0.9.0-beta.2")));
        assert!(!newer_than_writer("0.9.0-beta.2", Some("0.9.0-beta.2")));
        assert!(!newer_than_writer("0.9.0-beta.1", Some("0.9.0-beta.2")));
    }

    #[test]
    fn a_missing_link_goes_in_only_at_the_first_look_and_only_where_the_daw_is() {
        assert_eq!(step(ScriptState::NotInstalled, true, true, true), Step::PutIn);
        assert_eq!(step(ScriptState::NotInstalled, true, true, false), Step::PutIn);
        // Taken out with Take out after the first look: it stays out.
        assert_eq!(step(ScriptState::NotInstalled, false, true, true), Step::Leave);
        // No FL Studio or Bitwig folder on this computer.
        assert_eq!(step(ScriptState::NotInstalled, true, false, true), Step::Leave);
    }

    #[test]
    fn a_link_that_is_the_apps_own_is_left() {
        assert_eq!(step(ScriptState::Installed, true, true, true), Step::Leave);
        assert_eq!(step(ScriptState::Installed, false, true, true), Step::Leave);
    }

    fn went(folder: &str) -> Installed {
        Installed { went: vec![PathBuf::from(folder)], ..Installed::default() }
    }

    #[test]
    fn the_card_names_what_went_in_and_what_was_updated() {
        let done = Done {
            fl: Some((Step::PutIn, went("F"))),
            bitwig: Some((Step::Update, BitwigInstalled { installed: went("B"), ..BitwigInstalled::default() })),
            plugin: Some((Step::PutIn, went("C"))),
        };
        let (title, detail) = card(&done).unwrap();
        assert_eq!(title, "DAWmate is now in FL Studio and your CLAP plug-ins, and updated in Bitwig");
        assert!(detail.contains("pick DAWmate as the controller type"), "{detail}");
        assert!(detail.contains("new extension"), "{detail}");
        assert!(detail.contains("rescan the plug-ins"), "{detail}");
        assert!(detail.ends_with("Settings \u{203a} DAWs can take the links out again."), "{detail}");
    }

    #[test]
    fn a_card_for_the_plugin_alone_offers_no_take_out() {
        let done = Done { plugin: Some((Step::Update, went("C"))), ..Done::default() };
        let (title, detail) = card(&done).unwrap();
        assert_eq!(title, "DAWmate is updated in your CLAP plug-ins");
        assert_eq!(detail, "Restart your DAW for the new DAWmate plugin.");
        assert_eq!(listed(&["a", "b", "c"]), "a, b and c");
    }

    #[test]
    fn no_card_when_nothing_went_in() {
        assert!(card(&Done::default()).is_none());
        let failed = Installed { failed: vec![(PathBuf::from("F"), "in use".into())], ..Installed::default() };
        assert!(card(&Done { fl: Some((Step::Update, failed)), ..Done::default() }).is_none());
    }
}
