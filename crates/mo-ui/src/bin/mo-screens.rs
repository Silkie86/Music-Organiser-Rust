//! Draws the app's pages to PNG files without opening a window, so every pull request can show what it changed before
//! anyone builds it. Uses Slint's software renderer, so it runs the same on a CI machine with no screen.
//!
//! `mo-screens [folder]` writes one picture per place in Graphite, then the Songs page in every theme. The folder
//! defaults to `screenshots`. Then the shared parts on one page, and the parts over every page: activity cards, the key
//! sheet and the F12 note.

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
    // The roll with each note's name on it (it grows so each row has room).
    app.global::<mo_ui::MidiData>().set_roll_names(true);
    shots.push(("midi-note-names.png".into(), capture(&window)));
    app.global::<mo_ui::MidiData>().set_roll_names(false);
    mo_ui::midi::run_action(&app, "midi.moreLike");
    shots.push(("midi-more-like.png".into(), capture(&window)));
    mo_ui::midi::run_action(&app, "midi.back");
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
    // The old copies card: what moving plugin folders to another drive left, biggest first, with one that can't go yet.
    mo_ui::space::show_sample(&app, sample_space(), mo_app::space_old::OLD_COPIES);
    capture(&window);
    shots.push(("space-old.png".into(), capture(&window)));
    // The spare copies card: AAX copies (Pro Tools isn't installed) and VST2 copies whose VST3 your saves use, by
    // format, with one that can't go.
    mo_ui::space::show_sample(&app, sample_space(), mo_app::space_spare::SPARE_COPIES);
    capture(&window);
    let top = capture(&window);
    shots.push(("space-spare.png".into(), top.clone()));
    // The card turned down to its end: the VST2 copies, in two folders, each with its own, and the one that can't go
    // (its last lines once couldn't be scrolled to).
    let (x, y, turn) = SPARE_CARD_WHEEL;
    scroll(&window, x, y, turn);
    let down = capture(&window);
    hover(&window, -1.0, -1.0);
    if down == top {
        return Err(
            format!("space-spare-down.png: the wheel at {x}, {y} moved nothing; is it still on the card?").into()
        );
    }
    shots.push(("space-spare-down.png".into(), down));
    // Back to the card's top, for the shots that follow.
    scroll(&window, x, y, -turn);
    hover(&window, -1.0, -1.0);
    // One AAX copy unticked: its format's tick shows a dash, and Remove frees the rest.
    let spare = mo_app::space_spare::SPARE_COPIES;
    let mut removing = mo_app::space_spare::Removing::default();
    removing.tick(&[PathBuf::from(SAMPLE_UNTICKED)], false);
    mo_ui::space::show_sample(&app, mo_ui::space::Sample { removing, ..sample_space() }, spare);
    capture(&window);
    shots.push(("space-spare-ticks.png".into(), capture(&window)));
    // After Remove: what was freed, each copy in the Trash (the Recycle Bin on Windows) or why it stayed, and Put back.
    // The Recycle Bin line under Also taking room shows what the app put there.
    let (removing, kept) = sample_removed(&sample_spare_copies());
    let removed = mo_ui::space::Sample { removing, kept: Some(kept.clone()), ..sample_space() };
    mo_ui::space::show_sample(&app, removed, spare);
    capture(&window);
    shots.push(("space-spare-removed.png".into(), capture(&window)));
    // The Recycle Bin card: each clean-up still there, newest first, with its Put back; the older one's put back left
    // a copy behind, which says why.
    let mut put_backs = mo_app::space_bin::PutBacks::default();
    let stuck = mo_space::clean::Problem {
        path: kept.list[0].done.removed[0].path.clone(),
        why: "It couldn't be put back; it's still in the Recycle Bin.".to_owned(),
    };
    put_backs.done(spare, kept.list[0].when, vec![stuck]);
    let (removing, _) = sample_removed(&sample_spare_copies());
    let bin = mo_ui::space::Sample { removing, kept: Some(kept), put_backs, ..sample_space() };
    mo_ui::space::show_sample(&app, bin, mo_app::space_bin::RECYCLE_BIN);
    capture(&window);
    shots.push(("space-bin.png".into(), capture(&window)));
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
    // Each tab in the narrowest window, where the panel is at its narrowest: its rows wrap, so nothing runs past its
    // edge. On Saves, each line's buttons only show with room for them and the save's name.
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

    // Activity cards: one going, one that can't say how far it is yet, one done and one failed, plus one put away.
    // The one going has a title too long for a line, so its card must wrap it.
    let going = feed.start(mo_app::space_bitwig::LOOKING, "");
    going.progress(0.4, Some("148 of 371 projects"));
    let _starting = feed.start("Analysing sounds", "Getting ready");
    feed.start("Finding missing samples", "").done("All 31 found");
    feed.start("Copying Bitwig template", "").failed("The template folder isn't there any more");
    let hidden = feed.start("Measuring plugin folders", "");
    feed.dismiss(hidden.id());
    mo_ui::cards::refresh(&app, &feed);
    shots.push(("cards.png".into(), capture(&window)));
    app.set_cards(Default::default());
    app.set_put_away(0);

    // Tooltips: the mouse resting on a rail line, the song panel's Show in folder button and a song's date.
    show_song_panel(&app, 0, Tab::Overview);
    for (name, x, y) in [
        ("tip-rail.png", 60.0, 125.0),
        ("tip-button.png", SHOW_BUTTON.0, SHOW_BUTTON.1),
        ("tip-date.png", 530.0, 286.0),
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
    Ok(())
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
    finder::close(app);
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
    // The first tile's liked bar, saying why (its note comes up a moment after the mouse stops on it).
    hover(window, LIKED_BAR.0, LIKED_BAR.1);
    let hovered = capture_after(window, Duration::from_millis(80));
    let tip = capture_after(window, Duration::from_millis(750));
    if tip == hovered {
        return Err(format!("sounds-liked-tip.png: no tip came up at {}, {}", LIKED_BAR.0, LIKED_BAR.1));
    }
    shots.push(("sounds-liked-tip.png".into(), tip));
    hover(window, -1.0, -1.0);
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
    app.set_place(Place::Songs.index() as i32);
    Ok(())
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
    use mo_space::app_files::{AppFilesPart, part_of};
    const MB: u64 = 1 << 20;
    let part = |(name, what): (&'static str, &'static str), bytes: u64| AppFilesPart { name, what, bytes };
    vec![
        AppFilesPart { name: "Your library", what: "Projects, plugins, samples, tags and ratings", bytes: 6250 * MB },
        AppFilesPart { name: "Sound Lab", what: "Every sound's details, likes and look-alikes", bytes: 2460 * MB },
        part(part_of("models", true), 310 * MB),
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

/// The Space page's made-up finds: the drives, the app's files, Bitwig copies, old copies and spare plugin copies.
fn sample_space() -> mo_ui::space::Sample {
    mo_ui::space::Sample {
        drives: sample_drives(),
        parts: sample_app_files(),
        bitwig: sample_bitwig_copies(),
        old: sample_old_copies(),
        spare: sample_spare_copies(),
        ..Default::default()
    }
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

/// What Remove did with the made-up spare copies, at 15:20 on 4 Nov 2026 (UTC): every copy that can go was ticked, and
/// all went but Omnisphere's AAX copy, which changed since it was found. Then what the app keeps of what it put in the
/// Recycle Bin: that, and one copy an older removal sent the evening before.
fn sample_removed(found: &mo_app::space_spare::Found) -> (mo_app::space_spare::Removing, mo_app::binned::Kept) {
    use mo_app::binned::{Binned, Kept};
    use mo_space::clean::{Done, Problem, Removed, What};
    use mo_space::in_bin::InBin;
    use mo_space::plugin_space::SpaceKind;
    const WHEN: i64 = 1_793_805_600;
    const BEFORE: i64 = 1_793_743_500;
    let removed = |path: &str, bytes: u64, when: i64| Removed {
        path: path.to_owned(),
        real: path.to_owned(),
        bytes,
        what: What::Plugin(SpaceKind::Program),
        when,
        bin_id: Some(String::new()),
        binned: None,
        binned_zone: None,
    };
    let mut removing = mo_app::space_spare::Removing::default();
    let picked = removing.ticked_copies(Some(found));
    removing.start(picked.clone());
    let stays = |c: &&mo_space::spare::SpareCopy| c.name == "Omnisphere" && c.format == "AAX";
    let done = Done {
        removed: picked.iter().filter(|c| !stays(c)).map(|c| removed(&c.path, c.bytes, WHEN)).collect(),
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
        removed: vec![removed(r"C:\Program Files\Steinberg\VSTPlugins\Massive.dll", 96 << 20, BEFORE)],
        ..Done::default()
    };
    let list = vec![
        Binned { kind: mo_app::space_spare::SPARE_COPIES.to_owned(), when: BEFORE, done: older },
        Binned {
            kind: mo_app::space_spare::SPARE_COPIES.to_owned(),
            when: WHEN,
            done: Done { problems: Vec::new(), ..done },
        },
    ];
    let items = list.iter().map(|b| b.done.removed.len()).sum();
    let bytes = list.iter().map(Binned::bytes).sum();
    let in_bin = InBin { items, bytes, since: Some(BEFORE), drives: vec![("C:".to_owned(), bytes)], looked: true };
    (removing, Kept { seq: 1, list, in_bin, noted: true })
}

/// Made-up old copies left after moving plugin folders to I:, written as on Windows, biggest first: two that can go,
/// the old place of the folder the moved folders live in, and one that can't go yet.
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
    };
    let stays = "It's the only copy the plugin could use now (the link or the copy on the other drive isn't as the move \
                 left it), so it stays.";
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
            (id, mo_sounds::usage::Usage { projects, recent: projects / 2, years: vec![2025, 2026] })
        })
        .filter(|(_, u)| u.projects > 0)
        .collect();
    let weights = mo_sounds::liking::LikedWeights::default();
    mo_ui::sounds::Library::new(sounds, played, &weights).with_usage(usage, &weights)
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
    let sample = |path: &str, found: bool, unplayed: bool| ProjectSample { path: path.into(), found, unplayed };
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

/// Where the song panel's Show in folder and Older saves buttons are in the shots, below the two lines of filters. If
/// the page's layout moves them, the tooltip and menu shots come out the same as the plain panel, and mo-screens stops
/// with an error.
const SHOW_BUTTON: (f32, f32) = (1140.0, 368.0);
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
const REMOVE_OLDER_BUTTON: (f32, f32) = (1127.0, 820.0);
/// The Presets page's Plugin chip: the first in its filter row, so the other chips' words don't move it.
const PLUGIN_CHIP: (f32, f32) = (262.0, 156.0);
const PAGE_DOWN: f32 = 300.0;

/// Where the wheel turns the song panel down in the narrowest window, and by how much: on its status line, which
/// doesn't scroll on its own, as the lists and the arrangement in it do.
const NARROW_PANEL_WHEEL: (f32, f32, f32) = (760.0, 404.0, -2.0 * PAGE_DOWN);
const OLDER_BUTTON: (f32, f32) = (1005.0, 368.0);
/// Where the wheel turns the Space page's card down to its end, and by more than enough: on its list of parts.
const SPARE_CARD_WHEEL: (f32, f32, f32) = (1240.0, 600.0, -10.0 * PAGE_DOWN);
/// The Lock chip on the kit's second pad (Snare).
const SNARE_LOCK: (f32, f32) = (846.0, 333.0);
/// The first tile's liked bar on Sounds, under the Order line and the three lines of filters.
const LIKED_BAR: (f32, f32) = (350.0, 414.0);
/// A point on a clip in the arrangement, from the view's top left, for the hover and zoom shots.
const ARRANGEMENT_CLIP: (f32, f32) = (480.0, 180.0);

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
    data.invoke_tab_picked(1);
    capture(window);
    shots.push(("settings-folders.png".into(), capture(window)));
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

/// A left click at a point in the window.
fn click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Left;
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
}

/// Lets hover and slide-in animations finish, then draws the window.
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

fn capture(window: &MinimalSoftwareWindow) -> Vec<u8> {
    capture_after(window, Duration::from_millis(400))
}

/// Draws the window once its timers and animations have run for `wait`.
fn capture_after(window: &MinimalSoftwareWindow, wait: Duration) -> Vec<u8> {
    let start = std::time::Instant::now();
    while start.elapsed() < wait {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(Duration::from_millis(40));
    }
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut buffer = vec![Rgb8Pixel::default(); (WIDTH * HEIGHT) as usize];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, WIDTH as usize);
    });
    buffer.iter().flat_map(|p| [p.r, p.g, p.b]).collect()
}

fn slug(name: &str) -> String {
    name.to_lowercase().replace(' ', "-")
}
