//! One app-wide list of keyboard shortcuts (rule 15). Every action has an id, a plain description and a default key.
//! Only the keys you change are saved, under the `shortcuts` setting, as a JSON map of id to key, the same as the C#
//! app, so changes made in either app carry across. Keys are never printed on the pages themselves (Solomon,
//! 2026-10-29): they show in tooltips, next to menu lines and on the key sheet.

use std::collections::HashMap;

use crate::places::Place;

/// The setting the changed keys are saved under.
pub const SETTING_KEY: &str = "shortcuts";

/// Keys the app keeps for itself: F12 writes a note for Claude, Tab moves between boxes and buttons, and the arrow keys
/// move through the lists ([`list_keys`]).
pub const KEPT: [&str; 7] = ["F12", "Tab", "Shift+Tab", "Up", "Down", "Left", "Right"];

/// One thing a key can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub id: &'static str,
    /// What it does, as the key sheet and Settings › Shortcuts show it.
    pub what: &'static str,
    /// Its key out of the box, written like "Ctrl+Shift+E". Empty means no key until you give it one.
    pub default_key: &'static str,
}

const fn action(id: &'static str, what: &'static str, default_key: &'static str) -> Action {
    Action { id, what, default_key }
}

/// Every action that works everywhere in the app. Pages add their own lists later (`songs.*`, `playground.*`…);
/// a page's own key beats one of these.
pub const APP_ACTIONS: &[Action] = &[
    action("app.help", "Show the keys for this page", "?"),
    action("app.helpF1", "Show the keys for this page (the help key)", "F1"),
    action("app.jump", "Jump to anything: songs, sounds, presets, MIDI parts, plugins, settings", "Ctrl+K"),
    action("app.goProjects", "Go to Songs", "Ctrl+1"),
    action("app.goSounds", "Go to Sounds", "Ctrl+2"),
    action("app.goPresets", "Go to Presets", "Ctrl+3"),
    action("app.goMidi", "Go to MIDI", "Ctrl+4"),
    action("app.goPlugins", "Go to Plugins", "Ctrl+5"),
    action("app.goSpace", "Go to Space", "Ctrl+6"),
    action("app.goBackup", "Go to Backup", "Ctrl+7"),
    action("app.goSettings", "Go to Settings", "Ctrl+,"),
    action("app.prevPage", "The place above on the rail", "Ctrl+PageUp"),
    action("app.nextPage", "The place below on the rail", "Ctrl+PageDown"),
    action("app.back", "Back to the page you were on", "Alt+Left"),
    action("app.forward", "Forward again", "Alt+Right"),
    action("app.search", "Search this page", "Ctrl+F"),
    action("app.prevItem", "The item above in the list (even while typing a search)", "Ctrl+Up"),
    action("app.nextItem", "The item below in the list (even while typing a search)", "Ctrl+Down"),
    action("app.listTop", "Back to the top of the list", "Home"),
    action("app.prevTab", "The tab to the left on this page", "Ctrl+Left"),
    action("app.nextTab", "The tab to the right on this page", "Ctrl+Right"),
    action("app.play", "Play or stop what's picked", "Space"),
    // The same order on every page (the whole-app review of 6 Nov, X3-12): each press takes one step back.
    action("app.escape", "Step back: stop, then clear the search, then close the card", "Esc"),
    action("app.rescan", "Look for new and changed projects", "F5"),
    action("app.fullRescan", "Rescan everything: re-read every project, even unchanged ones", ""),
    action("app.fullScreen", "Full screen on or off", "F11"),
    action(
        "app.recycleBin",
        if cfg!(windows) {
            "Open the Recycle Bin, where everything the app removes goes"
        } else {
            "Open the Trash, where everything the app removes goes"
        },
        "Ctrl+Shift+Delete",
    ),
    action("app.sizeUp", "Make this page bigger", "Ctrl+="),
    action("app.sizeDown", "Make this page smaller, to fit more on screen", "Ctrl+-"),
    action("app.sizeReset", "This page back to the size of the rest", "Ctrl+0"),
];

/// The Songs page's own actions. The ids are the C# app's Projects page's, so a key changed in either app carries
/// across; `projects.openOwn` is new here, so the song line's menu and the panel's "Open in FL Studio" have a key too
/// (rule 15).
pub const SONGS_ACTIONS: &[Action] = &[
    action("projects.open", "Open the picked song (FL Studio songs go to Bitwig)", "Ctrl+O"),
    action("projects.openOwn", "Open the picked song in the DAW it was made in", "Ctrl+Shift+O"),
    action("projects.openInBitwig", "Open the picked song in Bitwig (FL Studio songs are converted)", "Ctrl+B"),
    action("projects.reveal", "Show the picked song's file in its folder", "Ctrl+Shift+E"),
    action("projects.copyPath", "Copy where the picked song's file is", "Ctrl+Shift+C"),
];

/// The Sounds page's own actions, with the C# Sound Lab's ids and keys. Esc stops the sound playing as it does on every
/// page (`app.escape`).
pub const SOUNDS_ACTIONS: &[Action] = &[
    action("sounds.favourite", "Make the picked sound a favourite, or not", "Ctrl+D"),
    action("sounds.tag", "Type a tag for the picked sound", "Ctrl+T"),
    action("sounds.toBitwig", "Put the picked sound into Bitwig", "Ctrl+B"),
    action("sounds.reveal", "Show the sound's file in Explorer", "Ctrl+Shift+E"),
    action("sounds.copyPath", "Copy where the sound's file is", "Ctrl+Shift+C"),
];

/// The Plugins page's own actions, with the C# ids. Clean up and Uninstall join with the page's second part.
pub const PLUGINS_ACTIONS: &[Action] = &[
    action("plugins.projects", "Show the songs that use the picked plugin", "Ctrl+O"),
    // F5, as Space's Measure again and MIDI's Read again (review X3-7). Not one of the picked plugin's actions.
    action("plugins.measure", "Measure your plugins again: which are installed and how much room each takes", "F5"),
    action("plugins.searchOnline", "Search the web for the picked plugin", "Ctrl+Shift+O"),
    action("plugins.cleanUp", "Clean up the picked plugin's presets and sounds (asks first)", "Ctrl+Shift+L"),
    action("plugins.uninstall", "Uninstall the picked plugin (asks first)", "Ctrl+Shift+U"),
    action("plugins.move", "Move the picked plugin to another drive (asks first)", "Ctrl+Shift+M"),
];

/// The Presets page's own actions on its Sounds section. Send to Bitwig takes the C# Sound Lab's key.
pub const PRESETS_ACTIONS: &[Action] = &[
    action("presets.export", "Export the picked sound as a preset file", "Ctrl+E"),
    action("presets.toBitwig", "Make the picked sound a Bitwig preset and send it to Bitwig", "Ctrl+B"),
    action("presets.audition", "Hear the picked sound in Bitwig; again to stop", "Ctrl+Shift+B"),
    action("presets.keepAudition", "Keep the sound you're hearing in Bitwig: its track stays", "Ctrl+Shift+K"),
    action("presets.exportShown", "Export every sound in the list as it's filtered now", "Ctrl+Shift+S"),
    action("presets.reveal", "Show the picked sound's song file or preset file in its folder", "Ctrl+Shift+E"),
    action("presets.showInSongs", "Show the picked sound's song on the Songs page", "Ctrl+O"),
    action("presets.copyPath", "Copy where the picked sound's song or file is", "Ctrl+Shift+C"),
    action("presets.useGuess", "Take your part fix away and use the guess again", "Ctrl+Backspace"),
    action("presets.part.bass", "The picked sound is really a bass", "Alt+1"),
    action("presets.part.lead", "The picked sound is really a lead", "Alt+2"),
    action("presets.part.chords", "The picked sound is really chords", "Alt+3"),
    action("presets.part.pad", "The picked sound is really a pad", "Alt+4"),
    action("presets.part.stabs", "The picked sound is really stabs or plucks", "Alt+5"),
    action("presets.part.arp", "The picked sound is really an arp", "Alt+6"),
    action("presets.part.drums", "The picked sound is really drums", "Alt+7"),
    action("presets.part.fx", "The picked sound is really FX", "Alt+8"),
];

/// The Presets page's Curves section's own actions (a list of its own: its keys are the Sounds section's).
pub const CURVES_ACTIONS: &[Action] = &[
    action("curves.toBitwig", "Save the picked curve into Bitwig's curves library", "Ctrl+B"),
    action("curves.reveal", "Show the picked curve's file, to drag onto a Bitwig lane", "Ctrl+Shift+F"),
    action("curves.rename", "Rename the picked curve", "F2"),
    action("curves.hide", "Hide the picked curve, or show it again", "Delete"),
    action("curves.revealSong", "Show the file of the song the picked curve came from in its folder", "Ctrl+Shift+E"),
    action("curves.showInSongs", "Show the song the picked curve came from on the Songs page", "Ctrl+O"),
];

/// The MIDI page's own actions, with the C# MIDI page's ids where it had them. What goes with this (midi.fit),
/// the playground and Save as join as those parts are built. Esc goes back to the whole list as one of its steps back
/// (`app.escape`). F5 reads the MIDI again on this page, as it measures again on Space. Put in the song writes into
/// your project (over what's in those bars on the track), so it has no key until you give it one. The octave keys are
/// Alt with the arrows here: the C# app's Shift+Alt+= and Shift+Alt+- can't be told on every keyboard (a window here
/// hears the character a key types, and with Shift down that's another on each layout).
pub const MIDI_ACTIONS: &[Action] = &[
    action("midi.moreLike", "Parts shaped like the picked one", "Ctrl+L"),
    action("midi.toBitwig", "Put the picked part into Bitwig as a clip on a new track", "Ctrl+B"),
    action(
        "midi.putInSong",
        "Have Bitwig record the picked part into your song's arranger, at the bars it fits, on the selected track",
        "",
    ),
    action("midi.playBitwig", "Play the picked part through the track selected in Bitwig, or stop it", "Ctrl+Shift+B"),
    action("midi.playFl", "Play the picked part through the channel selected in FL Studio, or stop it", "Ctrl+Shift+F"),
    action("midi.within", "The longer phrases the picked part is in", ""),
    action("midi.octaveUp", "Move the picked part up an octave where it's fitted to a song; remembered", "Alt+Up"),
    action(
        "midi.octaveDown",
        "Move the picked part down an octave where it's fitted to a song; remembered",
        "Alt+Down",
    ),
    action("midi.octaveApp", "Let the app pick the picked part's octave again", ""),
    action("midi.showInSongs", "Show the song the picked part is from on the Songs page", "Ctrl+O"),
    action("midi.source", "Show the picked part's file in its folder", "Ctrl+Shift+E"),
    action("midi.copyPath", "Copy where the picked part's file is", "Ctrl+Shift+C"),
    action("midi.readAgain", "Read MIDI again; again to stop", "F5"),
];

/// The Space page's own actions (new: the C# app had no Space page). F5 measures again on this page; elsewhere it
/// looks for new and changed projects. Show in folder has the key it has on the other pages. Fix writes new files, and
/// Remove and Put back move them, so they have no key until you give them one. A Mac or Linux has a Trash, as the
/// page's buttons say there.
pub const SPACE_ACTIONS: &[Action] = &[
    action("space.measure", "Look at the drives and measure the app's files again", "F5"),
    action("space.reveal", "Show the picked line's folder", "Ctrl+Shift+E"),
    action("space.fix", "Write a \"(fixed)\" Bitwig project beside each ticked save", ""),
    action(
        "space.removeFixed",
        if cfg!(windows) {
            "Send every \"(fixed)\" Bitwig project Fix wrote to the Recycle Bin, so the saves can be fixed again"
        } else {
            "Send every \"(fixed)\" Bitwig project Fix wrote to the Trash, so the saves can be fixed again"
        },
        "",
    ),
    action(
        "space.remove",
        if cfg!(windows) { "Send what's ticked to the Recycle Bin" } else { "Send what's ticked to the Trash" },
        "",
    ),
    action(
        "space.stop",
        if cfg!(windows) { "Stop sending to the Recycle Bin" } else { "Stop sending to the Trash" },
        "",
    ),
    action(
        "space.putBack",
        if cfg!(windows) {
            "Put back what the last clean-up sent to the Recycle Bin"
        } else {
            "Put back what the last clean-up sent to the Trash"
        },
        "",
    ),
];

/// The Backup page's own actions (new: the C# app had no Backup page), so its buttons have keys and Ctrl+K finds them
/// (review X3-21). Stop has no key until you give it one, as on Space.
pub const BACKUP_ACTIONS: &[Action] = &[
    action("backup.now", "Back up now: copy what's new or changed to the backup drive", "Ctrl+Enter"),
    action("backup.stop", "Stop the backup: what's copied so far stays on the drive", ""),
];

/// Every page's own list. A page's keys only work on that page, so two pages may share a key.
pub const PAGE_ACTIONS: &[&[Action]] = &[
    SONGS_ACTIONS,
    SOUNDS_ACTIONS,
    PRESETS_ACTIONS,
    CURVES_ACTIONS,
    MIDI_ACTIONS,
    PLUGINS_ACTIONS,
    SPACE_ACTIONS,
    BACKUP_ACTIONS,
];

/// Every page's own list with the page's name, in the order Settings › Shortcuts shows them (after the keys that work
/// everywhere).
pub const PAGES: &[(&str, &[Action])] = &[
    ("Songs", SONGS_ACTIONS),
    ("Sounds", SOUNDS_ACTIONS),
    ("Presets", PRESETS_ACTIONS),
    ("Curves", CURVES_ACTIONS),
    ("MIDI", MIDI_ACTIONS),
    ("Plugins", PLUGINS_ACTIONS),
    ("Space", SPACE_ACTIONS),
    ("Backup", BACKUP_ACTIONS),
];

/// The keys a page's list keeps for itself ([`KEPT`]: no action can have them), as the key sheet lists them under the
/// page's own: what each does there (the whole-app review of 6 Nov, X3-25). The pages without a list to step through
/// have none.
pub fn list_keys(place: Place) -> &'static [(&'static str, &'static str)] {
    const DOWN: (&str, &str) = ("Down", "From the search box into the list");
    match place {
        Place::Songs => &[("Up, Down", "The song above or below in the list"), DOWN],
        Place::Sounds => &[
            ("Up, Down", "The sound above or below the picked one"),
            ("Left, Right", "The sound beside it (on Tiles and Galaxy)"),
            ("Enter", "Play or stop it"),
            DOWN,
        ],
        Place::Presets => &[("Up, Down", "The sound or curve above or below in the list"), DOWN],
        Place::Midi => &[("Up, Down", "The part above or below in the list"), DOWN],
        Place::Plugins => &[("Up, Down", "The plugin above or below in the list"), DOWN],
        Place::Space | Place::Backup | Place::Settings => &[],
    }
}

/// The name Settings › Shortcuts gives the keys that work on every page.
pub const EVERYWHERE: &str = "Everywhere";

/// What an action does, in the words its tooltip and the key list use, or "" for an id in no list.
pub fn what_of(id: &str) -> &'static str {
    APP_ACTIONS.iter().chain(PAGE_ACTIONS.iter().copied().flatten()).find(|a| a.id == id).map_or("", |a| a.what)
}

/// The keys in use: the defaults with your changes on top.
#[derive(Clone, Debug)]
pub struct Shortcuts {
    changed: HashMap<String, String>,
    /// Each key in use, written by `normalise`, to the app-wide action it runs. Built once, not on every key press.
    by_key: HashMap<String, &'static str>,
    /// The same for each page's own list: the key to the page actions that have it, one per page.
    page_keys: HashMap<String, Vec<&'static str>>,
}

impl Default for Shortcuts {
    fn default() -> Shortcuts {
        Shortcuts::with_changes(HashMap::new())
    }
}

impl Shortcuts {
    /// Reads the `shortcuts` setting. A missing or unreadable value means "all defaults", never an error, so a bad
    /// setting can't stop the app opening.
    pub fn from_setting(json: Option<&str>) -> Shortcuts {
        Shortcuts::with_changes(json.and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default())
    }

    fn with_changes(changed: HashMap<String, String>) -> Shortcuts {
        let mut by_key = HashMap::new();
        // A key you gave an action takes it over from whichever action has it by default.
        for a in APP_ACTIONS {
            if let Some(k) = changed.get(a.id).filter(|k| !k.is_empty()) {
                by_key.insert(normalise(k), a.id);
            }
        }
        for a in APP_ACTIONS {
            if !changed.contains_key(a.id) && !a.default_key.is_empty() {
                by_key.entry(normalise(a.default_key)).or_insert(a.id);
            }
        }
        // The same for each page, among that page's own actions.
        let mut page_keys: HashMap<String, Vec<&'static str>> = HashMap::new();
        for page in PAGE_ACTIONS {
            let mut mine: HashMap<String, &'static str> = HashMap::new();
            for a in page.iter() {
                if let Some(k) = changed.get(a.id).filter(|k| !k.is_empty()) {
                    mine.insert(normalise(k), a.id);
                }
            }
            for a in page.iter() {
                if !changed.contains_key(a.id) && !a.default_key.is_empty() {
                    mine.entry(normalise(a.default_key)).or_insert(a.id);
                }
            }
            for (k, id) in mine {
                page_keys.entry(k).or_default().push(id);
            }
        }
        Shortcuts { changed, by_key, page_keys }
    }

    /// The `shortcuts` setting's text: only the keys changed from where they started, as the C# app keeps them (its
    /// own ids that this app has no action for yet are kept too, so nothing it saved is lost).
    pub fn to_setting(&self) -> String {
        let sorted: std::collections::BTreeMap<&String, &String> = self.changed.iter().collect();
        serde_json::to_string(&sorted).unwrap_or_else(|_| "{}".to_owned())
    }

    /// Whether any of this app's listed actions ([`is_listed`]) has a key other than the one it started with: what
    /// Settings shows Reset all for, so it never shows with no line marked changed. One not built yet may have lost its
    /// key ([`Shortcuts::set`]), or kept one from the old app; [`Shortcuts::reset_all`] puts those back too.
    pub fn any_changed(&self) -> bool {
        all_actions().any(|(page, a)| is_listed(page, a.id) && self.is_changed(a.id))
    }

    /// Whether an action's key isn't the one it started with.
    pub fn is_changed(&self, id: &str) -> bool {
        all_actions()
            .find(|(_, a)| a.id == id)
            .is_some_and(|(_, a)| normalise(self.key_of(id)) != normalise(a.default_key))
    }

    /// Gives an action a key ("" for none), as the C# app does: a key kept for the app (F12, Tab) can't be given; an
    /// action on the same page that had the key is left with none, so one key never does two things on a page; a
    /// page's key may also work everywhere, and the page's job wins there. Returns what happened, in words, for under
    /// the list ("" when there's nothing to say).
    pub fn set(&mut self, id: &str, key: &str) -> String {
        let Some((page, action)) = all_actions().find(|(_, a)| a.id == id) else { return String::new() };
        let key = written(key);
        if KEPT.iter().any(|k| normalise(k) == normalise(&key)) {
            return match key.as_str() {
                "F12" => "F12 is kept for writing a note to Claude.".to_owned(),
                "Tab" | "Shift+Tab" => format!("{key} is kept for moving between boxes and buttons."),
                _ => format!("{key} is kept for moving through the lists."),
            };
        }
        let same = |k: &str| !key.is_empty() && normalise(k) == normalise(&key);
        let mut message = String::new();
        let taken = all_actions().find(|(p, a)| a.id != id && *p == page && same(self.key_of(a.id))).map(|(_, a)| a);
        if let Some(taken) = taken {
            self.changed.insert(taken.id.to_owned(), String::new());
        }
        // One that isn't built yet isn't on the list to give its key back, so it's let go without a word, and what
        // else the key does is said instead (F5 is Rescan's, not built, and three pages' own).
        if let Some(taken) = taken.filter(|a| is_listed(page, a.id)) {
            message = format!("{key} was \"{}\", which has no key now. Default puts it back.", taken.what);
        } else if page != EVERYWHERE
            && let Some((_, wide)) =
                all_actions().find(|(p, a)| *p == EVERYWHERE && is_built(a.id) && same(self.key_of(a.id)))
        {
            message = format!("{key} is also \"{}\" everywhere else; on {page} it does this instead.", wide.what);
        } else if page == EVERYWHERE {
            let mut pages: Vec<&str> =
                all_actions().filter(|(p, a)| *p != EVERYWHERE && same(self.key_of(a.id))).map(|(p, _)| p).collect();
            pages.dedup();
            if !pages.is_empty() {
                let one = pages.len() == 1;
                message = format!(
                    "{} keep{} {key} for a job of {} own.",
                    pages.join(" and "),
                    if one { "s" } else { "" },
                    if one { "its" } else { "their" }
                );
            }
        }
        if normalise(&key) == normalise(action.default_key) {
            self.changed.remove(id);
        } else {
            self.changed.insert(id.to_owned(), key);
        }
        *self = Shortcuts::with_changes(std::mem::take(&mut self.changed));
        message
    }

    /// Every key back to the one it started with (the C# app's own ids that this app doesn't have yet stay as saved).
    pub fn reset_all(&mut self) {
        let mut changed = std::mem::take(&mut self.changed);
        changed.retain(|id, _| !all_actions().any(|(_, a)| a.id == id));
        *self = Shortcuts::with_changes(changed);
    }

    /// The key an action has now, or "" for none.
    pub fn key_of(&self, id: &str) -> &str {
        if let Some(k) = self.changed.get(id) {
            return k;
        }
        // A default key that another action has taken over is no longer this one's.
        if let Some(a) = APP_ACTIONS.iter().find(|a| a.id == id) {
            return if self.by_key.get(&normalise(a.default_key)) == Some(&a.id) { a.default_key } else { "" };
        }
        PAGE_ACTIONS
            .iter()
            .flat_map(|page| page.iter())
            .find(|a| a.id == id)
            .filter(|a| self.page_keys.get(&normalise(a.default_key)).is_some_and(|ids| ids.contains(&a.id)))
            .map_or("", |a| a.default_key)
    }

    /// The action of `page` (one of [`PAGE_ACTIONS`]) a key press stands for, if any. A page's own key beats an
    /// app-wide one, so this is asked first.
    pub fn page_action_for(&self, key: &str, page: &[Action]) -> Option<&'static str> {
        if key.is_empty() {
            return None;
        }
        self.page_keys.get(&normalise(key))?.iter().copied().find(|id| page.iter().any(|a| a.id == *id))
    }

    /// The app-wide action a key press stands for, if any. `key` is written like the list ("Ctrl+1").
    ///
    /// As in the C# app, only two keys are tried again another way, because keyboards differ in whether Shift shows
    /// in a symbol: "?" (Shift+/ on most of them), and Ctrl with the + key, which counts as Ctrl+=. Any other key with
    /// Shift is its own key: Shift+F11 isn't F11.
    pub fn action_for(&self, key: &str) -> Option<&'static str> {
        if key.is_empty() {
            return None;
        }
        let key = normalise(key);
        let other = match key.as_str() {
            "Shift+?" | "Shift+/" => "?",
            "Ctrl+Shift++" | "Ctrl+Shift+=" | "Ctrl++" => "Ctrl+=",
            _ => "",
        };
        [key.as_str(), other].into_iter().find_map(|k| self.by_key.get(k).copied())
    }
}

/// The app-wide actions the window does something for so far, besides going to a place ([`is_built`]). The rest
/// (Back and Forward, the tabs, Rescan, the page sizes) aren't built yet.
pub const BUILT: [&str; 13] = [
    "app.jump",
    "app.prevPage",
    "app.nextPage",
    "app.help",
    "app.helpF1",
    "app.fullScreen",
    "app.play",
    "app.recycleBin",
    "app.search",
    "app.escape",
    "app.prevItem",
    "app.nextItem",
    "app.listTop",
];

/// Whether the app-wide action `id` does something yet: one of [`BUILT`], or going to a place. One that doesn't is on
/// no list (the key sheet, Ctrl+K, Settings › Shortcuts), so every key the app shows works (the whole-app reviews of
/// 6 Nov, X3-22, and 10 Nov, X3-1).
pub fn is_built(id: &str) -> bool {
    BUILT.contains(&id) || Place::ALL.iter().any(|p| p.shortcut_id() == id)
}

/// Whether an action of `page` (one of [`all_actions`]) is listed: each page's own, and the app-wide ones that are
/// built ([`is_built`]).
pub fn is_listed(page: &str, id: &str) -> bool {
    page != EVERYWHERE || is_built(id)
}

/// Every action with its page's name ([`EVERYWHERE`] for the app-wide ones), in the order Settings shows them.
pub fn all_actions() -> impl Iterator<Item = (&'static str, &'static Action)> {
    APP_ACTIONS
        .iter()
        .map(|a| (EVERYWHERE, a))
        .chain(PAGES.iter().flat_map(|(p, list)| list.iter().map(move |a| (*p, a))))
}

/// A key as it's kept: in the list's spelling, with "?" on its own (however the keyboard says it's typed).
pub fn written(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    match normalise(key).as_str() {
        "Shift+?" | "Shift+/" | "?" => "?".to_owned(),
        k => k.to_owned(),
    }
}

/// Whether an action does its job again and again while its key is held: stepping through a list. Every other key
/// does its job once however long it's held, so holding F11 doesn't flick full screen on and off. The MIDI page's
/// octave keys are among those (the C# app's repeated): there are 7 octaves to be at, a held key would fly to the
/// last, and each step starts what's playing again.
pub fn repeats(id: &str) -> bool {
    matches!(id, "app.prevItem" | "app.nextItem")
}

/// Keys with names rather than one character, in the spelling the list uses.
const NAMED_KEYS: &[&str] = &[
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "F13",
    "F14",
    "F15",
    "F16",
    "F17",
    "F18",
    "F19",
    "F20",
    "F21",
    "F22",
    "F23",
    "F24",
    "PageUp",
    "PageDown",
    "Left",
    "Right",
    "Up",
    "Down",
    "Home",
    "End",
    "Insert",
    "Delete",
    "Backspace",
    "Esc",
    "Tab",
    "Enter",
    "Space",
];

/// Puts a key in one standard spelling so "shift+ctrl+e" and "Ctrl+Shift+E" match: modifiers in the order Ctrl, Shift,
/// Alt, Meta, then the key itself (letters upper case, named keys as `NAMED_KEYS` spells them).
pub fn normalise(key: &str) -> String {
    if key == "+" {
        return key.to_owned();
    }
    let mut parts: Vec<&str> = key.split('+').collect();
    // "Ctrl++" ends in an empty part for the plus key itself.
    let last = match parts.pop() {
        Some("") => "+",
        Some(k) => k,
        None => "",
    };
    let has = |m: &str| parts.iter().any(|p| p.eq_ignore_ascii_case(m));
    let mut out = String::new();
    let modifiers: [(&str, &[&str]); 4] = [
        ("Ctrl", &["ctrl", "control"]),
        ("Shift", &["shift"]),
        ("Alt", &["alt", "option"]),
        ("Meta", &["meta", "cmd"]),
    ];
    for (name, aliases) in modifiers {
        if aliases.iter().any(|a| has(a)) {
            out.push_str(name);
            out.push('+');
        }
    }
    if let Some(named) = NAMED_KEYS.iter().find(|n| n.eq_ignore_ascii_case(last)) {
        out.push_str(named);
    } else if last.eq_ignore_ascii_case("escape") {
        out.push_str("Esc");
    } else {
        out.push_str(&last.to_uppercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let all: Vec<_> = APP_ACTIONS.iter().chain(PAGE_ACTIONS.iter().flat_map(|p| p.iter())).map(|a| a.id).collect();
        let mut ids = all.clone();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
    }

    #[test]
    fn what_an_action_does_is_found_in_any_list() {
        assert_eq!(what_of("app.play"), "Play or stop what's picked");
        assert_eq!(what_of("projects.reveal"), "Show the picked song's file in its folder");
        assert_eq!(what_of("sounds.copyPath"), "Copy where the sound's file is");
        assert_eq!(what_of("space.reveal"), "Show the picked line's folder");
        assert_eq!(what_of("sounds.toBitwig"), "Put the picked sound into Bitwig");
        assert_eq!(what_of("no.such.action"), "");
    }

    #[test]
    fn page_keys_work_on_their_page_and_carry_changes() {
        let s = Shortcuts::default();
        assert_eq!(s.page_action_for("Ctrl+O", SONGS_ACTIONS), Some("projects.open"));
        assert_eq!(s.page_action_for("ctrl+shift+e", SONGS_ACTIONS), Some("projects.reveal"));
        assert_eq!(s.page_action_for("Ctrl+Shift+O", SONGS_ACTIONS), Some("projects.openOwn"));
        assert_eq!(s.page_action_for("Ctrl+O", &[]), None);
        assert_eq!(s.key_of("projects.copyPath"), "Ctrl+Shift+C");
        // A key changed in the C# app's Projects page carries across.
        let s = Shortcuts::from_setting(Some(r#"{"projects.open":"Enter","projects.reveal":"Ctrl+O"}"#));
        assert_eq!(s.page_action_for("Enter", SONGS_ACTIONS), Some("projects.open"));
        assert_eq!(s.page_action_for("Ctrl+O", SONGS_ACTIONS), Some("projects.reveal"));
        assert_eq!(s.key_of("projects.reveal"), "Ctrl+O");
        assert_eq!(s.page_action_for("Ctrl+Shift+E", SONGS_ACTIONS), None);
    }

    #[test]
    fn page_keys_are_unique_on_their_page_and_not_kept() {
        for page in PAGE_ACTIONS {
            let mut keys: Vec<_> =
                page.iter().map(|a| a.default_key).filter(|k| !k.is_empty()).map(normalise).collect();
            for k in &keys {
                assert!(!KEPT.contains(&k.as_str()), "{k} is kept by the app");
            }
            let n = keys.len();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), n, "two actions on one page share a default key");
        }
    }

    #[test]
    fn default_keys_are_unique_and_not_kept() {
        let mut keys: Vec<_> =
            APP_ACTIONS.iter().map(|a| a.default_key).filter(|k| !k.is_empty()).map(normalise).collect();
        for k in &keys {
            assert!(!KEPT.contains(&k.as_str()), "{k} is kept by the app");
        }
        let n = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), n, "two actions share a default key");
    }

    #[test]
    fn every_place_has_its_key() {
        let s = Shortcuts::default();
        for p in Place::ALL {
            assert!(APP_ACTIONS.iter().any(|a| a.id == p.shortcut_id()), "{:?} has no action", p);
            assert_eq!(s.action_for(s.key_of(p.shortcut_id())), Some(p.shortcut_id()));
        }
        assert_eq!(s.action_for("Ctrl+6"), Some("app.goSpace"));
        assert_eq!(s.action_for("Ctrl+7"), Some("app.goBackup"));
    }

    #[test]
    fn changed_keys_win_and_bad_settings_mean_defaults() {
        let s = Shortcuts::from_setting(Some(r#"{"app.goSounds":"Ctrl+Shift+S"}"#));
        assert_eq!(s.action_for("ctrl+shift+s"), Some("app.goSounds"));
        assert_eq!(s.action_for("Ctrl+2"), None);
        let bad = Shortcuts::from_setting(Some("not json"));
        assert_eq!(bad.action_for("Ctrl+2"), Some("app.goSounds"));
    }

    #[test]
    fn normalise_orders_modifiers() {
        assert_eq!(normalise("shift+ctrl+e"), "Ctrl+Shift+E");
        assert_eq!(normalise("Ctrl++"), "Ctrl++");
        assert_eq!(normalise("F11"), "F11");
        assert_eq!(normalise("Alt+Left"), "Alt+Left");
        assert_eq!(normalise("ctrl+pagedown"), "Ctrl+PageDown");
        assert_eq!(normalise("f11"), "F11");
    }

    #[test]
    fn a_changed_key_takes_over_from_a_default() {
        // The C# app saves only what you changed, so Ctrl+3 is still Presets' default here.
        let s = Shortcuts::from_setting(Some(r#"{"app.goPlugins":"Ctrl+3","app.goSounds":""}"#));
        assert_eq!(s.action_for("Ctrl+3"), Some("app.goPlugins"));
        assert_eq!(s.action_for("Ctrl+5"), None);
        assert_eq!(s.action_for("Ctrl+2"), None);
        // The key sheet then shows Ctrl+3 once: for Plugins, not also for Presets.
        assert_eq!(s.key_of("app.goPlugins"), "Ctrl+3");
        assert_eq!(s.key_of("app.goPresets"), "");
        assert_eq!(s.key_of("app.goMidi"), "Ctrl+4");
    }

    #[test]
    fn shift_and_plus_fall_back_like_the_csharp_app() {
        let s = Shortcuts::default();
        assert_eq!(s.action_for("Shift+?"), Some("app.help"));
        assert_eq!(s.action_for("Ctrl+Shift++"), Some("app.sizeUp"));
        assert_eq!(s.action_for("Ctrl++"), Some("app.sizeUp"));
        assert_eq!(s.action_for("Ctrl+Shift+Delete"), Some("app.recycleBin"));
        // Any other key with Shift is a key of its own.
        assert_eq!(s.action_for("Shift+F11"), None);
        assert_eq!(s.action_for("Shift+F1"), None);
        assert_eq!(s.action_for("Ctrl+Shift+PageDown"), None);
    }

    #[test]
    fn every_page_list_is_named_once() {
        assert_eq!(PAGES.len(), PAGE_ACTIONS.len());
        for (list, (_, named)) in PAGE_ACTIONS.iter().zip(PAGES) {
            assert_eq!(list, named);
        }
    }

    #[test]
    fn a_new_key_takes_over_and_says_who_lost_it() {
        let mut keys = Shortcuts::default();
        let said = keys.set("app.goSounds", "Ctrl+1");
        assert_eq!(keys.key_of("app.goSounds"), "Ctrl+1");
        assert_eq!(keys.key_of("app.goProjects"), "");
        assert_eq!(keys.action_for("Ctrl+1"), Some("app.goSounds"));
        assert!(said.contains("Go to Songs"), "{said}");
        assert!(keys.is_changed("app.goProjects") && keys.any_changed());
        // Back to its own: the other stays without, until Default puts it back.
        assert_eq!(keys.set("app.goSounds", "Ctrl+2"), "");
        assert!(!keys.is_changed("app.goSounds"));
        keys.reset_all();
        assert!(!keys.any_changed());
        assert_eq!(keys.key_of("app.goProjects"), "Ctrl+1");
    }

    #[test]
    fn kept_keys_cant_be_given_and_question_marks_are_one_key() {
        let mut keys = Shortcuts::default();
        assert!(keys.set("app.search", "F12").contains("note to Claude"));
        assert!(keys.set("app.search", "Tab").contains("kept"));
        assert_eq!(keys.key_of("app.search"), "Ctrl+F");
        keys.set("app.help", "Shift+?");
        assert_eq!(keys.key_of("app.help"), "?");
        assert!(!keys.is_changed("app.help"));
    }

    #[test]
    fn page_keys_and_everywhere_keys_say_which_wins() {
        let mut keys = Shortcuts::default();
        let said = keys.set("sounds.copyPath", "Ctrl+F");
        assert!(said.contains("on Sounds it does this instead"), "{said}");
        assert_eq!(keys.page_action_for("Ctrl+F", SOUNDS_ACTIONS), Some("sounds.copyPath"));
        assert_eq!(keys.action_for("Ctrl+F"), Some("app.search"));
        let said = keys.set("app.fullScreen", "Ctrl+Shift+E");
        assert!(said.starts_with("Songs and Sounds and Presets and Curves and MIDI and Space keep"), "{said}");
    }

    #[test]
    fn keys_not_built_yet_are_let_go_without_a_word() {
        // Every built one is on the list.
        for id in BUILT {
            assert!(APP_ACTIONS.iter().any(|a| a.id == id), "{id} isn't in the shortcut list");
        }
        assert!(is_built("app.goSpace") && !is_built("app.back") && !is_built("app.rescan"));
        assert!(is_listed("Songs", "projects.open") && !is_listed(EVERYWHERE, "app.sizeUp"));
        // Alt+Left is Back's, which isn't built: a page's key given it doesn't say it's also Back everywhere else, and
        // an app-wide one given it takes it from Back without naming it (the whole-app review of 10 Nov, X3-1).
        let mut keys = Shortcuts::default();
        assert_eq!(keys.set("projects.open", "Alt+Left"), "");
        assert_eq!(keys.set("app.search", "Alt+Right"), "");
        assert_eq!(keys.key_of("app.forward"), "");
        assert_eq!(keys.action_for("Alt+Right"), Some("app.search"));
        // A built one still says so.
        assert!(keys.set("projects.reveal", "F11").contains("everywhere else"));
        // F5 is Rescan's, which isn't built, and three pages' own: taken from Rescan without a word, what the pages do
        // with it is still said.
        assert_eq!(keys.set("app.search", "F5"), "MIDI and Plugins and Space keep F5 for a job of their own.");
        assert_eq!((keys.key_of("app.rescan"), keys.action_for("F5")), ("", Some("app.search")));
        // Search given its own key back: Rescan has lost F5, but it isn't on the list, so Reset all isn't shown for it
        // (it would have no line marked changed). Reset all still gives Rescan F5 back.
        keys.set("app.search", "Ctrl+F");
        keys.set("projects.open", "Ctrl+O");
        keys.set("projects.reveal", "Ctrl+Shift+E");
        assert!(keys.is_changed("app.rescan") && keys.is_changed("app.forward") && !keys.any_changed());
        keys.reset_all();
        assert_eq!((keys.key_of("app.rescan"), keys.key_of("app.forward")), ("F5", "Alt+Right"));
    }

    #[test]
    fn esc_steps_back_the_same_way_on_every_page() {
        // One action for Esc, everywhere: no page's own list has it (the whole-app review of 6 Nov, X3-12).
        let keys = Shortcuts::default();
        assert_eq!(keys.action_for("Esc"), Some("app.escape"));
        for page in PAGE_ACTIONS {
            assert!(page.iter().all(|a| normalise(a.default_key) != "Esc"), "a page's own action has Esc");
            assert_eq!(keys.page_action_for("Esc", page), None);
        }
    }

    #[test]
    fn the_arrow_keys_are_the_lists_and_every_list_says_so() {
        let mut keys = Shortcuts::default();
        for arrow in ["Up", "Down", "Left", "Right"] {
            assert!(keys.set("app.search", arrow).contains("kept for moving through the lists"), "{arrow}");
        }
        // With a key held, an arrow is a key of its own (as Ctrl+Up, which steps even while you type a search, and
        // Alt+Up, the MIDI page's octave up).
        assert_eq!(keys.set("app.search", "Shift+Alt+Up"), "");
        assert_eq!(keys.key_of("app.search"), "Shift+Alt+Up");
        assert_eq!(keys.page_action_for("Alt+Up", MIDI_ACTIONS), Some("midi.octaveUp"));
        assert_eq!(keys.page_action_for("Alt+Down", MIDI_ACTIONS), Some("midi.octaveDown"));
        // Held down, an octave key steps once: a held key would fly to the last octave.
        assert!(!repeats("midi.octaveUp") && !repeats("midi.octaveDown") && !repeats("midi.octaveApp"));
        // Each page with a list says what its arrows do on the key sheet; the pages without one say nothing.
        for place in [Place::Songs, Place::Sounds, Place::Presets, Place::Midi, Place::Plugins] {
            assert!(list_keys(place).iter().any(|(k, _)| k.contains("Up")), "{place:?}");
        }
        assert!(list_keys(Place::Space).is_empty() && list_keys(Place::Settings).is_empty());
    }

    #[test]
    fn backup_and_read_midi_again_have_keys() {
        // Their buttons have keys, and Ctrl+K finds them (X3-21).
        let keys = Shortcuts::default();
        assert_eq!(keys.page_action_for("Ctrl+Enter", BACKUP_ACTIONS), Some("backup.now"));
        assert_eq!(keys.key_of("backup.stop"), "");
        assert_eq!(keys.page_action_for("F5", MIDI_ACTIONS), Some("midi.readAgain"));
        assert!(PAGES.iter().any(|(name, list)| *name == "Backup" && *list == BACKUP_ACTIONS));
    }

    #[test]
    fn the_setting_keeps_the_csharp_apps_other_ids() {
        let mut keys = Shortcuts::from_setting(Some(r#"{"playground.loop":"Ctrl+J","app.search":"Ctrl+G"}"#));
        keys.set("app.search", "Ctrl+F");
        assert_eq!(keys.to_setting(), r#"{"playground.loop":"Ctrl+J"}"#);
        keys.set("app.rescan", "F6");
        keys.reset_all();
        assert_eq!(keys.to_setting(), r#"{"playground.loop":"Ctrl+J"}"#);
    }
}
