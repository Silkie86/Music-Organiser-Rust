//! What the app does by itself, a little after each start, about its links in the DAWs and its CLAP plugin (Solomon,
//! 2026-11-25: "both", to "the Bitwig extension and the FL script go in by themselves" and "the installer brings the
//! CLAP plugin"):
//!
//! - Only the installed copy does this (`update::installed`): a launcher build, which may be older or newer than it,
//!   leaves the links to Settings › DAWs, so two copies of the app never take turns putting their own in.
//! - A link that is there but isn't this copy's own is updated, as Settings › DAWs' Install would, when this app is
//!   newer than the one that last wrote that link ([`FL_BY_KEY`] and the others, kept by every write that went into
//!   every folder, Install's too: [`went_everywhere`]). A copy that couldn't be written, because the DAW had it open, is
//!   tried again at the next start, and only the copies that aren't the app's own yet are written then. So an update
//!   of the app updates the links, and a newer link put in by hand isn't swapped back for an older one. Only the link's
//!   own files are written (`mo_dawlink::scripts`), and FL's "Let the app make changes" line stays as the user set it.
//! - A link that isn't there goes in only at the first look ([`FIRST_LOOK_KEY`]), and only into a DAW whose user
//!   folder is there. After that it's the user's choice: one taken out with Take out stays out.
//! - The CLAP plugin the installer put beside the app goes into `Common Files\CLAP` the same way (Solomon picked that
//!   folder, with Windows' Yes): put in whenever it isn't there and no version has put it in yet ([`plugin_step`]),
//!   updated as the links are. Windows asks once for each version, never while a DAW has the plugin loaded, not again
//!   for a version you said no to, and after a step that failed only on another day. A test plugin put in by hand
//!   (the same plugin: a DAW would list it twice) keeps the app from putting it in, and makes it forget that a version
//!   put its own in (`tools/dev/install-test-plugin.ps1` takes that one out), so the app's own goes back in, with
//!   Windows' Yes, once the test plugin is gone.
//! - Settings › DAWs has a row for the plugin too, with Install (or Update) and Take out, each through Windows' Yes
//!   (the 26 Nov review's DK-2). Install keeps that this version put it in, as the look does; Take out keeps that you
//!   took it out ([`PLUGIN_TAKEN_OUT_KEY`]), so the look leaves it out, whatever version comes, until Install
//!   ([`plugin_kept_after`]).
//! - None of this runs when the settings can't be kept (what it did would be forgotten and done again), or after
//!   Install or Take out on Settings › DAWs in this run (that's your choice). Take out there also counts as the first
//!   look, so a link taken out before the first look isn't put back by it.
//!
//! Only the plugin asks (Windows' question, needed for `Common Files\CLAP`): a link's copy the DAW has open (Bitwig on
//! Windows) can't be replaced, so its update waits for a start when the DAW is closed, and Settings › DAWs offers
//! Update meanwhile.

use std::path::PathBuf;

use mo_dawlink::scripts::{BitwigInstalled, Installed, ScriptState};

use crate::update::Version;

/// The app's own setting kept once the first look at the DAWs has been made (so links only go in by themselves once).
pub const FIRST_LOOK_KEY: &str = "daw_links_first_look";

/// The app's own settings with the version of the app that last put each link in (FL Studio's script, Bitwig's
/// extension, the CLAP plugin): kept for each one only when it went in, by the look or by Settings › DAWs' Install.
pub const FL_BY_KEY: &str = "daw_links_fl_by";
pub const BITWIG_BY_KEY: &str = "daw_links_bitwig_by";
pub const PLUGIN_BY_KEY: &str = "daw_links_plugin_by";

/// The app's own setting kept while the CLAP plugin is out because you took it out on Settings › DAWs: the look
/// never puts it in or updates it then. Install on Settings › DAWs blanks it (blank counts as none).
pub const PLUGIN_TAKEN_OUT_KEY: &str = "daw_links_plugin_taken_out";

/// The app's own setting with the version whose CLAP plugin you said no to putting in (Windows asks for each version).
pub const PLUGIN_DECLINED_KEY: &str = "daw_links_plugin_declined";

/// The app's own setting with the version and day the administrator step last couldn't put the plugin in: it's tried
/// again on another day, not at every start.
pub const PLUGIN_FAILED_KEY: &str = "daw_links_plugin_failed";

/// The app's own setting with the Extensions folder Bitwig was last seen loading the extension from (the link's
/// `loadedFrom`), so the look updates that copy too when Bitwig isn't open to say.
pub const BITWIG_LOADED_FROM_KEY: &str = "bitwig_loaded_from";

/// What [`PLUGIN_FAILED_KEY`] keeps: the version, a space, the day.
pub fn failed_mark(version: &str, day: &str) -> String {
    format!("{version} {day}")
}

/// Whether this app (`this`, its version) is newer than the one that last wrote the links. True when none is kept,
/// or it can't be read.
pub fn newer_than_writer(this: &str, written_by: Option<&str>) -> bool {
    match written_by.and_then(Version::parse) {
        None => true,
        Some(writer) => Version::parse(this).is_some_and(|v| v > writer),
    }
}

/// Whether a write of a link counts as this version's, so its `_BY` key ([`FL_BY_KEY`] and the others) is kept: it went
/// in, and into every folder. When a folder couldn't take it (Bitwig had that copy open), the key isn't kept, so the
/// next start tries that folder again (the 26 Nov review's DK-1).
pub fn went_everywhere(done: &Installed) -> bool {
    !done.went.is_empty() && done.failed.is_empty()
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

/// What to do with the CLAP plugin: put in when it isn't there and no version has put it in yet (`put_in_before`:
/// [`PLUGIN_BY_KEY`] is kept), so one taken out by hand stays out; updated when this app is newer than the one that
/// wrote it. Left alone while you've taken it out on Settings › DAWs (`taken_out`: [`PLUGIN_TAKEN_OUT_KEY`] is kept).
pub fn plugin_step(state: ScriptState, put_in_before: bool, newer: bool, taken_out: bool) -> Step {
    match state {
        _ if taken_out => Step::Leave,
        ScriptState::Outdated if newer => Step::Update,
        ScriptState::NotInstalled if !put_in_before => Step::PutIn,
        _ => Step::Leave,
    }
}

/// A choice on Settings › DAWs' plugin row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginChoice {
    /// Install or Update: the app's own plugin goes into `Common Files\CLAP`.
    Install,
    /// Take out: the app's own plugin comes out of it.
    TakeOut,
}

/// What a choice on the plugin row did, once Windows' question was answered (or not asked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginDone {
    /// It went in (it's the app's own now), or came out (nothing of the app's is left there).
    Went,
    /// You said no to Windows' question: nothing changed.
    SaidNo,
    /// A program has the plugin loaded (a DAW, or its plug-in host): Windows was never asked, as the file can't change.
    InUse,
    /// Windows' step ran, but the plugin there isn't as it should be after it: most likely a DAW held it.
    NotChanged,
    /// It couldn't be done, with why (Windows can't be asked from here, there's no plugin beside the app...): closing
    /// the DAWs wouldn't help.
    Failed(String),
}

/// The app's own settings to keep after a choice on the plugin row (`version`: this app's), by their keys, a blank
/// value taking one away. Install that went keeps that this version put it in (as the look does, so the look only
/// updates it from a newer version) and that it isn't taken out any more; Take out that went keeps that you took it
/// out, so the look leaves it out until Install. A choice that didn't happen keeps nothing: things stay as they were.
pub fn plugin_kept_after(choice: PluginChoice, done: &PluginDone, version: &str) -> Vec<(&'static str, String)> {
    if *done != PluginDone::Went {
        return Vec::new();
    }
    match choice {
        PluginChoice::Install => vec![(PLUGIN_BY_KEY, version.to_owned()), (PLUGIN_TAKEN_OUT_KEY, String::new())],
        PluginChoice::TakeOut => vec![(PLUGIN_TAKEN_OUT_KEY, version.to_owned())],
    }
}

/// What a choice on the plugin row did, in words, for under the DAWs tab's cards. `folder`: where the plugin goes.
pub fn plugin_note(choice: PluginChoice, done: &PluginDone, folder: &std::path::Path) -> String {
    let (press, verb) = match choice {
        PluginChoice::Install => ("Install", "put the plugin in"),
        PluginChoice::TakeOut => ("Take out", "take the plugin out"),
    };
    match (choice, done) {
        (PluginChoice::Install, PluginDone::Went) => format!(
            "Put the DAWmate plugin in {}. Add it to a track in your DAW (in Bitwig, rescan the plug-ins in Settings \
             \u{203a} Locations if it isn't listed; a DAW that was open loads the new one when it starts again).",
            folder.display()
        ),
        (PluginChoice::TakeOut, PluginDone::Went) => format!(
            "Took the DAWmate plugin out of {}. It stays out (the app doesn't put it back by itself) until you press \
             Install.",
            folder.display()
        ),
        (_, PluginDone::SaidNo) => "You said no to Windows' question, so nothing changed.".to_owned(),
        (_, PluginDone::InUse) => format!(
            "A program has the DAWmate plugin loaded (a DAW, or its plug-in host), so Windows can't let anyone change \
             it: close your DAWs, then press {press} again."
        ),
        // Only what a DAW can cause says to close the DAWs.
        (_, PluginDone::NotChanged) => {
            format!("Couldn't {verb}: Windows' step didn't change it. Close your DAWs, then press {press} again.")
        }
        (_, PluginDone::Failed(why)) => {
            let why = why.trim_end_matches('.');
            format!("Couldn't {verb}: {why}.")
        }
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

/// The folders of a write that failed, by their full paths: `a`, `a and b`.
fn folders(failed: &[(PathBuf, String)]) -> String {
    let names: Vec<String> = failed.iter().map(|(f, _)| f.display().to_string()).collect();
    listed(&names.iter().map(String::as_str).collect::<Vec<_>>())
}

/// The card's title and words, when a link went in or was updated. None when nothing went in: a copy that couldn't be
/// written (the DAW has it open) is only logged then, as it's tried again at the next start and Settings › DAWs shows
/// it. When it went into some folders and not others, the card says so and names the ones it didn't (DK-1).
pub fn card(done: &Done) -> Option<(String, String)> {
    let mut put_in: Vec<&str> = Vec::new();
    let mut updated: Vec<&str> = Vec::new();
    // Went into some folders, not all: by the step, so the title says what it was.
    let mut partly_in: Vec<&str> = Vec::new();
    let mut partly_updated: Vec<&str> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut sort = |name, step: Step, all: bool| match (step == Step::PutIn, all) {
        (true, true) => put_in.push(name),
        (false, true) => updated.push(name),
        (true, false) => partly_in.push(name),
        (false, false) => partly_updated.push(name),
    };
    if let Some((step, fl)) = &done.fl
        && !fl.went.is_empty()
    {
        sort("FL Studio", *step, fl.failed.is_empty());
        if *step == Step::PutIn {
            words.push(
                "Restart FL Studio, then in Options \u{203a} MIDI settings pick DAWmate as the controller type of its \
                 port."
                    .to_owned(),
            );
        } else if !fl.removed.is_empty() {
            // The copy from before the rename went: FL's port still names that script, which is gone (DK-6).
            words.push(
                "Restart FL Studio, then in Options \u{203a} MIDI settings pick DAWmate as the controller type of the \
                 port the Music Organiser script was on: that older copy is taken out."
                    .to_owned(),
            );
        } else {
            words.push("Restart FL Studio to load the new script.".to_owned());
        }
        for (file, _) in &fl.left {
            words.push(format!(
                "The copy from before the rename, {}, couldn't be taken out, so FL Studio may list both: pick DAWmate.",
                file.display()
            ));
        }
        if !fl.failed.is_empty() {
            words.push(format!(
                "Couldn't put the script in {}: install it again from Settings \u{203a} DAWs once that folder can \
                 take it.",
                folders(&fl.failed)
            ));
        }
    }
    if let Some((step, bw)) = &done.bitwig
        && !bw.installed.went.is_empty()
    {
        sort("Bitwig", *step, bw.installed.failed.is_empty());
        if bw.installed.failed.is_empty() {
            words.push(if *step == Step::PutIn {
                "Bitwig picks up the extension by itself (restart Bitwig if it doesn't).".to_owned()
            } else {
                "Bitwig picks up the new extension by itself (restart Bitwig if it doesn't).".to_owned()
            });
        } else {
            // Most likely the copy Bitwig loads, which it keeps open: never said to be updated then.
            words.push(format!(
                "Couldn't put the new extension in {}: Bitwig keeps the copy it loads open, so it may still run the \
                 older one from there. It goes in at a later start when Bitwig is closed, or close Bitwig and install \
                 it again from Settings \u{203a} DAWs.",
                folders(&bw.installed.failed)
            ));
        }
    }
    let links = done.fl.as_ref().is_some_and(|(_, fl)| !fl.went.is_empty())
        || done.bitwig.as_ref().is_some_and(|(_, bw)| !bw.installed.went.is_empty());
    if let Some((step, plugin)) = &done.plugin
        && !plugin.went.is_empty()
    {
        sort("your CLAP plug-ins", *step, plugin.failed.is_empty());
        words.push(if *step == Step::PutIn {
            "Add the DAWmate plugin to a track in your DAW (in Bitwig, rescan the plug-ins in Settings \u{203a} \
             Locations if it isn't listed)."
                .to_owned()
        } else {
            "Restart your DAW for the new DAWmate plugin.".to_owned()
        });
    }
    let mut said: Vec<String> = Vec::new();
    for (names, what) in [
        (&put_in, "now in"),
        (&updated, "updated in"),
        (&partly_in, "only partly in"),
        (&partly_updated, "only partly updated in"),
    ] {
        if !names.is_empty() {
            said.push(format!("{what} {}", listed(names)));
        }
    }
    let title = match said.as_slice() {
        [] => return None,
        [one] => format!("DAWmate is {one}"),
        [rest @ .., last] => format!("DAWmate is {}, and {last}", rest.join(", ")),
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
    fn the_plugin_goes_in_until_a_version_has_put_it_in() {
        assert_eq!(plugin_step(ScriptState::NotInstalled, false, false, false), Step::PutIn);
        // Put in once, then taken out: it stays out.
        assert_eq!(plugin_step(ScriptState::NotInstalled, true, true, false), Step::Leave);
        assert_eq!(plugin_step(ScriptState::Outdated, true, true, false), Step::Update);
        assert_eq!(plugin_step(ScriptState::Outdated, true, false, false), Step::Leave);
        assert_eq!(plugin_step(ScriptState::Installed, false, true, false), Step::Leave);
        assert_eq!(failed_mark("1.0.0", "2026-11-25"), "1.0.0 2026-11-25");
    }

    /// DK-2: Take out on Settings › DAWs keeps the plugin out, whatever the look would have done, and from any version
    /// of the app, until Install.
    #[test]
    fn a_plugin_taken_out_on_settings_stays_out_until_install() {
        for state in [ScriptState::NotInstalled, ScriptState::Outdated, ScriptState::Installed] {
            for (before, newer) in [(false, false), (false, true), (true, false), (true, true)] {
                assert_eq!(plugin_step(state, before, newer, true), Step::Leave, "{state:?} {before} {newer}");
            }
        }
        let kept = plugin_kept_after(PluginChoice::TakeOut, &PluginDone::Went, "0.9.0-beta.3");
        assert_eq!(kept, vec![(PLUGIN_TAKEN_OUT_KEY, "0.9.0-beta.3".to_owned())]);
        // Install afterwards: kept as put in by this version, and no longer taken out (blank counts as none).
        let kept = plugin_kept_after(PluginChoice::Install, &PluginDone::Went, "0.9.0-beta.3");
        assert_eq!(kept, vec![(PLUGIN_BY_KEY, "0.9.0-beta.3".to_owned()), (PLUGIN_TAKEN_OUT_KEY, String::new())]);
        // Then the look goes by the version again: this one put it in, so only a newer one updates it.
        let newer = newer_than_writer("0.9.0-beta.3", Some("0.9.0-beta.3"));
        assert_eq!(plugin_step(ScriptState::Outdated, true, newer, false), Step::Leave);
        assert_eq!(
            plugin_step(ScriptState::Outdated, true, newer_than_writer("0.9.0", Some("0.9.0-beta.3")), false),
            Step::Update
        );
    }

    #[test]
    fn a_plugin_choice_that_didnt_happen_keeps_nothing() {
        for choice in [PluginChoice::Install, PluginChoice::TakeOut] {
            for done in [
                PluginDone::SaidNo,
                PluginDone::InUse,
                PluginDone::NotChanged,
                PluginDone::Failed("Access is denied.".into()),
            ] {
                assert!(plugin_kept_after(choice, &done, "1.0.0").is_empty(), "{choice:?} {done:?}");
            }
        }
    }

    #[test]
    fn a_plugin_choice_says_what_it_did() {
        let folder = PathBuf::from(r"C:\Program Files\Common Files\CLAP");
        let note = plugin_note(PluginChoice::Install, &PluginDone::Went, &folder);
        assert!(note.starts_with(r"Put the DAWmate plugin in C:\Program Files\Common Files\CLAP."), "{note}");
        let note = plugin_note(PluginChoice::TakeOut, &PluginDone::Went, &folder);
        assert!(note.contains("until you press Install"), "{note}");
        let note = plugin_note(PluginChoice::TakeOut, &PluginDone::InUse, &folder);
        assert!(note.ends_with("close your DAWs, then press Take out again."), "{note}");
        // What a DAW can cause says to close the DAWs; what it can't doesn't (the review of 27 Nov).
        let note = plugin_note(PluginChoice::Install, &PluginDone::NotChanged, &folder);
        assert_eq!(
            note,
            "Couldn't put the plugin in: Windows' step didn't change it. Close your DAWs, then press Install again."
        );
        let why = "Windows couldn't start the administrator step (The system cannot find the file specified).";
        let note = plugin_note(PluginChoice::TakeOut, &PluginDone::Failed(why.into()), &folder);
        assert_eq!(
            note,
            "Couldn't take the plugin out: Windows couldn't start the administrator step (The system cannot find the \
             file specified)."
        );
        assert!(!note.contains("Close your DAWs"), "{note}");
        assert_eq!(
            plugin_note(PluginChoice::Install, &PluginDone::SaidNo, &folder),
            "You said no to Windows' question, so nothing changed."
        );
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

    fn failed(folder: &str) -> (PathBuf, String) {
        (PathBuf::from(folder), "in use".into())
    }

    #[test]
    fn a_write_counts_only_when_every_folder_took_it() {
        assert!(went_everywhere(&went("K")));
        assert!(!went_everywhere(&Installed::default()), "nothing went in");
        // Bitwig had the Documents copy open: that folder is tried again at the next start (DK-1).
        let half = Installed { failed: vec![failed("D")], ..went("K") };
        assert!(!went_everywhere(&half));
        let none = Installed { failed: vec![failed("D")], ..Installed::default() };
        assert!(!went_everywhere(&none));
    }

    #[test]
    fn a_card_never_says_updated_when_a_folder_failed() {
        let installed = Installed { failed: vec![failed("C:\\Docs\\Bitwig Studio\\Extensions")], ..went("K") };
        let done = Done {
            bitwig: Some((Step::Update, BitwigInstalled { installed, ..BitwigInstalled::default() })),
            ..Done::default()
        };
        let (title, detail) = card(&done).unwrap();
        assert_eq!(title, "DAWmate is only partly updated in Bitwig");
        assert!(detail.contains("Couldn't put the new extension in C:\\Docs\\Bitwig Studio\\Extensions"), "{detail}");
        assert!(detail.contains("at a later start when Bitwig is closed"), "{detail}");
        assert!(!detail.contains("picks up the new extension"), "{detail}");
        // With another link that went in whole.
        let done = Done { fl: Some((Step::Update, went("F"))), ..done };
        assert_eq!(card(&done).unwrap().0, "DAWmate is updated in FL Studio, and only partly updated in Bitwig");
    }

    #[test]
    fn a_card_for_fls_script_from_before_the_rename_says_to_pick_dawmate() {
        let fl = Installed {
            removed: vec![PathBuf::from("F\\Music Organiser\\device_MusicOrganiser.py")],
            ..went("F\\DAWmate")
        };
        let (title, detail) = card(&Done { fl: Some((Step::Update, fl)), ..Done::default() }).unwrap();
        assert_eq!(title, "DAWmate is updated in FL Studio");
        assert!(detail.contains("pick DAWmate as the controller type of the port the Music Organiser script was on"));
        assert!(!detail.contains("to load the new script"), "{detail}");
        // The old copy couldn't be taken out: FL may list both.
        let fl = Installed { left: vec![failed("F\\Music Organiser\\device_MusicOrganiser.py")], ..went("F\\DAWmate") };
        let (_, detail) = card(&Done { fl: Some((Step::Update, fl)), ..Done::default() }).unwrap();
        assert!(
            detail.contains("device_MusicOrganiser.py, couldn't be taken out, so FL Studio may list both"),
            "{detail}"
        );
    }

    #[test]
    fn no_card_when_nothing_went_in() {
        assert!(card(&Done::default()).is_none());
        let failed = Installed { failed: vec![(PathBuf::from("F"), "in use".into())], ..Installed::default() };
        assert!(card(&Done { fl: Some((Step::Update, failed)), ..Done::default() }).is_none());
    }
}
