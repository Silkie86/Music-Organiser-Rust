//! The Space page's spare plugin copies (phase 4, part 4). Plugin installers often put each plugin in every format:
//! VST2, VST3, CLAP and AAX. A copy is spare when your saves use another format of that plugin, which stays installed,
//! or when it's AAX and Pro Tools, the only DAW that loads AAX, isn't on this computer. Solomon's F12 note (25 Oct
//! 2026): "if i have lots of aax plugins but i never use them or have a vst 2 version i never use show that".
//! `mo_space::spare` finds and measures them; this works out which formats your saves use (from the library, read
//! only) and says every word of the line and the card. Nothing is removed yet: sending the ones you pick to the Recycle
//! Bin, with Put back, comes next.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_formats::{case, paths};
use mo_library::plugins::PluginUse;
use mo_space::Stopped;
use mo_space::plugins::scan_files;
use mo_space::size::size_text;
use mo_space::spare::{self, SpareCopy, Used, Why};

use crate::shell::{folder_of, wrappable_path};
use crate::songs::{plural, thousands};
use crate::space::{Card, Line, Opens, Part, and_list};

/// The spare copies line's id.
pub const SPARE_COPIES: &str = "spare-copies";

/// What the line says while the first look is on.
pub const LOOKING: &str = "Looking through your plugin folders…";

/// What the page says under Biggest wins first when a look found none.
pub const NONE_FOUND: &str = "No spare plugin copies were found. ";

/// What the card says comes next.
pub const NEXT: &str = "Nothing is removed yet. Sending the copies you pick to the Recycle Bin, with Put back, comes \
    next.";

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

/// The room the copies that can go take, of `copies`.
fn free_bytes<'a>(copies: impl IntoIterator<Item = &'a SpareCopy>) -> u64 {
    copies.into_iter().filter(|c| c.can_remove()).fold(0u64, |sum, c| sum.saturating_add(c.bytes))
}

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
/// none has finished and none is on. With only copies that can't go, it shows no size: they free nothing.
pub fn line(found: Option<&Found>, looking: bool) -> Option<(u64, Line)> {
    let line = |why: String, size: String| Line { id: SPARE_COPIES, title: "Spare plugin copies", why, size };
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

/// The spare copies card: the room the ones that can go take, then each format (the one that frees most first) with
/// why its copies are spare, and under it each copy, biggest first. A format whose copies are all in one folder says
/// it once, with a link that opens it, and its copies take a line each; otherwise each copy says its folder, with a
/// link that shows it there.
pub fn card(found: Option<&Found>, looking: bool) -> Card {
    let groups = found.map(groups).unwrap_or_default();
    let mut parts = Vec::new();
    for g in &groups {
        let folder = g.one_folder();
        parts.push(Part {
            name: copies_words(g.copies.len(), g.format),
            what: group_why(g.format).to_owned(),
            // None for a format whose copies all can't go: it frees nothing (as the card's big figure says).
            size: if g.bytes() == 0 { String::new() } else { size_text(g.bytes()) },
            status: folder.map(|f| format!("All in {}", wrappable_path(f))).unwrap_or_default(),
            open: folder.map(|f| (Opens::Folder, PathBuf::from(f))),
            ..Part::default()
        });
        for copy in &g.copies {
            let in_folder = || wrappable_path(folder_of(trimmed(&copy.path)));
            let what = match (folder, file_words(copy)) {
                (Some(_), file) => file.unwrap_or_default().to_owned(),
                (None, Some(file)) => format!("{file}, in {}", in_folder()),
                (None, None) => format!("In {}", in_folder()),
            };
            let (status, problem) = status_words(copy);
            parts.push(Part {
                name: copy.name.clone(),
                what,
                size: size_text(copy.bytes),
                under: true,
                status,
                problem,
                open: folder.is_none().then(|| (Opens::InFolder, PathBuf::from(&copy.path))),
                ..Part::default()
            });
        }
    }
    let copies = found.map(|f| f.copies.as_slice()).unwrap_or_default();
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
        lead: "Plugin installers often put each plugin in every format: VST2, VST3, CLAP and AAX. These copies are in \
               a format none of your saves use; the copies your saves do use stay."
            .to_owned(),
        ticks: false,
        parts,
        note: if copies.is_empty() { String::new() } else { NEXT.to_owned() },
        room: String::new(),
        warning: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_library::PluginKind;
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
        let (bytes, line) = line(Some(&sample()), false).unwrap();
        assert_eq!(bytes, 650 * MB);
        assert_eq!(line.id, SPARE_COPIES);
        assert_eq!(line.why, "2 AAX and 2 VST2 copies your saves don't use");
        assert_eq!(line.size, "650 MB");
        let one = Found { copies: vec![aax("Serum", 300)] };
        assert_eq!(super::line(Some(&one), false).unwrap().1.why, "1 AAX copy your saves don't use");
        // While the first look is on, it says so; a look that found none, or none at all: no line.
        let first = super::line(None, true).unwrap();
        assert_eq!((first.0, first.1.why.as_str(), first.1.size.as_str()), (0, LOOKING, "Measuring…"));
        assert_eq!(super::line(Some(&Found::default()), false), None);
        assert_eq!(super::line(None, false), None);
        // A new look keeps the last one's line on show.
        assert_eq!(super::line(Some(&sample()), true).unwrap().0, 650 * MB);
        // Only copies that can't go: no room to free, so no size.
        let stuck = Found { copies: vec![SpareCopy { stamp: None, ..aax("Serum", 300) }] };
        let (bytes, line) = super::line(Some(&stuck), false).unwrap();
        assert_eq!((bytes, line.size.as_str()), (0, ""));
    }

    #[test]
    fn the_card_lists_each_format_then_its_copies() {
        let card = card(Some(&sample()), false);
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
        assert!(!card.ticks && card.parts.iter().all(|p| p.tick.is_none()));
        assert_eq!(card.note, NEXT);
        // Before the first look has finished, and once one found none.
        let first = super::card(None, true);
        assert_eq!((first.big.as_str(), first.big_note.as_str()), ("Measuring…", ""));
        assert!(first.parts.is_empty() && first.note.is_empty());
        let none = super::card(Some(&Found::default()), false);
        assert!(none.parts.is_empty() && none.note.is_empty() && none.big_note.is_empty());
    }

    #[test]
    fn a_copy_that_cant_go_says_why_and_frees_nothing() {
        let mut found = sample();
        found.copies[0].stamp = None;
        let card = card(Some(&found), false);
        assert_eq!(card.big, "350 MB");
        assert_eq!(card.big_note, "3 spare copies can go, and 1 can't");
        let serum = card.parts.iter().find(|p| p.under && p.size == "300 MB").unwrap();
        assert_eq!((serum.name.as_str(), serum.status.as_str(), serum.problem), ("Serum", CANT_GO, true));
        // Its format frees only what its others do, so VST2 now frees most and comes first.
        assert_eq!((card.parts[0].name.as_str(), card.parts[0].size.as_str()), ("2 VST2 copies", "290 MB"));
        // None of them can go: nothing to free, said in words.
        let stuck = Found { copies: found.copies.iter().map(|c| SpareCopy { stamp: None, ..c.clone() }).collect() };
        let card = super::card(Some(&stuck), false);
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
        let card = card(Some(&Found { copies: vec![serum, kontakt] }), false);
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
}
