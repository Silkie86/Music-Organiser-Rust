//! Draws the app's pages to PNG files without opening a window, so every pull request can show what it changed before
//! anyone builds it. Uses Slint's software renderer, so it runs the same on a CI machine with no screen.
//!
//! `mo-screens [folder]` writes one picture per place in Graphite, then the Songs page in every theme. The folder
//! defaults to `screenshots`. Then the shared parts on one page, and the parts over every page: activity cards, the key
//! sheet and the F12 note. Last, keys pressed on the window as on a keyboard, checked for what they did.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use mo_app::activity::Feed;
use mo_app::places::Place;
use mo_app::shortcuts::Shortcuts;
use mo_app::song_panel::Tab;
use mo_app::themes::{Look, THEMES};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Model, PhysicalSize, Rgb8Pixel};

const WIDTH: u32 = 1440;
const HEIGHT: u32 = 900;
/// The app's narrowest window (app.slint's min-width).
const NARROW: u32 = 900;
/// A window short enough that the Songs, Presets, MIDI and Plugins samples' lists scroll (app.slint's min-height).
const SHORT: u32 = 600;
/// Solomon's own window: an ultrawide screen at 100% (across, down).
const WIDE: (u32, u32) = (3440, 1369);

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "screenshots".into()));
    std::fs::create_dir_all(&out)?;

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    slint::platform::set_platform(Box::new(Headless(window.clone())))
        .map_err(|e| format!("couldn't set up drawing without a window: {e}"))?;

    // No event loop here to post background work back to, so the arrangement is read and drawn in place.
    mo_ui::arrangement::draw_here();
    let graphite = Look::from_settings(None, None);
    let feed = Feed::default();
    let app = mo_ui::new_app(&graphite, Shortcuts::default(), Place::Songs, feed.clone())?;
    app.show()?;
    // The rail's "In your DAWs", as when Bitwig has a song open: the screenshots never reach a real DAW (rule 11).
    let open = "Open in Bitwig now. A click shows it on Songs, or Settings \u{203a} DAWs when it isn't one of your \
                songs.";
    mo_ui::daws::show(
        &app,
        &[mo_app::rail_daws::RailDaw {
            daw: "Bitwig",
            text: "Night Drive".into(),
            linked: true,
            tip: open.into(),
            project: "Night Drive".into(),
        }],
    );
    mo_ui::songs::show_sample(&app, sample_songs());
    // The Sounds page, worked out in place too, from made-up sounds.
    mo_ui::sounds::compute_here();
    mo_ui::sounds::connect(
        &app,
        out.join("no sounds.db"),
        out.join("no library.db"),
        feed.clone(),
        &Shortcuts::default(),
    );
    app.set_place(Place::Sounds.index() as i32);
    let sounds_empty = capture(&window);
    // While the sounds are still being read, on the galaxy: one word in the list's place, not the galaxy's own on top.
    mo_ui::sounds::reading(&app);
    app.global::<mo_ui::SoundsData>().invoke_pick_view(2);
    let sounds_reading = capture(&window);
    // Tiles picked before the sounds arrive: the page keeps a view picked while they're read.
    app.global::<mo_ui::SoundsData>().invoke_pick_view(0);
    mo_ui::sounds::show(&app, sample_sounds(), None, Some("1"));
    mo_ui::sounds::index_known(&app, std::sync::Arc::new(sample_fingerprints()));
    // Once to lay the page out (the tiles' width tells the page how many fit in a row), then for real.
    capture(&window);

    // The MIDI page, from made-up parts with their notes, worked out in place.
    mo_ui::midi::work_here();
    mo_ui::midi::connect(&app, out.join("no library.db"), feed.clone(), &Shortcuts::default());
    mo_ui::midi::show(&app, Ok(mo_ui::midi::sample()));
    // Settings, from made-up settings: nothing is read from this computer or written.
    mo_ui::settings::connect(
        &app,
        mo_ui::settings::Setup {
            settings: mo_app::app_settings::AppSettings::default(),
            folder: out.join("no settings"),
            library: out.join("no library.db"),
            running_pace: mo_app::background::Pace::Fast,
            live: false,
            writable: false,
            feed: feed.clone(),
        },
    );
    // The Space page, from made-up drives: it only reads the real ones when it's shown in the app.
    mo_ui::space::connect(&app, out.join("no library.db"), feed.clone());
    mo_ui::space::show_sample(&app, sample_space(), "");

    let mut shots = Vec::new();
    for place in Place::ALL {
        app.set_place(place.index() as i32);
        shots.push((format!("{}-graphite.png", slug(place.name())), capture(&window)));
    }
    // The MIDI page with a part picked: its card, piano roll and key; then parts shaped like it.
    app.set_place(Place::Midi.index() as i32);
    mo_ui::midi::pick_row(&app, 0);
    shots.push(("midi-picked.png".into(), capture(&window)));
    // Its loop playing, with the sound matched to it (no SoundFont here, so the card also says how to choose one).
    app.global::<mo_ui::MidiData>().set_playing(true);
    shots.push(("midi-playing.png".into(), capture(&window)));
    app.global::<mo_ui::MidiData>().set_playing(false);
    // Playing through the track selected in Bitwig.
    mo_ui::midi::run_action(&app, "midi.playBitwig");
    app.global::<mo_ui::MidiData>().set_daw_note("Playing through \u{201c}Bass\u{201d} in Bitwig.".into());
    shots.push(("midi-playing-in-bitwig.png".into(), capture(&window)));
    mo_ui::midi::run_action(&app, "midi.playBitwig");
    // A part dragged out to a DAW from its line: dimmed while it's carried, with the card that follows the mouse.
    let still = capture(&window);
    app.global::<mo_ui::MidiData>().set_lifted_row(1);
    let lifted = capture(&window);
    let card = mo_ui::midi::card_at_row(&app, 1).ok_or("midi-drag.png: no part on the second line")?;
    let (x, y, w, h) = changed_box(&still, &lifted).ok_or("midi-drag.png: the second line didn't dim")?;
    shots.push(("midi-drag.png".into(), with_card(lifted, &card, &graphite, (x + w / 3, y + h / 2 + 20))));
    app.global::<mo_ui::MidiData>().set_lifted_row(-1);
    // The roll with each note's name on it (it grows so each row has room).
    app.global::<mo_ui::MidiData>().set_roll_names(true);
    shots.push(("midi-note-names.png".into(), capture(&window)));
    app.global::<mo_ui::MidiData>().set_roll_names(false);
    mo_ui::midi::run_action(&app, "midi.moreLike");
    shots.push(("midi-more-like.png".into(), capture(&window)));
    // Esc: back to the whole list (nothing plays now), as the page's one step back.
    mo_ui::midi::escape(&app);
    // Two kinds and a place picked on the shared filter chips, each choice with its count.
    let midi = app.global::<mo_ui::MidiData>();
    midi.invoke_toggle("kind".into(), "Basslines".into());
    midi.invoke_toggle("kind".into(), "Chords".into());
    midi.invoke_toggle("from".into(), "MIDI folders".into());
    shots.push(("midi-filters.png".into(), capture(&window)));
    midi.invoke_clear();

    // The Backup page, filled with made-up folders.
    app.set_place(Place::Backup.index() as i32);
    mo_ui::backup::show_lines(&app, mo_ui::backup::sample_lines());
    mo_ui::backup::show_older(&app, mo_ui::backup::sample_older(false));
    shots.push(("backup.png".into(), capture(&window)));
    // The Remove older versions menu, opened.
    let before = capture(&window);
    click(&window, REMOVE_OLDER_BUTTON.0, REMOVE_OLDER_BUTTON.1);
    let after = capture(&window);
    if before == after {
        return Err("backup-older-menu.png: the menu didn't open; has the Remove older versions button moved?".into());
    }
    shots.push(("backup-older-menu.png".into(), after));
    // A click beside the menu closes it. Then the question asked before any go, the page turned down to show it.
    click(&window, 600.0, 20.0);
    mo_ui::backup::show_older(&app, mo_ui::backup::sample_older(true));
    // Drawn twice first, so the page is laid out with the question's wrapped lines before it's turned down.
    capture(&window);
    capture(&window);
    scroll(&window, 700.0, 500.0, -PAGE_DOWN);
    shots.push(("backup-remove-older.png".into(), capture(&window)));
    scroll(&window, 700.0, 500.0, PAGE_DOWN);
    // The warning about files gone from your folders, folded to its line, then opened.
    mo_ui::backup::show_older(&app, mo_ui::backup::sample_older(false));
    mo_ui::backup::show_gone(&app, &mo_ui::backup::sample_gone());
    capture(&window);
    shots.push(("backup-gone.png".into(), capture(&window)));
    app.global::<mo_ui::BackupData>().set_gone_open(true);
    capture(&window);
    shots.push(("backup-gone-open.png".into(), capture(&window)));
    app.global::<mo_ui::BackupData>().set_gone_open(false);
    mo_ui::backup::show_gone(&app, &mo_app::backup_gone::Warning::default());
    // The mouse moved away: left where Backup's clicks put it, the tip of what's under it stays up over the next page.
    hover(&window, -1.0, -1.0);
    // The Plugins page, from made-up songs: a plugin picked, on each of its card's tabs.
    mo_ui::plugins::connect(&app, out.join("no library.db"), feed.clone(), &Shortcuts::default());
    app.set_place(Place::Plugins.index() as i32);
    let (plugins, copies) = sample_plugins();
    let serum = plugins.find("serum", "");
    let kontakt = plugins.find("kontakt7", "");
    let massive = plugins.find("massive", "");
    mo_ui::plugins::show_sample(&app, plugins.clone(), serum, 0, &copies);
    capture(&window);
    shots.push(("plugins.png".into(), capture(&window)));
    mo_ui::plugins::show_sample(&app, plugins.clone(), kontakt, 1, &[]);
    capture(&window);
    shots.push(("plugins-on-disk.png".into(), capture(&window)));
    mo_ui::plugins::show_sample(&app, plugins.clone(), serum, 2, &copies);
    capture(&window);
    shots.push(("plugins-copies.png".into(), capture(&window)));
    mo_ui::plugins::show_sample(&app, plugins, massive, 0, &[]);
    capture(&window);
    shots.push(("plugins-missing.png".into(), capture(&window)));
    // Kontakt 7's Clean up list: its sound library ticked, its presets waiting for a tick, one library kept.
    mo_ui::plugins::show_sample(&app, sample_plugins().0, kontakt, 1, &[]);
    mo_ui::plugins::show_clean_sample(
        &app,
        "Kontakt 7",
        "Tick what to remove. Everything goes to the Recycle Bin, so you can still put it back.",
        &[
            (r"D:\Libraries\Kontakt Factory Library 2", "Sounds", "46.2 GB", "", true, true),
            (
                r"D:\Libraries\Choir Omnia",
                "Sounds",
                "3.10 GB",
                "A project uses a sample in it (Choir Ah C3.wav).",
                false,
                false,
            ),
            (r"C:\Users\you\Documents\Native Instruments\Kontakt 7", "Presets and settings", "412 MB", "", true, false),
        ],
        "Frees 46.2 GB",
    );
    capture(&window);
    shots.push(("plugins-clean-up.png".into(), capture(&window)));
    // Kontakt 7's Uninstall card: its own uninstaller picked, the other version's left for you to pick.
    mo_ui::plugins::show_card_sample(
        &app,
        "Uninstall Kontakt 7?",
        "12 projects use it. They'll open with Kontakt 7 missing.",
        "This runs its own uninstaller, the same one Windows Settings, Apps uses. It opens in its own window and may \
         ask questions.",
        &[
            ("Native Instruments · 1.20 GB", "Native Instruments Kontakt 7", "", "", true, true),
            ("Native Instruments · 1.31 GB", "Native Instruments Kontakt 8", "", "", true, false),
        ],
        "",
        ("Uninstall", "Keep it"),
    );
    capture(&window);
    shots.push(("plugins-uninstall.png".into(), capture(&window)));
    settings_shots(&app, &window, &graphite, &mut shots);
    // Space with the App files card open: what each part of the app's folder takes, and C: nearly full.
    app.set_place(Place::Space.index() as i32);
    mo_ui::space::show_sample(&app, sample_space(), mo_app::space::APP_FILES);
    capture(&window);
    shots.push(("space-app-files.png".into(), capture(&window)));
    mo_ui::apply_look(&app, &Look::from_settings(Some("Paper"), None));
    capture(&window);
    shots.push(("space-paper.png".into(), capture(&window)));
    mo_ui::apply_look(&app, &graphite);
    // The Bitwig copies card: each project's copies of Library multisamples, biggest first, with Fix's ticks: one
    // project fixed with its other saves on show (one of them ticked), one that couldn't be fixed.
    let bitwig = mo_app::space_bitwig::BITWIG_COPIES;
    mo_ui::space::show_sample(&app, mo_ui::space::Sample { fixes: sample_fixes(), ..sample_space() }, bitwig);
    capture(&window);
    shots.push(("space-bitwig.png".into(), capture(&window)));
    // The old copies card: what moving plugin folders to another drive left, biggest first, with ticks: one that can't
    // go yet, files a try kept aside that wait for your tick, and one with presets saved since its move, which stays
    // until you look.
    let old = mo_app::space_old::OLD_COPIES;
    mo_ui::space::show_sample(&app, sample_space(), old);
    capture(&window);
    let top = capture(&window);
    shots.push(("space-old.png".into(), top.clone()));
    // The card turned down to its end: the one that can't go yet, the kept files with their tick off, and the one with
    // presets saved since, with no tick.
    shots.push(("space-old-down.png".into(), card_down(&window, &top, "space-old-down.png")?));
    // While Remove compares the ticked ones with their copies (a big sound library takes a while): Stop, which leaves
    // what isn't checked yet where it is.
    let found = sample_old_copies();
    let mut old_removing = mo_app::space_old::Removing::default();
    old_removing.start(old_removing.ticked_copies(Some(&found)));
    mo_ui::space::show_sample(&app, mo_ui::space::Sample { old_removing, ..sample_space() }, old);
    capture(&window);
    shots.push(("space-old-removing.png".into(), capture(&window)));
    // After Remove: what was freed, each old copy in the Trash (the Recycle Bin on Windows) or why it stayed, and Put
    // back.
    let (old_removing, old_binned) = sample_old_removed(&sample_old_copies());
    let kept = Some(sample_kept(old_binned.clone()));
    mo_ui::space::show_sample(&app, mo_ui::space::Sample { old_removing, kept, ..sample_space() }, old);
    capture(&window);
    shots.push(("space-old-removed.png".into(), capture(&window)));
    // The spare copies card: AAX copies (Pro Tools isn't installed) and VST2 copies whose VST3 your saves use, by
    // format, with one that can't go.
    mo_ui::space::show_sample(&app, sample_space(), mo_app::space_spare::SPARE_COPIES);
    capture(&window);
    let top = capture(&window);
    shots.push(("space-spare.png".into(), top.clone()));
    // The card turned down to its end: the VST2 copies, in two folders, each with its own, and the one that can't go
    // (its last lines once couldn't be scrolled to).
    shots.push(("space-spare-down.png".into(), card_down(&window, &top, "space-spare-down.png")?));
    // One AAX copy unticked: its format's tick shows a dash, and Remove frees the rest.
    let spare = mo_app::space_spare::SPARE_COPIES;
    let mut removing = mo_app::space_spare::Removing::default();
    removing.tick(&[PathBuf::from(SAMPLE_UNTICKED)], false);
    mo_ui::space::show_sample(&app, mo_ui::space::Sample { removing, ..sample_space() }, spare);
    capture(&window);
    shots.push(("space-spare-ticks.png".into(), capture(&window)));
    // After Remove: what was freed, each copy in the Trash (the Recycle Bin on Windows) or why it stayed, and Put back.
    // The Recycle Bin line under Also taking room shows what the app put there.
    let (removing, spare_binned) = sample_removed(&sample_spare_copies());
    let removed = mo_ui::space::Sample { removing, kept: Some(sample_kept(spare_binned.clone())), ..sample_space() };
    mo_ui::space::show_sample(&app, removed, spare);
    capture(&window);
    shots.push(("space-spare-removed.png".into(), capture(&window)));
    // The songs card: identical saves, old autosaves and a Studio One cache, biggest first, each with the songs it's
    // in; then audio no save seems to use, which isn't counted.
    mo_ui::space::show_sample(&app, sample_space(), mo_app::space_songs::COMPACT);
    capture(&window);
    let top = capture(&window);
    shots.push(("space-songs.png".into(), top.clone()));
    shots.push(("space-songs-down.png".into(), card_down(&window, &top, "space-songs-down.png")?));
    // Night Drive's old autosaves unticked: its line's tick is off, the old autosaves' tick shows a dash, and Remove
    // frees the rest.
    let compact = mo_app::space_songs::COMPACT;
    let mut songs_removing = mo_app::space_songs::Removing::default();
    songs_removing.tick(&sample_night_drive_autosaves(), false);
    mo_ui::space::show_sample(&app, mo_ui::space::Sample { songs_removing, ..sample_space() }, compact);
    capture(&window);
    shots.push(("space-songs-ticks.png".into(), capture(&window)));
    // While Remove measures the songs again and checks each ticked one: Stop, which leaves what hasn't gone yet where
    // it is.
    let shown = mo_app::space_songs::Shown::of(&sample_song_room(), chrono::Local::now().date_naive());
    let mut songs_removing = mo_app::space_songs::Removing::default();
    songs_removing.start(songs_removing.ticked_copies(Some(&shown)));
    mo_ui::space::show_sample(&app, mo_ui::space::Sample { songs_removing, ..sample_space() }, compact);
    capture(&window);
    shots.push(("space-songs-removing.png".into(), capture(&window)));
    // After Remove: what was freed, each song's old autosaves and the cache in the Trash (the Recycle Bin on Windows),
    // two autosaves that are among their song's newest 5 now, which stayed, and Put back.
    let (songs_removing, songs_binned) = sample_songs_removed(&shown);
    let kept = Some(sample_kept(songs_binned.clone()));
    mo_ui::space::show_sample(&app, mo_ui::space::Sample { songs_removing, kept, ..sample_space() }, compact);
    capture(&window);
    shots.push(("space-songs-removed.png".into(), capture(&window)));
    // The Recycle Bin card: each clean-up still there, newest first, with its Put back: your songs' old autosaves and
    // caches, the old copies, then the spare copies; the oldest one's put back left a copy behind, which says why.
    let kept = sample_kept([spare_binned, old_binned, songs_binned].concat());
    let mut put_backs = mo_app::space_bin::PutBacks::default();
    let stuck = mo_space::clean::Problem {
        path: kept.list[0].done.removed[0].path.clone(),
        why: "It couldn't be put back; it's still in the Recycle Bin.".to_owned(),
    };
    put_backs.done(spare, kept.list[0].when, vec![stuck]);
    let (removing, _) = sample_removed(&sample_spare_copies());
    let (old_removing, _) = sample_old_removed(&sample_old_copies());
    let (songs_removing, _) = sample_songs_removed(&shown);
    let bin =
        mo_ui::space::Sample { removing, old_removing, songs_removing, kept: Some(kept), put_backs, ..sample_space() };
    mo_ui::space::show_sample(&app, bin, mo_app::space_bin::RECYCLE_BIN);
    capture(&window);
    shots.push(("space-bin.png".into(), capture(&window)));
    // The older saves card: each song's saves from the middle of a sitting, with the days and what stays.
    mo_ui::space::show_sample(&app, sample_space(), mo_app::space_songs::THIN);
    capture(&window);
    shots.push(("space-thin.png".into(), capture(&window)));
    // The plugins no song has used lately: those never used, then those not in over 2 years, each with its link to its
    // card on the Plugins page.
    mo_ui::space::show_sample(&app, sample_space(), mo_app::space_plugins::UNUSED_PLUGINS);
    let top = capture(&window);
    shots.push(("space-plugins.png".into(), top.clone()));
    shots.push(("space-plugins-down.png".into(), card_down(&window, &top, "space-plugins-down.png")?));
    // The copies of the same sound in your sample folders, as information: the sounds whose copies take most first,
    // each with its files, the one to keep first, one hidden in the app; then those with only a very similar copy.
    mo_ui::space::show_sample(&app, sample_space(), mo_app::space_sounds::SOUND_COPIES);
    let top = capture(&window);
    shots.push(("space-copies.png".into(), top.clone()));
    shots.push(("space-copies-down.png".into(), card_down(&window, &top, "space-copies-down.png")?));
    // The page turned down to Also taking room: the app's own files, then the copies of the same sound.
    mo_ui::space::show_sample(&app, sample_space(), "");
    capture(&window);
    scroll(&window, 700.0, 600.0, -10.0 * PAGE_DOWN);
    hover(&window, -1.0, -1.0);
    shots.push(("space-also.png".into(), capture(&window)));
    scroll(&window, 700.0, 600.0, 10.0 * PAGE_DOWN);
    hover(&window, -1.0, -1.0);
    capture(&window);
    space_keys_checks(&app, &window, &mut shots)?;
    mo_ui::space::show_sample(&app, sample_space(), "");
    presets_shots(&app, &window, &mut shots)?;
    app.set_place(Place::Songs.index() as i32);
    for theme in THEMES.iter().skip(1) {
        mo_ui::apply_look(&app, &Look::from_settings(Some(theme.name), None));
        shots.push((format!("songs-{}.png", slug(theme.name)), capture(&window)));
    }

    mo_ui::apply_look(&app, &graphite);
    // The filters with more than one pick: FL Studio, and two statuses at once.
    let songs = app.global::<mo_ui::SongsData>();
    let before = capture(&window);
    for (chip, choice) in [("daw", "FL Studio"), ("status", "Idea"), ("status", "Finished")] {
        songs.invoke_toggle_choice(chip.into(), choice.into());
    }
    let after = capture(&window);
    if before == after {
        return Err("songs-filters.png: picking FL Studio, Idea and Finished changed nothing; are they wired?".into());
    }
    shots.push(("songs-filters.png".into(), after));
    songs.invoke_clear();
    // A search nothing matches: the list says so, with Clear the filters, as Sounds and MIDI do (review X3-7).
    songs.invoke_search_edited("no song is called this".into());
    if songs.get_shown() != 0 {
        return Err("songs-no-match.png: the search still matches a song".into());
    }
    shots.push(("songs-no-match.png".into(), capture(&window)));
    songs.invoke_clear();
    // A song line's right-click menu (Night Drive, an FL Studio song): the panel's buttons with their keys (X3-24).
    let before = capture(&window);
    right_click(&window, 400.0, 290.0);
    let menu = capture(&window);
    if before == menu {
        return Err("songs-menu.png: the right click on Night Drive opened nothing; has the list moved?".into());
    }
    shots.push(("songs-menu.png".into(), menu));
    // Away again: a click above the page shuts the menu, and the panel is put away.
    click(&window, 600.0, 20.0);
    songs.invoke_close_panel();
    // The narrowest window the app allows: a filter too long for a line of its own wraps inside, so every chip stays
    // in sight.
    save_narrow(&window, &out, "songs-narrow.png")?;
    shots.push(("sounds-empty.png".into(), sounds_empty));
    shots.push(("sounds-reading.png".into(), sounds_reading));
    sounds_shots(&app, &window, &graphite, &mut shots)?;
    // The Sounds filters in the narrowest window, every one in sight, and each view's own choices too. There the
    // List, History and Rediscover lines take two lines each and the Kits pads put their names before their sounds',
    // so every name has room.
    app.set_place(Place::Sounds.index() as i32);
    save_narrow(&window, &out, "sounds-narrow.png")?;
    let sounds = app.global::<mo_ui::SoundsData>();
    for (view, name) in [
        (1, "sounds-list-narrow.png"),
        (2, "sounds-galaxy-narrow.png"),
        (3, "sounds-history-narrow.png"),
        (4, "sounds-rediscover-narrow.png"),
        (5, "sounds-kits-narrow.png"),
    ] {
        sounds.invoke_pick_view(view);
        save_narrow(&window, &out, name)?;
    }
    sounds.invoke_pick_view(0);
    app.set_place(Place::Songs.index() as i32);

    // The song panel, on the first song, on Overview and on Plugins.
    for tab in [Tab::Overview, Tab::Plugins, Tab::Saves] {
        show_song_panel(&app, 0, tab);
        shots.push((format!("song-panel-{}.png", slug(tab.name())), capture(&window)));
    }
    // A click on an older save picks it for the song's own buttons (the UX review of 10 Nov, B10); another song
    // lets go of it.
    click(&window, SECOND_SAVE.0, SECOND_SAVE.1);
    if app.global::<mo_ui::SongsData>().get_picked_save().is_empty() {
        return Err("song-panel-save-picked.png: no save was picked; has the Saves tab's second line moved?".into());
    }
    shots.push(("song-panel-save-picked.png".into(), capture(&window)));
    show_song_panel(&app, 1, Tab::Saves);
    if !app.global::<mo_ui::SongsData>().get_picked_save().is_empty() {
        return Err("another song was picked, and the last one's save is still what the buttons act on".into());
    }
    // Each tab in the narrowest window, where the panel is at its narrowest: its rows wrap, so nothing runs past its
    // edge.
    for tab in Tab::ALL {
        show_song_panel(&app, 0, tab);
        save_narrow(&window, &out, &format!("song-panel-{}-narrow.png", slug(tab.name())))?;
    }
    // There the panel is taller than the window, so it scrolls: the Overview turned down to its arrangement, which
    // keeps the room it needs.
    show_song_panel(&app, 0, Tab::Overview);
    save_narrow_scrolled(&window, &out, "song-panel-overview-narrow-down.png", Some(NARROW_PANEL_WHEEL))?;
    opens_at_top(&app, &window)?;
    // The arrangement: the mouse on a clip, zoomed in about the mouse, the Bitwig view, and in a light theme.
    app.global::<mo_ui::SongsData>().set_tab(Tab::Overview.index() as i32);
    let arrangement = app.global::<mo_ui::ArrangementData>();
    arrangement.invoke_moved(ARRANGEMENT_CLIP.0, ARRANGEMENT_CLIP.1);
    let hovered = capture(&window);
    shots.push(("arrangement-hover.png".into(), hovered.clone()));
    // The clip under the mouse dragged out to a DAW: dimmed while it's carried, with the card that follows the mouse
    // (and, as in the app, nothing said about what's under the mouse meanwhile), then lit once it has landed.
    let Some(card) = mo_ui::arrangement::show_lifted(&app, ARRANGEMENT_CLIP.0, ARRANGEMENT_CLIP.1, false) else {
        return Err(
            "arrangement-drag-lifted.png: no clip to drag at the hover point; has the sample song moved?".into()
        );
    };
    let lifted = capture(&window);
    let (x, y, w, h) = changed_box(&hovered, &lifted).ok_or("arrangement-drag-lifted.png: the clip didn't dim")?;
    arrangement.set_hover_text("".into());
    let lifted = capture(&window);
    shots.push((
        "arrangement-drag-lifted.png".into(),
        with_card(lifted, &card, &graphite, (x + w / 2 + 36, y + h / 2 + 22)),
    ));
    mo_ui::arrangement::show_lifted(&app, ARRANGEMENT_CLIP.0, ARRANGEMENT_CLIP.1, true);
    shots.push(("arrangement-drag-landed.png".into(), capture_after(&window, Duration::from_millis(150))));
    arrangement.set_landed(false);
    capture_after(&window, Duration::from_millis(700));
    arrangement.invoke_wheel(ARRANGEMENT_CLIP.0, 0.0, 4.0, true, false);
    shots.push(("arrangement-zoomed.png".into(), capture(&window)));
    arrangement.invoke_left();
    arrangement.invoke_double_clicked();
    arrangement.invoke_pick_view(1);
    shots.push(("arrangement-bitwig-view.png".into(), capture(&window)));
    mo_ui::apply_look(&app, &Look::from_settings(Some("Paper"), None));
    mo_ui::arrangement::show_preview(&app, mo_ui::arrangement::sample_preview());
    mo_ui::arrangement::show_peaks(&app, mo_ui::arrangement::sample_peaks());
    shots.push(("arrangement-light.png".into(), capture(&window)));
    mo_ui::apply_look(&app, &graphite);
    // A song with things to look at, its Health strip open.
    mo_ui::songs::show_picked(&app, 1, sample_detail(), Tab::Overview);
    app.global::<mo_ui::SongsData>().set_health_open(true);
    shots.push(("song-panel-health.png".into(), capture(&window)));
    app.global::<mo_ui::SongsData>().set_health_open(false);
    app.global::<mo_ui::SongsData>().invoke_close_panel();

    // The shared parts every page is built from, in Graphite and in a light and a square theme.
    app.set_parts_gallery(true);
    shots.push(("parts-graphite.png".into(), capture(&window)));
    for theme in ["Paper", "Hardware"] {
        mo_ui::apply_look(&app, &Look::from_settings(Some(theme), None));
        shots.push((format!("parts-{}.png", slug(theme)), capture(&window)));
    }
    mo_ui::apply_look(&app, &graphite);
    app.set_parts_gallery(false);

    // What the app is working on, in the rail: one going (its title too long for a line there, so it's cut and its
    // tip has it all), one that can't say how far it is yet and one done; and the one that failed, as a card.
    let going = feed.start(mo_app::space_bitwig::LOOKING, "");
    going.progress(0.4, Some("148 of 371 projects"));
    let _starting = feed.start("Analysing sounds", "Getting ready");
    feed.start("Finding missing samples", "").done("All 31 found");
    feed.start("Copying Bitwig template", "").failed("The template folder isn't there any more");
    mo_ui::cards::refresh(&app, &feed);
    shots.push(("cards.png".into(), capture(&window)));
    app.set_cards(Default::default());
    app.set_jobs(Default::default());

    // Tooltips: the mouse resting on a rail line, the song panel's Show in folder button and a song's date.
    show_song_panel(&app, 0, Tab::Overview);
    for (name, x, y) in [
        ("tip-rail.png", 60.0, 125.0),
        ("tip-button.png", SHOW_BUTTON.0, SHOW_BUTTON.1),
        ("tip-date.png", 740.0, 228.0),
    ] {
        hover(&window, x, y);
        // Before the tip's wait is over, then after: if nothing came up, the mouse missed what it was meant to rest on.
        let before = capture_after(&window, Duration::from_millis(150));
        let after = capture_after(&window, Duration::from_millis(750));
        if before == after {
            return Err(format!("{name}: no tooltip came up at {x}, {y}; has the layout moved?").into());
        }
        shots.push((name.into(), after));
    }
    // The Older saves menu, opened.
    let before = capture(&window);
    click(&window, OLDER_BUTTON.0, OLDER_BUTTON.1);
    let after = capture(&window);
    if before == after {
        return Err("older-saves.png: the menu didn't open; has the Older saves button moved?".into());
    }
    shots.push(("older-saves.png".into(), after));
    click(&window, 300.0, 880.0);
    hover(&window, -1.0, -1.0);
    app.global::<mo_ui::SongsData>().invoke_close_panel();

    finder_shots(&app, &window, &mut shots);

    // The sheet lists the keys of the page on show (Songs): the shots before set the place without the page change.
    mo_ui::refresh_keys(&app);
    app.set_sheet_open(true);
    shots.push(("key-sheet.png".into(), capture(&window)));
    app.set_sheet_open(false);

    app.set_note_text(
        "The rail jumps a little when I click Space, and the Settings line looks too close to it.".into(),
    );
    app.set_note_has_picture(true);
    app.set_note_open(true);
    shots.push(("note.png".into(), capture(&window)));

    for (name, pixels) in shots {
        let path = out.join(&name);
        image::save_buffer(&path, &pixels, WIDTH, HEIGHT, image::ExtendedColorType::Rgb8)?;
        println!("{}", path.display());
    }
    app.set_note_open(false);
    // After the shots: it leaves a line picked on each of those pages.
    opened_in_sight(&app, &window)?;
    key_checks(&app)?;
    card_keys_checks(&app, &window, &out)?;
    rail_checks(&app)?;
    wide_shots(&app, &window, &out)?;
    tag_box_checks(&app, &window)?;
    Ok(())
}

/// The tag box on a sound's card keeps the keys as the tags typed into it push it down the card: the card scrolls
/// after it. Slint takes the keys from a part out of sight at the next key (even Enter's own release), and then nothing
/// had them, so no key worked until a click (it showed with the small words a step bigger, the UX review's D4). In a
/// short window, so the card shows only its top and Ctrl+T finds the box at its foot. Last, as the tags it adds would
/// show in the shots.
fn tag_box_checks(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    let sounds = app.global::<mo_ui::SoundsData>();
    window.set_size(PhysicalSize::new(WIDTH, SHORT));
    start_on(app, Place::Sounds);
    sounds.invoke_pick_view(0);
    sounds.invoke_select(0);
    capture(window);
    press(app, "t", true);
    capture(window);
    let mut wrong = None;
    for tag in [
        "glassy", "wide", "crunchy", "soft", "metallic", "hollow", "bouncy", "gritty", "smooth", "tight", "roomy",
        "noisy", "dry", "wet", "lofi", "vintage",
    ] {
        if picked_has_tag(app, tag) {
            wrong = Some(format!("the picked sound has the tag {tag} already: type a new word"));
            break;
        }
        type_in(app, window, tag);
        if !picked_has_tag(app, tag) {
            wrong = Some(format!("the box lost the keys as the tags before {tag} pushed it down the card"));
            break;
        }
    }
    // With the keys in the box as the card closes, they come back to the list: F1 works.
    sounds.invoke_close_panel();
    capture(window);
    let opened = f1_works(app, window);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    capture(window);
    match wrong {
        Some(wrong) => Err(format!("tag box: {wrong}")),
        None if !opened => {
            Err("tag box: once its card closed after the tags, F1 did nothing: no part had the keys".into())
        }
        None => Ok(()),
    }
}

/// Every place once more in a window as wide as Solomon's screen (3440 x 1369, the size of his F12 notes' pictures),
/// saved as wide-<place>.png: a page that only looks right at 1440 across stretches its lines' columns a screen
/// apart there (the UX review of 10 Nov, A1). After the checks, so each page has the line they left picked.
fn wide_shots(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    out: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    window.set_size(PhysicalSize::new(WIDE.0, WIDE.1));
    for place in Place::ALL {
        app.set_place(place.index() as i32);
        // Once to lay the page out at this width (the tiles' width tells Sounds how many fit in a row), then for real.
        capture_sized(window, WIDE, Duration::from_millis(400));
        let pixels = capture_sized(window, WIDE, Duration::from_millis(400));
        let path = out.join(format!("wide-{}.png", slug(place.name())));
        image::save_buffer(&path, &pixels, WIDE.0, WIDE.1, image::ExtendedColorType::Rgb8)?;
        println!("{}", path.display());
    }
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    Ok(())
}

/// The keys come back to a page's list when a box on its card goes with them (the whole-app review of 10 Nov, U-1):
/// Sounds' tag box, given the keys by Ctrl+T or by the right-click menu's Add a tag…, as its card closes (Esc, its ×,
/// or a read while another app's window is in front), and a curve's rename box on Presets as it closes. Nothing had
/// the keys after, so no key worked until a click: F1 must open the key sheet each time. The menu, with its Add a tag…
/// line, is saved as sounds-menu.png. After the shots: it adds tags and leaves a curve picked.
fn card_keys_checks(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    out: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use slint::platform::Key;
    let sounds = app.global::<mo_ui::SoundsData>();
    let tagged = |tag: &str| picked_has_tag(app, tag);
    let keys_work = |after: &str| -> Result<(), String> {
        if f1_works(app, window) {
            Ok(())
        } else {
            Err(format!("card keys: {after}, F1 did nothing: no part had the keys"))
        }
    };
    start_on(app, Place::Sounds);
    sounds.invoke_pick_view(0);
    sounds.invoke_select(0);
    capture(window);
    // Ctrl+T, a tag typed and Enter, then Esc in the empty box until the card closes (the first may stop the sound).
    press(app, "t", true);
    capture(window);
    type_in(app, window, "airy");
    if !tagged("airy") {
        return Err("card keys: Ctrl+T then typing on Sounds added no tag: its box didn't get the keys".into());
    }
    for _ in 0..3 {
        if sounds.get_panel_shown() {
            press(app, Key::Escape, false);
            capture(window);
        }
    }
    if sounds.get_panel_shown() {
        return Err("card keys: Esc in the sound card's empty tag box didn't close the card".into());
    }
    keys_work("once Esc closed the sound's card from its tag box")?;
    // The right-click menu's Add a tag… on a tile (picked by the click): the menu goes, and the box has the keys.
    let (x, y) = MENU_TILE;
    right_click(window, x, y);
    let menu = capture(window);
    if !sounds.get_panel_shown() {
        return Err(format!("sounds-menu.png: the right click at {x}, {y} picked no tile; has the grid moved?").into());
    }
    let path = out.join("sounds-menu.png");
    image::save_buffer(&path, &menu, WIDTH, HEIGHT, image::ExtendedColorType::Rgb8)?;
    println!("{}", path.display());
    // Its third line, under Play and Favourite.
    click(window, x + 60.0, y + MENU_LINES_TOP + 2.5 * MENU_LINE);
    capture(window);
    type_in(app, window, "bright");
    if !tagged("bright") {
        return Err("card keys: the menu's Add a tag… then typing added no tag: the box didn't get the keys".into());
    }
    sounds.invoke_close_panel();
    capture(window);
    keys_work("once the sound's card closed by its × with the keys in its tag box")?;
    // The card closing while another app's window is in front (a read of the library dropped its sound), with the
    // keys in its tag box: the box lost them to that window, not to this one, so they come back to the list with the
    // window (the code review of the fix). The system's word comes first, as the window's watcher has it.
    sounds.invoke_select(0);
    capture(window);
    press(app, "t", true);
    capture(window);
    let front = |active: bool| {
        app.global::<mo_ui::AppKeys>().set_window_active(active);
        window.dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(active));
        capture(window);
    };
    front(false);
    sounds.invoke_close_panel();
    capture(window);
    front(true);
    keys_work("once the sound's card closed while another app's window was in front, with the keys in its tag box")?;
    // Presets: a curve's Rename, a letter typed, then Esc in the box.
    start_on(app, Place::Presets);
    mo_ui::presets::show_section(app, mo_app::presets::Section::Curves);
    mo_ui::presets::pick_row(app, 0);
    capture(window);
    let presets = app.global::<mo_ui::PresetsData>();
    presets.invoke_curve_chosen(mo_app::presets::curve_act::RENAME.into());
    capture(window);
    let named = presets.get_rename_text();
    press(app, "x", false);
    if !presets.get_renaming() || presets.get_rename_text() == named {
        return Err("card keys: Rename on a curve didn't give its box the keys".into());
    }
    press(app, Key::Escape, false);
    capture(window);
    if presets.get_renaming() {
        return Err("card keys: Esc in a curve's rename box didn't close it".into());
    }
    keys_work("once Esc closed a curve's rename box")?;
    mo_ui::presets::show_section(app, mo_app::presets::Section::Sounds);
    mo_ui::presets::pick_row(app, usize::MAX);
    Ok(())
}

/// Whether the sound picked on Sounds has `tag` among the tags its card shows.
fn picked_has_tag(app: &mo_ui::AppWindow, tag: &str) -> bool {
    let tags = app.global::<mo_ui::SoundsData>().get_picked_tags();
    (0..tags.row_count()).any(|i| tags.row_data(i).is_some_and(|t| t.text == tag))
}

/// `words` typed where the keys are, then Enter.
fn type_in(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow, words: &str) {
    for c in words.chars() {
        press(app, c.to_string(), false);
    }
    press(app, slint::platform::Key::Return, false);
    capture(window);
}

/// Whether F1 opens the key sheet, as it does when some part has the keys. The sheet is closed again.
fn f1_works(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> bool {
    press(app, slint::platform::Key::F1, false);
    let opened = app.get_sheet_open();
    app.set_sheet_open(false);
    capture(window);
    opened
}

/// The rail's "In your DAWs" line (the 6 Nov review's X3-9, and the code review on it): a click shows the song Bitwig
/// has open on Songs when it's one of yours (by the name Bitwig shows, in any case), or Settings on its DAWs tab when
/// it isn't. The lines are made up: nothing here reaches a real DAW (rule 11).
fn rail_checks(app: &mo_ui::AppWindow) -> Result<(), String> {
    use mo_app::rail_daws::RailDaw;
    use mo_app::settings::Tab;
    mo_ui::daws::wire(app);
    let open = |project: &str| RailDaw {
        daw: "Bitwig",
        text: project.into(),
        linked: true,
        project: project.into(),
        ..RailDaw::default()
    };
    start_on_songs(app);
    mo_ui::daws::show(app, &[open("glass harbour")]);
    app.invoke_daw_line_clicked(0);
    let shown = app.global::<mo_ui::SongsData>().get_song().name;
    if app.get_place() != Place::Songs.index() as i32 || shown != "Glass Harbour" {
        return Err(format!("rail: a click on Bitwig's glass harbour showed {shown:?} (place {})", app.get_place()));
    }
    // Not one of your songs: Settings, on the DAWs tab.
    mo_ui::daws::show(app, &[open("Not One Of Yours")]);
    app.invoke_daw_line_clicked(0);
    let tab = app.global::<mo_ui::SettingsData>().get_tab();
    if app.get_place() != Place::Settings.index() as i32 || tab != Tab::Daws.index() as i32 {
        return Err(format!("rail: a click on a song not yours went to place {}, tab {tab}", app.get_place()));
    }
    Ok(())
}

/// Keys pressed on the window as on a keyboard, and what they did (the 6 Nov review's X3-12 and X3-22, and the code
/// review on them): the keys still work after Ctrl+F and a page change, Esc closes the key sheet before the page under
/// it sees the key (and Ctrl+K over the sheet first), Esc stops what plays on another page, a held Esc takes one step,
/// and Ctrl+F leaves Ctrl+K open. Each check starts on Songs with the keys on the window, so one going wrong doesn't
/// hide another.
fn key_checks(app: &mo_ui::AppWindow) -> Result<(), String> {
    use slint::platform::Key;
    let songs = app.global::<mo_ui::SongsData>();
    let midi = app.global::<mo_ui::MidiData>();
    let finder = app.global::<mo_ui::FinderData>();
    let searched = |want: &str| songs.get_search() == want;
    app.set_note_open(false);
    // Songs: Ctrl+F puts the keys in its search box, so typing searches.
    start_on_songs(app);
    press(app, "f", true);
    press(app, "a", false);
    if !searched("a") {
        return Err(format!("keys: Ctrl+F then A on Songs searched {:?}", songs.get_search()));
    }
    // The search box goes with its page: the keys come back to the window, so Ctrl+1 still works.
    press(app, "2", true);
    press(app, "1", true);
    if app.get_place() != Place::Songs.index() as i32 {
        return Err("keys: after Ctrl+F and Ctrl+2, Ctrl+1 didn't go back to Songs".into());
    }
    // Esc closes the key sheet, and the search under it, which has the keys, stays.
    start_on_songs(app);
    press(app, "f", true);
    press(app, "a", false);
    press(app, Key::F1, false);
    if !app.get_sheet_open() {
        return Err("keys: F1 in the search box didn't open the key sheet".into());
    }
    press(app, Key::Escape, false);
    if app.get_sheet_open() || !searched("a") {
        return Err(format!("keys: Esc on the key sheet left it open or searched {:?}", songs.get_search()));
    }
    // Esc stops the MIDI page's loop from here (it plays on after its page goes), and does only that.
    start_on_songs(app);
    press(app, "f", true);
    press(app, "a", false);
    midi.set_playing(true);
    press(app, Key::Escape, false);
    if midi.get_playing() || !searched("a") {
        return Err("keys: Esc on Songs didn't stop the MIDI loop first".into());
    }
    // A held Esc takes one step: the search clears, and the song's panel stays until Esc is pressed again.
    midi.set_playing(false);
    start_on_songs(app);
    press(app, "f", true);
    press(app, "a", false);
    mo_ui::songs::show_picked(app, 1, sample_detail(), Tab::Overview);
    press(app, Key::Escape, false);
    for _ in 0..3 {
        held(app, Key::Escape);
    }
    if !searched("") || !mo_ui::songs::has_pick() {
        return Err("keys: a held Esc took more than one step".into());
    }
    press(app, Key::Escape, false);
    if mo_ui::songs::has_pick() {
        return Err("keys: Esc didn't put the song's panel away".into());
    }
    // Ctrl+F leaves Ctrl+K open, its box keeping the keys; Esc closes it.
    start_on_songs(app);
    press(app, "k", true);
    let asked = app.global::<mo_ui::AppKeys>().get_search();
    press(app, "f", true);
    if !finder.get_open() || app.global::<mo_ui::AppKeys>().get_search() != asked {
        return Err("keys: Ctrl+F with Ctrl+K open went to the page's search".into());
    }
    press(app, Key::Escape, false);
    if finder.get_open() {
        return Err("keys: Esc didn't close Ctrl+K".into());
    }
    // Ctrl+K over the key sheet closes first, then the sheet.
    start_on_songs(app);
    press(app, Key::F1, false);
    press(app, "k", true);
    press(app, Key::Escape, false);
    if finder.get_open() || !app.get_sheet_open() {
        return Err("keys: Esc with Ctrl+K over the key sheet didn't close Ctrl+K first".into());
    }
    press(app, Key::Escape, false);
    if app.get_sheet_open() {
        return Err("keys: the second Esc didn't close the key sheet".into());
    }
    Ok(())
}

/// Space's Esc and right click (the whole-app review of 10 Nov, X3-2): Esc closes the picked line's card, as on every
/// page, and a right click on a line picks it and opens its menu, its card's buttons with their keys (space-menu.png).
/// The menu is shut by a click outside it, so nothing on it runs: nothing is shown in a folder, measured or moved.
/// Ends with no line picked.
fn space_keys_checks(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    shots: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    use slint::platform::Key;
    let data = app.global::<mo_ui::SpaceData>();
    let picked = || data.get_picked().to_string();
    // Space on show for the keys, as going there notes it (without its looks), until the checks end, and the keys on
    // the window, as Ctrl+K gives them back when it closes. Esc's first step stops what plays (a sound from the Sounds
    // shots): taken here, with no card open, so the next Esc is the one that closes it.
    let noted = mo_ui::place();
    mo_ui::note_place(Place::Space);
    mo_ui::space::show_sample(app, sample_space(), "");
    mo_ui::finder::toggle(app);
    slint::platform::update_timers_and_animations();
    mo_ui::finder::close(app);
    press(app, Key::Escape, false);
    // The App files card open: one Esc closes it.
    mo_ui::space::show_sample(app, sample_space(), mo_app::space::APP_FILES);
    capture(window);
    press(app, Key::Escape, false);
    capture(window);
    if !picked().is_empty() {
        return Err(format!("space keys: Esc left the {} card open", picked()));
    }
    // Old copies after moving folders, right-clicked: picked, its card opens, and its menu over the page.
    let before = capture(window);
    let (x, y) = SPACE_OLD_LINE;
    right_click(window, x, y);
    let after = capture(window);
    if picked() != mo_app::space_old::OLD_COPIES || data.get_line_menu().row_count() == 0 || before == after {
        return Err(format!("space-menu.png: the right click at {x}, {y} picked {:?}; has the line moved?", picked()));
    }
    shots.push(("space-menu.png".into(), after));
    // A click outside shuts it (and does nothing else: the card stays open); the keys are back on the window, so Esc
    // closes the card.
    click(window, SPACE_EMPTY.0, SPACE_EMPTY.1);
    capture(window);
    if picked() != mo_app::space_old::OLD_COPIES {
        return Err(format!("space keys: the click that shut the line's menu picked {:?} instead", picked()));
    }
    press(app, Key::Escape, false);
    capture(window);
    if !picked().is_empty() {
        return Err("space keys: Esc after a line's menu left its card open".into());
    }
    // The shots after go on with the place noted as before (Ctrl+K lists the actions of the place noted).
    if let Some(place) = noted {
        mo_ui::note_place(place);
    }
    Ok(())
}

/// Songs on show, with the keys on the window (as Ctrl+K gives them back when it closes) and nothing open over it.
fn start_on_songs(app: &mo_ui::AppWindow) {
    start_on(app, Place::Songs);
}

/// `place` on show, with the keys on the window and nothing open over it, as [`start_on_songs`].
fn start_on(app: &mo_ui::AppWindow, place: Place) {
    app.invoke_place_chosen(place.index() as i32);
    app.set_sheet_open(false);
    mo_ui::finder::toggle(app);
    slint::platform::update_timers_and_animations();
    mo_ui::finder::close(app);
    slint::platform::update_timers_and_animations();
}

/// Presses and lets go of `key` on the window, with Ctrl held when `ctrl`, then lets the window catch up.
fn press(app: &mo_ui::AppWindow, key: impl Into<slint::SharedString>, ctrl: bool) {
    use slint::platform::{Key, WindowEvent};
    let window = app.window();
    let text = key.into();
    if ctrl {
        window.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
    }
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text });
    if ctrl {
        window.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
    }
    slint::platform::update_timers_and_animations();
}

/// One more press of `key` from holding it down (the keyboard's repeat).
fn held(app: &mo_ui::AppWindow, key: impl Into<slint::SharedString>) {
    app.window().dispatch_event(slint::platform::WindowEvent::KeyPressRepeated { text: key.into() });
    slint::platform::update_timers_and_animations();
}

/// The Presets page: made-up sounds with one picked, then the Curves section with a curve picked.
fn presets_shots(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    shots: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), Box<dyn std::error::Error>> {
    use mo_app::presets::Section;
    app.set_place(Place::Presets.index() as i32);
    mo_ui::presets::show_sample(app);
    shots.push(("presets-list.png".into(), capture(window)));
    // The Plugin filter's menu, open, with two plugins picked and ticked, and the Synths chip lit; then a click beside
    // the menu closes it, and the filters are cleared again.
    let data = app.global::<mo_ui::PresetsData>();
    data.invoke_toggle("plugin".into(), "Serum".into());
    data.invoke_toggle("plugin".into(), "Vital".into());
    data.invoke_toggle("type".into(), "Synths".into());
    let before = capture(window);
    click(window, PLUGIN_CHIP.0, PLUGIN_CHIP.1);
    let after = capture(window);
    if before == after {
        return Err("filter-menu.png: the Plugin menu didn't open; has its chip moved?".into());
    }
    shots.push(("filter-menu.png".into(), after));
    click(window, 1000.0, 40.0);
    data.invoke_clear();
    mo_ui::presets::pick_row(app, 3);
    shots.push(("presets-picked.png".into(), capture(window)));
    mo_ui::presets::show_audition_sample(app, true);
    shots.push(("presets-auditioning.png".into(), capture(window)));
    mo_ui::presets::show_audition_sample(app, false);
    mo_ui::presets::show_section(app, Section::Curves);
    shots.push(("presets-curves.png".into(), capture(window)));
    mo_ui::presets::pick_row(app, 0);
    shots.push(("presets-curve-picked.png".into(), capture(window)));
    mo_ui::presets::show_section(app, Section::Sounds);
    mo_ui::presets::pick_row(app, usize::MAX);
    Ok(())
}

/// Ctrl+K: opened from the Songs page with nothing typed, a search that finds a song and a sound, only the sounds
/// (Tab), and an action with its key.
fn finder_shots(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow, shots: &mut Vec<(String, Vec<u8>)>) {
    use mo_ui::finder;
    finder::search_here();
    let songs = mo_app::finder::songs(&sample_songs(), chrono::Local::now().naive_local());
    finder::add_source(app, "songs", std::sync::Arc::new(songs));
    app.set_place(Place::Songs.index() as i32);
    let data = app.global::<mo_ui::FinderData>();
    let typed = |text: &str| {
        data.set_query(text.into());
        data.invoke_edited(text.into());
    };
    finder::toggle(app);
    shots.push(("finder-open.png".into(), capture(window)));
    typed("glass");
    shots.push(("finder-search.png".into(), capture(window)));
    typed("");
    data.invoke_key("\t".into(), false, false);
    data.invoke_key("\t".into(), false, false);
    data.invoke_key(char::from(slint::platform::Key::DownArrow).to_string().into(), false, false);
    shots.push(("finder-sounds.png".into(), capture(window)));
    data.invoke_key("\t".into(), false, false);
    data.invoke_key("\t".into(), false, false);
    data.invoke_key("\t".into(), false, false);
    typed("full");
    shots.push(("finder-action.png".into(), capture(window)));
    // Every kind again: a Settings tab found by what's on it, and the MIDI parts the MIDI page's sample has.
    data.invoke_kind_picked(0);
    let settings = std::sync::Arc::new(mo_app::finder::settings());
    finder::add_source(app, "settings", settings);
    typed("bass");
    shots.push(("finder-midi.png".into(), capture(window)));
    typed("theme");
    shots.push(("finder-settings.png".into(), capture(window)));
    // A narrow window: the kinds' row scrolls, and the last kind picked is still in view.
    typed("");
    window.set_size(PhysicalSize::new(560, HEIGHT));
    data.invoke_kind_picked(data.get_kinds().row_count() as i32 - 1);
    shots.push(("finder-narrow.png".into(), capture(window)));
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    finder::close(app);
}

/// Ctrl+K to the last song, preset, MIDI part and plugin of their pages' lists, each found by its name and opened with
/// Enter: first from another page (Sounds), then from its own page with a search hiding it. In a short window, so each
/// list scrolls. Each list must be at its end just as the wheel leaves it there by hand, the whole last line in sight.
/// Before 7 Nov a page made when it was shown (after the turn was asked) stayed at its top, a list given back the lines
/// a search hid turned with the height of the fewer it had, Plugins didn't turn at all, and a long turn landed on the
/// top of a line, up to a line short of the end: the last line was cut off.
fn opened_in_sight(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    let finder = app.global::<mo_ui::FinderData>();
    let open = |name: &str| {
        mo_ui::finder::toggle(app);
        finder.set_query(name.into());
        finder.invoke_edited(name.into());
        finder.invoke_key(char::from(slint::platform::Key::Return).to_string().into(), false, false);
    };
    let songs = app.global::<mo_ui::SongsData>();
    let presets = app.global::<mo_ui::PresetsData>();
    let midi = app.global::<mo_ui::MidiData>();
    let plugins = app.global::<mo_ui::PluginsData>();
    // Each page's search box, as typing in it would set it.
    let search = |place: Place, text: &str| match place {
        Place::Songs => {
            songs.set_search(text.into());
            songs.invoke_search_edited(text.into());
        }
        Place::Presets => {
            presets.set_search(text.into());
            presets.invoke_search_edited(text.into());
        }
        Place::Plugins => {
            plugins.set_search(text.into());
            plugins.invoke_search_edited(text.into());
        }
        _ => {
            midi.set_search(text.into());
            midi.invoke_search_edited(text.into());
        }
    };
    // The name on each list's last line as the page has it now.
    fn end<T>(lines: slint::ModelRc<T>) -> Option<T> {
        lines.row_count().checked_sub(1).and_then(|i| lines.row_data(i))
    }
    let last = |place: Place| {
        let name = match place {
            Place::Songs => end(songs.get_lines()).map(|l| l.name),
            Place::Presets => end(presets.get_lines()).map(|l| l.name),
            Place::Plugins => end(plugins.get_lines()).map(|l| l.name),
            _ => end(midi.get_lines()).map(|l| l.title),
        };
        name.unwrap_or_default().to_string()
    };
    mo_ui::presets::sample_in_finder(app);
    // The Plugins page has the sample its shots left; Ctrl+K finds the same plugins.
    let found = mo_app::finder::plugins(&sample_plugins().0);
    mo_ui::finder::add_source(app, "plugins", std::sync::Arc::new(found));
    window.set_size(PhysicalSize::new(WIDTH, SHORT));
    let mut wrong = Vec::new();
    for (place, name, hides) in [
        (Place::Songs, "Old Tapes", "Night"),
        (Place::Presets, "Warm Pad", "Sub"),
        (Place::Midi, "Flute hook", "Keys"),
        (Place::Plugins, "Arturia Pigments", "Serum"),
    ] {
        let at_end = last(place);
        if !at_end.starts_with(name) {
            wrong.push(format!("{name} isn't the last line of its list now, {at_end} is: open that"));
            continue;
        }
        for own_page in [false, true] {
            let how = if own_page {
                format!("{name} from its page, a search hiding it")
            } else {
                format!("{name} from Sounds")
            };
            if own_page {
                app.set_place(place.index() as i32);
                search(place, hides);
            } else {
                app.set_place(Place::Sounds.index() as i32);
            }
            capture(window);
            open(name);
            let first = capture(window);
            if app.get_place() != place.index() as i32 {
                wrong.push(format!("{how}: Enter didn't open it on its page (is it still Ctrl+K's first hit?)"));
                search(place, "");
                continue;
            }
            // The list turned up a little by hand, then back down past its end, where the wheel stops.
            scroll(window, SHORT_LIST_WHEEL.0, SHORT_LIST_WHEEL.1, SHORT_TURN);
            hover(window, -1.0, -1.0);
            let up = capture(window);
            scroll(window, SHORT_LIST_WHEEL.0, SHORT_LIST_WHEEL.1, -2.0 * SHORT_TURN);
            hover(window, -1.0, -1.0);
            let end = capture(window);
            if same_in(&up, &end, SHORT_LIST) {
                wrong.push(format!("{how}: the wheel moved nothing (is it still over the list, and does it scroll?)"));
            } else if !same_in(&first, &end, SHORT_LIST) {
                wrong.push(format!("{how}: not at its end as the wheel leaves it (is its last line cut off?)"));
            }
            search(place, "");
        }
    }
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    songs.invoke_close_panel();
    presets.invoke_close_panel();
    midi.invoke_close_panel();
    plugins.invoke_close_panel();
    app.set_place(Place::Songs.index() as i32);
    capture(window);
    if wrong.is_empty() { Ok(()) } else { Err(format!("Ctrl+K, opened low down a list: {}", wrong.join("; "))) }
}

/// The Sounds page: a sound picked and playing in its panel, a filter on, the List and History views, and the tiles in
/// a light theme.
fn sounds_shots(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    graphite: &Look,
    shots: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    app.set_place(Place::Sounds.index() as i32);
    let data = app.global::<mo_ui::SoundsData>();
    // Autoplay is on, so picking a sound plays it.
    data.invoke_select(6);
    data.set_played(0.35);
    shots.push(("sounds-panel.png".into(), capture(window)));
    // A sound deep in a pack, with folder names too long for a line, and six tags: its card wraps them, at the usual
    // width and as narrow as it goes, with nothing cut off on the right. Found by name, as the shuffled order changes
    // with every run.
    data.set_search("Boom Kick".into());
    data.invoke_search_edited("Boom Kick".into());
    data.invoke_select(0);
    capture(window);
    shots.push(("sounds-panel-long.png".into(), capture(window)));
    // The same card's switch on its other side: the projects that use the sound, newest first, with the card turned
    // down to them.
    data.set_card_list(1);
    capture(window);
    let top = capture(window);
    let down = card_down(window, &top, "sounds-panel-used-in.png").map_err(|e| e.to_string())?;
    shots.push(("sounds-panel-used-in.png".into(), down));
    data.set_card_list(0);
    let wide = data.get_panel_width();
    data.set_panel_width(320.0);
    capture(window);
    shots.push(("sounds-panel-narrow.png".into(), capture(window)));
    data.set_panel_width(wide);
    data.set_search("".into());
    data.invoke_search_edited("".into());
    // Two types at once, with the Clear types chip.
    let type_at = |i: usize| data.get_type_filter().chips.row_data(i).map(|c| c.text).unwrap_or_default();
    let (first, second) = (type_at(0), type_at(1));
    let before = capture(window);
    data.invoke_toggle_choice("type".into(), first);
    data.invoke_toggle_choice("type".into(), second.clone());
    let after = capture(window);
    if before == after {
        return Err("sounds-types.png: picking two types changed nothing; are the type chips wired?".into());
    }
    shots.push(("sounds-types.png".into(), after));
    // One type, a key and the keys that mix well with it, and two tempos.
    data.invoke_toggle_choice("type".into(), second);
    data.invoke_toggle_choice("key".into(), "A minor".into());
    data.invoke_toggle("mix-well".into());
    data.invoke_toggle_choice("tempo".into(), "110 to 130".into());
    data.invoke_toggle_choice("tempo".into(), "130 to 150".into());
    let filtered = capture(window);
    if filtered == before {
        return Err("sounds-filtered.png: the key and tempo picks changed nothing".into());
    }
    shots.push(("sounds-filtered.png".into(), filtered));
    data.invoke_clear();
    // Hide duplicates on, with three sounds the same audio (the first three by id): one of them shows, and the chip
    // says how many it hides.
    let mut ids: Vec<i64> = match mo_ui::sounds::known() {
        mo_ui::sounds::Known::Read(list, _) => list.sounds().iter().map(|s| s.id).collect(),
        _ => Vec::new(),
    };
    ids.sort_unstable();
    ids.truncate(3);
    let same = mo_sounds::copies::SameAudio { groups: vec![ids], ..Default::default() };
    mo_ui::sounds::show_copies_for_screens(app, Some(&same));
    let all = capture(window);
    data.invoke_toggle("hide-copies".into());
    let hidden = capture(window);
    if hidden == all {
        return Err("sounds-duplicates.png: Hide duplicates changed nothing".into());
    }
    shots.push(("sounds-duplicates.png".into(), hidden));
    data.invoke_toggle("hide-copies".into());
    mo_ui::sounds::show_copies_for_screens(app, None);
    // A tile dragged out to a DAW: dimmed while it's carried, with the card that follows the mouse a little way on
    // from it, then lit for a moment once it has landed.
    let still = capture(window);
    data.set_lifted_row(2);
    let lifted = capture(window);
    let card = mo_ui::sounds::card_at_row(app, 2).ok_or("sounds-drag-lifted.png: no sound in the third tile")?;
    let (x, y, w, h) = changed_box(&still, &lifted).ok_or("sounds-drag-lifted.png: the third tile didn't dim")?;
    shots.push(("sounds-drag-lifted.png".into(), with_card(lifted, &card, graphite, (x + w / 2 + 40, y + h / 2 + 24))));
    data.set_lifted_row(-1);
    data.set_landed_row(2);
    shots.push(("sounds-drag-landed.png".into(), capture_after(window, Duration::from_millis(150))));
    data.set_landed_row(-1);
    // Its glow fades before the next picture.
    capture_after(window, Duration::from_millis(700));
    data.invoke_pick_view(1);
    shots.push(("sounds-list.png".into(), capture(window)));
    // Two years picked on the year strip.
    data.invoke_years_picked(1, 2);
    capture(window);
    shots.push(("sounds-years.png".into(), capture(window)));
    data.invoke_years_cleared();
    data.invoke_pick_view(3);
    shots.push(("sounds-history.png".into(), capture(window)));
    data.invoke_close_panel();
    data.invoke_pick_view(2);
    data.invoke_select(3);
    capture(window);
    shots.push(("sounds-galaxy.png".into(), capture(window)));
    // A search that leaves no stars: the list's word says so over the dimmed galaxy, once.
    data.set_search("Nothing called this".into());
    data.invoke_search_edited("Nothing called this".into());
    capture(window);
    shots.push(("sounds-galaxy-none.png".into(), capture(window)));
    data.set_search("".into());
    data.invoke_search_edited("".into());
    // Lit by length: long sounds big and bright, one-shots small; then all alike, to compare.
    data.invoke_galaxy_light_by("Length".into());
    capture(window);
    shots.push(("sounds-galaxy-length.png".into(), capture(window)));
    data.invoke_galaxy_light_by("Nothing".into());
    capture(window);
    shots.push(("sounds-galaxy-alike.png".into(), capture(window)));
    data.invoke_galaxy_light_by("Liked".into());
    capture(window);
    // A sweep across the map: the trail through the swept stars and the Swept card.
    data.invoke_galaxy_down(40.0, 120.0, false);
    for step in 1..=60 {
        data.invoke_galaxy_move(40.0 + step as f32 * 12.0, 120.0 + step as f32 * 6.0);
        // Slow enough that each star passed gets its turn to play.
        if step % 3 == 0 {
            std::thread::sleep(Duration::from_millis(95));
        }
    }
    data.invoke_galaxy_up(false);
    capture(window);
    shots.push(("sounds-galaxy-swept.png".into(), capture(window)));
    // The arrow keys on the map: Right goes to the nearest star that way, and it's picked.
    data.invoke_step(1, false);
    capture(window);
    shots.push(("sounds-galaxy-arrow.png".into(), capture(window)));
    // A line on the Swept card clicked, then Down: the card has the arrow keys and goes to the next sound on it.
    let (cx, cy) = SWEPT_LINE;
    click(window, cx, cy);
    data.invoke_step(2, false);

    capture(window);
    shots.push(("sounds-galaxy-swept-keys.png".into(), capture(window)));
    // The names over a zoomed-out map (a 29-sound library is too small to have neighbourhoods, so two are set here).
    let label = |region: i32, name: &str, x: f32, y: f32, colour: i32| mo_ui::GalaxyLabel {
        region,
        name: name.into(),
        x,
        y,
        colour,
        fade: 1.0,
    };
    data.set_galaxy_labels(slint::ModelRc::new(slint::VecModel::from(vec![
        label(0, "KICK", 220.0, 200.0, 0),
        label(1, "PAD", 420.0, 460.0, 5),
    ])));
    capture(window);
    shots.push(("sounds-galaxy-names.png".into(), capture(window)));
    // Flown in: the mini-map shows the whole map with a box round the part on show.
    data.set_galaxy_labels(slint::ModelRc::new(slint::VecModel::from(Vec::<mo_ui::GalaxyLabel>::new())));
    data.invoke_galaxy_move(300.0, 300.0);
    data.invoke_galaxy_wheel(8.0);
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(16));
        capture(window);
    }
    shots.push(("sounds-galaxy-mini.png".into(), capture(window)));
    data.invoke_pick_view(4);
    shots.push(("sounds-rediscover.png".into(), capture(window)));
    // Rediscover's year strip, two years dragged over.
    data.invoke_re_years_picked(0, 1);
    capture(window);
    shots.push(("sounds-rediscover-years.png".into(), capture(window)));
    data.invoke_re_years_cleared();
    // Two types picked on Rediscover, as on the Type filter, with its Clear types chip.
    data.invoke_re_pick("type".into(), "Kick".into());
    data.invoke_re_pick("type".into(), "Snare".into());
    capture(window);
    shots.push(("sounds-rediscover-types.png".into(), capture(window)));
    data.invoke_re_pick("type".into(), "".into());
    // A forgotten folder clicked on Rediscover: the List shows its own sounds, with a chip saying which folder.
    data.invoke_pick_folder(r"D:\Samples\Drums".into());
    capture(window);
    shots.push(("sounds-folder.png".into(), capture(window)));
    data.invoke_toggle("folder".into());
    data.invoke_pick_view(5);
    data.invoke_kit_roll();
    data.invoke_pad_lock(0);
    capture(window);
    // The Snare pad's Lock, clicked as you would: the pad's own buttons take their click, not the line under them.
    click(window, SNARE_LOCK.0, SNARE_LOCK.1);
    shots.push(("sounds-kits.png".into(), capture(window)));
    data.invoke_pick_view(0);
    mo_ui::apply_look(app, &Look::from_settings(Some("Paper"), None));
    capture(window);
    shots.push(("sounds-paper.png".into(), capture(window)));
    mo_ui::apply_look(app, graphite);
    found_turned_down(app, window)?;
    // Your say on the card, last of the Sounds shots as it stays on the page (the screenshots keep none of it): your
    // own type, a tag added and one taken away, and the heart taken off (the tile's goes too). The tiles
    // are still turned down to their end from the check before: the search leaves one, at the top.
    data.set_search("Boom Kick".into());
    data.invoke_search_edited("Boom Kick".into());
    data.invoke_select(0);
    // Drawn after each, as a click would be: the tiles' list isn't made again many times between two draws.
    capture(window);
    data.invoke_set_type("Perc".into());
    capture(window);
    data.invoke_add_tag("warm".into());
    capture(window);
    data.invoke_remove_tag("Dark".into());
    capture(window);
    data.invoke_toggle_favourite();
    capture(window);
    let top = capture(window);
    shots.push(("sounds-panel-yours.png".into(), top.clone()));
    // Turned down to its tags (the new one in among them) and the box to add one.
    shots.push((
        "sounds-panel-yours-tags.png".into(),
        card_down(window, &top, "sounds-panel-yours-tags.png").map_err(|e| e.to_string())?,
    ));
    // The mouse resting on that box: its tip says what it's for and its key, as the menu's line does.
    let (x, y, turn) = SPACE_CARD_WHEEL;
    scroll(window, x, y, turn);
    hover(window, TAG_BOX.0, TAG_BOX.1);
    let before = capture_after(window, Duration::from_millis(150));
    let after = capture_after(window, Duration::from_millis(750));
    hover(window, -1.0, -1.0);
    scroll(window, x, y, -turn);
    hover(window, -1.0, -1.0);
    if before == after {
        let (x, y) = TAG_BOX;
        return Err(format!("sounds-tag-tip.png: no tooltip came up at {x}, {y}; has the card moved?"));
    }
    shots.push(("sounds-tag-tip.png".into(), after));
    data.set_search("".into());
    data.invoke_search_edited("".into());
    app.set_place(Place::Songs.index() as i32);
    Ok(())
}

/// The List and the tiles turned down half a page more each time, then a search that leaves one sound and that sound
/// picked, both before the list is drawn again: it shows, at the top. Turned down a screen or so, the list once lost
/// every row here and showed nothing but its scrollbar (7 Nov): the pick turned it before it had laid out the rows the
/// search had just given it. Ends on the tiles turned down to their end, the mouse away.
fn found_turned_down(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    let data = app.global::<mo_ui::SoundsData>();
    let search = |text: &str| {
        data.set_search(text.into());
        data.invoke_search_edited(text.into());
    };
    // Drawn with no wait: nothing here needs time to settle, and a list lays out its rows as it's drawn.
    let draw = || capture_after(window, Duration::ZERO);
    let (x, y) = (590.0, 600.0);
    // From its top each time.
    let turn_down = |by: f32| {
        scroll(window, x, y, 10.0 * PAGE_DOWN);
        if by > 0.0 {
            scroll(window, x, y, -by);
        }
        hover(window, -1.0, -1.0);
    };
    for (view, name, first) in [(1, "List", FIRST_LINE), (0, "tiles", FIRST_TILE)] {
        data.invoke_pick_view(view);
        // At the top first: a list made afresh brings the picked sound into view a moment later.
        turn_down(0.0);
        let mut before = draw();
        for halves in 1..=16 {
            let by = halves as f32 * PAGE_DOWN / 2.0;
            turn_down(by);
            let down = draw();
            if down == before {
                if halves == 1 {
                    return Err(format!(
                        "sounds {name}: the wheel at {x}, {y} moved nothing; is it still on the list?"
                    ));
                }
                // At its end: turned further, it moves no more.
                break;
            }
            before = down;
            search("Boom Kick");
            data.invoke_select(0);
            if plain(&draw(), first) {
                return Err(format!(
                    "sounds {name}: turned down {by} px (or to its end), a search that left one sound, picked before \
                     the next draw, left it showing nothing"
                ));
            }
            search("");
        }
    }
    turn_down(100.0 * PAGE_DOWN);
    draw();
    Ok(())
}

/// Whether every pixel in a box (x, y, width, height, in pixels) of a picture of the window is the same colour.
fn plain(pixels: &[u8], (x, y, width, height): (u32, u32, u32, u32)) -> bool {
    let at = |x: u32, y: u32| {
        let i = ((y * WIDTH + x) * 3) as usize;
        pixels.get(i..i + 3)
    };
    let first = at(x, y);
    (y..y + height).all(|y| (x..x + width).all(|x| at(x, y) == first))
}

/// Whether two pictures of the window are the same inside `(x, y, width, height)`.
fn same_in(a: &[u8], b: &[u8], (x, y, width, height): (u32, u32, u32, u32)) -> bool {
    (y..y + height).all(|y| {
        let at = ((y * WIDTH + x) * 3) as usize;
        let row = at..at + (width * 3) as usize;
        a.get(row.clone()) == b.get(row)
    })
}

/// Made-up drives for the Space page: C: nearly full with the app's files, a big drive of projects and samples, the
/// Bitwig drive, and a backup drive that isn't plugged in.
fn sample_drives() -> Vec<mo_app::space::Drive> {
    use mo_app::space::{Drive, Holds};
    const GB: u64 = 1 << 30;
    let drive = |name: &str, free: u64, total: u64, holds: Vec<Holds>| Drive {
        name: name.into(),
        free: free * GB,
        total: total * GB,
        here: total > 0,
        holds,
    };
    vec![
        drive("C:", 8, 476, vec![Holds::Samples, Holds::AppFiles]),
        drive("E:", 0, 0, vec![Holds::Backup]),
        drive("I:", 1229, 3726, vec![Holds::Projects, Holds::Samples]),
        drive("K:", 655, 1863, vec![Holds::Projects]),
    ]
}

/// The app's own files, made up, with the parts' words as the app gives them.
fn sample_app_files() -> Vec<mo_space::app_files::AppFilesPart> {
    use mo_space::app_files::{AppDatabase, AppFilesPart, caches, part_of, sound_lab_copies};
    const MB: u64 = 1 << 20;
    let part = |(name, what): (&'static str, &'static str), bytes: u64| AppFilesPart { name, what, bytes };
    let named = |db: &AppDatabase| (db.name, db.what);
    let backup = sound_lab_copies(PathBuf::new(), PathBuf::new());
    let cache = caches([PathBuf::new()]);
    vec![
        AppFilesPart { name: "Your library", what: "Projects, plugins, samples, tags and ratings", bytes: 6250 * MB },
        AppFilesPart { name: "Sound Lab", what: "Every sound's details, likes and look-alikes", bytes: 2460 * MB },
        part(named(&backup[0]), 2310 * MB),
        part(part_of("models", true), 310 * MB),
        part(named(&cache[0]), 140 * MB),
        part(part_of("logs", true), 48 * MB),
        part(part_of("notes", true), 21 * MB),
        part(part_of("bitwig template", false), 4 * MB),
    ]
}

/// Made-up Bitwig projects on the Bitwig drive with copies of Library multisamples, written as on Windows (the page
/// splits them the same way here). Each has its own save; GARAGE FOR BW 1 is fixed and has an older save and an
/// auto-backup, Night Drive has three more saves.
fn sample_bitwig_copies() -> mo_app::space_bitwig::Copies {
    use mo_app::space_bitwig::{Copies, Project};
    use mo_space::bitwig::{LibraryCopy, Save};
    const MB: u64 = 1 << 20;
    // Saved `days` after 1 Oct 2026, at 20:15.
    let save = |folder: &str, name: &str, days: u64| Save {
        path: format!(r"{folder}\{name}.bwproject").into(),
        modified: Some(std::time::UNIX_EPOCH + Duration::from_secs(1_790_799_300 + days * 86_400)),
        below: name.contains('\\'),
        fixed: None,
    };
    let project = |name: &str, copies: &[(&str, u64)], others: &[(&str, u64)]| {
        let folder = format!(r"K:\Bitwig Studio\Projects\{name}");
        let copies = copies
            .iter()
            .map(|&(sound, mb)| LibraryCopy {
                in_project: format!(r"{folder}\multi-samples\{sound}-3-2-2.multisample"),
                library_file: format!(r"K:\Bitwig Studio\Library\{sound}.multisample"),
                bytes: mb * MB,
            })
            .collect();
        let saves = std::iter::once(save(&folder, name, 33)).chain(others.iter().map(|&(n, d)| save(&folder, n, d)));
        Project { folder: folder.clone().into(), copies, saves: saves.collect() }
    };
    let mut garage = project(
        "GARAGE FOR BW 1",
        &[("Kicks 1 Select", 96), ("OHH 1 Select", 74), ("Snares 2 Select", 72)],
        &[("GARAGE FOR BW 1 v2", 20), (r"auto-backups\GARAGE FOR BW 1-14", 32)],
    );
    garage.saves[0].fixed =
        Some((r"K:\Bitwig Studio\Projects\GARAGE FOR BW 1\GARAGE FOR BW 1 (fixed).bwproject".into(), true));
    Copies {
        library: vec![r"K:\Bitwig Studio\Library".into()],
        folders: 31,
        projects: vec![
            garage,
            project(
                "Night Drive",
                &[("Kicks 1 Select", 96), ("Pads Warm", 41)],
                &[("Night Drive v3", 30), ("Night Drive v2", 12), ("Night Drive v1", 4)],
            ),
            project("Bounce 2", &[("OHH 1 Select", 74)], &[]),
            project("Sketch 14", &[("Bass Mono", 12), ("Keys Soft", 9)], &[]),
        ],
    }
}

/// The Space page's made-up finds: the drives, the app's files, Bitwig copies, old copies, spare plugin copies and
/// what your songs could free.
fn sample_space() -> mo_ui::space::Sample {
    mo_ui::space::Sample {
        drives: sample_drives(),
        parts: sample_app_files(),
        bitwig: sample_bitwig_copies(),
        old: sample_old_copies(),
        spare: sample_spare_copies(),
        songs: sample_song_room(),
        plugins: sample_unused_plugins(),
        copies: sample_sound_copies(),
        ..Default::default()
    }
}

/// Made-up copies of the same sound, as Hide duplicates finds them, grouped by the Space page's own look with the switch
/// on: a kick your songs use with two exact copies, a vocal chop copied and renamed, and small hats.
fn sample_sound_copies() -> mo_app::space_sounds::Found {
    use std::collections::HashMap;

    use mo_app::sounds::SoundList;
    use mo_app::space_sounds::look;
    use mo_sounds::Sound;
    use mo_sounds::copies::SameAudio;
    use mo_sounds::usage::Usage;
    const KB: u64 = 1024;
    let mut sounds = vec![
        Sound::new(1, r"D:\Samples\Drums\Kicks\Kick 808 Long.wav"),
        Sound::new(2, r"D:\Samples\Packs\Trap Essentials\Kicks\Kick 808 Long.wav"),
        Sound::new(3, r"E:\Old Backup\Samples\Kicks\Kick 808 Long.wav"),
        Sound::new(8, r"D:\Samples\Vocals\Vox Chop 03.wav"),
        Sound::new(9, r"D:\Samples\Vox\Vox Chop 03 copy.wav"),
    ];
    let mut groups = vec![vec![1, 2, 3], vec![8, 9]];
    let mut sizes = HashMap::from([(1, 310 * KB), (2, 310 * KB), (3, 310 * KB), (8, 160 * KB), (9, 160 * KB)]);
    // Small hats, each with an exact copy in a pack you downloaded.
    for n in 0..6 {
        let mut group = Vec::new();
        for (copy, folder) in [r"D:\Samples\Drums\Hats", r"E:\Downloads\Hat Pack"].into_iter().enumerate() {
            let id = 100 + n * 2 + copy as i64;
            sounds.push(Sound::new(id, &format!(r"{folder}\Hat {:02}.wav", n + 1)));
            sizes.insert(id, 18 * KB);
            group.push(id);
        }
        groups.push(group);
    }
    let mut list = SoundList::new(sounds);
    list.set_used(HashMap::from([(1, Usage { projects: 4, ..Usage::default() })]));
    list.set_copies(&SameAudio { groups, sizes });
    look(&list, true)
}

/// Made-up plugins no song has used lately, as the old app measured them: some never used, some not in over 2 years.
fn sample_unused_plugins() -> mo_app::space_plugins::Found {
    use mo_app::space_plugins::{Found, Measured};
    use mo_space::plugin_space::{PluginSpace, SpaceFolder, SpaceKind};
    const MB: u64 = 1 << 20;
    let at =
        |year: i32, month: u32| chrono::NaiveDate::from_ymd_opt(year, month, 14).and_then(|d| d.and_hms_opt(20, 0, 0));
    let plugin =
        |name: &str, vendor: &str, used: Option<(chrono::NaiveDateTime, usize)>, held: &[(SpaceKind, &str, u64)]| {
            Measured {
                name: name.to_owned(),
                vendor: vendor.to_owned(),
                key: name.to_lowercase(),
                own: String::new(),
                last_used: used.map(|(t, _)| t),
                songs: used.map_or(0, |(_, n)| n),
                room: PluginSpace {
                    key: name.to_lowercase(),
                    name: name.to_owned(),
                    vendor: vendor.to_owned(),
                    folders: held
                        .iter()
                        .map(|(kind, path, mb)| SpaceFolder {
                            path: (*path).to_owned(),
                            kind: *kind,
                            bytes: mb * MB,
                            shared_with: Vec::new(),
                            added: false,
                        })
                        .collect(),
                },
                stands_in: false,
            }
        };
    let (program, presets, sounds) = (SpaceKind::Program, SpaceKind::Presets, SpaceKind::Sounds);
    let mut plugins = vec![
        plugin(
            "Omnisphere",
            "Spectrasonics",
            at(2022, 3).map(|t| (t, 3)),
            &[
                (program, r"C:\Program Files\Common Files\VST3\Omnisphere.vst3", 85),
                (presets, r"C:\ProgramData\Spectrasonics\Omnisphere\Settings Library", 120),
                (sounds, r"I:\Spectrasonics\STEAM\Omnisphere", 39_100),
            ],
        ),
        plugin(
            "Kontakt 6",
            "Native Instruments",
            None,
            &[
                (program, r"C:\Program Files\Common Files\VST3\Kontakt.vst3", 410),
                (sounds, r"I:\Kontakt Factory Library", 22_100),
            ],
        ),
        plugin(
            "Addictive Drums 2",
            "XLN Audio",
            at(2021, 8).map(|t| (t, 2)),
            &[
                (program, r"C:\Program Files\Common Files\VST3\Addictive Drums 2.vst3", 60),
                (sounds, r"I:\XLN Audio\Addictive Drums 2", 10_040),
            ],
        ),
        plugin(
            "Trilian",
            "Spectrasonics",
            None,
            &[
                (program, r"C:\Program Files\Common Files\VST3\Trilian.vst3", 70),
                (sounds, r"I:\Spectrasonics\STEAM\Trilian", 8_300),
            ],
        ),
        plugin(
            "Massive",
            "Native Instruments",
            at(2023, 6).map(|t| (t, 12)),
            &[
                (program, r"C:\Program Files\VSTPlugins\Massive.dll", 60),
                (presets, r"C:\Users\Solomon\Documents\Native Instruments\Massive", 410),
            ],
        ),
        plugin(
            "Ozone 9",
            "iZotope",
            None,
            &[
                (program, r"C:\Program Files\Common Files\VST3\iZotope Ozone 9.vst3", 610),
                (presets, r"C:\Users\Solomon\Documents\iZotope\Ozone 9", 40),
            ],
        ),
        plugin(
            "Sylenth1",
            "LennarDigital",
            at(2019, 11).map(|t| (t, 7)),
            &[(program, r"C:\Program Files\VSTPlugins\Sylenth1.dll", 30)],
        ),
    ];
    plugins.sort_by_key(|p| std::cmp::Reverse(p.bytes()));
    let measured = chrono::NaiveDate::from_ymd_opt(2026, 11, 3).and_then(|d| d.and_hms_opt(9, 30, 0));
    // Serum is used this year, so it isn't on the card.
    let used = vec![plugin(
        "Serum",
        "Xfer Records",
        at(2026, 10).map(|t| (t, 31)),
        &[(program, r"C:\Program Files\Common Files\VST3\Serum.vst3", 30)],
    )];
    Found { plugins, used, years: 2, now: measured.unwrap_or_default(), sizes: true, measured, gone: 0 }
}

/// Made-up songs with room to free, written as on Windows: Bitwig songs with copies of a save, old auto-backups and
/// saves from the middle of a sitting; FL Studio songs with old autosaves in their Backup folder and many saves in one
/// sitting; a Studio One song's cache; and bounces no save names.
fn sample_song_room() -> mo_app::space_songs::Found {
    use mo_app::space_songs::{Found, SongRoom};
    use mo_space::song::{CompactItem, CompactKind, CompactSettings, SongSpaceReport};
    const MB: u64 = 1 << 20;
    let at =
        |day: u32, hour: u32| chrono::NaiveDate::from_ymd_opt(2026, 10, day).and_then(|d| d.and_hms_opt(hour, 12, 0));
    let item = |kind: CompactKind, path: String, mb: u64, day: u32, hour: u32| CompactItem {
        kind,
        path,
        bytes: mb * MB,
        modified: at(day, hour),
        same_as: None,
    };
    // `n` things of a kind in a folder, each made on one of `days` in turn.
    let many = |folder: &str, name: &str, kind: CompactKind, n: u32, mb: u64, days: &[u32]| -> Vec<CompactItem> {
        (0..n)
            .map(|i| {
                let day = days[i as usize % days.len()];
                let path = match kind {
                    CompactKind::Backup if name.ends_with(".flp") => {
                        format!(
                            r"{folder}\Backup\{} (overwritten at {:02}h{:02}).flp",
                            &name[..name.len() - 4],
                            20 + i % 4,
                            i % 60
                        )
                    }
                    CompactKind::Backup => format!(r"{folder}\auto-backups\{name} ({i}).bwproject"),
                    _ => format!(
                        r"{folder}\{name}_{}.{}",
                        i + 2,
                        if folder.contains("FL") { "flp" } else { "bwproject" }
                    ),
                };
                item(kind, path, mb, day, 10 + i % 9)
            })
            .collect()
    };
    let song = |name: &str, opens: &str, saves: usize, items: Vec<CompactItem>| SongRoom {
        name: name.to_owned(),
        opens: opens.to_owned(),
        saves,
        report: SongSpaceReport { items, ..SongSpaceReport::default() },
    };
    let night = r"D:\Music\Projects\Night Drive";
    let garage = r"D:\Music\Projects\GARAGE FOR BW 1";
    let fl = r"D:\Music\Projects\FL";
    let studio_one = r"D:\Music\Projects\Studio One\Tape Echo Idea";
    let mut night_items = many(night, "Night Drive", CompactKind::SameSave, 3, 96, &[3]);
    night_items.extend(many(night, "Night Drive", CompactKind::Backup, 12, 88, &[3, 4, 9]));
    night_items.extend([
        item(CompactKind::UnusedAudio, format!(r"{night}\Bounces\Night Drive bounce 3.wav"), 412, 9, 23),
        item(CompactKind::UnusedAudio, format!(r"{night}\Recordings\Vox comp (old).wav"), 186, 4, 21),
        item(CompactKind::UnusedAudio, format!(r"{night}\Recordings\Vox take 2.wav"), 92, 4, 21),
    ]);
    let mut garage_items = many(garage, "GARAGE FOR BW 1", CompactKind::Backup, 20, 21, &[12, 14]);
    garage_items.extend(many(garage, "GARAGE FOR BW 1", CompactKind::SameSave, 2, 22, &[14]));
    garage_items.extend(many(garage, "GARAGE FOR BW 1", CompactKind::ThinnedSave, 14, 22, &[3, 4, 9]));
    let mut bounce_items = many(fl, "Bounce 2.flp", CompactKind::Backup, 31, 2, &[11]);
    bounce_items.extend(many(fl, "Bounce 2", CompactKind::ThinnedSave, 31, 2, &[11]));
    let didums_days: Vec<u32> = (2..=29).step_by(2).collect();
    let didums_items = many(fl, "Didums", CompactKind::ThinnedSave, 46, 3, &didums_days);
    let studio_items = vec![CompactItem {
        kind: CompactKind::Cache,
        path: format!(r"{studio_one}\Cache"),
        bytes: 418 * MB,
        modified: None,
        same_as: None,
    }];
    let songs = vec![
        song("Night Drive", &format!(r"{night}\Night Drive_7.bwproject"), 9, night_items),
        song("GARAGE FOR BW 1", &format!(r"{garage}\GARAGE FOR BW 1_19.bwproject"), 19, garage_items),
        song("Bounce 2", &format!(r"{fl}\Bounce 2_33.flp"), 33, bounce_items),
        song("Didums", &format!(r"{fl}\Didums_77.flp"), 52, didums_items),
        song("Tape Echo Idea", &format!(r"{studio_one}\Tape Echo Idea.song"), 3, studio_items),
    ];
    Found { songs, settings: CompactSettings::default() }
}

/// Where the made-up Night Drive's old autosaves are, as [`sample_song_room`] has them: unticked in the ticks shot.
fn sample_night_drive_autosaves() -> Vec<PathBuf> {
    let night = r"D:\Music\Projects\Night Drive";
    (0..12).map(|i| PathBuf::from(format!(r"{night}\auto-backups\Night Drive ({i}).bwproject"))).collect()
}

/// What Remove did with the made-up songs' old autosaves and caches, at 16:00 on 4 Nov 2026 (UTC): everything ticked
/// out of the box went, but two of GARAGE FOR BW 1's autosaves, which are among its newest 5 now (it was saved again
/// since the look), stayed. Then what it put in the Recycle Bin.
fn sample_songs_removed(
    shown: &mo_app::space_songs::Shown,
) -> (mo_app::space_songs::Removing, Vec<mo_app::binned::Binned>) {
    use mo_app::binned::Binned;
    use mo_space::clean::{Done, Problem, Removed, What};
    const WHEN: i64 = 1_793_808_000;
    let mut removing = mo_app::space_songs::Removing::default();
    let picked = removing.ticked_copies(Some(shown));
    removing.start(picked.clone());
    let stays = |i: &&mo_app::space_songs::SongItem| {
        &*i.song == "GARAGE FOR BW 1"
            && (i.item.path.ends_with("(0).bwproject") || i.item.path.ends_with("(1).bwproject"))
    };
    let done = Done {
        removed: picked
            .iter()
            .filter(|i| !stays(i))
            .map(|i| Removed {
                path: i.item.path.clone(),
                real: i.item.path.clone(),
                bytes: i.item.bytes,
                what: What::Song(i.item.kind),
                when: WHEN,
                bin_id: Some(String::new()),
                binned: None,
                binned_zone: None,
                stamp: None,
            })
            .collect(),
        problems: picked
            .iter()
            .filter(stays)
            .map(|i| Problem {
                path: i.item.path.clone(),
                why: "Measured again just now, it isn't one of its song's old autosaves any more, so it stays.".into(),
            })
            .collect(),
        went_with: Vec::new(),
    };
    removing.sent(WHEN, done.clone());
    let compact = mo_app::space_songs::COMPACT;
    (
        removing,
        vec![Binned {
            kind: compact.to_owned(),
            when: WHEN,
            done: Done { problems: Vec::new(), ..done },
            plugin: None,
        }],
    )
}

/// Made-up spare plugin copies, written as on Windows, biggest first: AAX copies in Avid's folder (Pro Tools isn't
/// installed), and VST2 copies in two folders whose VST3 or CLAP your saves use, one of them a link that can't go.
fn sample_spare_copies() -> mo_app::space_spare::Found {
    use mo_space::spare::{SpareCopy, Why};
    const MB: u64 = 1 << 20;
    let avid = r"C:\Program Files\Common Files\Avid\Audio\Plug-Ins";
    let aax = |name: &str, mb: u64| SpareCopy {
        name: name.to_owned(),
        format: "AAX".to_owned(),
        path: format!(r"{avid}\{name}.aaxplugin"),
        bytes: mb * MB,
        stamp: Some(String::new()),
        why: Why::NoProTools,
    };
    let vst2 = |name: &str, file: &str, folder: &str, mb: u64, used: &[&str], saves: u32| SpareCopy {
        name: name.to_owned(),
        format: "VST2".to_owned(),
        path: format!(r"{folder}\{file}"),
        bytes: mb * MB,
        stamp: Some(String::new()),
        why: Why::OtherFormatsUsed {
            used: used.iter().map(|u| (*u).to_owned()).collect(),
            projects: saves,
            used_copies: Vec::new(),
        },
    };
    let plugins = r"C:\Program Files\VSTPlugins";
    let steinberg = r"C:\Program Files\Steinberg\VSTPlugins";
    let mut copies = vec![
        aax("Kontakt 7", 212),
        aax("Pigments", 148),
        aax("Omnisphere", 64),
        aax("Serum", 58),
        aax("FabFilter Pro-Q 3", 31),
        aax("Valhalla VintageVerb", 12),
        vst2("Kontakt 7", "Kontakt 7.dll", plugins, 186, &["VST3"], 212),
        vst2("Pigments", "Pigments.dll", steinberg, 139, &["VST3", "CLAP"], 64),
        vst2("Serum", "Serum_x64.dll", plugins, 24, &["VST3"], 412),
        SpareCopy { stamp: None, ..vst2("Diva", "Diva(x64).dll", steinberg, 21, &["VST3"], 37) },
    ];
    copies.sort_by_key(|c| std::cmp::Reverse(c.bytes));
    mo_app::space_spare::Found { copies }
}

/// The made-up AAX copy unticked in the ticks shot.
const SAMPLE_UNTICKED: &str = r"C:\Program Files\Common Files\Avid\Audio\Plug-Ins\FabFilter Pro-Q 3.aaxplugin";

/// One made-up thing a clean-up sent to the Recycle Bin at `when`: a plugin's folder or file of `kind`.
fn sample_binned(
    path: &str,
    bytes: u64,
    kind: mo_space::plugin_space::SpaceKind,
    when: i64,
) -> mo_space::clean::Removed {
    mo_space::clean::Removed {
        path: path.to_owned(),
        real: path.to_owned(),
        bytes,
        what: mo_space::clean::What::Plugin(kind),
        when,
        bin_id: Some(String::new()),
        binned: None,
        binned_zone: None,
        stamp: None,
    }
}

/// What the app keeps of what the made-up clean-ups in `list` (oldest first) put in the Recycle Bin, all of it on C:.
fn sample_kept(list: Vec<mo_app::binned::Binned>) -> mo_app::binned::Kept {
    use mo_app::binned::{Binned, Kept};
    let items = list.iter().map(|b| b.done.removed.len()).sum();
    let bytes = list.iter().map(Binned::bytes).sum();
    let since = list.iter().map(|b| b.when).min();
    let in_bin = mo_space::in_bin::InBin { items, bytes, since, drives: vec![("C:".to_owned(), bytes)], looked: true };
    Kept { seq: 1, list, in_bin, noted: true }
}

/// What Remove did with the made-up spare copies, at 15:20 on 4 Nov 2026 (UTC): every copy that can go was ticked, and
/// all went but Omnisphere's AAX copy, which changed since it was found. Then what it put in the Recycle Bin, with one
/// copy an older removal sent the evening before, oldest first.
fn sample_removed(found: &mo_app::space_spare::Found) -> (mo_app::space_spare::Removing, Vec<mo_app::binned::Binned>) {
    use mo_app::binned::Binned;
    use mo_space::clean::{Done, Problem};
    use mo_space::plugin_space::SpaceKind;
    const WHEN: i64 = 1_793_805_600;
    const BEFORE: i64 = 1_793_743_500;
    let mut removing = mo_app::space_spare::Removing::default();
    let picked = removing.ticked_copies(Some(found));
    removing.start(picked.clone());
    let stays = |c: &&mo_space::spare::SpareCopy| c.name == "Omnisphere" && c.format == "AAX";
    let done = Done {
        removed: picked
            .iter()
            .filter(|c| !stays(c))
            .map(|c| sample_binned(&c.path, c.bytes, SpaceKind::Program, WHEN))
            .collect(),
        problems: picked
            .iter()
            .filter(stays)
            .map(|c| Problem {
                path: c.path.clone(),
                why: "It changed since it was found (an update?), so it stays.".into(),
            })
            .collect(),
        went_with: Vec::new(),
    };
    removing.sent(WHEN, done.clone());
    let older = Done {
        removed: vec![sample_binned(
            r"C:\Program Files\Steinberg\VSTPlugins\Massive.dll",
            96 << 20,
            SpaceKind::Program,
            BEFORE,
        )],
        ..Done::default()
    };
    let spare = mo_app::space_spare::SPARE_COPIES;
    let list = vec![
        Binned { kind: spare.to_owned(), when: BEFORE, done: older, plugin: None },
        Binned { kind: spare.to_owned(), when: WHEN, done: Done { problems: Vec::new(), ..done }, plugin: None },
    ];
    (removing, list)
}

/// What Remove did with the made-up old copies, at 15:40 on 4 Nov 2026 (UTC): the ones ticked out of the box went, but
/// Kontakt's factory library, which an instrument was saved into since the look, stayed. Then what it put in the
/// Recycle Bin.
fn sample_old_removed(found: &mo_app::space_old::Found) -> (mo_app::space_old::Removing, Vec<mo_app::binned::Binned>) {
    use mo_app::binned::Binned;
    use mo_space::clean::{Done, Problem};
    use mo_space::old_copies::Leftover;
    use mo_space::plugin_space::SpaceKind;
    const WHEN: i64 = 1_793_806_800;
    let mut removing = mo_app::space_old::Removing::default();
    let picked = removing.ticked_copies(Some(found));
    removing.start(picked.clone());
    let stays = |l: &&Leftover| l.path.contains("Kontakt");
    let done = Done {
        removed: picked
            .iter()
            .filter(|l| !stays(l))
            .map(|l| sample_binned(&l.path, l.bytes, SpaceKind::Sounds, WHEN))
            .collect(),
        problems: picked
            .iter()
            .filter(stays)
            .map(|l| Problem {
                path: l.path.clone(),
                why: "Some of it is only in the old copy, newer there, or not the same as on the other drive: \
                      Instruments\\My Grand.nki. It may have been saved while the move ran, deleted from the other \
                      drive since, gone bad on the way, or open in a program now. Look before deleting it."
                    .into(),
            })
            .collect(),
        went_with: Vec::new(),
    };
    removing.sent(WHEN, done.clone());
    let old = mo_app::space_old::OLD_COPIES;
    (
        removing,
        vec![Binned { kind: old.to_owned(), when: WHEN, done: Done { problems: Vec::new(), ..done }, plugin: None }],
    )
}

/// Made-up old copies left after moving plugin folders to I:, written as on Windows, biggest first: two that can go,
/// the old place of the folder the moved folders live in, one that can't go yet, files a try kept aside (nothing to
/// check them against, so not ticked), and one with presets saved since the move (so it stays until you look).
fn sample_old_copies() -> mo_app::space_old::Found {
    use mo_space::after_move::OldHome;
    use mo_space::old_copies::{Left, Leftover, Reminder};
    const MB: u64 = 1 << 20;
    let moved = |day: u32| chrono::NaiveDate::from_ymd_opt(2026, 10, day).and_then(|d| d.and_hms_opt(14, 3, 0));
    let left = |path: &str, mb: u64, what: Left, now_in: &str, day: Option<u32>, stays: Option<&str>| Leftover {
        path: path.to_owned(),
        bytes: mb * MB,
        what,
        now_in: now_in.to_owned(),
        since: day.and_then(moved),
        stays: stays.map(str::to_owned),
        only: stays.is_none().then(Vec::new),
    };
    let stays = "It's the only copy the plugin could use now (the link or the copy on the other drive isn't as the move \
                 left it), so it stays.";
    let kept = OldHome {
        path: r"I:\Plugin Data\Spitfire Audio (kept from before)".to_owned(),
        bytes: 1_240 * MB,
        note: "replaced by linking again".to_owned(),
        copy: String::new(),
        kind: 2,
    };
    let saved = [r"Diva.data\Presets\Diva\Bass\Sub Wobble.h2p", r"Diva.data\Presets\Diva\Pads\Night Pad.h2p"];
    let leftovers = vec![
        left(
            r"C:\Program Files\Common Files\Spitfire Audio (old copy)",
            62_870,
            Left::OldCopy(0),
            r"I:\Plugin Data\Spitfire Audio",
            Some(26),
            None,
        ),
        left(
            r"C:\Users\Public\Documents\Native Instruments\Kontakt Factory Library (old copy)",
            9_420,
            Left::OldCopy(1),
            r"I:\Plugin Data\Kontakt Factory Library",
            Some(29),
            None,
        ),
        left(r"C:\Plugin Data", 5_210, Left::Behind(0), r"I:\Plugin Data", None, None),
        left(
            r"C:\ProgramData\Arturia (old copy)",
            4_300,
            Left::OldCopy(2),
            r"I:\Plugin Data\Arturia",
            Some(31),
            Some(stays),
        ),
        Leftover { only: None, ..left(&kept.path, 1_240, Left::Kept(kept.clone()), "", None, None) },
        Leftover {
            only: Some(saved.map(str::to_owned).to_vec()),
            ..left(
                r"C:\Users\Public\Documents\u-he (old copy)",
                717,
                Left::OldCopy(3),
                r"I:\Plugin Data\u-he",
                Some(30),
                None,
            )
        },
    ];
    let home = OldHome {
        path: r"C:\Plugin Data".to_owned(),
        bytes: 5_210 * MB,
        note: "its old place".to_owned(),
        copy: r"I:\Plugin Data".to_owned(),
        kind: 2,
    };
    mo_app::space_old::Found { reminder: Reminder { leftovers }, moved: Vec::new(), homes: vec![home] }
}

/// What Fix did to the made-up projects: GARAGE FOR BW 1's other saves on show with its older save ticked, and
/// Sketch 14's save not fixed (which takes its tick off, so Fix isn't pressed again for it by mistake).
fn sample_fixes() -> mo_app::space_bitwig::Fixes {
    use mo_app::space_bitwig::{Fixes, Fixing};
    let garage = r"K:\Bitwig Studio\Projects\GARAGE FOR BW 1";
    let sketch = r"K:\Bitwig Studio\Projects\Sketch 14\Sketch 14.bwproject";
    let mut fixes = Fixes::default();
    fixes.shown.insert(garage.into());
    fixes.ticks.insert(format!(r"{garage}\GARAGE FOR BW 1 v2.bwproject").into(), true);
    let why = "nothing was written: the project uses none of the other copies";
    fixes.done(sketch.into(), Fixing::Failed(why.to_owned()));
    fixes
}

/// Made-up fingerprints for the made-up sounds: sounds of a type, and of a length, sit near each other.
fn sample_fingerprints() -> mo_sounds::index::SoundIndex {
    let features = (1..=29i64)
        .map(|id| {
            let v: Vec<f32> =
                (0..16).map(|d| ((id * 7 + d * 3) % 5) as f32 + if d == id % 4 { 6.0 } else { 0.0 }).collect();
            (id, v)
        })
        .collect();
    mo_sounds::index::SoundIndex::build(&features, &Default::default(), 29)
}

/// Made-up sounds of many types, with a made-up waveform each, and a few plays for History.
fn sample_sounds() -> mo_ui::sounds::Library {
    use mo_sounds::{Sound, SoundAnalysis};
    // (folder, name, type, seconds, tempo, key)
    type Made = (&'static str, &'static str, &'static str, f64, Option<f64>, Option<(i32, bool)>);
    const SOUNDS: &[Made] = &[
        ("Drums", "Tight Kick 01", "Kick", 0.4, None, None),
        ("Drums", "Boom Kick", "Kick", 0.9, None, Some((9, true))),
        ("Drums", "Dusty Snare", "Snare", 0.5, None, None),
        ("Drums", "Rim Shot 3", "Snare", 0.2, None, None),
        ("Drums", "Closed Hat", "Hi-hat", 0.1, None, None),
        ("Drums", "Open Hat Long", "Hi-hat", 0.8, None, None),
        ("Loops", "Rolling Bass 140", "Bass", 6.9, Some(140.0), Some((9, true))),
        ("Bass", "Sub Drop", "Bass", 2.1, None, Some((4, true))),
        ("Bass", "Reese Stab", "Bass", 0.7, None, Some((2, true))),
        ("Drums", "Big Clap", "Clap", 0.6, None, None),
        ("Pads", "Glass Pad", "Pad", 8.0, None, Some((0, false))),
        ("Pads", "Warm Choir", "Pad", 12.4, None, Some((7, false))),
        ("Leads", "Saw Lead 7", "Lead", 1.2, None, Some((9, true))),
        ("Vox", "Hey Chop", "Vocal", 0.5, None, Some((4, true))),
        ("Vox", "Ooh Phrase 95", "Vocal", 5.1, Some(95.0), Some((2, false))),
        ("FX", "Riser 8 Bars", "FX", 13.7, Some(140.0), None),
        ("FX", "Tape Stop", "FX", 1.4, None, None),
        ("Perc", "Shaker Loop 120", "Perc", 4.0, Some(120.0), None),
        ("Perc", "Wood Block", "Perc", 0.3, None, Some((11, false))),
        ("Keys", "Rhodes Chord Am9", "Chord", 2.8, None, Some((9, true))),
        ("Keys", "Pluck Arp 128", "Pluck", 7.5, Some(128.0), Some((5, false))),
        ("Drums", "Crash Wide", "Cymbal", 3.2, None, None),
        ("Drums", "Low Tom", "Tom", 0.7, None, None),
        ("Keys", "Felt Piano C", "Keys", 4.4, None, Some((0, false))),
        ("Strings", "Long Strings Dm", "Strings", 9.9, None, Some((2, true))),
        ("Brass", "Horn Stab", "Brass", 0.9, None, Some((10, false))),
        ("Loops", "Break 172", "Drum loop", 5.6, Some(172.0), None),
        ("Loops", "Hats 128", "Drum loop", 7.5, Some(128.0), None),
        ("Misc", "Room Tone", "", 3.0, None, None),
    ];
    let day = |d: u32, h: u32| {
        chrono::NaiveDate::from_ymd_opt(2026, 10, d).and_then(|d| d.and_hms_opt(h, 15, 0)).unwrap_or_default()
    };
    let sounds = SOUNDS
        .iter()
        .enumerate()
        .map(|(i, &(folder, name, ty, seconds, bpm, key))| {
            let is_loop = bpm.is_some();
            // A made-up waveform: hits fall away fast, loops pulse on the beat, long sounds swell.
            let waveform: Vec<f32> = (0..160)
                .map(|x| {
                    let t = x as f32 / 160.0;
                    let wobble = 0.75 + 0.25 * ((x * 7 + i * 13) % 11) as f32 / 10.0;
                    let shape = if is_loop {
                        1.0 - (t * 8.0).fract() * 0.7
                    } else if seconds < 2.0 {
                        (-t * 6.0).exp()
                    } else {
                        (t * 3.0).min(1.0) * (1.0 - t * 0.5)
                    };
                    (shape * wobble).clamp(0.02, 1.0)
                })
                .collect();
            // The picked one sits deep in a pack, with folder names too long for a line: its card must wrap them.
            let path = if name == "Boom Kick" {
                format!(
                    r"D:\Samples\Packs\Vintage_Drum_Machines_Collection_Vol_2\Kicks_And_Bass_Drums\Processed\{name}.wav"
                )
            } else {
                format!(r"D:\Samples\{folder}\{name}.wav")
            };
            Sound {
                id: i as i64 + 1,
                path,
                error: None,
                analysis: Some(SoundAnalysis {
                    seconds,
                    is_loop,
                    bpm,
                    key_tonic: key.map(|k| k.0),
                    key_minor: key.is_some_and(|k| k.1),
                    waveform: waveform.into(),
                    ..SoundAnalysis::default()
                }),
                sound_type: if ty.is_empty() { mo_sounds::types::UNKNOWN.into() } else { ty.into() },
                type_source: if i % 5 == 0 { "learned".into() } else { "name".into() },
                type_confidence: 0.9,
                user_type: None,
                name_type: None,
                tags: if name == "Boom Kick" {
                    ["Dark", "Punchy", "Live set", "Sub heavy", "Analog", "Long tail"].map(String::from).to_vec()
                } else if i % 4 == 0 {
                    vec!["Dark".into()]
                } else {
                    Vec::new()
                },
                favourite: i % 6 == 1,
                hidden: false,
                rating: 0,
                plays: 0,
                // Spread over a few years, for the year strip.
                added: chrono::NaiveDate::from_ymd_opt(2026 - (i as i32 % 6), 10, 1 + (i as u32 % 28))
                    .and_then(|d| d.and_hms_opt(12, 15, 0))
                    .unwrap_or_default(),
                activity: Default::default(),
            }
        })
        .collect();
    let played = vec![(7, day(31, 22)), (13, day(31, 21)), (2, day(30, 18)), (11, day(28, 9)), (20, day(20, 14))];
    // A few made-up songs using some of them, for the Used column and Most liked.
    let usage = (1..=29)
        .filter(|id| id % 3 != 0)
        .map(|id: i64| {
            let projects = (id * 7 % 11) as u32;
            // Which of the made-up projects below: each a different one.
            let mut project_ids: Vec<i64> = (0..i64::from(projects)).map(|n| (id * 3 + n) % 12).collect();
            project_ids.sort_unstable();
            (id, mo_sounds::usage::Usage { projects, recent: projects / 2, years: vec![2025, 2026], project_ids })
        })
        .filter(|(_, u)| u.projects > 0)
        .collect();
    // Twelve made-up projects for the card's Used in list, saved a week or so apart (names invented here, none of
    // Solomon's: rule 4).
    let names = [
        "Night drive",
        "Sunday loop",
        "Glass rooms",
        "Low tide",
        "Paper planes",
        "Static bloom",
        "Late bus",
        "Copper sky",
        "Slow motion",
        "Bright side",
        "Field notes",
        "Neon rain",
    ];
    let now = chrono::Local::now().timestamp();
    let projects = (0i64..)
        .zip(names)
        .map(|(id, name)| {
            let (daw, ending) = if id % 3 == 2 { ("Bitwig Studio", "bwproject") } else { ("FL Studio", "flp") };
            let path = format!(r"C:\Music\{daw}\{name}.{ending}");
            (id, mo_library::ProjectFile { id, path, mtime: now - id * 9 * 86_400 })
        })
        .collect();
    let weights = mo_sounds::liking::LikedWeights::default();
    mo_ui::sounds::Library::new(sounds, played, &weights).with_usage(usage, projects, &weights)
}

/// A made-up library for the Songs page: names, plugins and dates invented here, none of Solomon's (rule 4).
fn sample_songs() -> mo_app::songs::Songs {
    use chrono::{Duration as Days, Local};
    use mo_library::Save;
    let now = Local::now().naive_local();
    let ago = |days: i64| now - Days::days(days);
    let save = |name: &str, daw: &str, days: i64| Save {
        name: name.into(),
        daw: daw.into(),
        folder: format!(r"C:\Music\{daw}"),
        path: format!(r"C:\Music\{daw}\{name}"),
        modified: ago(days),
        size: 1,
        ..Save::default()
    };
    let mut saves = vec![
        Save {
            tempo: Some(140.0),
            detected_key: "G minor".into(),
            rating: 4,
            favourite: true,
            ..save("Night Drive_14", "FL Studio", 1)
        },
        save("Night Drive_13", "FL Studio", 3),
        save("Night Drive_2", "FL Studio", 40),
        Save {
            tempo: Some(122.0),
            missing_samples: 3,
            status: "Mixing".into(),
            tags: vec!["dark".into(), "live set".into()],
            ..save("Glass Harbour", "Bitwig Studio", 4)
        },
        save("Glass Harbor", "Bitwig Studio", 300),
        Save { tempo: Some(92.5), musical_key: "D major".into(), rating: 3, ..save("Slow Orbit v3", "Studio One", 12) },
        save("Slow Orbit v2", "Studio One", 20),
        Save {
            tempo: Some(128.0),
            missing_plugins: 1,
            tags: vec!["dark".into()],
            ..save("Copper Lines", "FL Studio", 70)
        },
        Save { tempo: Some(174.0), rating: 5, status: "Finished".into(), ..save("Paper Moons_31", "FL Studio", 200) },
        Save { tempo: Some(110.0), ..save("Low Tide", "Bitwig Studio", 420) },
        Save {
            tempo: Some(136.0),
            status: "Released".into(),
            rating: 5,
            favourite: true,
            ..save("Fieldwork_9", "FL Studio", 800)
        },
        Save { tempo: Some(100.0), ..save("Untitled_4", "FL Studio", 1300) },
        Save { tempo: Some(85.0), ..save("Old Tapes", "FL Studio", 2200) },
    ];
    saves.sort_by_key(|s| std::cmp::Reverse(s.modified));
    // Each save its own id, as in the library (the rail's line finds a song by its save's id).
    for (id, save) in (1..).zip(&mut saves) {
        save.id = id;
    }
    let tags = vec!["dark".into(), "live set".into()];
    mo_app::songs::Songs::from_saves(&saves, &mo_library::SongChoices::default(), tags, 3, Some(false))
}

/// A made-up record for the first sample song's newest save.
fn sample_detail() -> mo_library::SaveDetail {
    use mo_library::{PluginKind, ProjectPlugin, ProjectSample};
    let plugin = |name: &str, vendor: &str, kind: PluginKind, installed: bool| ProjectPlugin {
        name: name.into(),
        vendor: vendor.into(),
        kind,
        format: "VST3".into(),
        path: String::new(),
        installed,
    };
    let sample =
        |path: &str, found: bool, unplayed: bool| ProjectSample { path: path.into(), found, unplayed, muted: false };
    mo_library::SaveDetail {
        plugins: vec![
            plugin("Night Pad", "Made Up Audio", PluginKind::Instrument, true),
            plugin("Glass Bass", "Pretend Instruments", PluginKind::Instrument, false),
            plugin("Room Echo", "Made Up Audio", PluginKind::Effect, true),
        ],
        samples: vec![
            sample(r"D:\Samples\Kicks\Kick 01.wav", false, false),
            sample(r"D:\Samples\Snares\Snare Tight.wav", true, false),
            sample(r"D:\Samples\FX\Riser Long.wav", true, true),
        ],
        tracks: vec!["Kick".into(), "Snare".into(), "Night Pad".into(), "Glass Bass".into()],
        details: vec![("Time signature".into(), "4/4".into()), ("FL Studio version".into(), "21.2.3".into())],
        ..mo_library::SaveDetail::default()
    }
}

/// Where the song panel's Show in folder and Older saves buttons are in the shots, beside the filter column. If
/// the page's layout moves them, the tooltip and menu shots come out the same as the plain panel, and mo-screens stops
/// with an error.
const SHOW_BUTTON: (f32, f32) = (925.0, 323.0);
/// Made-up songs and plugins for the Plugins page: some missing, some unused for years, one measured library.
fn sample_plugins() -> (mo_app::plugins::Plugins, Vec<mo_library::plugins::ScannedPlugin>) {
    use mo_app::plugins::{Plugins, Read, SaveBrief, SongBrief};
    use mo_library::PluginKind::{Effect, Instrument};
    use mo_library::plugins::{PluginUse, ScanCounts, StoredFolder, StoredSpace};
    let day = |y: i32, m: u32| {
        chrono::NaiveDate::from_ymd_opt(y, m, 12).and_then(|d| d.and_hms_opt(20, 0, 0)).unwrap_or_default()
    };
    let songs: [(&str, &str, i32, u32, u32); 8] = [
        ("Didums", "FL Studio", 2026, 10, 14),
        ("Night Drive", "Bitwig", 2026, 9, 6),
        ("garage sept", "FL Studio", 2025, 9, 9),
        ("Odd One", "FL Studio", 2024, 3, 3),
        ("Lowlands", "Studio One", 2023, 6, 4),
        ("Basement 4", "FL Studio", 2021, 2, 2),
        ("Tape Loops", "FL Studio", 2019, 11, 5),
        ("Old Rave", "FL Studio", 2017, 5, 1),
    ];
    let mut saves = Vec::new();
    let mut song_of = std::collections::HashMap::new();
    let mut briefs = Vec::new();
    let mut id = 0i64;
    for (i, (name, daw, y, m, n)) in songs.iter().enumerate() {
        let first = id + 1;
        for k in 0..*n {
            id += 1;
            let save_name = if k == 0 { (*name).to_owned() } else { format!("{name}_{}", n - k) };
            saves.push(SaveBrief {
                id,
                name: save_name.clone(),
                path: format!(r"I:\Projects\{name}\{save_name}.flp"),
                daw: (*daw).into(),
                modified: day(*y - i32::from(k > 2), m.saturating_sub(k).max(1)),
            });
            song_of.insert(id, i);
        }
        briefs.push(SongBrief { name: (*name).into(), latest: first });
    }
    // Which songs use which plugin: (name, vendor, format, kind, installed, songs by number).
    type Uses<'a> = (&'a str, &'a str, &'a str, mo_library::PluginKind, bool, &'a [usize]);
    let table: [Uses; 18] = [
        ("Serum", "Xfer Records", "VST3", Instrument, true, &[0, 1, 2, 3, 5, 6]),
        ("Pro-Q 3", "FabFilter", "VST3", Effect, true, &[0, 1, 2, 3, 4, 5, 6, 7]),
        ("Valhalla VintageVerb", "Valhalla DSP", "VST3", Effect, true, &[0, 2, 3, 5]),
        ("Kontakt 7", "Native Instruments", "VST3", Instrument, true, &[7]),
        ("Massive", "Native Instruments", "VST2", Instrument, false, &[6]),
        ("Nexus", "reFX", "VST2", Instrument, false, &[3, 5, 7]),
        ("Ozone 9", "iZotope", "VST3", Effect, false, &[3, 5]),
        ("Sylenth1", "LennarDigital", "VST2", Instrument, false, &[7]),
        ("Diva", "u-he", "CLAP", Instrument, true, &[1]),
        ("Phase Plant", "Kilohearts", "CLAP", Instrument, true, &[0, 1]),
        ("Decapitator", "Soundtoys", "VST3", Effect, true, &[0, 2, 3]),
        ("Fruity Reverb 2", "Image-Line", "Native", Effect, true, &[0, 2, 3, 5, 6, 7]),
        ("Sytrus", "Image-Line", "Native", Instrument, true, &[2, 6]),
        ("Polymer", "Bitwig", "Native", Instrument, true, &[1]),
        ("Pro Compressor", "PreSonus", "Native", Effect, true, &[4]),
        ("CLA-76 Stereo", "Waves", "VST3", Effect, true, &[4, 5]),
        ("RC-20 Retro Color", "XLN Audio", "VST3", Effect, false, &[5]),
        ("Omnisphere", "Spectrasonics", "VST2", Instrument, true, &[6, 7]),
    ];
    let mut uses = Vec::new();
    for (name, vendor, format, kind, installed, used) in table {
        for &song in used {
            for s in saves.iter().filter(|s| song_of.get(&s.id) == Some(&song)) {
                uses.push(PluginUse {
                    save: s.id,
                    name: name.into(),
                    key: mo_library::read::name_key(name),
                    vendor: vendor.into(),
                    kind,
                    format: format.into(),
                    installed,
                    daw: s.daw.clone(),
                });
            }
        }
    }
    const MB: u64 = 1 << 20;
    const GB: u64 = 1 << 30;
    let space = |name: &str, folders: &[(&str, u8, u64, &[&str])]| StoredSpace {
        key: mo_library::read::name_key(name),
        name: name.into(),
        vendor: String::new(),
        folders: folders
            .iter()
            .map(|(p, k, b, w)| StoredFolder {
                path: (*p).into(),
                kind: *k,
                bytes: *b,
                shared_with: w.iter().map(|s| (*s).to_owned()).collect(),
                added: false,
            })
            .collect(),
        measured: Some(day(2026, 10)),
    };
    let spaces = vec![
        space(
            "Serum",
            &[
                (r"C:\Program Files\Common Files\VST3\Serum.vst3", 0, 40 * MB, &[]),
                (r"C:\Users\silki\Documents\Xfer\Serum Presets", 1, 2 * GB, &[]),
            ],
        ),
        space("FabFilter Pro-Q 3", &[(r"C:\Program Files\Common Files\VST3\FabFilter Pro-Q 3.vst3", 0, 60 * MB, &[])]),
        space(
            "Kontakt 7",
            &[
                (r"C:\Program Files\Common Files\VST3\Kontakt 7.vst3", 0, 600 * MB, &[]),
                (r"K:\Native Instruments\Kontakt Libraries", 2, 182 * GB, &[]),
                (r"C:\Users\Public\Documents\Native Instruments", 1, 3 * GB, &["kontakt8"]),
            ],
        ),
        space(
            "Omnisphere",
            &[
                (r"C:\Program Files\VSTPlugins\Omnisphere.dll", 0, 300 * MB, &[]),
                (r"C:\ProgramData\Spectrasonics\STEAM", 2, 64 * GB, &[]),
            ],
        ),
        space(
            "Arturia Pigments",
            &[
                (r"C:\Program Files\Common Files\VST3\Pigments.vst3", 0, 200 * MB, &[]),
                (r"C:\ProgramData\Arturia\Presets", 1, 4 * GB, &[]),
            ],
        ),
    ];
    let read = Read {
        uses,
        saves,
        song_of,
        songs: briefs,
        spaces,
        installed_names: vec!["Ozone 11".into(), "Serum".into(), "Kontakt 7".into(), "Arturia Pigments".into()],
        scans: ScanCounts { files: 214, plugins: 380, failed: 3 },
        looked_for_installed: true,
        unused_years: 2,
        needed_count: 6,
        saved_fixes: Vec::new(),
        gone: Vec::new(),
    };
    let copies = vec![
        mo_library::plugins::ScannedPlugin {
            path: r"C:\Program Files\Common Files\VST3\Serum.vst3".into(),
            format: "VST3".into(),
            id: "56535458667358736572756D00000000".into(),
            name: "Serum".into(),
            vendor: "Xfer Records".into(),
            version: "1.368".into(),
            kind: "instrument".into(),
            shell: String::new(),
            parameters: Some(2623),
            error: String::new(),
        },
        mo_library::plugins::ScannedPlugin {
            path: r"C:\Program Files\VSTPlugins\Serum_x64.dll".into(),
            format: "VST2".into(),
            id: "XfsX".into(),
            name: "Serum_x64".into(),
            vendor: "Xfer Records".into(),
            version: "1368".into(),
            kind: "instrument".into(),
            ..Default::default()
        },
    ];
    (Plugins::new(read, day(2026, 11)), copies)
}

/// The Backup page's Remove older versions button, and how far the page is turned down to show the question it asks.
const REMOVE_OLDER_BUTTON: (f32, f32) = (1150.0, 845.0);
/// The Presets page's Plugin menu: the first thing in its filter column.
const PLUGIN_CHIP: (f32, f32) = (288.0, 175.0);
const PAGE_DOWN: f32 = 300.0;

/// Where the wheel turns the song panel down in the narrowest window, and by how much: on its status line, which
/// doesn't scroll on its own, as the lists and the arrangement in it do.
const NARROW_PANEL_WHEEL: (f32, f32, f32) = (760.0, 404.0, -2.0 * PAGE_DOWN);
const OLDER_BUTTON: (f32, f32) = (1205.0, 283.0);
/// Space's Old copies after moving folders line, the page at its top, and a place on the page with nothing on it (right
/// of its name, left of a card).
const SPACE_OLD_LINE: (f32, f32) = (400.0, 516.0);
const SPACE_EMPTY: (f32, f32) = (1000.0, 60.0);
/// Where the wheel turns a page's card (Space's, or Sounds') down to its end, and by more than enough: on its parts.
const SPACE_CARD_WHEEL: (f32, f32, f32) = (1240.0, 600.0, -10.0 * PAGE_DOWN);
/// The second line on the galaxy's Swept card, after the sweep.
const SWEPT_LINE: (f32, f32) = (760.0, 410.0);
/// The Lock chip on the kit's second pad (Snare).
const SNARE_LOCK: (f32, f32) = (846.0, 333.0);
/// A point on a clip in the arrangement, from the view's top left, for the hover and zoom shots.
const ARRANGEMENT_CLIP: (f32, f32) = (480.0, 180.0);
/// Inside the first tile of the Sounds grid, and the first line of its List, at their top (x, y, width, height in
/// pixels): all one colour there, the list shows nothing.
const FIRST_TILE: (u32, u32, u32, u32) = (250, 320, 200, 70);
const FIRST_LINE: (u32, u32, u32, u32) = (250, 335, 400, 30);
/// A point on a Sounds tile to right-click, and where a right-click menu's lines start under the mouse (its card's
/// padding) and how tall each is (studio.slint's MenuItems and MenuLine).
const MENU_TILE: (f32, f32) = (560.0, 345.0);
const MENU_LINES_TOP: f32 = 4.0;
const MENU_LINE: f32 = 32.0;
/// The Sounds card's Add a tag box, the card turned down to its end (with a sound's six tags over it).
const TAG_BOX: (f32, f32) = (1130.0, 534.0);
/// The Songs, Presets, MIDI and Plugins lists in the short window: left of the activity cards, which move on their own.
const SHORT_LIST: (u32, u32, u32, u32) = (230, 200, 760, 400);
/// The mouse over each of those lists (Songs' is narrow beside its panel; Plugins' starts low), for the wheel.
/// The second line of a song's Saves tab (an older save), at 1440 x 900.
const SECOND_SAVE: (f32, f32) = (1025.0, 484.0);
const SHORT_LIST_WHEEL: (f32, f32) = (600.0, 540.0);
/// A short wheel turn: less than those lists show, so turning back down goes through lines the list has made.
const SHORT_TURN: f32 = 100.0;

/// Moves the mouse to a point in the window (a point outside it moves it away).
fn hover(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let event = if x < 0.0 {
        slint::platform::WindowEvent::PointerExited
    } else {
        slint::platform::WindowEvent::PointerMoved { position }
    };
    window.dispatch_event(event);
}

/// Settings' tabs, from made-up folders and sound outputs: General in Graphite and Paper, then smaller, then Folders
/// and Developer.
fn settings_shots(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    graphite: &Look,
    shots: &mut Vec<(String, Vec<u8>)>,
) {
    app.set_place(Place::Settings.index() as i32);
    let outputs = ["Speakers (Realtek)".to_owned(), "Focusrite USB".to_owned()];
    mo_ui::settings::show_outputs(app, &outputs, "Sounds play through Speakers (Realtek).");
    let data = app.global::<mo_ui::SettingsData>();
    data.invoke_tab_picked(0);
    capture(window);
    shots.push(("settings-general.png".into(), capture(window)));
    scroll(window, 900.0, 500.0, -PAGE_DOWN);
    shots.push(("settings-general-lower.png".into(), capture(window)));
    scroll(window, 900.0, 500.0, -PAGE_DOWN);
    shots.push(("settings-about.png".into(), capture(window)));
    scroll(window, 900.0, 500.0, 2.0 * PAGE_DOWN);
    data.invoke_theme_picked(4);
    data.invoke_accent_picked(2);
    shots.push(("settings-general-paper.png".into(), capture(window)));
    data.invoke_back_to_normal();
    mo_ui::apply_look(app, graphite);
    data.invoke_size_picked(1);
    capture(window);
    shots.push(("settings-general-80.png".into(), capture(window)));
    data.invoke_size_picked(3);
    let moved = mo_space::folder_move::MovedFolder {
        from: "C:\\Users\\Solomon\\Documents\\Native Instruments".into(),
        to: "I:\\Plugin data\\Native Instruments".into(),
        old_copy: String::new(),
        bytes: 0,
        moved: String::new(),
        key: String::new(),
        kind: 2,
        shared_with: Vec::new(),
    };
    mo_ui::settings::show_folders(
        app,
        Ok(mo_app::settings::folder_groups(
            vec!["I:\\Fl Studio Data\\FL Studio\\Projects".into(), "K:\\Bitwig Studio\\Projects".into()],
            vec!["I:\\Samples".into(), "G:\\Splice".into()],
            vec!["L:\\New Midi".into()],
            vec![],
            vec!["C:\\Program Files\\Common Files\\VST3".into(), "C:\\Program Files\\Common Files\\CLAP".into()],
            &[moved],
        )),
    );
    // Exports go into a folder chosen on another drive (an invented one).
    let exports = r"I:\Music Organiser Exports";
    mo_ui::settings::show_exports(
        app,
        &mo_app::settings::exports_card(Some(exports), &Ok(PathBuf::from(exports)), true),
    );
    data.invoke_tab_picked(1);
    capture(window);
    shots.push(("settings-folders.png".into(), capture(window)));
    // The Exports card, at the bottom of the tab.
    scroll(window, 900.0, 500.0, -4.0 * PAGE_DOWN);
    shots.push(("settings-folders-exports.png".into(), capture(window)));
    scroll(window, 900.0, 500.0, 4.0 * PAGE_DOWN);
    data.invoke_tab_picked(2);
    mo_ui::settings::show_bitwig(
        app,
        mo_app::settings::bitwig_status(&Ok(mo_dawlink::bitwig::Hello {
            link: 3,
            product: "Bitwig Studio".into(),
            version: "6.1.5".into(),
            project: "Night drive".into(),
            modified: true,
            changes: true,
            runtime: "java".into(),
            extension: "3.18".into(),
            loopback: Some(true),
            ..Default::default()
        })),
    );
    // A Bitwig Studio folder chosen on another drive (its Use Documents shows): only the made-up settings change.
    mo_ui::settings::remember(mo_app::app_settings::BITWIG_FOLDER_KEY, r"K:\Bitwig Studio");
    mo_ui::settings::show_bitwig_extension(
        app,
        mo_dawlink::scripts::ScriptState::Outdated,
        "3.19",
        vec![std::path::PathBuf::from(r"K:\Bitwig Studio\Extensions")],
    );
    mo_ui::settings::show_fl(
        app,
        mo_dawlink::scripts::ScriptState::Outdated,
        vec![std::path::PathBuf::from(r"I:\Fl Studio Data\FL Studio\Settings\Hardware\Music Organiser")],
    );
    capture(window);
    shots.push(("settings-daws.png".into(), capture(window)));
    // No Bitwig Studio folder in Documents and none chosen: the card says to choose it (the 10 Nov review's X3-5).
    mo_ui::settings::remember(mo_app::app_settings::BITWIG_FOLDER_KEY, "");
    mo_ui::settings::show_bitwig_extension(app, mo_dawlink::scripts::ScriptState::NotInstalled, "3.19", Vec::new());
    capture(window);
    shots.push(("settings-daws-no-folder.png".into(), capture(window)));
    data.invoke_tab_picked(3);
    capture(window);
    shots.push(("settings-shortcuts.png".into(), capture(window)));
    data.invoke_tab_picked(4);
    capture(window);
    shots.push(("settings-developer.png".into(), capture(window)));
    data.invoke_tab_picked(0);
}

/// The mouse wheel turned over a point: a negative `dy` moves the page up, to show what's below.
fn scroll(window: &MinimalSoftwareWindow, x: f32, y: f32, dy: f32) {
    let position = slint::LogicalPosition::new(x, y);
    window.dispatch_event(slint::platform::WindowEvent::PointerScrolled { position, delta_x: 0.0, delta_y: dy });
}

/// A page's card (Space's, or Sounds') turned down to its end (`top` is how it looked before; `name` names the shot),
/// then back to its top for the shots that follow. Err when the wheel moved nothing: is it still on the card?
fn card_down(window: &MinimalSoftwareWindow, top: &[u8], name: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let (x, y, turn) = SPACE_CARD_WHEEL;
    scroll(window, x, y, turn);
    let down = capture(window);
    hover(window, -1.0, -1.0);
    scroll(window, x, y, -turn);
    hover(window, -1.0, -1.0);
    if down == top {
        return Err(format!("{name}: the wheel at {x}, {y} moved nothing; is it still on the card?").into());
    }
    Ok(down)
}

/// A right click at a point in the window.
fn right_click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Right;
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
}

/// A left click at a point in the window.
fn click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Left;
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
}

/// The box (x, y, width, height, in pixels) round every pixel that differs between two pictures of the window, or
/// None when none does.
fn changed_box(a: &[u8], b: &[u8]) -> Option<(i32, i32, i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for (i, (pa, pb)) in a.as_chunks::<3>().0.iter().zip(b.as_chunks::<3>().0).enumerate() {
        if pa != pb {
            let (x, y) = ((i as u32 % WIDTH) as i32, (i as u32 / WIDTH) as i32);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    (x1 >= x0).then_some((x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// Lays the card that follows the mouse over a picture of the window, with the mouse's point at `at`: as it's drawn
/// over every app while something is dragged out, a moment after it was picked up.
fn with_card(mut pixels: Vec<u8>, payload: &mo_app::drag_out::Payload, look: &Look, at: (i32, i32)) -> Vec<u8> {
    let start = std::time::Instant::now();
    let mut flight = mo_app::drag_card::Flight::new(payload, look, mo_ui::drag::card_fonts(look.theme.font), start);
    let Some(card) = flight.frame(start + Duration::from_millis(400), 1.0) else { return pixels };
    // On a screen the card goes past the window's edge; here it's kept inside the picture.
    let at = (
        at.0.min(WIDTH as i32 - (card.width as i32 - card.hot_x)),
        at.1.min(HEIGHT as i32 - (card.height as i32 - card.hot_y)),
    );
    for (row, line) in card.rgba.chunks_exact(card.width as usize * 4).enumerate() {
        for (column, p) in line.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (at.0 - card.hot_x + column as i32, at.1 - card.hot_y + row as i32);
            if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
                continue;
            }
            let under = (y as usize * WIDTH as usize + x as usize) * 3;
            let see_through = 255 - u32::from(p[3]);
            for (c, &over) in pixels.get_mut(under..under + 3).unwrap_or_default().iter_mut().zip(p) {
                // The card's colours are already times its alpha.
                *c = (u32::from(over) + (u32::from(*c) * see_through + 127) / 255).min(255) as u8;
            }
        }
    }
    pixels
}

/// The song panel on the song at `row` of the sample library, on `tab`, with the sample arrangement in it.
fn show_song_panel(app: &mo_ui::AppWindow, row: usize, tab: Tab) {
    mo_ui::songs::show_picked(app, row, sample_detail(), tab);
    mo_ui::arrangement::show_preview(app, mo_ui::arrangement::sample_preview());
    mo_ui::arrangement::show_peaks(app, mo_ui::arrangement::sample_peaks());
}

/// In the narrowest window, another song picked while the song panel is turned down opens at its top, at the new
/// song's name: the wheel turned back up there moves nothing. The first song is picked again after, as it was.
fn opens_at_top(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), Box<dyn std::error::Error>> {
    let (x, y, turn) = NARROW_PANEL_WHEEL;
    window.set_size(PhysicalSize::new(NARROW, HEIGHT));
    let top = capture(window);
    scroll(window, x, y, turn);
    let turned = capture(window);
    show_song_panel(app, 1, Tab::Overview);
    let picked = capture(window);
    scroll(window, x, y, -turn);
    let up = capture(window);
    // The mouse moved away, as the wheel left it on whatever is there now (a button, whose tip would stay up).
    hover(window, -1.0, -1.0);
    show_song_panel(app, 0, Tab::Overview);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    if turned == top {
        return Err(
            format!("song panel: the wheel at {x}, {y} moved nothing; does it still land on what scrolls?").into()
        );
    }
    if up != picked {
        return Err(
            "song panel: another song picked while it was turned down kept its place; it should open at its top".into(),
        );
    }
    Ok(())
}

/// The window at the narrowest the app allows, saved at that width as `name` in `out`; the window is set back after.
fn save_narrow(
    window: &MinimalSoftwareWindow,
    out: &std::path::Path,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    save_narrow_scrolled(window, out, name, None)
}

/// [`save_narrow`], with the mouse wheel first turned at a point of the narrow window when `wheel` says where and by
/// how much (x, y, turn; a negative turn shows what's below), and turned back after, the mouse then moved away. It
/// stops with an error if the turn moves nothing, so a point that no longer lands on what scrolls can't pass unseen.
fn save_narrow_scrolled(
    window: &MinimalSoftwareWindow,
    out: &std::path::Path,
    name: &str,
    wheel: Option<(f32, f32, f32)>,
) -> Result<(), Box<dyn std::error::Error>> {
    window.set_size(PhysicalSize::new(NARROW, HEIGHT));
    let mut narrow = capture(window);
    if let Some((x, y, turn)) = wheel {
        scroll(window, x, y, turn);
        let turned = capture(window);
        scroll(window, x, y, -turn);
        hover(window, -1.0, -1.0);
        if turned == narrow {
            window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
            return Err(
                format!("{name}: the wheel at {x}, {y} moved nothing; does it still land on what scrolls?").into()
            );
        }
        narrow = turned;
    }
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    let row = WIDTH as usize * 3;
    let narrow: Vec<u8> = narrow.chunks(row).flat_map(|line| &line[..NARROW as usize * 3]).copied().collect();
    let path = out.join(name);
    image::save_buffer(&path, &narrow, NARROW, HEIGHT, image::ExtendedColorType::Rgb8)?;
    println!("{}", path.display());
    Ok(())
}

/// Lets hover and slide-in animations finish, then draws the window.
fn capture(window: &MinimalSoftwareWindow) -> Vec<u8> {
    capture_after(window, Duration::from_millis(400))
}

/// Draws the window once its timers and animations have run for `wait`.
fn capture_after(window: &MinimalSoftwareWindow, wait: Duration) -> Vec<u8> {
    capture_sized(window, (WIDTH, HEIGHT), wait)
}

/// [`capture_after`] for a window of another size (`size` is the size it was set to: across, down).
fn capture_sized(window: &MinimalSoftwareWindow, size: (u32, u32), wait: Duration) -> Vec<u8> {
    let start = std::time::Instant::now();
    while start.elapsed() < wait {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(Duration::from_millis(40));
    }
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut buffer = vec![Rgb8Pixel::default(); (size.0 * size.1) as usize];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, size.0 as usize);
    });
    buffer.iter().flat_map(|p| [p.r, p.g, p.b]).collect()
}

fn slug(name: &str) -> String {
    name.to_lowercase().replace(' ', "-")
}
