//! The Space page's spare plugin copies (phase 4, part 4). Plugin installers often put each plugin in every format:
//! VST2, VST3, CLAP and AAX. A copy is spare when your saves use another format of that plugin, which stays installed,
//! or when it's AAX and Pro Tools, the only DAW that loads AAX, isn't on this computer. Solomon's F12 note (25 Oct
//! 2026): "if i have lots of aax plugins but i never use them or have a vst 2 version i never use show that".
//! `mo_space::spare` finds, measures and removes them; this works out which formats your saves use (from the library,
//! read only), says every word of the line and the card, and keeps track of Remove: the ticked copies go to the
//! Recycle Bin, each checked again just before, and Put back returns them for as long as they're there (the page's
//! question 5). As agreed in the mock-up, every copy that can go is ticked out of the box, as in the C# app, and Remove
//! asks nothing more: the Recycle Bin and Put back are the way back.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_formats::{case, paths};
use mo_library::plugins::PluginUse;
use mo_space::Stopped;
use mo_space::clean::{Bin, Done};
use mo_space::plugins::scan_files;
use mo_space::size::size_text;
use mo_space::spare::{self, SpareCopy, Used, Why};

use crate::shell::{folder_of, wrappable_path};
use crate::songs::{plural, thousands};
use crate::space::{Card, Line, Opens, Part, and_list};
use crate::space_bin::BIN;
use crate::space_removing::{
    self, Looked, Removable, back_words, free_bytes, freed_words, gone, result_figures, result_words, sent_words,
    stayed,
};

/// The spare copies line's id.
pub const SPARE_COPIES: &str = "spare-copies";

/// What the line says while the first look is on.
pub const LOOKING: &str = "Looking through your plugin folders…";

/// What the page says under Biggest wins first when a look found none.
pub const NONE_FOUND: &str = "No spare plugin copies were found. ";

pub use crate::space_removing::{ADMIN, ADMIN_BACK};

/// The activity card's title while Remove is on.
pub const REMOVING: &str = "Removing spare plugin copies";

/// Why every ticked copy stays when Remove couldn't read your plugin folders or which plugins your saves use, to check
/// each copy is still spare.
pub const CANT_CHECK: &str = "Your plugin folders or which plugins your saves use couldn't be read to check it again (the \
    log says why), so it stays.";

/// Why a copy that's listed can't go: what [`spare::look`] leaves without a stamp.
pub const CANT_GO: &str = "This copy can't go from here: it's a link to another place, it's on a network drive or only \
    online in OneDrive, or some of it couldn't be read.";

/// What a look for spare copies found: each one, biggest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub copies: Vec<SpareCopy>,
}

impl Found {
    /// The room the ones that can go take.
    pub fn bytes(&self) -> u64 {
        free_bytes(&self.copies)
    }
}

impl Looked for Found {
    type Item = SpareCopy;
    fn items(&self) -> &[SpareCopy] {
        &self.copies
    }
}

/// Every copy that can go is ticked out of the box, as in the C# app.
impl Removable for SpareCopy {
    fn path(&self) -> &str {
        &self.path
    }
    fn bytes(&self) -> u64 {
        self.bytes
    }
    fn can_go(&self) -> bool {
        self.can_remove()
    }
}

/// How removing spare copies goes while the app runs: the copies you unticked, and where Remove has got to.
pub type Removing = space_removing::Removing<SpareCopy>;

/// Where removing spare copies has got to.
pub type Stage = space_removing::Stage<SpareCopy>;

/// A removal of spare copies that ran.
pub type Sent = space_removing::Sent<SpareCopy>;

/// Each plugin your saves use (a DAW's own devices left out), with the saves that use each format: what
/// [`spare::find`] takes, as the C# app worked it out for its spare copies (`PluginFormatsUsed`). A save whose
/// plugin's format isn't known ("" or an old FL Studio save's "VST") is counted too, so that plugin keeps every copy.
pub fn formats_used(uses: &[PluginUse]) -> Used {
    let mut out = Used::new();
    for u in uses.iter().filter(|u| !u.format.eq_ignore_ascii_case("Native") && !u.name.trim().is_empty()) {
        out.entry(u.name.clone()).or_default().entry(u.format.clone()).or_default().insert(u.save);
    }
    out
}

/// The folders to look in: your plugin folders as Settings lists them (`chosen`; none means the standard ones,
/// `standard`), then the AAX folders (`aax`), which are never on that list.
pub fn plugin_folders(chosen: &[String], standard: Vec<PathBuf>, aax: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = chosen.iter().filter(|f| !f.trim().is_empty()).map(PathBuf::from).collect();
    if folders.is_empty() {
        folders = standard;
    }
    folders.extend(aax);
    folders
}

/// Looks for spare copies: every plugin file in `folders`, then each one in a format `used` (from [`formats_used`])
/// says none of your saves use, measured. `pro_tools` says whether Pro Tools is installed. Reads and measures plugin
/// files: call it away from the window's thread. Stopped, it says so and finds nothing.
pub fn look(folders: &[PathBuf], used: &Used, pro_tools: bool, stop: &AtomicBool) -> Result<Found, Stopped> {
    let files = scan_files(folders, stop)?;
    let copies = spare::find(&files, used, pro_tools, |path| spare::look(path, stop));
    if stop.load(Ordering::Relaxed) {
        return Err(Stopped);
    }
    Ok(Found { copies })
}

/// Sends the copies you picked to the Recycle Bin, each checked again first: every plugin file in `folders` is found
/// again and checked against `used` and `pro_tools` as they are now (only which copies are spare counts there, so
/// nothing is measured), then [`spare::remove`] checks each copy just before it goes. Copies Windows guards go together
/// at the end, Windows asking once. Reads and changes the disk, and waits for Windows' question: call it away from the
/// window's thread.
pub fn remove(
    picked: &[SpareCopy],
    folders: &[PathBuf],
    used: &Used,
    pro_tools: bool,
    bin: &dyn Bin,
    stop: &AtomicBool,
) -> Done {
    match scan_files(folders, stop) {
        Ok(files) => {
            let now = spare::find(&files, used, pro_tools, |_| (0, None));
            spare::remove(picked, &now, bin, stop)
        }
        Err(Stopped) => stayed(picked, "Remove stopped before it went."),
    }
}

/// The spare copies in one format.
struct Group<'a> {
    /// As the first copy names it: "AAX", "VST2".
    format: &'a str,
    copies: Vec<&'a SpareCopy>,
}

impl Group<'_> {
    fn bytes(&self) -> u64 {
        free_bytes(self.copies.iter().copied())
    }

    /// The folder every copy is in, when they're all in one. Letter case counts: on Linux "Foo" and "foo" are two
    /// folders, and on Windows the copies found in one folder all come with the same letters.
    fn one_folder(&self) -> Option<&str> {
        let first = folder_of(trimmed(&self.copies.first()?.path));
        let same = self.copies.iter().all(|c| folder_of(trimmed(&c.path)) == first);
        (same && !first.is_empty()).then_some(first)
    }
}

/// A copy's path without a slash at its end (a bundle is a folder), so its name and folder can be read from it.
fn trimmed(path: &str) -> &str {
    path.trim_end_matches(['\\', '/'])
}

/// The copies by format, the one that frees most first; each group's copies keep their order (biggest first). Formats
/// whose names differ only in letter case are one.
fn groups(found: &Found) -> Vec<Group<'_>> {
    let mut out: Vec<Group<'_>> = Vec::new();
    for copy in &found.copies {
        match out.iter_mut().find(|g| case::eq_ignore_case(g.format, &copy.format)) {
            Some(g) => g.copies.push(copy),
            None => out.push(Group { format: &copy.format, copies: vec![copy] }),
        }
    }
    // A stable sort: formats that free the same keep the order they were met in.
    out.sort_by_key(|g| std::cmp::Reverse(g.bytes()));
    out
}

/// "1 AAX copy", "90 AAX copies".
fn copies_words(n: usize, format: &str) -> String {
    if n == 1 { format!("1 {format} copy") } else { format!("{} {format} copies", thousands(n)) }
}

/// The formats, as a sentence says them: "90 AAX copies", "90 AAX and 40 VST2 copies", "90 AAX, 40 VST2 and 1 CLAP
/// copies".
fn formats_words(groups: &[Group<'_>]) -> String {
    match groups {
        [] => String::new(),
        [g] => copies_words(g.copies.len(), g.format),
        _ => {
            let counts: Vec<String> =
                groups.iter().map(|g| format!("{} {}", thousands(g.copies.len()), g.format)).collect();
            format!("{} copies", and_list(&counts))
        }
    }
}

/// How many can go, and how many can't: "130 spare copies", "128 spare copies can go, and 2 can't".
fn count_words(found: &Found) -> String {
    let go = found.copies.iter().filter(|c| c.can_remove()).count();
    let staying = found.copies.len() - go;
    let spare = |n: usize| if n == 1 { "1 spare copy".to_owned() } else { format!("{} spare copies", thousands(n)) };
    match (go, staying) {
        (0, 0) => String::new(),
        (0, m) => format!("{} can't go", spare(m)),
        (n, 0) => spare(n),
        (n, m) => format!("{} can go, and {} can't", spare(n), thousands(m)),
    }
}

/// The spare copies line under Biggest wins first, with the room the ones that can go would free (what the wins are
/// sorted by). While the first look is on (`looking`, nothing found yet) it says so; none when a look found none, or
/// none has finished and none is on. With only copies that can't go, it shows no size: they free nothing. Once a
/// removal's result is on show (`removing`), it says how that went instead, freeing nothing more, until Measure again.
pub fn line(found: Option<&Found>, looking: bool, removing: &Removing) -> Option<(u64, Line)> {
    let line = |why: String, size: String| Line { id: SPARE_COPIES, title: "Spare plugin copies", why, size };
    match removing.stage() {
        Stage::Sent(sent) => return Some((0, line(sent_words(sent, &spare_copies), freed_words(sent)))),
        Stage::PuttingBack(sent) => return Some((0, line("Putting them back…".to_owned(), freed_words(sent)))),
        Stage::PutBack(sent, problems) => return Some((0, line(back_words(sent, problems), String::new()))),
        Stage::Picking | Stage::Sending(_) => {}
    }
    match found {
        Some(f) if !f.copies.is_empty() => {
            let bytes = f.bytes();
            let why = format!("{} your saves don't use", formats_words(&groups(f)));
            Some((bytes, line(why, if bytes == 0 { String::new() } else { size_text(bytes) })))
        }
        None if looking => Some((0, line(LOOKING.to_owned(), "Measuring…".to_owned()))),
        _ => None,
    }
}

/// What the page says under Biggest wins first about spare copies: that a look found none, or nothing.
pub fn none_note(found: Option<&Found>) -> &'static str {
    match found {
        Some(f) if f.copies.is_empty() => NONE_FOUND,
        _ => "",
    }
}

/// Why a group's copies are spare.
fn group_why(format: &str) -> &'static str {
    if case::eq_ignore_case(format, "AAX") {
        "Only Pro Tools loads AAX, and it isn't on this computer."
    } else {
        "Your saves use another format of each of these, and that copy stays."
    }
}

/// What a copy's own line says under its name: why it's spare, when that isn't the same for every copy in its format
/// (the formats your saves use instead, and in how many saves), or why it can't go.
fn status_words(copy: &SpareCopy) -> (String, bool) {
    if !copy.can_remove() {
        return (CANT_GO.to_owned(), true);
    }
    match &copy.why {
        Why::NoProTools => (String::new(), false),
        // Never with none: a copy is only spare beside a used format's copy.
        Why::OtherFormatsUsed { used, .. } if used.is_empty() => (String::new(), false),
        Why::OtherFormatsUsed { used, projects, .. } => {
            let formats = and_list(used);
            let saves = usize::try_from(*projects).unwrap_or(usize::MAX);
            (format!("Used as {formats} in {}.", plural(saves, "save")), false)
        }
    }
}

/// A copy's file name, when it says more than the plugin's name with its format's ending ("Serum_x64.dll" for Serum,
/// but not "Kontakt 7.aaxplugin" for Kontakt 7).
fn file_words(copy: &SpareCopy) -> Option<&str> {
    let file = paths::file_name(trimmed(&copy.path));
    (!case::eq_ignore_case(paths::stem(file), &copy.name)).then_some(file)
}

/// What a copy's own line says under its name: its file when that says more than its name, and its folder unless its
/// format's copies are all in one (`folder`), which their format's line says: the folder it's in, or the one it came
/// from while it's in the bin (`binned`).
fn copy_what(copy: &SpareCopy, folder: Option<&str>, binned: bool) -> String {
    let in_folder = || wrappable_path(folder_of(trimmed(&copy.path)));
    let (from, that) = if binned { ("From", "from") } else { ("In", "in") };
    match (folder, file_words(copy)) {
        (Some(_), file) => file.unwrap_or_default().to_owned(),
        (None, Some(file)) => format!("{file}, {that} {}", in_folder()),
        (None, None) => format!("{from} {}", in_folder()),
    }
}

/// A format's own line: how many copies, why they're spare, the room `bytes` (none when it's 0), and the folder they're
/// all in, with a link that opens it.
fn group_part(g: &Group<'_>, bytes: u64) -> Part {
    let folder = g.one_folder();
    Part {
        name: copies_words(g.copies.len(), g.format),
        what: group_why(g.format).to_owned(),
        size: if bytes == 0 { String::new() } else { size_text(bytes) },
        status: folder.map(|f| format!("All in {}", wrappable_path(f))).unwrap_or_default(),
        open: folder.map(|f| (Opens::Folder, PathBuf::from(f))),
        ..Part::default()
    }
}

/// What the card says about Remove, above it: what it does, the checks, and where the copies go (`can_put_back`: the
/// app can put them back here, which a Mac's Trash doesn't let it). Copies in Program Files (`guarded`: Windows guards
/// them) go together, Windows asking once, and come back together the same way.
fn remove_note(can_put_back: bool, guarded: bool) -> String {
    let checks = format!(
        "Remove sends the ticked copies to the {BIN}, each checked again just before it goes: one that changed since, \
         or that a save uses now, stays."
    );
    match (can_put_back, guarded) {
        (true, false) => format!("{checks} Put back returns them until you empty the {BIN}."),
        (true, true) => {
            format!("{checks} {ADMIN} Put back returns them until you empty the {BIN}, with Windows asking once again.")
        }
        (false, _) => format!("{checks} Until you empty the {BIN}, Finder's Put Back returns them."),
    }
}

/// The spare copies card. While picking (and while Remove is on): the room the ones that can go take, then each format
/// (the one that frees most first) with why its copies are spare, and under it each copy, biggest first, with its tick
/// (every copy that can go, ticked out of the box; a format's tick is for all its copies). A format whose copies are
/// all in one folder says it once, with a link that opens it, and its copies take a line each; otherwise each copy says
/// its folder, with a link that shows it there. `guarded` says whether Windows guards a copy's place (the app passes
/// `mo_space::plugin_space::is_install_folder`): the card says Windows asks once for those. Once a removal has run, the
/// card shows how it went instead ([`Removing`]), until Measure again.
pub fn card(
    found: Option<&Found>,
    looking: bool,
    removing: &Removing,
    can_put_back: bool,
    guarded: impl Fn(&str) -> bool,
) -> Card {
    match removing.stage() {
        Stage::Sent(sent) | Stage::PuttingBack(sent) | Stage::PutBack(sent, _) => {
            result_card(sent, removing.stage(), can_put_back)
        }
        Stage::Picking | Stage::Sending(_) => picking_card(found, looking, removing, can_put_back, guarded),
    }
}

fn picking_card(
    found: Option<&Found>,
    looking: bool,
    removing: &Removing,
    can_put_back: bool,
    guarded: impl Fn(&str) -> bool,
) -> Card {
    let picking = *removing.stage() == Stage::Picking;
    let groups = found.map(groups).unwrap_or_default();
    let mut parts = Vec::new();
    for g in &groups {
        let folder = g.one_folder();
        let can_go: Vec<&SpareCopy> = g.copies.iter().copied().filter(|c| c.can_remove()).collect();
        let ticked = can_go.iter().filter(|c| removing.ticked(c)).count();
        let mut line = group_part(g, g.bytes());
        if picking && !can_go.is_empty() {
            line.tick = Some((can_go.iter().map(|c| PathBuf::from(&c.path)).collect(), ticked == can_go.len()));
            line.partly = ticked > 0 && ticked < can_go.len();
        }
        parts.push(line);
        for copy in &g.copies {
            let (status, problem) = status_words(copy);
            parts.push(Part {
                name: copy.name.clone(),
                what: copy_what(copy, folder, false),
                size: size_text(copy.bytes),
                under: true,
                tick: (picking && copy.can_remove()).then(|| (vec![PathBuf::from(&copy.path)], removing.ticked(copy))),
                status,
                problem,
                open: folder.is_none().then(|| (Opens::InFolder, PathBuf::from(&copy.path))),
                ..Part::default()
            });
        }
    }
    let copies = found.map(|f| f.copies.as_slice()).unwrap_or_default();
    let can_go = copies.iter().any(SpareCopy::can_remove);
    let note = match removing.stage() {
        Stage::Sending(_) => format!("Checking each ticked copy again, then sending it to the {BIN}…"),
        _ if can_go => remove_note(can_put_back, removing.each_ticked(found).any(|c| guarded(&c.path))),
        _ => String::new(),
    };
    Card {
        kind: "SPARE COPIES",
        title: "Spare plugin copies",
        big: match found {
            None if looking => "Measuring…".to_owned(),
            None => String::new(),
            Some(f) if f.bytes() == 0 && !f.copies.is_empty() => "Nothing to free".to_owned(),
            Some(f) => size_text(f.bytes()),
        },
        big_note: found.map(count_words).unwrap_or_default(),
        lead: LEAD.to_owned(),
        ticks: can_go,
        parts,
        note,
        room: String::new(),
        warning: String::new(),
    }
}

/// The card's lead.
const LEAD: &str = "Plugin installers often put each plugin in every format: VST2, VST3, CLAP and AAX. These copies are \
    in a format none of your saves use; the copies your saves do use stay.";

/// "1 spare copy", "10 spare copies".
fn spare_copies(n: usize) -> String {
    if n == 1 { "1 spare copy".to_owned() } else { format!("{} spare copies", thousands(n)) }
}

/// The card once Remove has run: what was freed, what went and what stayed (each copy picked, by format, with how it
/// went), and what Put back does (for the copies that went as administrator, from Program Files, Windows asks once);
/// then, once Put back has run, what came back.
fn result_card(sent: &Sent, stage: &Stage, can_put_back: bool) -> Card {
    let picked = Found { copies: sent.picked.clone() };
    let mut parts = Vec::new();
    for g in &groups(&picked) {
        let folder = g.one_folder();
        let went = g.copies.iter().filter(|c| sent.went(c)).fold(0u64, |sum, c| sum.saturating_add(c.bytes));
        parts.push(group_part(g, went));
        for copy in &g.copies {
            let (status, problem) = result_words(*copy, sent, stage);
            // Where it is, with a link that shows it there, only while it's there: not once it went, until it's back.
            let gone = gone(*copy, sent, stage);
            parts.push(Part {
                name: copy.name.clone(),
                what: copy_what(copy, folder, gone),
                size: size_text(copy.bytes),
                under: true,
                status,
                problem,
                open: (folder.is_none() && !gone).then(|| (Opens::InFolder, PathBuf::from(&copy.path))),
                ..Part::default()
            });
        }
    }
    let (big, big_note, note) = result_figures(sent, stage, can_put_back, &spare_copies);
    Card {
        kind: "SPARE COPIES",
        title: "Spare plugin copies",
        big,
        big_note,
        lead: LEAD.to_owned(),
        ticks: false,
        parts,
        note,
        room: String::new(),
        warning: String::new(),
    }
}

/// The activity card's line while Remove is on.
pub fn removing_words() -> String {
    format!("Each one is checked again just before it goes to the {BIN}.")
}

/// The activity card's line once Remove is done: what was freed, what's in the bin, what went but wasn't found there,
/// and what stayed. Err when nothing went (the card then fails).
pub fn removed_words(done: &Done) -> Result<String, String> {
    space_removing::removed_words(done, &|n| {
        if n == 1 { "1 copy".to_owned() } else { format!("{} copies", thousands(n)) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space_bin::{PutBacks, yourself_words};
    use crate::space_removing::crashed_words;
    use mo_library::PluginKind;
    use mo_space::clean::Problem;
    use std::collections::{HashMap, HashSet};

    const MB: u64 = 1 << 20;

    /// Text as it reads, without the marks that let a path wrap after its slashes.
    fn read(text: &str) -> String {
        text.replace('\u{200B}', "")
    }

    const AVID: &str = r"C:\Program Files\Common Files\Avid\Audio\Plug-Ins";

    fn aax(name: &str, mb: u64) -> SpareCopy {
        SpareCopy {
            name: name.to_owned(),
            format: "AAX".to_owned(),
            path: format!(r"{AVID}\{name}.aaxplugin"),
            bytes: mb * MB,
            stamp: Some("stamp".to_owned()),
            why: Why::NoProTools,
        }
    }

    fn vst2(name: &str, folder: &str, mb: u64, used: &[&str], saves: u32) -> SpareCopy {
        SpareCopy {
            name: name.to_owned(),
            format: "VST2".to_owned(),
            path: format!(r"{folder}\{name}_x64.dll"),
            bytes: mb * MB,
            stamp: Some("stamp".to_owned()),
            why: Why::OtherFormatsUsed {
                used: used.iter().map(|u| (*u).to_owned()).collect(),
                projects: saves,
                used_copies: Vec::new(),
            },
        }
    }

    fn sample() -> Found {
        let mut copies = vec![
            aax("Serum", 300),
            vst2("Serum", r"C:\Program Files\VSTPlugins", 250, &["VST3"], 412),
            aax("Pro-Q 3", 60),
            vst2("Diva", r"C:\Program Files\Steinberg\VSTPlugins", 40, &["VST3", "CLAP"], 1),
        ];
        copies.sort_by_key(|c| std::cmp::Reverse(c.bytes));
        Found { copies }
    }

    /// The card while picking, with no copy in a place Windows guards.
    fn picking(found: Option<&Found>, looking: bool) -> Card {
        card(found, looking, &Removing::default(), true, |_| false)
    }

    /// The line while picking.
    fn line_now(found: Option<&Found>, looking: bool) -> Option<(u64, Line)> {
        line(found, looking, &Removing::default())
    }

    fn uses(save: i64, name: &str, format: &str) -> PluginUse {
        PluginUse {
            save,
            name: name.to_owned(),
            key: mo_library::read::name_key(name),
            vendor: String::new(),
            kind: PluginKind::Instrument,
            format: format.to_owned(),
            installed: true,
            daw: "FL Studio".to_owned(),
        }
    }

    #[test]
    fn formats_are_counted_by_save() {
        let used = formats_used(&[
            uses(1, "Serum", "VST3"),
            // Twice in one save counts once.
            uses(1, "Serum", "VST3"),
            uses(2, "Serum", "VST3"),
            uses(2, "Serum", "VST2"),
            // A DAW's own device has no files, and a nameless row says nothing.
            uses(3, "Delay", "Native"),
            uses(3, "Reverb", "native"),
            uses(3, " ", "VST3"),
            // A format no one knows is kept, so that plugin keeps every copy.
            uses(4, "Sylenth1", "VST"),
        ]);
        let saves = |ids: &[i64]| ids.iter().copied().collect::<HashSet<i64>>();
        assert_eq!(
            used["Serum"],
            HashMap::from([("VST3".to_owned(), saves(&[1, 2])), ("VST2".to_owned(), saves(&[2]))])
        );
        assert_eq!(used["Sylenth1"], HashMap::from([("VST".to_owned(), saves(&[4]))]));
        assert_eq!(used.len(), 2);
    }

    #[test]
    fn your_plugin_folders_come_first_then_the_aax_ones() {
        let standard = vec![PathBuf::from(r"C:\Program Files\Common Files\VST3")];
        let aax = vec![PathBuf::from(AVID)];
        assert_eq!(
            plugin_folders(&[r"I:\VST".to_owned(), " ".to_owned()], standard.clone(), aax.clone()),
            vec![PathBuf::from(r"I:\VST"), PathBuf::from(AVID)]
        );
        // None chosen: the standard ones.
        assert_eq!(plugin_folders(&[], standard.clone(), aax), [standard, vec![PathBuf::from(AVID)]].concat());
    }

    #[test]
    fn the_line_names_each_format_with_the_room_it_frees() {
        let (bytes, line) = line_now(Some(&sample()), false).unwrap();
        assert_eq!(bytes, 650 * MB);
        assert_eq!(line.id, SPARE_COPIES);
        assert_eq!(line.why, "2 AAX and 2 VST2 copies your saves don't use");
        assert_eq!(line.size, "650 MB");
        let one = Found { copies: vec![aax("Serum", 300)] };
        assert_eq!(line_now(Some(&one), false).unwrap().1.why, "1 AAX copy your saves don't use");
        // While the first look is on, it says so; a look that found none, or none at all: no line.
        let first = line_now(None, true).unwrap();
        assert_eq!((first.0, first.1.why.as_str(), first.1.size.as_str()), (0, LOOKING, "Measuring…"));
        assert_eq!(line_now(Some(&Found::default()), false), None);
        assert_eq!(line_now(None, false), None);
        // A new look keeps the last one's line on show.
        assert_eq!(line_now(Some(&sample()), true).unwrap().0, 650 * MB);
        // Only copies that can't go: no room to free, so no size.
        let stuck = Found { copies: vec![SpareCopy { stamp: None, ..aax("Serum", 300) }] };
        let (bytes, line) = line_now(Some(&stuck), false).unwrap();
        assert_eq!((bytes, line.size.as_str()), (0, ""));
    }

    #[test]
    fn the_card_lists_each_format_then_its_copies() {
        let card = picking(Some(&sample()), false);
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("650 MB", "4 spare copies"));
        let names: Vec<&str> = card.parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["2 AAX copies", "Serum", "Pro-Q 3", "2 VST2 copies", "Serum", "Diva"]);
        // AAX frees most, so it comes first; its copies are all in one folder, said once.
        let avid = &card.parts[0];
        assert_eq!(
            (avid.what.as_str(), avid.size.as_str(), read(&avid.status).as_str(), avid.under),
            (
                "Only Pro Tools loads AAX, and it isn't on this computer.",
                "360 MB",
                format!("All in {AVID}").as_str(),
                false
            )
        );
        assert_eq!(avid.open, Some((Opens::Folder, PathBuf::from(AVID))));
        // So each of its copies takes a line: its name says its file, and its folder's link shows it.
        let serum = &card.parts[1];
        assert_eq!(
            (serum.what.as_str(), serum.size.as_str(), serum.status.as_str(), serum.under, serum.open.is_none()),
            ("", "300 MB", "", true, true)
        );
        // The VST2 copies are in two folders, so each says its own, with the formats your saves use instead.
        let vst2s = &card.parts[3];
        assert_eq!((vst2s.size.as_str(), vst2s.status.as_str(), vst2s.open.is_none()), ("290 MB", "", true));
        let serum = &card.parts[4];
        assert_eq!(read(&serum.what), r"Serum_x64.dll, in C:\Program Files\VSTPlugins");
        assert_eq!((serum.status.as_str(), serum.problem), ("Used as VST3 in 412 saves.", false));
        let shown = Some((Opens::InFolder, PathBuf::from(r"C:\Program Files\VSTPlugins\Serum_x64.dll")));
        assert_eq!(serum.open, shown);
        assert_eq!(card.parts[5].status, "Used as VST3 and CLAP in 1 save.");
        // Every copy that can go is ticked out of the box, and each format's tick is for all of its copies.
        assert!(card.ticks);
        let aax_paths = vec![
            PathBuf::from(format!(r"{AVID}\Serum.aaxplugin")),
            PathBuf::from(format!(r"{AVID}\Pro-Q 3.aaxplugin")),
        ];
        assert_eq!((card.parts[0].tick.clone(), card.parts[0].partly), (Some((aax_paths, true)), false));
        assert!(card.parts.iter().filter(|p| p.under).all(|p| p.tick.as_ref().is_some_and(|(_, on)| *on)));
        assert_eq!(card.note, remove_note(true, false));
        // Before the first look has finished, and once one found none.
        let first = picking(None, true);
        assert_eq!((first.big.as_str(), first.big_note.as_str()), ("Measuring…", ""));
        assert!(first.parts.is_empty() && first.note.is_empty());
        let none = picking(Some(&Found::default()), false);
        assert!(none.parts.is_empty() && none.note.is_empty() && none.big_note.is_empty());
    }

    #[test]
    fn a_copy_that_cant_go_says_why_and_frees_nothing() {
        let mut found = sample();
        found.copies[0].stamp = None;
        let card = picking(Some(&found), false);
        assert_eq!(card.big, "350 MB");
        assert_eq!(card.big_note, "3 spare copies can go, and 1 can't");
        let serum = card.parts.iter().find(|p| p.under && p.size == "300 MB").unwrap();
        assert_eq!((serum.name.as_str(), serum.status.as_str(), serum.problem), ("Serum", CANT_GO, true));
        // Its format frees only what its others do, so VST2 now frees most and comes first.
        assert_eq!((card.parts[0].name.as_str(), card.parts[0].size.as_str()), ("2 VST2 copies", "290 MB"));
        // None of them can go: nothing to free, said in words.
        let stuck = Found { copies: found.copies.iter().map(|c| SpareCopy { stamp: None, ..c.clone() }).collect() };
        let card = picking(Some(&stuck), false);
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("Nothing to free", "4 spare copies can't go"));
        // And each format's own line shows no size, as it frees nothing either.
        assert!(card.parts.iter().filter(|p| !p.under).all(|p| p.size.is_empty()));
    }

    #[test]
    fn a_file_is_named_when_its_name_says_more() {
        let serum = vst2("Serum", r"C:\VST", 1, &["VST3"], 1);
        assert_eq!(file_words(&serum), Some("Serum_x64.dll"));
        assert_eq!(file_words(&aax("Kontakt 7", 1)), None);
        // In a format whose copies are in more than one folder, each says its folder, with its file when that says
        // more (letter case aside).
        let kontakt = SpareCopy { path: r"D:\VST\kontakt 7.dll".to_owned(), ..vst2("Kontakt 7", r"D:\VST", 1, &[], 1) };
        let card = picking(Some(&Found { copies: vec![serum, kontakt] }), false);
        assert_eq!(read(&card.parts[1].what), r"Serum_x64.dll, in C:\VST");
        assert_eq!(read(&card.parts[2].what), r"In D:\VST");
    }

    #[test]
    fn counts_read_as_a_sentence() {
        let with = |go: usize, staying: usize| {
            let mut copies = vec![aax("A", 1); go];
            copies.extend(vec![SpareCopy { stamp: None, ..aax("B", 1) }; staying]);
            count_words(&Found { copies })
        };
        assert_eq!(with(1, 0), "1 spare copy");
        assert_eq!(with(1200, 0), "1,200 spare copies");
        assert_eq!(with(0, 1), "1 spare copy can't go");
        assert_eq!(with(5, 1), "5 spare copies can go, and 1 can't");
        assert_eq!(with(0, 0), "");
        let three = Found {
            copies: vec![
                aax("A", 3),
                vst2("B", r"C:\VST", 2, &["VST3"], 1),
                SpareCopy { format: "CLAP".to_owned(), ..vst2("C", r"C:\CLAP", 1, &["VST3"], 1) },
            ],
        };
        assert_eq!(formats_words(&groups(&three)), "1 AAX, 1 VST2 and 1 CLAP copies");
    }

    #[test]
    fn the_note_under_the_wins_says_when_none_were_found() {
        assert_eq!(none_note(Some(&Found::default())), NONE_FOUND);
        assert_eq!(none_note(Some(&sample())), "");
        assert_eq!(none_note(None), "");
    }

    #[test]
    fn with_nowhere_to_look_it_finds_none() {
        let stop = AtomicBool::new(false);
        assert_eq!(look(&[], &HashMap::new(), false, &stop), Ok(Found::default()));
        // Stopped before it starts: it says so.
        assert_eq!(look(&[], &HashMap::new(), false, &AtomicBool::new(true)), Err(Stopped));
    }

    fn path(copy: &SpareCopy) -> PathBuf {
        PathBuf::from(&copy.path)
    }

    #[test]
    fn ticks_pick_what_goes_and_a_formats_tick_takes_them_all() {
        let found = sample();
        let mut removing = Removing::default();
        assert_eq!(
            removing.remove_button(Some(&found), false, &PutBacks::default()),
            Some(("Remove 650 MB".to_owned(), true))
        );
        // While a look is on, Remove waits for it.
        assert_eq!(
            removing.remove_button(Some(&found), true, &PutBacks::default()),
            Some(("Remove 650 MB".to_owned(), false))
        );
        // Unticking one AAX copy: its format's tick shows a dash, and Remove frees the rest.
        let pro_q = found.copies.iter().find(|c| c.name == "Pro-Q 3").unwrap();
        removing.tick(&[path(pro_q)], false);
        let card = card(Some(&found), false, &removing, true, |_| false);
        assert_eq!((card.parts[0].tick.as_ref().map(|t| t.1), card.parts[0].partly), (Some(false), true));
        assert_eq!(
            removing.remove_button(Some(&found), false, &PutBacks::default()),
            Some(("Remove 590 MB".to_owned(), true))
        );
        // The format's tick then ticks them all again; unticked, none of them goes.
        let all_aax = card.parts[0].tick.clone().unwrap().0;
        removing.tick(&all_aax, true);
        assert_eq!(removing.ticked_copies(Some(&found)).len(), 4);
        removing.tick(&all_aax, false);
        let card = super::card(Some(&found), false, &removing, true, |_| false);
        assert_eq!((card.parts[0].tick.as_ref().map(|t| t.1), card.parts[0].partly), (Some(false), false));
        let formats: Vec<String> = removing.ticked_copies(Some(&found)).into_iter().map(|c| c.format).collect();
        assert_eq!(formats, ["VST2", "VST2"]);
        for c in &found.copies {
            removing.tick(&[path(c)], false);
        }
        assert_eq!(
            removing.remove_button(Some(&found), false, &PutBacks::default()),
            Some(("Nothing ticked".to_owned(), false))
        );
    }

    #[test]
    fn a_copy_that_cant_go_has_no_tick() {
        let mut found = sample();
        found.copies[0].stamp = None;
        let card = picking(Some(&found), false);
        let serum = card.parts.iter().find(|p| p.under && p.size == "300 MB").unwrap();
        assert_eq!(serum.tick, None);
        // Its format's tick is for the one that can go.
        assert_eq!(card.parts[1].tick.as_ref().map(|t| t.0.len()), Some(1));
        // None can go: no ticks and no Remove.
        let stuck = Found { copies: found.copies.iter().map(|c| SpareCopy { stamp: None, ..c.clone() }).collect() };
        let card = picking(Some(&stuck), false);
        assert!(!card.ticks && card.parts.iter().all(|p| p.tick.is_none()) && card.note.is_empty());
        assert_eq!(Removing::default().remove_button(Some(&stuck), false, &PutBacks::default()), None);
        assert_eq!(Removing::default().remove_button(None::<&Found>, true, &PutBacks::default()), None);
    }

    #[test]
    fn the_card_says_when_windows_will_ask() {
        let found = sample();
        let guarded = |p: &str| p.starts_with(AVID);
        let card = card(Some(&found), false, &Removing::default(), true, guarded);
        assert_eq!(card.note, remove_note(true, true));
        assert!(card.note.contains(ADMIN) && card.note.ends_with("with Windows asking once again."));
        // Only for the ticked ones.
        let mut removing = Removing::default();
        let aax: Vec<PathBuf> = found.copies.iter().filter(|c| c.format == "AAX").map(path).collect();
        removing.tick(&aax, false);
        assert_eq!(super::card(Some(&found), false, &removing, true, guarded).note, remove_note(true, false));
        // Where the app can't put things back (a Mac), the card says what can.
        assert!(super::card(Some(&found), false, &removing, false, guarded).note.contains("Finder's Put Back"));
        // Once copies went as administrator (from Program Files), the result says Windows asks once to put those back;
        // not when all went without asking.
        let sent = |as_admin: bool| {
            let mut removing = Removing::default();
            let picked = removing.ticked_copies(Some(&found));
            removing.start(picked.clone());
            let went = |c: &SpareCopy| mo_space::clean::Removed {
                stamp: (as_admin && guarded(&c.path)).then(|| "stamp".to_owned()),
                ..removed(c)
            };
            removing.sent(1, Done { removed: picked.iter().map(went).collect(), ..Done::default() });
            super::card(Some(&found), false, &removing, true, guarded).note
        };
        assert!(sent(true).contains(ADMIN_BACK));
        assert!(!sent(false).contains(ADMIN_BACK));
    }

    fn removed(copy: &SpareCopy) -> mo_space::clean::Removed {
        mo_space::clean::Removed {
            path: copy.path.clone(),
            real: copy.path.clone(),
            bytes: copy.bytes,
            what: mo_space::clean::What::Plugin(mo_space::plugin_space::SpaceKind::Program),
            when: 7,
            bin_id: Some("1".to_owned()),
            binned: None,
            binned_zone: None,
            stamp: None,
        }
    }

    #[test]
    fn remove_shows_how_it_went_then_put_back_does() {
        let found = sample();
        let mut removing = Removing::default();
        let picked = removing.ticked_copies(Some(&found));
        removing.start(picked.clone());
        assert!(removing.busy());
        assert_eq!(
            removing.remove_button(Some(&found), false, &PutBacks::default()),
            Some(("Removing…".to_owned(), false))
        );
        // Ticks wait while it's on.
        removing.tick(&[path(&picked[0])], false);
        assert_eq!(removing.ticked_copies(Some(&found)).len(), 4);
        let sending = super::card(Some(&found), false, &removing, true, |_| false);
        assert!(sending.parts.iter().all(|p| p.tick.is_none()));
        assert!(sending.note.starts_with("Checking each ticked copy again"));

        // Three went; Diva's VST2 changed since it was found, so it stayed.
        let diva = picked.iter().find(|c| c.name == "Diva").unwrap();
        let done = Done {
            removed: picked.iter().filter(|c| c.name != "Diva").map(removed).collect(),
            problems: vec![Problem { path: diva.path.clone(), why: "It changed since it was found.".to_owned() }],
            went_with: Vec::new(),
        };
        assert_eq!(
            removed_words(&done),
            Ok(format!("Freed 610 MB: 3 copies are in the {BIN}; 1 stayed (the Space page says why)."))
        );
        removing.sent(42, done.clone());
        assert!(!removing.busy() && removing.shows_result());
        assert_eq!(removing.to_put_back().map(|s| s.when), Some(42));
        assert_eq!(removing.remove_button(Some(&found), false, &PutBacks::default()), None);
        assert_eq!(removing.put_back_button(true, &PutBacks::default()), Some(("Put back".to_owned(), true)));
        assert_eq!(removing.put_back_button(false, &PutBacks::default()), None);
        let card = super::card(Some(&found), false, &removing, true, |_| false);
        assert_eq!(
            (card.big.as_str(), card.big_note.as_str()),
            ("Freed 610 MB", format!("3 spare copies are in the {BIN}, and 1 stayed").as_str())
        );
        assert!(card.note.starts_with(&format!("They're in the {BIN}. Put back returns each one")));
        assert!(!card.ticks && card.parts.iter().all(|p| p.tick.is_none()));
        let status = |card: &Card, name: &str, size: &str| {
            let p = card.parts.iter().find(|p| p.under && p.name == name && p.size == size).unwrap();
            (p.status.clone(), p.problem, p.open.is_some())
        };
        assert_eq!(status(&card, "Serum", "300 MB"), (format!("In the {BIN}."), false, false));
        assert_eq!(status(&card, "Diva", "40 MB"), ("It changed since it was found.".to_owned(), true, true));
        // A copy in the bin says the folder it came from; one that stayed, the folder it's in.
        let what =
            |card: &Card, size: &str| card.parts.iter().find(|p| p.under && p.size == size).unwrap().what.clone();
        assert!(what(&card, "250 MB").starts_with("Serum_x64.dll, from C:"), "{}", what(&card, "250 MB"));
        assert!(what(&card, "40 MB").starts_with("Diva_x64.dll, in C:"), "{}", what(&card, "40 MB"));
        // Each format's line says what of it went.
        let formats: Vec<(&str, &str)> =
            card.parts.iter().filter(|p| !p.under).map(|p| (p.name.as_str(), p.size.as_str())).collect();
        assert_eq!(formats, [("2 AAX copies", "360 MB"), ("2 VST2 copies", "250 MB")]);
        // The line says so too, and frees nothing more.
        let (bytes, l) = line(Some(&found), false, &removing).unwrap();
        assert_eq!(
            (bytes, l.why.as_str(), l.size.as_str()),
            (0, format!("3 spare copies are in the {BIN}, and 1 stayed").as_str(), "Freed 610 MB")
        );

        // Put back: one couldn't come back.
        removing.putting_back(41);
        assert!(!removing.busy(), "another removal's put back changes nothing here");
        removing.putting_back(42);
        assert!(removing.busy());
        assert_eq!(removing.put_back_button(true, &PutBacks::default()), Some(("Putting back…".to_owned(), false)));
        assert_eq!(
            status(&super::card(Some(&found), false, &removing, true, |_| false), "Serum", "300 MB").0,
            "Putting back…"
        );
        let serum_aax = picked.iter().find(|c| c.name == "Serum" && c.format == "AAX").unwrap();
        let stuck = Problem { path: serum_aax.path.clone(), why: "It couldn't be put back.".to_owned() };
        removing.put_back(42, vec![stuck]);
        let card = super::card(Some(&found), false, &removing, true, |_| false);
        assert_eq!(
            (card.big.as_str(), card.big_note.as_str()),
            ("Put back", "2 are back where they were, and 1 couldn't come back")
        );
        assert_eq!(card.note, yourself_words());
        assert_eq!(
            status(&card, "Serum", "300 MB"),
            ("It couldn't come back: It couldn't be put back.".to_owned(), true, false)
        );
        assert_eq!(status(&card, "Serum", "250 MB"), ("Back where it was.".to_owned(), false, true));
        assert_eq!(
            line(Some(&found), false, &removing).unwrap().1.why,
            "2 are back where they were, and 1 couldn't come back"
        );
        assert_eq!(removing.put_back_button(true, &PutBacks::default()), None);

        // Measure again: back to picking, with what the new look finds.
        removing.again();
        assert_eq!(*removing.stage(), Stage::Picking);
        assert_eq!(line(Some(&found), false, &removing), line_now(Some(&found), false));
    }

    #[test]
    fn one_thing_goes_or_comes_back_at_a_time() {
        let found = sample();
        let mut removing = Removing::default();
        // While a clean-up is put back from the Recycle Bin card, Remove waits.
        let mut put_backs = PutBacks::default();
        put_backs.start(SPARE_COPIES, 5);
        assert_eq!(removing.remove_button(Some(&found), false, &put_backs), Some(("Remove 650 MB".to_owned(), false)));
        // And this card's Put back waits for it too.
        let picked = removing.ticked_copies(Some(&found));
        removing.start(picked.clone());
        removing.sent(42, Done { removed: picked.iter().map(removed).collect(), ..Done::default() });
        assert_eq!(removing.put_back_button(true, &put_backs), Some(("Put back".to_owned(), false)));
        put_backs.done(SPARE_COPIES, 5, Vec::new());
        assert_eq!(removing.put_back_button(true, &put_backs), Some(("Put back".to_owned(), true)));
        // Every copy stays, saying why, when Remove couldn't check them again or stopped with an error.
        assert!(stayed(&picked, CANT_CHECK).problems.iter().all(|p| p.why == CANT_CHECK));
        assert!(crashed_words().ends_with(&format!("is in the {BIN}.")));
    }

    #[test]
    fn a_copy_that_went_but_wasnt_found_in_the_bin_isnt_counted_there() {
        let found = sample();
        let mut removing = Removing::default();
        let picked = removing.ticked_copies(Some(&found));
        removing.start(picked.clone());
        // All four went, but Windows removed Serum's AAX copy for good (too big for the Recycle Bin).
        let serum = picked.iter().find(|c| c.name == "Serum" && c.format == "AAX").unwrap();
        let lost = Problem { path: serum.path.clone(), why: mo_space::clean::NOT_IN_BIN.to_owned() };
        let done = Done { removed: picked.iter().map(removed).collect(), problems: vec![lost], went_with: Vec::new() };
        assert_eq!(
            removed_words(&done),
            Ok(format!("Freed 650 MB: 3 copies are in the {BIN}, and 1 went but wasn't found there."))
        );
        removing.sent(9, done);
        let status = |card: &Card, size: &str| {
            let p = card.parts.iter().find(|p| p.under && p.size == size).unwrap();
            (p.status.clone(), p.problem, p.open.is_some(), read(&p.what))
        };
        let card = super::card(Some(&found), false, &removing, true, |_| false);
        assert_eq!(
            (card.big.as_str(), card.big_note.as_str()),
            ("Freed 650 MB", format!("3 spare copies are in the {BIN}, and 1 went but wasn't found there").as_str())
        );
        let gone = (mo_space::clean::NOT_IN_BIN.to_owned(), true, false, String::new());
        assert_eq!(status(&card, "300 MB"), gone);
        // Put back puts back the three found there, and once they're back, says so of them.
        let sent = removing.to_put_back().unwrap();
        assert_eq!(sent.coming_back().removed.len(), 3);
        assert!(sent.coming_back().removed.iter().all(|r| r.path != serum.path));
        removing.putting_back(9);
        removing.put_back(9, Vec::new());
        let card = super::card(Some(&found), false, &removing, true, |_| false);
        assert_eq!(card.big_note, "All 3 are back where they were");
        assert_eq!(status(&card, "300 MB"), gone);
        assert_eq!(status(&card, "250 MB").0, "Back where it was.");
        assert!(status(&card, "250 MB").2, "what's back is shown where it is");

        // None found in the bin: nothing to put back, and the card doesn't say they're there.
        let mut removing = Removing::default();
        removing.start(picked.clone());
        let lost = |c: &SpareCopy| Problem { path: c.path.clone(), why: mo_space::clean::NOT_IN_BIN.to_owned() };
        let done = Done {
            removed: picked.iter().map(removed).collect(),
            problems: picked.iter().map(lost).collect(),
            went_with: Vec::new(),
        };
        assert_eq!(removed_words(&done), Ok(format!("Freed 650 MB: 4 copies went but weren't found in the {BIN}.")));
        removing.sent(10, done);
        assert_eq!((removing.to_put_back(), removing.put_back_button(true, &PutBacks::default())), (None, None));
        let card = super::card(Some(&found), false, &removing, true, |_| false);
        assert_eq!(card.big_note, format!("4 spare copies went but weren't found in the {BIN}"));
        assert_eq!(card.note, "");
    }

    #[test]
    fn when_nothing_could_go_it_says_why() {
        let found = sample();
        let mut removing = Removing::default();
        let picked = removing.ticked_copies(Some(&found));
        removing.start(picked.clone());
        let done = stayed(&picked, "Your saves couldn't be read again, so it stays.");
        assert_eq!(removed_words(&done), Err("None could go: the Space page says why.".to_owned()));
        removing.sent(1, done);
        assert_eq!(removing.to_put_back(), None);
        assert_eq!(removing.put_back_button(true, &PutBacks::default()), None);
        let card = super::card(Some(&found), false, &removing, true, |_| false);
        assert_eq!(
            (card.big.as_str(), card.big_note.as_str(), card.note.as_str()),
            ("Nothing freed", "None could go: each one's line says why", "")
        );
        assert!(card.parts.iter().filter(|p| p.under).all(|p| p.problem));
        assert_eq!(line(Some(&found), false, &removing).unwrap().1.size, "");
        assert_eq!(removed_words(&Done::default()), Err("Nothing was removed.".to_owned()));
    }

    #[test]
    fn a_removal_that_all_went_says_so() {
        let found = Found { copies: vec![aax("Serum", 300)] };
        let done = Done { removed: vec![removed(&found.copies[0])], ..Done::default() };
        assert_eq!(removed_words(&done), Ok(format!("Freed 300 MB: 1 copy is in the {BIN}.")));
        let mut removing = Removing::default();
        removing.start(found.copies.clone());
        removing.sent(5, done);
        assert_eq!(line(Some(&found), false, &removing).unwrap().1.why, format!("1 spare copy is in the {BIN}"));
        removing.put_back(5, Vec::new());
        let card = super::card(Some(&found), false, &removing, true, |_| false);
        assert_eq!((card.big_note.as_str(), card.note.as_str()), ("It's back where it was", ""));
    }
}
