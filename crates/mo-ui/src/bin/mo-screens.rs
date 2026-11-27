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
/// Tall enough for the filter column's foot (Favourites and its Sounds like them slider) to show.
const TALL_FAVOURITES: u32 = 1180;
/// Solomon's own window: an ultrawide screen at 100% (across, down).
const WIDE: (u32, u32) = (3440, 1369);

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The pages are filled with made-up things: nothing is read ahead from a library that isn't there.
    mo_ui::ahead::off();
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
    // The folder tree folded away for the shots whose fixed points were placed before it; its own shots open it.
    mo_ui::sounds::fold_folders_for_screens(&app, true);
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
            // Made up: only the C# app's choice of program for Studio One's songs, which Settings › DAWs shows as
            // chosen in the old app (nothing else here reads it).
            settings: mo_app::app_settings::AppSettings::from_parts(
                None,
                [(
                    mo_app::shell::daw_exe_key("Studio One"),
                    r"C:\Program Files\PreSonus\Studio One 6\Studio One.exe".to_owned(),
                )]
                .into(),
            )
            .unwrap_or_default(),
            folder: out.join("no settings"),
            library: out.join("no library.db"),
            running_pace: mo_app::background::Pace::Fast,
            live: false,
            writable: false,
            feed: feed.clone(),
        },
    );
    // Every part at its usual width: the made-up settings keep none.
    mo_ui::widths::connect(&app);
    // The Space page, from made-up drives: it only reads the real ones when it's shown in the app.
    mo_ui::space::connect(&app, out.join("no library.db"), feed.clone());
    mo_ui::space::show_sample(&app, sample_space(), "");
    // Settings › General's Updates switch, with nothing looked up.
    mo_ui::update::connect(&app, feed.clone(), false);

    let mut shots = Vec::new();
    for place in Place::ALL {
        show_place(&app, place);
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
    midi_right_click_while_playing(&app)?;
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
    // "Fits my song": the parts that go with a made-up song, the best fit first, each with how far to move it. Off
    // again after, with no songs offered, so the pictures that follow have the page as it was.
    mo_ui::midi::fit_sample(&app, true);
    if midi.get_fit_song().is_empty() || midi.get_shown() == 0 {
        return Err("midi-fits.png: no part was found to fit the made-up song".into());
    }
    // The best fit picked: its card says how it's moved to fit, and its roll and chords show it moved so.
    mo_ui::midi::pick_row(&app, 0);
    let moved = midi.get_picked();
    if !moved.fit.starts_with("Moved ") {
        return Err("midi-fits.png: the card of the part picked doesn't say how it's moved to fit".into());
    }
    // It plays with the song's parts it fits, as the chip, the Play button and the card's sound line say.
    let with_song = midi.get_sound_line();
    if !midi.get_fit_with_song()
        || !midi.get_play_with_song()
        || !with_song.contains(" · with Sub from Night drive (bars 1–8), at the song's ")
    {
        return Err(format!("midi-fits.png: the part picked doesn't play with the song: \"{with_song}\"").into());
    }
    // Night drive is an FL Studio song: Play in Bitwig's tip doesn't promise its bars going round there.
    if midi.get_bitwig_with_song() {
        return Err("midi-fits.png: Play in Bitwig says it plays with a song that isn't a Bitwig project".into());
    }
    // Off, it stays picked as it is in the library: the card no longer says it's moved, and its roll and chords are
    // its own.
    mo_ui::midi::fit_sample(&app, false);
    let own = midi.get_picked();
    if !midi.get_fit_song().is_empty() || !midi.get_fit_none_title().is_empty() {
        return Err("midi-fits.png: \"Fits my song\" didn't go off".into());
    }
    if own.title != moved.title || !own.fit.is_empty() || own.roll_low == moved.roll_low || own.chords == moved.chords {
        return Err("midi-fits.png: the part picked is still shown moved with \"Fits my song\" off".into());
    }
    if midi.get_play_with_song() || midi.get_sound_line().contains("Night drive") {
        return Err("midi-fits.png: the part picked still plays with the song with \"Fits my song\" off".into());
    }
    // On again with the part still picked: its card follows the list.
    mo_ui::midi::fit_sample(&app, true);
    let again = midi.get_picked();
    if (&again.fit, &again.roll_low, &again.chords) != (&moved.fit, &moved.roll_low, &moved.chords) {
        return Err("midi-fits.png: the part picked isn't shown moved when the list becomes \"Fits my song\"'s".into());
    }
    if !midi.get_play_with_song() || midi.get_sound_line() != with_song {
        return Err(
            "midi-fits.png: the part picked doesn't play with the song once the list is its song's again".into()
        );
    }
    shots.push(("midi-fits.png".into(), capture(&window)));
    // What of the song the list is fitted to is under the switch, to pick by hand: its parts as chips, the app's
    // lit (the bassline), and its bars on the slider (all eight). None of it is yours yet.
    let parts = || {
        let chips = midi.get_fit_parts().chips;
        chips.iter().map(|c| format!("{}{} {}", if c.on { '+' } else { '-' }, c.text, c.count)).collect::<Vec<_>>()
    };
    let bars = || (midi.get_fit_bars_from(), midi.get_fit_bars_to(), midi.get_fit_bars_length());
    let picker =
        || format!("{:?} {:?} \"{}\" yours: {}", parts(), bars(), midi.get_fit_bars_text(), midi.get_fit_pick_yours());
    let apps = picker();
    if !midi.get_fit_pick_shown()
        || apps != r#"["-Keys melody", "+Sub bassline", "-Lead melody"] (0, 8, 8) "Bars 1–8 of 8" yours: false"#
    {
        return Err(format!("midi-fits-pick.png: the app's pick of the song isn't shown: {apps}").into());
    }
    // The parts lit are the app's (the menu's "The app's pick" is the line ticked), and the slider is there.
    if midi.get_fit_parts_yours() || !midi.get_fit_bars_shown() {
        return Err("midi-fits-pick.png: the app's parts are said to be yours, or the bars have no slider".into());
    }
    let apps_list = (midi.get_result(), midi.get_shown());
    // The lead's chip pressed, and the slider's higher handle dragged to the fifth bar line: the list is fitted to
    // the bassline and the lead over bars 1 to 4, and the app's pick is offered back.
    midi.invoke_fit_part_toggled("Lead".into());
    midi.invoke_fit_bars_picked(0, 4);
    let yours = picker();
    if yours != r#"["-Keys melody", "+Sub bassline", "+Lead melody"] (0, 4, 8) "Bars 1–4 of 8" yours: true"#
        || !midi.get_fit_parts_yours()
        || !midi.get_result().starts_with("To play over Sub and Lead, bars 1–4 · ")
    {
        return Err(format!("midi-fits-pick.png: a pick by hand isn't shown or fitted to: {yours}").into());
    }
    shots.push(("midi-fits-pick.png".into(), capture(&window)));
    // The narrowest window has no room for the chips beside the slider: the parts are in the filter's menu.
    save_narrow(&window, &out, "midi-fits-pick-narrow.png")?;
    // Off and on again, the song is fitted to what you picked of it: it's remembered for the song.
    mo_ui::midi::fit_sample(&app, false);
    if midi.get_fit_pick_shown() {
        return Err("midi-fits-pick.png: the picker is still shown with \"Fits my song\" off".into());
    }
    mo_ui::midi::fit_sample(&app, true);
    if picker() != yours {
        return Err(format!("midi-fits-pick.png: the song's pick wasn't remembered: {}", picker()).into());
    }
    // The switch pressed: the parts go back to the app (the song's melodies now, to put chords under), the bars
    // stay yours.
    midi.invoke_fit_wants_picked(1);
    if picker() != r#"["+Keys melody", "-Sub bassline", "+Lead melody"] (0, 4, 8) "Bars 1–4 of 8" yours: true"# {
        return Err(
            format!("midi-fits-pick.png: the switch didn't give the parts back to the app: {}", picker()).into()
        );
    }
    // "The app's pick": all of it is the app's again, and so is the list once the switch is back too.
    midi.invoke_fit_apps_pick();
    midi.invoke_fit_wants_picked(0);
    if picker() != apps || (midi.get_result(), midi.get_shown()) != apps_list {
        return Err(format!("midi-fits-pick.png: The app's pick didn't bring the app's back: {}", picker()).into());
    }
    // "Play with the song" off: it plays alone at its own tempo, still moved to fit; and on again.
    midi.invoke_fit_with_song_toggled();
    let alone = midi.get_sound_line();
    if midi.get_fit_with_song()
        || midi.get_play_with_song()
        || alone.contains("Night drive")
        || !alone.contains("BPM")
        || midi.get_picked().fit != moved.fit
    {
        return Err(format!("midi-fits.png: with the chip off the part doesn't play alone: \"{alone}\"").into());
    }
    midi.invoke_fit_with_song_toggled();
    if !midi.get_fit_with_song() || !midi.get_play_with_song() || midi.get_sound_line() != with_song {
        return Err("midi-fits.png: the chip on again didn't bring the song back".into());
    }
    // Dragged out from its line it goes moved too, under a name that says so.
    let carried = mo_ui::midi::card_at_row(&app, 0).and_then(|c| c.cards.first().map(|c| c.title.clone()));
    if carried != Some(format!("{} (3 semitones up)", moved.title)) {
        return Err(format!("midi-fits.png: the best fit is dragged out as {carried:?}, not moved").into());
    }
    // A part that goes to another octave as well (the flute hook: 5 up for the key and an octave down) goes out
    // with the whole move.
    let carried = |row| mo_ui::midi::card_at_row(&app, row).and_then(|c| c.cards.first().map(|c| c.title.clone()));
    let lines = usize::try_from(midi.get_shown()).unwrap_or(0);
    if !(0..lines)
        .filter_map(carried)
        .any(|title| title.starts_with("Flute hook") && title.ends_with("(7 semitones down)"))
    {
        return Err("midi-fits.png: no part of the list is dragged out moved to another octave".into());
    }
    // The octave by hand, on the picked part's card (the best fit, 3 semitones up for the key): Octave down takes
    // it an octave under that. Its card, its roll, its buttons and its line's drag all say so, a list worked out
    // again keeps it there, Shift+Alt+Up takes it back up, and The app's octave makes it the app's again.
    let octave_buttons = || midi.get_octave_actions().iter().map(|a| a.text.to_string()).collect::<Vec<_>>();
    if octave_buttons() != ["Octave down", "Octave up"] {
        return Err("midi-fits-octave.png: the card of a part that's fitted has no octave buttons".into());
    }
    if !mo_ui::midi::run_action(&app, "midi.octaveDown") {
        return Err("midi-fits-octave.png: Octave down did nothing".into());
    }
    let lower = midi.get_picked();
    if carried(0) != Some(format!("{} (9 semitones down)", moved.title))
        || !lower.fit.starts_with("Moved 9 semitones down")
        || !lower.fit.contains("at the octave you chose")
        || lower.roll_low == moved.roll_low
        || octave_buttons() != ["Octave down", "Octave up", "The app's octave"]
    {
        return Err("midi-fits-octave.png: the part isn't an octave down everywhere after Octave down".into());
    }
    shots.push(("midi-fits-octave.png".into(), capture(&window)));
    // The song saved again (the Songs page gives its list again): the list follows it, by its saves, and in the
    // list worked out again the part is still where you put it.
    if !mo_ui::midi::fit_sample_follows(&app) || midi.get_shown() == 0 {
        return Err("midi-fits.png: the list didn't follow its song when the songs were given again".into());
    }
    if carried(0) != Some(format!("{} (9 semitones down)", moved.title)) || midi.get_picked().fit != lower.fit {
        return Err("midi-fits-octave.png: a list worked out again lost the octave chosen".into());
    }
    // Shift+Alt+Up, as a key: up an octave, back where the app had it, and still yours. Held down it steps once only.
    let alt_up = |repeat| {
        let up = char::from(slint::platform::Key::UpArrow).to_string();
        app.global::<mo_ui::AppKeys>().invoke_key(up.into(), false, true, true, false, repeat)
    };
    let pressed = alt_up(false);
    let upper = midi.get_picked();
    if !pressed
        || !upper.fit.starts_with("Moved 3 semitones up")
        || !upper.fit.contains("at the octave you chose")
        || upper.roll_low != moved.roll_low
        || octave_buttons().len() != 3
    {
        return Err("midi-fits-octave.png: Shift+Alt+Up didn't take the part up an octave".into());
    }
    if !alt_up(true) || midi.get_picked().fit != upper.fit {
        return Err("midi-fits-octave.png: Shift+Alt+Up held down went on stepping, or wasn't the page's key".into());
    }
    // Up to the last octave (3 up): there the key is still the page's, and does nothing rather than something else.
    let stepped = (0..3).all(|_| alt_up(false));
    let top = midi.get_picked();
    if !stepped || !top.fit.starts_with("Moved 3 octaves and 3 semitones up") {
        return Err(format!("midi-fits-octave.png: three more Shift+Alt+Up gave \"{}\"", top.fit).into());
    }
    if !alt_up(false) || midi.get_picked().fit != top.fit {
        return Err("midi-fits-octave.png: at the last octave Shift+Alt+Up wasn't the page's key any more".into());
    }
    let back = mo_ui::midi::run_action(&app, "midi.octaveApp");
    let again = midi.get_picked();
    if !back || (&again.fit, &again.roll_low) != (&moved.fit, &moved.roll_low) || octave_buttons().len() != 2 {
        return Err("midi-fits-octave.png: The app's octave didn't bring the part back".into());
    }
    // Fitted to a Bitwig song read by its arranger: Bitwig has the bars a part was found for, so its card offers Put
    // in the song beside Send to Bitwig, and Play in Bitwig plays it with those bars going round. (Nothing leaves
    // the page here: no Bitwig is asked.)
    mo_ui::midi::fit_sample_bitwig(&app);
    if midi.get_fit_song() != "Glass" || midi.get_shown() == 0 {
        return Err("midi-fits-bitwig.png: no part was found to fit the made-up Bitwig song".into());
    }
    mo_ui::midi::pick_row(&app, 0);
    let buttons = || midi.get_actions().iter().map(|a| (a.id.to_string(), a.enabled)).collect::<Vec<_>>();
    if buttons().get(..2) != Some(&[("midi.toBitwig".to_owned(), true), ("midi.putInSong".to_owned(), true)]) {
        return Err(format!("midi-fits-bitwig.png: the card doesn't offer Put in the song: {:?}", buttons()).into());
    }
    if !midi.get_bitwig_with_song() {
        return Err("midi-fits-bitwig.png: Play in Bitwig doesn't say it plays with the Bitwig song's bars".into());
    }
    if !mo_ui::midi::run_action(&app, "midi.putInSong") {
        return Err("midi-fits-bitwig.png: Put in the song isn't one of the page's actions".into());
    }
    shots.push(("midi-fits-bitwig.png".into(), capture(&window)));
    // An FL Studio song has no bars in Bitwig: no such button on its parts' cards.
    mo_ui::midi::fit_sample(&app, true);
    mo_ui::midi::pick_row(&app, 0);
    if buttons().iter().any(|(id, _)| id == "midi.putInSong") {
        return Err("midi-fits.png: Put in the song is offered for a song that isn't a Bitwig project".into());
    }
    mo_ui::midi::fit_sample(&app, false);
    if !midi.get_fit_song().is_empty() || !midi.get_fit_none_title().is_empty() || !midi.get_picked().fit.is_empty() {
        return Err("midi-fits.png: \"Fits my song\" didn't go off".into());
    }
    // The first part picked again, as the pictures that follow have the page.
    mo_ui::midi::pick_row(&app, 0);

    // The Backup page, filled with made-up folders.
    app.set_place(Place::Backup.index() as i32);
    mo_ui::backup::show_lines(&app, mo_ui::backup::sample_lines());
    mo_ui::backup::show_older(&app, mo_ui::backup::sample_older(false));
    shots.push(("backup.png".into(), capture(&window)));
    // The Remove older versions menu, opened.
    let before = capture(&window);
    // The card's words above the button take two lines or three, as the made-up backups' first date (a year ago
    // today) is shorter or longer: the button is at one of the two heights.
    let mut after = before.clone();
    for y in REMOVE_OLDER_BUTTON.1 {
        click(&window, REMOVE_OLDER_BUTTON.0, y);
        after = capture(&window);
        if before != after {
            break;
        }
    }
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
    // The strip above the list opened: the plugins to install as cards, and the room to free.
    let plugins_data = app.global::<mo_ui::PluginsData>();
    plugins_data.set_strip_open(true);
    capture(&window);
    shots.push(("plugins-strip.png".into(), capture(&window)));
    plugins_data.set_strip_open(false);
    mo_ui::plugins::show_sample(&app, plugins.clone(), kontakt, 1, &[]);
    capture(&window);
    shots.push(("plugins-on-disk.png".into(), capture(&window)));
    mo_ui::plugins::show_sample(&app, plugins.clone(), serum, 2, &copies);
    capture(&window);
    shots.push(("plugins-copies.png".into(), capture(&window)));
    mo_ui::plugins::show_sample(&app, plugins, massive, 0, &[]);
    capture(&window);
    shots.push(("plugins-missing.png".into(), capture(&window)));
    // Moved folders whose link stopped working (the whole-app review of 19 Nov, PM-2), each a chip with a sign for
    // what's wrong (Solomon, 24 Nov 2026): one whose link leads elsewhere, one whose drive came back under another
    // letter, one an update put a real folder back in place of, a second copy, and one whose drive isn't plugged in;
    // Link again as a sign.
    let (broken, tried) = sample_broken_links();
    mo_ui::plugins::show_links_sample(&app, broken, tried);
    capture(&window);
    shots.push(("plugins-links.png".into(), capture(&window)));
    mo_ui::plugins::show_links_sample(&app, Vec::new(), std::collections::BTreeMap::new());
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
    // The same card just after a start: what the last run's look found, marked with when it's from, while this run's
    // look checks it; Fix and the Recycle Bin buttons wait for that (Solomon, 25 Nov 2026).
    let from = std::time::UNIX_EPOCH + Duration::from_secs(1_795_543_320);
    let kept = mo_ui::space::Sample { bitwig_from: Some(from), ..sample_space() };
    mo_ui::space::show_sample(&app, kept, bitwig);
    capture(&window);
    shots.push(("space-bitwig-kept.png".into(), capture(&window)));
    mo_ui::space::show_sample(&app, mo_ui::space::Sample { fixes: sample_fixes(), ..sample_space() }, bitwig);
    capture(&window);
    // The same card at its narrowest: a button whose words are wider than the card puts them on two lines, and a
    // save's two links go one under the other, so nothing runs past the card's edge (Solomon's F12 note, 17 Nov 2026).
    // Then wide: four buttons to a line, and the links side by side.
    let space = app.global::<mo_ui::SpaceData>();
    let was = space.get_panel_width();
    space.set_panel_width(320.0);
    capture(&window);
    shots.push(("space-bitwig-narrow.png".into(), capture(&window)));
    space.set_panel_width(700.0);
    capture(&window);
    shots.push(("space-bitwig-wide.png".into(), capture(&window)));
    space.set_panel_width(was);
    capture(&window);
    // A click on the "i" by the card's name shows how it works at once, not once the mouse has rested (Solomon's F12
    // note, 25 Nov 2026: "clicking on info next to bitwig copies does nothing"). Below the "i", where the tip goes:
    // the "i" itself only lights up under the mouse.
    let below_info = |shot: &[u8]| crop_rgb(shot, WIDTH, (1080, 72), (320, 130));
    let before = below_info(&capture(&window));
    hover(&window, BITWIG_INFO.0, BITWIG_INFO.1);
    click(&window, BITWIG_INFO.0, BITWIG_INFO.1);
    let clicked = capture_after(&window, Duration::from_millis(100));
    if below_info(&clicked) == before {
        return Err("space-bitwig-info.png: a click on the card's \"i\" showed nothing at once; has it moved?".into());
    }
    shots.push(("space-bitwig-info.png".into(), clicked));
    hover(&window, -1.0, -1.0);
    // The tick above the card's ticks, clicked while all it ticks are ticked: every save is unticked, and Fix has none.
    space.invoke_tick_all(false);
    shots.push(("space-bitwig-untick.png".into(), capture(&window)));
    if space.get_card_all().ticked || space.get_card_all().partly {
        return Err("space-bitwig-untick.png: the tick for all still shows something ticked".into());
    }
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
    presets_shots(&app, &window, &graphite, &mut shots)?;
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
    // The seam between the list and the panel, dragged right: the list wider than its third.
    let widths = app.global::<mo_ui::Widths>();
    let list_width = || if widths.get_songs_list() > 0.0 { widths.get_songs_list() } else { SONGS_LIST_WIDTH };
    let held = drag_seam(&app, &window, SONGS_SEAM, 60.0, 1.0, "songs.list", list_width)?;
    shots.push(("songs-list-wider.png".into(), held));
    // A column of the list: Last saved, wider by its heading's left edge (Song, which stretches, gives the room).
    // The list made wider first (by `SONGS_WIDER_LIST`), so the Song column has room to give beside the Songs / Next
    // up switch.
    widths.set_songs_list(SONGS_LIST_WIDTH + SONGS_WIDER_LIST);
    let held = drag_seam(&app, &window, SONGS_SAVED_SEAM, -40.0, -1.0, "songs.saved", || widths.get_songs_saved())?;
    widths.invoke_reset("songs.list".into());
    shots.push(("songs-column-wider.png".into(), held));
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
    // The track names dragged wider (as their right edge does), then back to their usual width.
    let usual = arrangement.get_header_width();
    arrangement.invoke_names_resizing(230.0);
    let wider = capture(&window);
    if arrangement.get_header_width() != 230.0 {
        return Err(format!(
            "arrangement-names-wider.png: the names are {} wide, not 230",
            arrangement.get_header_width()
        )
        .into());
    }
    shots.push(("arrangement-names-wider.png".into(), wider));
    app.global::<mo_ui::Widths>().invoke_reset("songs.tracks".into());
    capture(&window);
    if arrangement.get_header_width() != usual {
        return Err(format!(
            "arrangement: the names' usual width came back as {}, not {usual}",
            arrangement.get_header_width()
        )
        .into());
    }
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
    // A song's to-dos on its card, and Next up: every to-do left, under each song.
    mo_ui::songs::show_picked(&app, 1, sample_detail(), Tab::Overview);
    shots.push(("songs-todos.png".into(), capture(&window)));
    app.global::<mo_ui::SongsData>().set_next_up(true);
    shots.push(("songs-next-up.png".into(), capture(&window)));
    app.global::<mo_ui::SongsData>().set_next_up(false);
    app.global::<mo_ui::SongsData>().invoke_close_panel();
    // The board: a column a status, Archived folded; then a card picked, its panel over the board's right.
    let data = app.global::<mo_ui::SongsData>();
    let picked = data.get_selected();
    data.set_selected(-1);
    data.set_board(true);
    data.invoke_board_shown();
    shots.push(("songs-board.png".into(), capture(&window)));
    data.set_board_archived_open(true);
    data.invoke_board_shown();
    shots.push(("songs-board-archived.png".into(), capture(&window)));
    data.set_board_archived_open(false);
    if let Some(night) = mo_ui::songs::sample_row("Night Drive") {
        mo_ui::songs::show_picked(&app, night, sample_detail(), Tab::Overview);
    }
    shots.push(("songs-board-card.png".into(), capture(&window)));
    data.set_board(false);
    // Back to the song picked before, for the shots after.
    mo_ui::songs::show_picked(&app, usize::try_from(picked).unwrap_or(1), sample_detail(), Tab::Overview);
    data.invoke_close_panel();
    // The Dust off card above the list: put off in every other shot, so the list stays where their clicks expect it.
    let mut dusty = sample_songs();
    dusty.later.clear();
    mo_ui::songs::show_sample(&app, dusty);
    shots.push(("songs-dust-off.png".into(), capture(&window)));
    mo_ui::songs::show_picked(&app, 0, sample_detail(), Tab::Overview);
    shots.push(("songs-time-spent.png".into(), capture(&window)));
    app.global::<mo_ui::SongsData>().invoke_close_panel();
    // A song's bounces on its card, and one on the player bar (paused: nothing is played here).
    if let Some(copper) = mo_ui::songs::sample_row("Copper Lines") {
        mo_ui::songs::show_picked(&app, copper, sample_detail(), Tab::Overview);
    }
    let bar = app.global::<mo_ui::SongPlayer>();
    bar.set_shown(true);
    bar.set_title("Copper Lines".into());
    bar.set_file("Copper Lines (final).wav".into());
    bar.set_path(r"C:\Music\Exports\Copper Lines (final).wav".into());
    bar.set_place(0.32);
    bar.set_time("1:12 / 3:42".into());
    shots.push(("songs-sound.png".into(), capture(&window)));
    bar.set_shown(false);
    app.global::<mo_ui::SongsData>().invoke_close_panel();
    mo_ui::songs::show_sample(&app, sample_songs());

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
    let starting = feed.start("Analysing sounds", "Getting ready");
    feed.start("Finding missing samples", "").done("All 31 found");
    feed.start("Copying Bitwig template", "").failed("The template folder isn't there any more");
    mo_ui::cards::refresh(&app, &feed);
    shots.push(("cards.png".into(), capture(&window)));
    // More cards that stay than fit: the newest show, and a line above counts the ones waiting behind them. The same
    // card said twice is one card.
    for part in ["Bass", "Keys", "Lead", "Pad", "Pluck"] {
        feed.told(&format!("{part} sent to Bitwig"), "Bitwig has another project open, so its bar isn't marked.");
    }
    feed.told("Pluck sent to Bitwig", "Bitwig has another project open, so its bar isn't marked.");
    mo_ui::cards::refresh(&app, &feed);
    shots.push(("cards-more.png".into(), capture(&window)));
    // Gone from the feed itself, not only from the window: the rail's timer reads the feed again once the done one's
    // time is up, and would put them back in whichever shot comes a few seconds later.
    going.done("");
    starting.done("");
    for card in feed.visible(std::time::Instant::now()) {
        feed.dismiss(card.id);
    }
    mo_ui::cards::refresh(&app, &feed);

    // Tooltips: the mouse resting on a rail line, the song panel's Show in folder button, a song's date, and a song's
    // details cut short beside the panel, which show whole (Solomon's note of 15 Nov 2026).
    show_song_panel(&app, 0, Tab::Overview);
    for (name, x, y) in [
        ("tip-rail.png", 60.0, 125.0),
        ("tip-button.png", SHOW_BUTTON.0, SHOW_BUTTON.1),
        ("tip-date.png", 740.0, 228.0),
        ("tip-cut.png", 560.0, 248.0),
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
    lists_keep_their_place(&app, &window)?;
    key_checks(&app)?;
    card_keys_checks(&app, &window, &out)?;
    rail_checks(&app)?;
    wide_shots(&app, &window, &out)?;
    types_fill_the_column(&app, &window)?;
    tag_box_checks(&app, &window)?;
    stopping_by_click(&app, &window)?;
    keys_come_back_checks(&app, &window)?;
    developer_keeps_its_place(&app, &window)?;
    developer_checklist_end(&app, &window, &out)?;
    short_swept_card(&app, &window, &out)?;
    swept_card_checks(&app, &window)?;
    rediscover_without_fingerprints(&app, &window, &out)?;
    Ok(())
}

/// Rediscover when the sounds' fingerprints couldn't be read: it says why, isn't left busy (it said "Picking sounds…"
/// and "Shuffling…" for the rest of the run: the whole-app review of 19 Nov, SN-6), and offers no Shuffle that can't
/// shuffle (the code review of that fix). The fingerprints come back after, and it digs sounds out again.
fn rediscover_without_fingerprints(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    out: &std::path::Path,
) -> Result<(), String> {
    let data = app.global::<mo_ui::SoundsData>();
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    start_on(app, Place::Sounds);
    data.invoke_pick_view(4);
    capture(window);
    let why = "made up for the screenshots";
    mo_ui::sounds::index_failed_for_screens(app, why);
    let pixels = capture(window);
    let path = out.join("sounds-rediscover-cant-dig.png");
    image::save_buffer(&path, &pixels, WIDTH, HEIGHT, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
    println!("{}", path.display());
    let failed = (data.get_re_busy(), data.get_re_cant_dig(), data.get_re_empty_title(), data.get_re_empty_text());
    data.invoke_re_shuffle();
    capture(window);
    let shuffled = (data.get_re_busy(), data.get_re_empty_title());
    mo_ui::sounds::index_known(app, std::sync::Arc::new(sample_fingerprints()));
    capture(window);
    let back = (data.get_re_busy(), data.get_re_cant_dig(), data.get_re_empty_title());
    data.invoke_pick_view(0);
    capture(window);
    let (busy, cant_dig, title, text) = failed;
    if busy || !cant_dig || title != "Can't dig sounds out" || !text.contains(why) {
        return Err(format!(
            "rediscover: with no fingerprints it said {title:?}, {text:?}, busy {busy}, Shuffle offered {}",
            !cant_dig
        ));
    }
    if shuffled.0 || shuffled.1 != title {
        return Err(format!(
            "rediscover: Shuffle pressed with no fingerprints left it busy {}, saying {:?}",
            shuffled.0, shuffled.1
        ));
    }
    if back.1 || back.2 == title {
        return Err(format!(
            "rediscover: once the fingerprints came back it still said {:?} (busy {})",
            back.2, back.0
        ));
    }
    Ok(())
}

/// A sweep across the galaxy, from its top left down to the right, slow enough that each star passed gets its turn to
/// play: the trail through the swept stars, and the Swept card.
fn sweep(data: &mo_ui::SoundsData<'_>) {
    data.invoke_galaxy_down(40.0, 120.0, false);
    for step in 1..=60 {
        data.invoke_galaxy_move(40.0 + step as f32 * 12.0, 120.0 + step as f32 * 6.0);
        if step % 3 == 0 {
            std::thread::sleep(Duration::from_millis(95));
        }
    }
    data.invoke_galaxy_up(false);
}

/// The galaxy's Swept card (the whole-app review of 14 Nov): the wheel over it stays with it, so the map under it
/// doesn't zoom (SO-8); a double-click on a swept star counts as a play (SO-6); and its lines go with the trail when
/// the stars are laid out again, so none goes on carrying its place in the old list (SO-5). Last, as it lays the map
/// out again.
fn swept_card_checks(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    let data = app.global::<mo_ui::SoundsData>();
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    start_on(app, Place::Sounds);
    data.invoke_pick_view(2);
    // As on the galaxy's pictures: a sound's card open beside the map, and the whole map on show (the mini-map's
    // picture flew in), so the sweep passes the same stars and the card is where the wheel turns.
    pick_sound(app, 3);
    data.invoke_galaxy_reset();
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(16));
        capture(window);
    }
    data.invoke_galaxy_swept_clear();
    sweep(&data);
    capture(window);
    if data.get_galaxy_swept().row_count() == 0 {
        return Err("swept card: the sweep across the galaxy left no Swept card".into());
    }
    // Down a line on the card: the map glides to its star, and every frame of the glide moves the trail with the map.
    // The card's lines stay the lines they were: one made again under a mouse that isn't moving lost its shade and
    // its tooltip (the whole-app review of 19 Nov, SN-10).
    let lines = data.get_galaxy_swept();
    data.invoke_galaxy_swept_step(1);
    // Its first frame moves the current line; the rest of the glide changes none, so a line under a resting mouse keeps
    // its shade (lines changed on every frame lost it as lines made again did: the code review of the SN-10 fix).
    std::thread::sleep(Duration::from_millis(16));
    capture(window);
    let changed = mo_ui::sounds::swept_lines_changed();
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(16));
        capture(window);
    }
    if data.get_galaxy_swept() != lines {
        return Err("swept card: the glide to a line's star made the card's lines again".into());
    }
    let more = mo_ui::sounds::swept_lines_changed() - changed;
    if more > 0 {
        return Err(format!("swept card: the glide to a line's star changed the card's lines {more} times"));
    }
    // The tip of a line the mouse rests on, then the view changed under the mouse (as a forgotten folder's click
    // does): the tip goes with the view, as it does with a page. It stayed over the next view, and the card's lines
    // staying put let it come up. Looked for over the List, drawn the same both times.
    hover(window, -1.0, -1.0);
    data.invoke_pick_view(1);
    let without = capture(window);
    data.invoke_pick_view(2);
    capture(window);
    let (x, y) = SWEPT_LINE;
    hover(window, x, y);
    let resting = capture(window);
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(16));
        capture(window);
    }
    let tipped = capture(window);
    data.invoke_pick_view(1);
    let after = capture(window);
    hover(window, -1.0, -1.0);
    data.invoke_pick_view(2);
    capture(window);
    if !changed_within(&resting, &tipped, SWEPT_TIP) {
        return Err(format!("swept card: no tip came up with the mouse resting on the line at {x}, {y}"));
    }
    if changed_within(&without, &after, SWEPT_TIP) {
        return Err("swept card: a line's tip stayed over the List once the Galaxy went from under the mouse".into());
    }
    // The wheel turned up over the card (a positive turn), then time for a zoom to fly in: the map left of the card
    // stays as it was. Up, as down would show nothing: the whole map can't go further out.
    let before = capture(window);
    let (x, y) = SWEPT_CARD_WHEEL;
    scroll(window, x, y, PAGE_DOWN);
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(16));
        capture(window);
    }
    let after = capture(window);
    hover(window, -1.0, -1.0);
    if changed_within(&before, &after, GALAXY_LEFT) {
        return Err(format!(
            "swept card: the wheel at {x}, {y} over the Swept card zoomed the map; is the card still there?"
        ));
    }
    // A double-click on the star swept last: it's picked and played, a play you meant, so History lists it first (SO-6).
    let Some(star) = data.get_galaxy_trail_dots().row_data(0) else {
        return Err("swept card: the sweep left no trail on the map".into());
    };
    data.invoke_galaxy_move(star.x, star.y);
    data.invoke_galaxy_down(star.x, star.y, false);
    data.invoke_galaxy_double();
    data.invoke_galaxy_up(false);
    capture(window);
    let played = data.get_picked().name.to_string();
    data.invoke_pick_view(3);
    let first = data.get_lines().row_data(0).map(|line| line.name.to_string());
    data.invoke_pick_view(2);
    capture(window);
    if first.as_deref() != Some(played.as_str()) {
        return Err(format!(
            "swept card: {played:?}, double-clicked on the map, isn't first in History ({first:?} is)"
        ));
    }
    // New fingerprints: the stars are to be laid out again, and the trail goes with them, card and all, at once. With
    // another view on show, so the map isn't laid out again on the spot: here that happens on this thread at once,
    // hiding a card left as it was; in the app it takes seconds on the pool, and the galaxy came back to the old lines.
    data.invoke_pick_view(1);
    mo_ui::sounds::index_known(app, std::sync::Arc::new(sample_fingerprints()));
    let left = data.get_galaxy_swept().row_count();
    data.invoke_pick_view(2);
    capture(window);
    if left > 0 {
        return Err(format!(
            "swept card: {left} lines stayed on the Swept card once the stars were to be laid out again"
        ));
    }
    mo_ui::sounds::stop_on_escape(app);
    Ok(())
}

/// The galaxy's Swept card in a short window, with more lines than it has room for: they give way between its top and
/// its Clear line, which both stay in sight, and the line Back and Next are on stays in sight among them, down to the
/// oldest (the whole-app review of 14 Nov, SO-8: the card now stops above the mini-map and below the map's settings,
/// and cut off its end, Clear and all). Saved as sounds-galaxy-swept-short.png.
fn short_swept_card(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    out: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let data = app.global::<mo_ui::SoundsData>();
    window.set_size(PhysicalSize::new(WIDTH, SHORT));
    start_on(app, Place::Sounds);
    data.invoke_pick_view(2);
    pick_sound(app, 3);
    data.invoke_galaxy_reset();
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(16));
        capture(window);
    }
    data.invoke_galaxy_swept_clear();
    // Across the whole map and back, a row at a time, for a long trail.
    for row in 0..SHORT_SWEEP_ROWS {
        let y = 25.0 + row as f32 * 50.0;
        let (from, to) = if row % 2 == 0 { (10.0, 530.0) } else { (530.0, 10.0) };
        sweep_along(&data, (from, y), (to, y));
    }
    capture(window);
    let lines = data.get_galaxy_swept().row_count();
    // Down the card to its oldest line, as Down does.
    for _ in 1..lines {
        data.invoke_galaxy_swept_step(1);
    }
    let pixels = capture_sized(window, (WIDTH, SHORT), Duration::from_millis(400));
    let path = out.join("sounds-galaxy-swept-short.png");
    image::save_buffer(&path, &pixels, WIDTH, SHORT, image::ExtendedColorType::Rgb8)?;
    println!("{}", path.display());
    let oldest = i32::try_from(lines).unwrap_or(0) - 1;
    let was_at = data.get_galaxy_swept_at();
    // The line Back and Next are on shows lowest on the card, so the one above it is the line swept after it.
    let (x, y) = SHORT_SWEPT_ABOVE;
    click(window, x, y);
    capture(window);
    let clicked = data.get_galaxy_swept_at();
    let (clear_x, clear_y) = SHORT_SWEPT_CLEAR;
    click(window, clear_x, clear_y);
    capture(window);
    let left = data.get_galaxy_swept().row_count();
    mo_ui::sounds::stop_on_escape(app);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    capture(window);
    if lines < SHORT_SWEPT_LINES {
        return Err(format!("short swept card: the sweeps left {lines} lines, too few to fill the card").into());
    }
    if was_at != oldest {
        return Err(format!("short swept card: Down to the oldest line left Back and Next on line {was_at}").into());
    }
    if clicked != oldest - 1 {
        return Err(format!(
            "short swept card: the click at {x}, {y} went to line {clicked}, not to line {}, just above the one Back \
             and Next were on: was that out of sight?",
            oldest - 1
        )
        .into());
    }
    if left > 0 {
        return Err(format!(
            "short swept card: Clear at {clear_x}, {clear_y} left {left} lines: is it cut off the card's foot?"
        )
        .into());
    }
    Ok(())
}

/// A sweep across the galaxy from `from` to `to` (on the map, in points), as [`sweep`]'s.
fn sweep_along(data: &mo_ui::SoundsData<'_>, from: (f32, f32), to: (f32, f32)) {
    let (dx, dy) = ((to.0 - from.0) / 60.0, (to.1 - from.1) / 60.0);
    data.invoke_galaxy_down(from.0, from.1, false);
    for step in 1..=60 {
        data.invoke_galaxy_move(from.0 + step as f32 * dx, from.1 + step as f32 * dy);
        if step % 3 == 0 {
            std::thread::sleep(Duration::from_millis(95));
        }
    }
    data.invoke_galaxy_up(false);
}

/// Whether any pixel differs between two pictures of the window inside a box (x, y, width, height, in pixels).
fn changed_within(a: &[u8], b: &[u8], (x, y, width, height): (u32, u32, u32, u32)) -> bool {
    (y..y + height).any(|row| {
        let at = ((row * WIDTH + x) * 3) as usize..((row * WIDTH + x + width) * 3) as usize;
        a.get(at.clone()) != b.get(at)
    })
}

/// The filter column shows as many types as it has room for as tick lines, and only the rest in More types (Solomon,
/// 11 Nov 2026: "the more types should fill is there more space"): more than the usual nine in a tall window, the nine
/// in a short one, on the Type filter and on Rediscover alike, and as many again when a view comes back after the
/// window changed. A type ticked on the last line stays there, with as many lines as before (Clear filters comes into
/// room kept for it), and More types' Any keeps it picked. (Rediscover's lines may grow when what it digs out for the
/// type changes the words under them: only its ticked line is held to its place.) With nothing picked, the room kept
/// for what comes as you pick is still free under the column's last line: less, and something in the column is drawn
/// taller than it was measured, so the types would push its foot out of sight (Rediscover's words under its choices
/// did: Slint counted them as one line).
fn types_fill_the_column(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    use mo_app::sounds::TYPE_CHIPS;
    let sounds = app.global::<mo_ui::SoundsData>();
    let types = || {
        let f = sounds.get_type_filter();
        (f.chips.row_count(), f.more.row_count())
    };
    let re_types = || {
        let f = sounds.get_re_type_filter();
        (f.chips.row_count(), f.more.row_count())
    };
    let line = |rediscover: bool, at: usize| {
        let f = if rediscover { sounds.get_re_type_filter() } else { sounds.get_type_filter() };
        f.chips.row_data(at).map(|c| (c.text.to_string(), c.on)).unwrap_or_default()
    };
    let sized = |height: u32| {
        window.set_size(PhysicalSize::new(WIDTH, height));
        capture_sized(window, (WIDTH, height), Duration::from_millis(400))
    };
    // The sizes the column counts its room in (studio.slint's Heights and Space).
    let heights = app.global::<mo_ui::Heights>();
    let (tick_line, button, gap) =
        (heights.get_tick_line(), heights.get_button(), app.global::<mo_ui::Space>().get_md());
    let kept_free = |name: &str, shot: &[u8], free: f32| match column_room_below(shot, WIDTH) {
        Some(room) if f64::from(room) >= f64::from(free) => Ok(()),
        room => Err(format!(
            "types: {name}'s filter column ends {room:?} pixels above its foot, not the {free} kept for what comes as \
             you pick: is something in it drawn taller than it was measured?"
        )),
    };
    start_on(app, Place::Sounds);
    sounds.invoke_pick_view(0);
    let shot = sized(WIDE.1);
    let (lines, more) = types();
    if lines <= TYPE_CHIPS || more == 0 {
        return Err(format!(
            "types: {lines} type lines and {more} in More types at {WIDTH} x {}, where the column has room for more \
             than {TYPE_CHIPS} and not for all",
            WIDE.1
        ));
    }
    // Keys that mix well and Clear filters, each with the space above it.
    kept_free("The Type filter", &shot, tick_line + gap + button + gap)?;
    let (last, _) = line(false, lines - 1);
    sounds.invoke_toggle_choice("type".into(), last.clone().into());
    sized(WIDE.1);
    let ticked = (types(), line(false, lines - 1));
    sounds.invoke_clear_chip("more-types".into());
    sized(WIDE.1);
    let kept = line(false, lines - 1);
    sounds.invoke_clear_chip("type".into());
    if ticked != ((lines, more), (last.clone(), true)) || kept != (last.clone(), true) {
        return Err(format!(
            "types: {last}, ticked on the last type line, then More types' Any: {ticked:?}, then {kept:?}"
        ));
    }
    // Rediscover in the usual window: more than nine, and the rest in More types.
    sized(HEIGHT);
    sounds.invoke_pick_view(4);
    let shot = sized(HEIGHT);
    let (re_lines, re_more) = re_types();
    if re_lines <= TYPE_CHIPS || re_more == 0 {
        return Err(format!(
            "types: Rediscover shows {re_lines} type lines and {re_more} in More types at {WIDTH} x {HEIGHT}, where \
             it has room for more than {TYPE_CHIPS} and not for all"
        ));
    }
    // Clear types, with the space above it.
    kept_free("Rediscover", &shot, button + gap)?;
    let (last, _) = line(true, re_lines - 1);
    sounds.invoke_re_pick("type".into(), last.clone().into());
    sized(HEIGHT);
    let ticked = line(true, re_lines - 1);
    sounds.invoke_re_pick("more-types".into(), "".into());
    sized(HEIGHT);
    let kept = line(true, re_lines - 1);
    sounds.invoke_re_pick("type".into(), "".into());
    if ticked != (last.clone(), true) || kept != (last.clone(), true) {
        return Err(format!(
            "types: on Rediscover, {last} ticked on the last type line, then More types' Any: {ticked:?}, then {kept:?}"
        ));
    }
    // Shorter while Rediscover shows: its types go back to nine, and so do the Type filter's once it's back.
    sized(SHORT);
    let re_short = re_types();
    sounds.invoke_pick_view(0);
    sized(SHORT);
    let back = types();
    sized(WIDE.1);
    let again = types();
    sized(HEIGHT);
    if re_short.0 != TYPE_CHIPS || back != (TYPE_CHIPS, lines + more - TYPE_CHIPS) || again != (lines, more) {
        return Err(format!(
            "types: in a short window Rediscover showed {} type lines and the Type filter {} ({} in More types), \
             then {} once tall again (not {TYPE_CHIPS}, {TYPE_CHIPS}, {} and {lines})",
            re_short.0,
            back.0,
            back.1,
            again.0,
            lines + more - TYPE_CHIPS
        ));
    }
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
    pick_sound(app, 0);
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

/// Stopping the sound playing without its Stop button (Solomon, 11 Nov 2026: "maybe it should be clicking it again
/// stops the sound so we can still have a right click menu", then "maybe only there over the waveform right click
/// should stop it"). A click on its tile stops it, and it stays picked. A right click on the tile's name opens the
/// tile's menu, and the sound goes on: the pick for the menu starts and stops nothing. A right click over its waveform,
/// on the tile or on its card, stops it and opens no menu, and a right click on the tile once it's stopped opens its
/// menu and plays nothing, Autoplay or not. A click on the card's waveform plays from there. With the card open first,
/// so the tiles keep their places as they're picked, and clicks on one spot further apart than a double-click. Last,
/// as it leaves a tile picked.
fn stopping_by_click(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    use slint::platform::Key;
    let sounds = app.global::<mo_ui::SoundsData>();
    let name = || sounds.get_picked().name.to_string();
    let plays = mo_ui::sounds::plays;
    let wait = || {
        capture(window);
        capture(window)
    };
    start_on(app, Place::Sounds);
    sounds.invoke_clear();
    sounds.invoke_pick_view(0);
    pick_sound(app, 0);
    capture(window);
    if plays() {
        press(app, Key::Escape, false);
    }
    wait();
    // A click on a tile picks it; with Autoplay on it plays (else Play does).
    let (x, y) = MENU_TILE;
    click(window, x, y);
    wait();
    if !plays() {
        sounds.invoke_play();
        wait();
    }
    let tile = name();
    if !sounds.get_playing() {
        return Err(format!("stop by click: the click at {x}, {y} picked nothing that plays; has the grid moved?"));
    }
    click(window, x, y);
    wait();
    if plays() || name() != tile || !sounds.get_panel_shown() {
        return Err(format!("stop by click: a click on {tile} as it played didn't stop it, or didn't leave it picked"));
    }
    sounds.invoke_play();
    let before = wait();
    right_click(window, x, y);
    let menu = wait();
    if !plays() || name() != tile || same_in(&before, &menu, MENU_OVER) {
        return Err(format!(
            "stop by click: a right click on {tile}'s name as it played stopped it, or opened no menu"
        ));
    }
    // Away again: a click above the page shuts the menu.
    click(window, 600.0, 20.0);
    let before = wait();
    let (wx, wy) = TILE_WAVE;
    right_click(window, wx, wy);
    let after = wait();
    if plays() || !same_in(&before, &after, MENU_OVER) {
        return Err(format!(
            "stop by click: a right click on {tile}'s waveform as it played didn't stop it, or opened its menu"
        ));
    }
    // Stopped now: a right click on it opens its menu, and with Autoplay on (as here) its pick for the menu still
    // plays nothing, as a click would.
    right_click(window, x, y);
    let menu = wait();
    if plays() || name() != tile || same_in(&after, &menu, MENU_OVER) {
        return Err(format!(
            "stop by click: a right click on {tile} once it had stopped played it (Autoplay {}), or opened no menu",
            if sounds.get_autoplay() { "on" } else { "off" }
        ));
    }
    click(window, 600.0, 20.0);
    wait();
    sounds.invoke_play();
    wait();
    let (cx, cy) = CARD_WAVE;
    right_click(window, cx, cy);
    wait();
    if plays() {
        return Err("stop by click: a right click on the card's waveform as its sound played didn't stop it".into());
    }
    click(window, cx, cy);
    wait();
    if !plays() {
        return Err("stop by click: a click on the card's waveform didn't play the sound from there".into());
    }
    press(app, Key::Escape, false);
    hover(window, -1.0, -1.0);
    capture(window);
    Ok(())
}

/// The keys come back when the part that had them goes, as they do on Sounds since the 10 Nov review's U-1 (the
/// whole-app review of 14 Nov): the Songs card closing by Esc in its empty tag box, given the keys by Ctrl+T (SG-5,
/// AP-1); the sound card giving its place to Favourites from Bitwig with the keys in its tag box (AP-4); a
/// filter-column slider pressed, then hidden with its view (AP-2); and Settings › Shortcuts: Ctrl+F finds its search
/// (AP-7), a key's button clicked after typing there hears the next key, the words staying (AP-3), Esc takes the
/// words off (AP-7), another tab after typing there leaves the keys with the window (AP-1), and a line listening
/// stops as another tab shows (AP-6). F1 must open the key sheet each time. Every part runs, and all that went wrong
/// is said at once. Last, as it adds tags.
fn keys_come_back_checks(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    let wrong: Vec<String> = [
        songs_card_keys(app, window),
        sound_card_gives_way(app, window),
        slider_keys(app, window),
        shortcuts_keys(app, window),
    ]
    .into_iter()
    .filter_map(Result::err)
    .collect();
    if wrong.is_empty() { Ok(()) } else { Err(format!("keys: {}", wrong.join("; "))) }
}

/// Whether F1 opens the key sheet now, or what went wrong `after` what.
fn keys_work(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow, after: &str) -> Result<(), String> {
    if f1_works(app, window) { Ok(()) } else { Err(format!("{after}, F1 did nothing: no part had the keys")) }
}

/// Songs: Ctrl+T, a tag typed and Enter, then Esc in the empty box closes the card.
fn songs_card_keys(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    use slint::platform::Key;
    let songs = app.global::<mo_ui::SongsData>();
    let song_tagged = |tag: &str| {
        let tags = songs.get_song_tags();
        (0..tags.row_count()).any(|i| tags.row_data(i).is_some_and(|t| t.text == tag))
    };
    start_on_songs(app);
    songs.set_search("".into());
    songs.invoke_search_edited("".into());
    mo_ui::songs::show_picked(app, 1, sample_detail(), Tab::Overview);
    capture(window);
    press(app, "t", true);
    capture(window);
    type_in(app, window, "dusty");
    if !song_tagged("dusty") {
        return Err("Ctrl+T then typing on Songs added no tag: the card's tag box didn't get the keys".into());
    }
    press(app, Key::Escape, false);
    capture(window);
    if mo_ui::songs::has_pick() {
        return Err("Esc in the song card's empty tag box didn't close the card".into());
    }
    keys_work(app, window, "once Esc closed the song's card from its tag box")
}

/// Sounds: Favourites from Bitwig takes the sound card's place while its tag box has the keys.
fn sound_card_gives_way(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    let sounds = app.global::<mo_ui::SoundsData>();
    start_on(app, Place::Sounds);
    sounds.invoke_pick_view(0);
    pick_sound(app, 0);
    // With Autoplay on the pick plays: stopped, so a check after it has its Esc for itself.
    mo_ui::sounds::stop_on_escape(app);
    capture(window);
    press(app, "t", true);
    capture(window);
    type_in(app, window, "murky");
    if !picked_has_tag(app, "murky") {
        return Err("Ctrl+T then typing on Sounds added no tag: its box didn't get the keys".into());
    }
    sounds.invoke_bitwig_toggle();
    capture(window);
    let worked = keys_work(app, window, "once Favourites from Bitwig took the sound card's place, keys in its tag box");
    sounds.invoke_bitwig_toggle();
    capture(window);
    worked
}

/// Rediscover's How far afield pressed, then Tiles: the slider goes with its view. No filter picked, so the column
/// holds what it held in the shots.
fn slider_keys(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    let sounds = app.global::<mo_ui::SoundsData>();
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    start_on(app, Place::Sounds);
    sounds.invoke_clear();
    sounds.invoke_pick_view(4);
    capture(window);
    capture(window);
    let before = sounds.get_re_adventure();
    let (x, y) = AFIELD_SLIDER;
    click(window, x, y);
    capture(window);
    let moved = (sounds.get_re_adventure() - before).abs() >= 0.05;
    sounds.invoke_pick_view(0);
    capture(window);
    if !moved {
        return Err(format!("the press at {x}, {y} moved no slider on Rediscover; has the filter column moved?"));
    }
    keys_work(app, window, "once Rediscover's How far afield was pressed and Tiles took its view's place")
}

/// Settings › Shortcuts: Ctrl+F, words typed, then a key's button clicked: the next key is the action's, and the words
/// stay; Esc takes them off; words typed, then the General tab; a line listening, then the General tab. Play's key
/// goes back to its own, and the search is left empty.
fn shortcuts_keys(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    use mo_app::settings::Tab as SettingsTab;
    use slint::platform::Key;
    let settings = app.global::<mo_ui::SettingsData>();
    let tab = |tab: SettingsTab| {
        settings.invoke_tab_picked(tab.index() as i32);
        capture(window);
    };
    let typed = |words: &str| {
        for c in words.chars() {
            press(app, c.to_string(), false);
        }
        capture(window);
    };
    let searched = || settings.get_keys_search().to_string();
    // Esc's first step stops what plays: nothing does here.
    mo_ui::sounds::stop_on_escape(app);
    let empty_search = || {
        settings.set_keys_search("".into());
        settings.invoke_keys_search_edited("".into());
        capture(window);
    };
    let mut wrong = Vec::new();
    start_on(app, Place::Settings);
    tab(SettingsTab::Shortcuts);
    empty_search();
    press(app, "f", true);
    typed("play");
    // Only with the keys in the search box (Ctrl+F) can the steps that need them there run; the rest run anyway.
    let found = searched() == "play";
    if !found {
        wrong.push(format!("Ctrl+F then typing on Settings › Shortcuts searched {:?}", searched()));
        settings.set_keys_search("play".into());
        settings.invoke_keys_search_edited("play".into());
        capture(window);
    }
    if found {
        settings.invoke_listen("app.play".into());
        capture(window);
        press(app, "p", false);
        capture(window);
        let given = mo_ui::shortcuts().key_of("app.play").to_owned();
        let words = searched();
        settings.invoke_default_key("app.play".into());
        capture(window);
        if given != "P" || words != "play" {
            wrong.push(format!(
                "P after typing in Shortcuts' search and clicking Play's key gave it {given:?}, the search saying \
                 {words:?}"
            ));
        }
    }
    press(app, Key::Escape, false);
    capture(window);
    if !searched().is_empty() {
        wrong.push(format!("Esc on Shortcuts left its search's words {:?}", searched()));
        empty_search();
    }
    // Words typed in the search, then the General tab: the box goes with its tab.
    if found {
        press(app, "f", true);
        typed("x");
        tab(SettingsTab::General);
        let worked = keys_work(app, window, "once Settings' General tab took Shortcuts' place, keys in its search box");
        wrong.extend(worked.err());
    }
    // A line listening, then the General tab: it listens no more.
    tab(SettingsTab::Shortcuts);
    empty_search();
    settings.invoke_listen("app.play".into());
    capture(window);
    tab(SettingsTab::General);
    if mo_ui::settings::listening() {
        wrong.push("Shortcuts went on listening for a key once Settings' General tab showed".into());
        press(app, Key::Escape, false);
    }
    if wrong.is_empty() { Ok(()) } else { Err(wrong.join("; ")) }
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
        show_place(app, place);
        // Once to lay the page out at this width (the tiles' width tells Sounds how many fit in a row), then for real.
        capture_sized(window, WIDE, Duration::from_millis(400));
        let pixels = capture_sized(window, WIDE, Duration::from_millis(400));
        let path = out.join(format!("wide-{}.png", slug(place.name())));
        image::save_buffer(&path, &pixels, WIDE.0, WIDE.1, image::ExtendedColorType::Rgb8)?;
        println!("{}", path.display());
    }
    // A card dragged narrower than it's shown in a wide window (his F12 notes of 25 Nov, 10:03: every card but the
    // song's stuck): the list grows past its usual widest to take the room, instead of the card staying as it was.
    show_place(app, Place::Presets);
    let presets = app.global::<mo_ui::PresetsData>();
    let kept = presets.get_panel_width();
    capture_sized(window, WIDE, Duration::from_millis(400));
    let usual = capture_sized(window, WIDE, Duration::from_millis(400));
    presets.set_panel_width(app.global::<mo_ui::Widths>().invoke_usual("presets.card".into()) + 300.0);
    capture_sized(window, WIDE, Duration::from_millis(400));
    let narrower = capture_sized(window, WIDE, Duration::from_millis(400));
    presets.set_panel_width(kept);
    if usual == narrower {
        return Err("wide-presets-card-dragged.png: a card dragged to 300 px over its usual width looks the same in a wide window".into());
    }
    let path = out.join("wide-presets-card-dragged.png");
    image::save_buffer(&path, &narrower, WIDE.0, WIDE.1, image::ExtendedColorType::Rgb8)?;
    println!("{}", path.display());
    // A 1920 px laptop screen at Windows' 125%, the window filling it: the MIDI list is 620 px wide there, too narrow
    // for the From column beside the part's name, so it takes the narrow layout (the 26 Nov review's MZ-2).
    const LAPTOP: (u32, u32) = (1536, 864);
    window.set_size(PhysicalSize::new(LAPTOP.0, LAPTOP.1));
    show_place(app, Place::Midi);
    capture_sized(window, LAPTOP, Duration::from_millis(400));
    let laptop = capture_sized(window, LAPTOP, Duration::from_millis(400));
    let path = out.join("midi-laptop-125.png");
    image::save_buffer(&path, &laptop, LAPTOP.0, LAPTOP.1, image::ExtendedColorType::Rgb8)?;
    println!("{}", path.display());
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
    pick_sound(app, 0);
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
    pick_sound(app, 0);
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
    start_on(app, Place::Curves);
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

/// `place` on show without the keys moving: the rail's light, and the Presets page's section for Presets or Curves.
fn show_place(app: &mo_ui::AppWindow, place: Place) {
    use mo_app::presets::Section;
    match place {
        Place::Presets => mo_ui::presets::show_section(app, Section::Sounds),
        Place::Curves => mo_ui::presets::show_section(app, Section::Curves),
        _ => {}
    }
    app.set_place(place.index() as i32);
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

/// Picks the sound on `row` of the Sounds page and, with Autoplay on, plays it from its start, as a click on it did
/// before a click on the sound playing stopped it: what plays stops first (Esc's first step). For a check or a shot
/// that picks a sound to look at it; [`stopping_by_click`] clicks the one playing.
fn pick_sound(app: &mo_ui::AppWindow, row: i32) {
    mo_ui::sounds::stop_on_escape(app);
    app.global::<mo_ui::SoundsData>().invoke_select(row);
}

/// One more press of `key` from holding it down (the keyboard's repeat).
fn held(app: &mo_ui::AppWindow, key: impl Into<slint::SharedString>) {
    app.window().dispatch_event(slint::platform::WindowEvent::KeyPressRepeated { text: key.into() });
    slint::platform::update_timers_and_animations();
}

/// The Presets page: made-up sounds with one picked, then the Curves page with a curve picked; a sound and a curve
/// dragged out from their lines.
fn presets_shots(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    graphite: &Look,
    shots: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), Box<dyn std::error::Error>> {
    show_place(app, Place::Presets);
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
    shots.push(("filter-menu.png".into(), after.clone()));
    // Its search has the keys as it opens: typing narrows the lines to those holding the words.
    for k in ["p", "h"] {
        press(app, k, false);
    }
    let searched = capture(window);
    if searched == after {
        return Err(
            "filter-search.png: typing in the Plugin menu didn't narrow it; has its search lost the keys?".into()
        );
    }
    shots.push(("filter-search.png".into(), searched));
    click(window, 1000.0, 40.0);
    data.invoke_clear();
    mo_ui::presets::pick_row(app, 3);
    shots.push(("presets-picked.png".into(), capture(window)));
    // A song's Serum 1 sound: its card offers Put in Serum 2. Then the line before is picked again for the shots after.
    mo_ui::presets::pick_row(app, 0);
    shots.push(("presets-serum.png".into(), capture(window)));
    mo_ui::presets::pick_row(app, 3);
    mo_ui::presets::show_audition_sample(app, true);
    shots.push(("presets-auditioning.png".into(), capture(window)));
    mo_ui::presets::show_audition_sample(app, false);
    // A sound dragged out onto a track from its line: dimmed while it's carried, with the card that follows the mouse
    // (a Bitwig preset made of it).
    let still = capture(window);
    data.set_lifted_row(1);
    let lifted = capture(window);
    let card = mo_ui::presets::card_at_row(app, 1).ok_or("presets-drag.png: no sound on the second line")?;
    let (x, y, w, h) = changed_box(&still, &lifted).ok_or("presets-drag.png: the second line didn't dim")?;
    shots.push(("presets-drag.png".into(), with_card(lifted, &card, graphite, (x + w / 3, y + h / 2 + 20))));
    data.set_lifted_row(-1);
    capture(window);
    show_place(app, Place::Curves);
    shots.push(("curves-list.png".into(), capture(window)));
    mo_ui::presets::pick_row(app, 0);
    shots.push(("curves-picked.png".into(), capture(window)));
    // Hear: the play line on the picture and its marker on the ruler, the button saying Stop.
    mo_ui::curve_editor::show_playing_sample(app, Some(5.5));
    shots.push(("curves-hear.png".into(), capture(window)));
    mo_ui::curve_editor::show_playing_sample(app, None);
    capture(window);
    // The mouse on a point: it grows, and the hint line says what it does.
    let keys = mo_app::curve_touch::Keys::default();
    mo_ui::curve_editor::mouse_sample(app, 0, 12.0, 0.95, keys);
    shots.push(("curves-point.png".into(), capture(window)));
    // The point dragged: the old shape stays faint behind, the read-out says where it is, Undo lights.
    mo_ui::curve_editor::mouse_sample(app, 1, 12.0, 0.95, keys);
    mo_ui::curve_editor::mouse_sample(app, 0, 9.0, 0.7, keys);
    shots.push(("curves-point-drag.png".into(), capture(window)));
    mo_ui::curve_editor::mouse_sample(app, 2, 9.0, 0.7, keys);
    mo_ui::curve_editor::mouse_sample(app, 4, 9.0, 0.7, keys);
    shots.push(("curves-edited.png".into(), capture(window)));
    // Another curve picked and then this one again: it comes back with its edit (this once closed the app).
    mo_ui::presets::pick_row(app, 1);
    mo_ui::presets::pick_row(app, 0);
    capture(window);
    if !mo_ui::curve_editor::undo(app, true) {
        return Err("curves-edited.png: the dragged point left nothing to undo".into());
    }
    capture(window);
    // Empty space dragged: a stretch of time is highlighted, with its frame, its bar and its handles.
    mo_ui::curve_editor::mouse_sample(app, 1, 4.0, 0.8, keys);
    mo_ui::curve_editor::mouse_sample(app, 0, 8.0, 0.8, keys);
    mo_ui::curve_editor::mouse_sample(app, 2, 8.0, 0.8, keys);
    if mo_ui::curve_editor::highlight_sample() != Some((4.0, 8.0)) {
        return Err("curves-highlight.png: dragging empty space didn't highlight beats 4 to 8".into());
    }
    mo_ui::curve_editor::mouse_sample(app, 0, 8.0, 0.3, keys);
    shots.push(("curves-highlight.png".into(), capture(window)));
    // Its right side dragged in: the part plays at double speed, and holds where it ends.
    mo_ui::curve_editor::mouse_sample(app, 1, 8.0, 0.3, keys);
    mo_ui::curve_editor::mouse_sample(app, 0, 6.0, 0.3, keys);
    shots.push(("curves-highlight-speed.png".into(), capture(window)));
    mo_ui::curve_editor::mouse_sample(app, 2, 6.0, 0.3, keys);
    if mo_ui::curve_editor::highlight_sample() != Some((4.0, 6.0)) || !mo_ui::curve_editor::undo(app, true) {
        return Err("curves-highlight-speed.png: the dragged side didn't speed the part up".into());
    }
    // The highlight kept as a clip: it shows on the strip over the picture, picked.
    if mo_ui::curve_editor::highlight_sample() != Some((4.0, 8.0))
        || !mo_ui::curve_editor::act(app, mo_app::presets::curve_act::KEEP_CLIP)
    {
        return Err("curves-clip.png: the highlight wasn't kept as a clip".into());
    }
    shots.push(("curves-clip.png".into(), capture(window)));
    if !mo_ui::curve_editor::undo(app, true) {
        return Err("curves-clip.png: keeping the clip left nothing to undo".into());
    }
    mo_ui::curve_editor::mouse_sample(app, 1, 14.0, 0.9, keys);
    mo_ui::curve_editor::mouse_sample(app, 2, 14.0, 0.9, keys);
    mo_ui::curve_editor::mouse_sample(app, 4, 14.0, 0.9, keys);
    capture(window);
    // Randomise: a new curve from parts of the curves shown, each part a clip named after the curve it came from.
    if !mo_ui::presets::randomise_with(app, 7) {
        return Err("curves-random.png: nothing was rolled".into());
    }
    shots.push(("curves-random.png".into(), capture(window)));
    if !mo_ui::curve_editor::undo(app, true) {
        return Err("curves-random.png: the roll left nothing to undo".into());
    }
    capture(window);
    // Hear with the metronome on, four steps slower (100 BPM), the play line part way.
    mo_ui::curve_editor::set_hear(app, Some(true), None);
    for _ in 0..4 {
        mo_ui::curve_editor::set_hear(app, None, Some(-1));
    }
    mo_ui::curve_editor::show_playing_sample(app, Some(5.5));
    shots.push(("curves-hear.png".into(), capture(window)));
    mo_ui::curve_editor::show_playing_sample(app, None);
    mo_ui::curve_editor::set_hear(app, Some(false), None);
    mo_ui::curve_editor::set_hear(app, None, Some(0));
    capture(window);
    // Steps painted with Ctrl held, the stroke still in hand: the old curve faint behind, the read-out by the mouse.
    let paint = mo_app::curve_touch::Keys { ctrl: true, ..keys };
    mo_ui::curve_editor::mouse_sample(app, 1, 5.1, 0.7, paint);
    mo_ui::curve_editor::mouse_sample(app, 0, 6.1, 0.3, paint);
    mo_ui::curve_editor::mouse_sample(app, 0, 7.1, 0.9, paint);
    mo_ui::curve_editor::mouse_sample(app, 0, 8.6, 0.5, paint);
    shots.push(("curves-paint.png".into(), capture(window)));
    mo_ui::curve_editor::mouse_sample(app, 2, 8.6, 0.5, paint);
    if !mo_ui::curve_editor::undo(app, true) {
        return Err("curves-paint.png: the stroke left nothing to undo".into());
    }
    mo_ui::curve_editor::mouse_sample(app, 4, 12.0, 0.9, keys);
    capture(window);
    // The swing mark dragged a third of a step later (triplet swing), still in hand.
    mo_ui::curve_editor::ruler_sample(app, 1, 0.25);
    mo_ui::curve_editor::ruler_sample(app, 0, 0.25 + 0.25 / 3.0);
    shots.push(("curves-swing.png".into(), capture(window)));
    mo_ui::curve_editor::ruler_sample(app, 2, 0.25 + 0.25 / 3.0);
    if !mo_ui::curve_editor::undo(app, true) {
        return Err("curves-swing.png: the swing left nothing to undo".into());
    }
    mo_ui::curve_editor::ruler_sample(app, 4, 0.0);
    capture(window);
    // Joining curves: the + on the second line adds its curve at the end of the one in the editor, as a clip.
    if !mo_ui::presets::add_curve(app, 1) {
        return Err("curves-joined.png: the second line's curve wasn't added".into());
    }
    shots.push(("curves-joined.png".into(), capture(window)));
    if !mo_ui::curve_editor::undo(app, true) {
        return Err("curves-joined.png: the join left nothing to undo".into());
    }
    capture(window);
    // Points picked in a box (Alt-drag over empty space), the box still in hand.
    let pick = mo_app::curve_touch::Keys { alt: true, ..keys };
    mo_ui::curve_editor::mouse_sample(app, 0, 1.0, 0.97, pick);
    mo_ui::curve_editor::mouse_sample(app, 1, 1.0, 0.97, pick);
    mo_ui::curve_editor::mouse_sample(app, 0, 15.0, 0.0, pick);
    if mo_ui::curve_editor::picked() == 0 {
        return Err("curves-picked.png: the box picked no points".into());
    }
    shots.push(("curves-picked.png".into(), capture(window)));
    mo_ui::curve_editor::mouse_sample(app, 2, 15.0, 0.0, pick);
    mo_ui::curve_editor::escape(app);
    mo_ui::curve_editor::mouse_sample(app, 4, 12.0, 0.9, keys);
    capture(window);
    // A curve dragged out onto an automation lane from its line, its shape on the card.
    let still = capture(window);
    data.set_curve_lifted_row(1);
    let lifted = capture(window);
    let card = mo_ui::presets::curve_card_at_row(1).ok_or("curves-drag.png: no curve on the second line")?;
    let (x, y, w, h) = changed_box(&still, &lifted).ok_or("curves-drag.png: the second line didn't dim")?;
    shots.push(("curves-drag.png".into(), with_card(lifted, &card, graphite, (x + w / 3, y + h / 2 + 20))));
    data.set_curve_lifted_row(-1);
    capture(window);
    show_place(app, Place::Presets);
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

/// Every list keeps its place (Solomon's F12 note of 11 Nov): turned down, its page left and shown again, it's where it
/// was, and the button over its bottom right corner takes it back to its top, as Home does. Before, a page made again
/// put its list back at its top. In the short window, where every list turns; each is left at its top for what follows.
fn lists_keep_their_place(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    use slint::platform::Key;
    let sounds = app.global::<mo_ui::SoundsData>();
    let view_before = sounds.get_view();
    window.set_size(PhysicalSize::new(WIDTH, SHORT));
    let mut wrong = Vec::new();
    let ((x, y), list) = (SHORT_LIST_WHEEL, SHORT_LIST);
    for (name, place, view, (button_x, button_y)) in LISTS {
        start_on(app, place);
        // Presets shows its sounds: the place itself picks the section.
        if place == Place::Sounds {
            sounds.invoke_pick_view(view);
            // With a sound picked, so its card is at the side, as the Sounds lines in LISTS have it. The Kits
            // shots let go of the picked sound when the kit they rolled at random didn't hold it; then the tiles
            // took the card's room too, their button sat at the far right, and the click picked a tile instead
            // (the check failed on and off, 11 Nov).
            if !sounds.get_panel_shown() {
                pick_sound(app, 0);
            }
        }
        // Drawn once first: a list made for another view turns to the picked sound a turn later.
        hover(window, -1.0, -1.0);
        capture(window);
        press(app, Key::Home, false);
        let top = capture(window);
        scroll(window, x, y, -2.0 * SHORT_TURN);
        hover(window, -1.0, -1.0);
        let down = capture(window);
        if same_in(&top, &down, list) {
            wrong.push(format!("{name}: the wheel at {x}, {y} moved nothing (is it still over the list?)"));
            continue;
        }
        start_on(app, Place::Backup);
        capture(window);
        start_on(app, place);
        // Drawn twice: the page is made in the first draw, and what it does a turn later shows in the second.
        capture(window);
        if !same_in(&down, &capture(window), list) {
            wrong.push(format!("{name}: shown again after another page, it isn't where it was"));
        }
        click(window, button_x, button_y);
        hover(window, -1.0, -1.0);
        if !same_in(&top, &capture(window), list) {
            wrong.push(format!("{name}: the button at {button_x}, {button_y} didn't take it back to its top"));
        }
        scroll(window, x, y, -2.0 * SHORT_TURN);
        hover(window, -1.0, -1.0);
        if same_in(&top, &capture(window), list) {
            wrong.push(format!("{name}: the wheel moved nothing after the button"));
            continue;
        }
        press(app, Key::Home, false);
        if !same_in(&top, &capture(window), list) {
            wrong.push(format!("{name}: Home didn't take it back to its top"));
        }
    }
    sounds.invoke_pick_view(view_before);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    start_on_songs(app);
    capture(window);
    if wrong.is_empty() { Ok(()) } else { Err(format!("A list's place: {}", wrong.join("; "))) }
}

/// Each list [`lists_keep_their_place`] turns, in the short window: its name, its page with the Sounds view, and the
/// middle of its button (the wheel and the comparing are over [`SHORT_LIST`], as for [`opened_in_sight`]); on Sounds,
/// with a sound's card open. Not the Presets page's curves: the sample's four fit in the window.
const LISTS: [(&str, Place, i32, (f32, f32)); 6] = [
    ("Songs", Place::Songs, 0, (737.0, 556.0)),
    ("MIDI", Place::Midi, 0, (881.0, 556.0)),
    ("Plugins", Place::Plugins, 0, (881.0, 556.0)),
    ("Presets", Place::Presets, 0, (901.0, 556.0)),
    ("Sounds tiles", Place::Sounds, 0, (901.0, 556.0)),
    ("Sounds List", Place::Sounds, 1, (901.0, 556.0)),
];

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

/// The folder tree beside the filters (Solomon, 24 Nov 2026): open with two folders picked and one hidden, a folder
/// found by its search box, a folder picked alone, and folded away to its tab with picks on.
fn folder_shots(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    shots: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let data = app.global::<mo_ui::SoundsData>();
    data.invoke_close_panel();
    data.invoke_pick_view(0);
    mo_ui::sounds::fold_folders_for_screens(app, false);
    data.invoke_tree_opened(r"D:\Samples".into());
    // Before and after a tick: no line moves (its chip goes under the lines), so the box stays under the mouse.
    capture(window);
    shots.push(("sounds-folders-before-tick.png".into(), capture(window)));
    data.invoke_tree_toggled(r"D:\Samples\Drums".into());
    capture(window);
    shots.push(("sounds-folders-after-tick.png".into(), capture(window)));
    data.invoke_tree_toggled(r"D:\Samples\Loops".into());
    data.invoke_tree_menu_asked(r"D:\Samples\FX".into());
    data.invoke_tree_menu_chosen("hide".into());
    capture(window);
    shots.push(("sounds-folders.png".into(), capture(window)));
    // The filter column and the tree, each dragged narrower by its right edge (at 1440 x 900 the list and card already
    // have less than their least beside them, so neither may go wider here).
    let widths = app.global::<mo_ui::Widths>();
    let held = drag_seam(app, window, FILTERS_SEAM, -40.0, 1.0, "sounds.filters", || widths.get_sounds_filters())?;
    shots.push(("sounds-filters-narrower.png".into(), held));
    let held = drag_seam(app, window, TREE_SEAM, -80.0, 1.0, "sounds.folders", || widths.get_sounds_folders())?;
    shots.push(("sounds-folders-narrower.png".into(), held));
    // A type picked too: the tree keeps only the folders with kicks, each counting its kicks.
    data.invoke_toggle_choice("type".into(), "Kick".into());
    capture(window);
    shots.push(("sounds-folders-kicks.png".into(), capture(window)));
    data.invoke_toggle_choice("type".into(), "Kick".into());
    // Many picks: every one in the strip under the list, wrapping; what's in them under the tree.
    let more = ["Bass", "Brass", "Keys", "Leads", "Pads", "Perc", "Strings", "Vox"];
    for f in more {
        data.invoke_tree_toggled(format!(r"D:\Samples\{f}").into());
    }
    capture(window);
    shots.push(("sounds-folders-strip.png".into(), capture(window)));
    // Many hidden folders too: three rows of chips in sight, the rest a scroll away (the whole-app review of 26 Nov,
    // ST-9: the strip grew with no limit and took the list's room). Made-up folders, shown again after.
    let old: Vec<String> = (1..=40).map(|n| format!(r"E:\Old packs\Bounce pack {n:02}")).collect();
    // Each one hidden, or shown again when it is.
    let toggle_old = || {
        for path in &old {
            data.invoke_tree_menu_asked(path.as_str().into());
            data.invoke_tree_menu_chosen("hide".into());
        }
    };
    toggle_old();
    capture(window);
    let many = capture(window);
    // The wheel over the chips turns the strip down to the last of them.
    let (x, y) = STRIP_WHEEL;
    scroll(window, x, y, -10.0 * PAGE_DOWN);
    let down = capture(window);
    scroll(window, x, y, 10.0 * PAGE_DOWN);
    hover(window, -1.0, -1.0);
    if down == many {
        return Err(format!("sounds-folders-strip-down.png: the wheel at {x}, {y} moved nothing; is the strip there?"));
    }
    shots.push(("sounds-folders-strip-many.png".into(), many));
    shots.push(("sounds-folders-strip-down.png".into(), down));
    toggle_old();
    for f in more {
        data.invoke_tree_toggled(format!(r"D:\Samples\{f}").into());
    }
    // Found by its search box: the deep pack folder, a chain of folders that each hold only the next one.
    data.set_tree_search("processed".into());
    data.invoke_tree_search_edited("processed".into());
    data.invoke_tree_menu_asked(
        r"D:\Samples\Packs\Vintage_Drum_Machines_Collection_Vol_2\Kicks_And_Bass_Drums\Processed".into(),
    );
    data.invoke_tree_menu_chosen("just".into());
    capture(window);
    shots.push(("sounds-folders-search.png".into(), capture(window)));
    data.set_tree_search("".into());
    data.invoke_tree_search_edited("".into());
    // A scan going through the sample folder: its line says so until the scan has listened to its new sounds.
    // Its own library name, and the screens library's state put back after: later shots read the newest scan state.
    let scan_db = std::path::Path::new("folder-tree-shot");
    let before = mo_app::sound_scan::latest_state();
    let scanning = mo_app::sound_scan::ScanState {
        running: true,
        looking: true,
        scanning: vec![r"D:\Samples".into()],
        ..Default::default()
    };
    mo_app::sound_scan::show_state(scan_db, scanning);
    data.invoke_tree_search_edited("".into());
    capture(window);
    shots.push(("sounds-folders-scanning.png".into(), capture(window)));
    mo_app::sound_scan::show_state(scan_db, mo_app::sound_scan::ScanState::default());
    if let Some(before) = before {
        mo_app::sound_scan::show_state(std::path::Path::new("screens-sounds.db"), before);
    }
    data.invoke_tree_search_edited("".into());
    data.invoke_tree_fold();
    capture(window);
    shots.push(("sounds-folders-folded.png".into(), capture(window)));
    // Back as the other shots expect: nothing picked or hidden, folded away.
    data.invoke_tree_cleared();
    data.invoke_tree_taken_off(r"D:\Samples\FX".into(), true);
    mo_ui::sounds::fold_folders_for_screens(app, true);
    Ok(())
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
    pick_sound(app, 6);
    data.set_played(0.35);
    data.set_tile_played(0.35);
    shots.push(("sounds-panel.png".into(), capture(window)));
    // Sounds like this leads on (Solomon's F12 note, 25 Nov 2026: clicking its first line went round in a circle): a
    // click on its first line picks that sound, and its own list offers neither it nor the sounds it came through. By
    // name, as the made-up sounds' names all differ.
    let mut came = vec![data.get_picked().name.to_string()];
    for _ in 0..4 {
        let first = data.get_alike().row_data(0).ok_or("sounds like this: no line to click")?;
        data.invoke_pick_alike(first.place);
        let now = data.get_picked().name.to_string();
        let offered: Vec<String> = data.get_alike().iter().map(|line| line.name.to_string()).collect();
        if offered.contains(&now) || came.iter().any(|c| offered.contains(c)) {
            return Err(format!("sounds like this: {now} offers itself or the way back: {offered:?}, came {came:?}"));
        }
        came.push(now);
    }
    pick_sound(app, 6);
    data.set_played(0.35);
    data.set_tile_played(0.35);
    part_shots(app, window, shots)?;
    // A sound deep in a pack, with folder names too long for a line, and six tags: its card wraps them, at the usual
    // width and as narrow as it goes, with nothing cut off on the right. Found by name, as the shuffled order changes
    // with every run.
    data.set_search("Boom Kick".into());
    data.invoke_search_edited("Boom Kick".into());
    pick_sound(app, 0);
    capture(window);
    shots.push(("sounds-panel-long.png".into(), capture(window)));
    // The mouse resting on that path, cut short at its start: its tip shows the whole of it.
    hover(window, CARD_PATH.0, CARD_PATH.1);
    let before = capture_after(window, Duration::from_millis(150));
    let after = capture_after(window, Duration::from_millis(750));
    hover(window, -1.0, -1.0);
    if before == after {
        return Err(format!("sounds-path-tip.png: no tip came up at {CARD_PATH:?}; has the card's path moved?"));
    }
    shots.push(("sounds-path-tip.png".into(), after));
    // The same card's switch on its other side: the songs that use the sound, newest first, each once by its newest
    // save that uses it. They show with no turning, as the card's path keeps to one line. Then Every save: each save.
    data.set_card_list(1);
    capture(window);
    shots.push(("sounds-panel-used-in.png".into(), capture(window)));
    data.set_every_save(true);
    capture(window);
    shots.push(("sounds-panel-every-save.png".into(), capture(window)));
    data.set_every_save(false);
    data.set_card_list(0);
    let wide = data.get_panel_width();
    data.set_panel_width(320.0);
    capture(window);
    shots.push(("sounds-panel-narrow.png".into(), capture(window)));
    // As narrow, the songs with Every save on: its chip has a line of its own, so the card doesn't cut it off.
    data.set_card_list(1);
    data.set_every_save(true);
    capture(window);
    shots.push(("sounds-panel-narrow-every-save.png".into(), capture(window)));
    data.set_every_save(false);
    data.set_card_list(0);
    data.set_panel_width(wide);
    // The seam left of the card: its tip on the mouse, then dragged 100 px left (the card that much wider, its width
    // and limits showing while it's held), let go, and put back to its usual width as a double-click does.
    hover(window, CARD_SEAM.0, CARD_SEAM.1);
    capture(window);
    shots.push(("sounds-seam-tip.png".into(), capture(window)));
    let (seam_at, button) =
        (slint::LogicalPosition::new(CARD_SEAM.0, CARD_SEAM.1), slint::platform::PointerEventButton::Left);
    let dragged_to = slint::LogicalPosition::new(CARD_SEAM.0 - 100.0, CARD_SEAM.1);
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position: seam_at, button });
    window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: dragged_to });
    capture(window);
    shots.push(("sounds-seam-dragging.png".into(), capture(window)));
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position: dragged_to, button });
    let dragged = data.get_panel_width();
    // Never dragged, the card is given 0 and shown at its usual width in this window.
    let usual = app.global::<mo_ui::Widths>().invoke_usual("sounds.card".into());
    if (dragged - usual - 100.0).abs() > 1.0 {
        return Err(format!(
            "sounds seam: dragging it 100 px left made the card {dragged} wide, from {usual}; has the seam moved from {CARD_SEAM:?}?"
        ));
    }
    app.global::<mo_ui::Widths>().invoke_reset("sounds.card".into());
    if data.get_panel_width() != wide {
        return Err(format!("sounds seam: the usual width put the card at {}, not {wide}", data.get_panel_width()));
    }
    hover(window, -1.0, -1.0);
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
    // One type, a key and the keys that mix well with it, and a span of tempos.
    data.invoke_toggle_choice("type".into(), second);
    data.invoke_toggle_choice("key".into(), "A minor".into());
    data.invoke_toggle("mix-well".into());
    data.invoke_tempo_picked(110, 150);
    let filtered = capture(window);
    if filtered == before {
        return Err("sounds-filtered.png: the key and tempo picks changed nothing".into());
    }
    shots.push(("sounds-filtered.png".into(), filtered));
    data.invoke_clear();
    // The Tempo slider's higher handle, dragged from its end to 150 BPM: the lower one stays where it is.
    let button = slint::platform::PointerEventButton::Left;
    let at = |x: f32| slint::LogicalPosition::new(x, TEMPO_TRACK_Y);
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position: at(TEMPO_HIGH_END_X), button });
    hover(window, TEMPO_150_X, TEMPO_TRACK_Y);
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position: at(TEMPO_150_X), button });
    hover(window, -1.0, -1.0);
    if (data.get_tempo_low(), data.get_tempo_high()) != (60, 150) {
        return Err(format!(
            "The Tempo slider's higher handle wasn't dragged to 150: it says {} to {}. Has the filter column moved?",
            data.get_tempo_low(),
            data.get_tempo_high()
        ));
    }
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
    // Favourites, then the sounds most like them too: the slider halfway (7 like each).
    // A tall window, so the slider under Favourites shows (at the foot of the filter column).
    window.set_size(PhysicalSize::new(WIDTH, TALL_FAVOURITES));
    data.invoke_toggle("favourites".into());
    let only = capture_sized(window, (WIDTH, TALL_FAVOURITES), Duration::from_millis(400));
    data.invoke_like_favourites_set(0.4);
    let like = capture_sized(window, (WIDTH, TALL_FAVOURITES), Duration::from_millis(400));
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    if like == only {
        return Err("sounds-like-favourites.png: the Sounds like them slider changed nothing".into());
    }
    // The window's lowest [`HEIGHT`] rows, where the slider is, as the shots are all that size.
    let foot = |shot: Vec<u8>| shot[((TALL_FAVOURITES - HEIGHT) * WIDTH * 3) as usize..].to_vec();
    shots.push(("sounds-favourites.png".into(), foot(only)));
    shots.push(("sounds-like-favourites.png".into(), foot(like)));
    data.invoke_like_favourites_set(0.0);
    data.invoke_toggle("favourites".into());
    // Favourites from Bitwig: its card in the sound card's place, some multisamples ticked, then after a look.
    let names =
        ["Clap 1 Select", "Kicks 1 Select", "OHH 2 Select", "rim 1", "Snare 3 Select", "Stylus Drums - Snare 2"];
    let ticked = ["Kicks 1 Select", "rim 1", "Snare 3 Select"];
    let shot = |said, missing: &'static [&'static str]| mo_ui::sounds::BitwigShot {
        names: &names,
        ticked: &ticked,
        said,
        missing,
        saveable: missing.len(),
    };
    mo_ui::sounds::show_bitwig_for_screens(app, Some(shot("", &[])));
    shots.push(("sounds-bitwig-favourites.png".into(), capture(window)));
    const MISSING: [&str; 3] = [
        "rim 1 › Rim 07.wav: no sound by this name",
        "Snare 3 Select › Hmy_Snr.wav: same name, different audio",
        "Snare 3 Select › Hyper Snare.wav: no sound by this name",
    ];
    let said = "Found 368 of 371 samples in your sound library: 370 sounds now favourites (12 already were). 210 \
        were given the type their multisample names. 40 were the same sound once the silence Bitwig added before and \
        after them was left out. 3 samples not found: 2 are only in the multisample (Bitwig may have recorded them, \
        or their files aren't in your sample folders); 1 has a sound of yours by its name, but different audio.";
    mo_ui::sounds::show_bitwig_for_screens(app, Some(shot(said, &MISSING)));
    shots.push(("sounds-bitwig-favourites-done.png".into(), capture(window)));
    mo_ui::sounds::show_bitwig_for_screens(app, None);
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
    scroll_bar_checks(app, window, shots)?;
    // Two years picked on the year strip.
    data.invoke_years_picked(1, 2);
    capture(window);
    shots.push(("sounds-years.png".into(), capture(window)));
    data.invoke_years_cleared();
    // The strip going by when each sound was first used in a song, not when its file was changed: picked in its menu
    // with the mouse, as a pick there was once lost when the menu closed (Solomon's beta note of 18 Nov).
    let before = capture(window);
    click(window, YEARS_BY_NAME.0, YEARS_BY_NAME.1);
    if capture(window) == before {
        return Err("sounds-years-first-used.png: the Years by menu didn't open; has the year strip moved?".into());
    }
    hover(window, YEARS_BY_FIRST_USED.0, YEARS_BY_FIRST_USED.1);
    capture(window);
    click(window, YEARS_BY_FIRST_USED.0, YEARS_BY_FIRST_USED.1);
    hover(window, -1.0, -1.0);
    capture(window);
    let by = data.get_years_by();
    if by != "First used" {
        return Err(format!("sounds-years-first-used.png: First used in the Years by menu left the years by {by}"));
    }
    shots.push(("sounds-years-first-used.png".into(), capture(window)));
    data.invoke_years_by_picked("Changed".into());
    data.invoke_pick_view(3);
    shots.push(("sounds-history.png".into(), capture(window)));
    data.invoke_close_panel();
    data.invoke_pick_view(2);
    pick_sound(app, 3);
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
    sweep(&data);
    capture(window);
    shots.push(("sounds-galaxy-swept.png".into(), capture(window)));
    // The arrow keys on the map: Right goes to the nearest star that way, and it's picked.
    data.invoke_step(1, false);
    capture(window);
    shots.push(("sounds-galaxy-arrow.png".into(), capture(window)));
    // A line on the Swept card clicked, then Down: the card has the arrow keys and goes to the next sound on it.
    let (cx, cy) = SWEPT_LINE;
    click(window, cx, cy);
    // Away again, as the line's tip would come up over the next line under a mouse resting there.
    hover(window, -1.0, -1.0);
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
    // Flown in: the mini-map shows the whole map with a box round the part on show. From the whole map again: going
    // down the Swept card moved the map to its sound, and the zoom would have flown into an empty corner.
    data.set_galaxy_labels(slint::ModelRc::new(slint::VecModel::from(Vec::<mo_ui::GalaxyLabel>::new())));
    data.invoke_galaxy_reset();
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(16));
        capture(window);
    }
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
    tiles_keep_their_place(app, window)?;
    // Your say on the card, last of the Sounds shots as it stays on the page (the screenshots keep none of it): your
    // own type, a tag added and one taken away, and the heart taken off (the tile's goes too). The tiles
    // are still turned down to their end from the check before: the search leaves one, at the top.
    data.set_search("Boom Kick".into());
    data.invoke_search_edited("Boom Kick".into());
    pick_sound(app, 0);
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
    folder_shots(app, window, shots)?;
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
            pick_sound(app, 0);
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

/// A resize that changes how many tiles fit in a row keeps the tiles you were looking at in sight (Solomon's note of
/// 15 Nov 2026: "strange behaviour when i scroll fast and resize"). With none picked, the first tile in sight is in
/// the first row in sight still in the narrow window, and back in the wide one the tiles are where they were. They
/// used to stay turned as far down, which showed other tiles. Starts and ends on the tiles turned down to their end.
fn tiles_keep_their_place(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    let data = app.global::<mo_ui::SoundsData>();
    data.invoke_close_panel();
    capture(window);
    // The tiles of the first row in sight, as their places among the tiles. A row of tiles and the gap under it: 112
    // and 8 points (sounds.slint's TileSize, Space.sm).
    let first_row = || {
        let row = (-data.get_tiles_y() / 120.0).floor().max(0.0) as usize;
        data.get_tile_rows()
            .row_data(row)
            .map(|r| r.tiles.iter().map(|t| t.row).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let (turned, first) = (data.get_tiles_y(), first_row().first().copied());
    let Some(first) = first.filter(|_| turned < 0.0) else {
        return Err(format!("sounds tiles: not turned down ({turned} px), or no tile in sight"));
    };
    // Drawn a few times: the first draw finds the new count of tiles a row, the next lays out the rows made again for
    // it, and then the tiles turn.
    let settle = |size: (u32, u32)| {
        window.set_size(PhysicalSize::new(size.0, size.1));
        for _ in 0..4 {
            capture_sized(window, size, Duration::ZERO);
        }
    };
    settle((NARROW, HEIGHT));
    let narrow = first_row();
    settle((WIDTH, HEIGHT));
    let back = (data.get_tiles_y(), first_row());
    if !narrow.contains(&first) || back.1.first() != Some(&first) || (back.0 - turned).abs() > 0.5 {
        return Err(format!(
            "sounds tiles: the first in sight was {first} turned {turned} px; the first row in sight in the narrow \
             window held {narrow:?}, and back in the wide one {:?} turned {} px",
            back.1, back.0
        ));
    }
    // Another view drawn before the window changes: the tiles are gone then, so only the tiles made again see the new
    // width. Changed while they're still there, it's their own resize that keeps them in sight, not the tiles made
    // again, and taking the fixes below out changed nothing (the check passed without them).
    let away_from_tiles = |view: i32| {
        data.invoke_pick_view(view);
        for _ in 0..2 {
            capture_sized(window, (WIDTH, HEIGHT), Duration::ZERO);
        }
        settle((NARROW, HEIGHT));
    };
    // The window made narrower while the List is on show: back on Tiles, with fewer a row, the same tiles are in
    // sight. They turned to their top (the whole-app review of 19 Nov, SN-9), as the tiles made again said they were
    // turned to their top; so would closing the card on the List, which widens the tiles.
    away_from_tiles(1);
    data.invoke_pick_view(0);
    for _ in 0..4 {
        capture_sized(window, (NARROW, HEIGHT), Duration::ZERO);
    }
    let (away, in_sight) = (data.get_tiles_y(), first_row());
    settle((WIDTH, HEIGHT));
    if !in_sight.contains(&first) {
        return Err(format!(
            "sounds tiles: the first in sight was {first}; narrowed on the List and back on Tiles, the first row in \
             sight held {in_sight:?}, turned {away} px"
        ));
    }
    // The same from the Galaxy, which makes no list: the tiles are made again for the view they were made for last.
    // From their first frame back: they came back where they were kept with the new count, which showed other tiles,
    // and turned only a turn later (the code review of the SN-9 fix).
    away_from_tiles(2);
    data.invoke_pick_view(0);
    capture_sized(window, (NARROW, HEIGHT), Duration::ZERO);
    let (at_once, first_frame) = (data.get_tiles_y(), first_row());
    for _ in 0..3 {
        capture_sized(window, (NARROW, HEIGHT), Duration::ZERO);
    }
    let (away, in_sight) = (data.get_tiles_y(), first_row());
    settle((WIDTH, HEIGHT));
    if !in_sight.contains(&first) || !first_frame.contains(&first) {
        return Err(format!(
            "sounds tiles: the first in sight was {first}; narrowed on the Galaxy and back on Tiles, the first row in \
             sight held {first_frame:?} turned {at_once} px at first, then {in_sight:?} turned {away} px"
        ));
    }
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
    let backup = sound_lab_copies(PathBuf::new(), PathBuf::new(), PathBuf::new(), PathBuf::new());
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
    list.set_used(HashMap::from([(1, Usage { projects: 4, songs: 3, ..Usage::default() })]));
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
        // A render that came out blank: silent all through, so Hide silent sounds leaves it out, as it starts.
        ("Misc", "Blank Render", "", 2.0, None, None),
    ];
    let day = |d: u32, h: u32| {
        chrono::NaiveDate::from_ymd_opt(2026, 10, d).and_then(|d| d.and_hms_opt(h, 15, 0)).unwrap_or_default()
    };
    let sounds = SOUNDS
        .iter()
        .enumerate()
        .map(|(i, &(folder, name, ty, seconds, bpm, key))| {
            let is_loop = bpm.is_some();
            let silent = name == "Blank Render";
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
            // The picked one sits deep in a pack, with folder names too long for a line: its card shows as much of the
            // path's end as fits on one.
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
                    peak_db: if silent { -120.0 } else { 0.0 },
                    waveform: if silent { vec![0.0; 160] } else { waveform }.into(),
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
    // Twelve made-up saves of seven songs for the card's Used in list, saved a week or so apart, newest first (names
    // invented here, none of Solomon's: rule 4). Boom Kick, the card's sound, is in saves 6 to 8: two of Static bloom's
    // and Late bus, so its card lists two songs, and Every save the three saves.
    let saves: [(&str, &str, bool); 12] = [
        ("Night drive_4", "Night drive", false),
        ("Sunday loop", "Sunday loop", false),
        ("Glass rooms", "Glass rooms", true),
        ("Night drive_3", "Night drive", false),
        ("Low tide_2", "Low tide", true),
        ("Paper planes", "Paper planes", false),
        ("Static bloom_3", "Static bloom", true),
        ("Late bus", "Late bus", false),
        ("Static bloom_2", "Static bloom", true),
        ("Night drive_2", "Night drive", false),
        ("Low tide", "Low tide", true),
        ("Static bloom", "Static bloom", true),
    ];
    let song_of = |id: i64| {
        let name = saves.get(usize::try_from(id).unwrap_or(usize::MAX)).map_or("", |s| s.1);
        saves.iter().position(|s| s.1 == name).map_or(u32::MAX, |at| at as u32)
    };
    // A few made-up songs using some of them, for the Used column and Most liked.
    let usage = (1..=29)
        .filter(|id| id % 3 != 0)
        .map(|id: i64| {
            let projects = (id * 7 % 11) as u32;
            // Which of the made-up saves above: each a different one.
            let mut project_ids: Vec<i64> = (0..i64::from(projects)).map(|n| (id * 3 + n) % 12).collect();
            project_ids.sort_unstable();
            // A song's saves count once.
            let mut songs: Vec<u32> = project_ids.iter().map(|&p| song_of(p)).collect();
            songs.sort_unstable();
            songs.dedup();
            let songs = songs.len() as u32;
            // First used a year or so apart, back to 2019.
            let first_used =
                chrono::NaiveDate::from_ymd_opt(2019 + (id % 8) as i32, 3, 1).and_then(|d| d.and_hms_opt(12, 0, 0));
            (
                id,
                mo_sounds::usage::Usage {
                    projects,
                    recent: projects / 2,
                    songs,
                    recent_songs: songs / 2,
                    years: vec![2025, 2026],
                    project_ids,
                    first_used,
                },
            )
        })
        .filter(|(_, u)| u.projects > 0)
        .collect();
    let now = chrono::Local::now().timestamp();
    let projects = (0i64..)
        .zip(saves)
        .map(|(id, (name, song, bitwig))| {
            let (daw, ending) = if bitwig { ("Bitwig Studio", "bwproject") } else { ("FL Studio", "flp") };
            let path = format!(r"C:\Music\{daw}\{name}.{ending}");
            let file = mo_library::ProjectFile {
                id,
                path,
                mtime: now - id * 9 * 86_400,
                song: song_of(id),
                song_name: song.into(),
                recent: id < 6,
                copy: false,
            };
            (id, file)
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
    let todo = |text: &str, done: bool, days: i64| mo_library::Todo {
        text: text.into(),
        done,
        added: (now - Days::days(days)).and_utc().timestamp(),
    };
    let mut saves = vec![
        Save {
            tempo: Some(140.0),
            detected_key: "G minor".into(),
            rating: 4,
            favourite: true,
            minutes_spent: Some(41 * 60 + 12),
            ..save("Night Drive_14", "FL Studio", 1)
        },
        save("Night Drive_13", "FL Studio", 3),
        save("Night Drive_2", "FL Studio", 40),
        Save {
            tempo: Some(122.0),
            missing_samples: 3,
            status: "Mixing".into(),
            tags: vec!["dark".into(), "live set".into()],
            todos: vec![
                todo("Fix the 3 missing samples", false, 9),
                todo("Master is clipping at the end", false, 9),
                todo("Swap the pad for a Rhodes", true, 13),
                todo("Vocal chop for the break", false, 5),
            ],
            ..save("Glass Harbour", "Bitwig Studio", 4)
        },
        save("Glass Harbor", "Bitwig Studio", 300),
        Save { tempo: Some(92.5), musical_key: "D major".into(), rating: 3, ..save("Slow Orbit v3", "Studio One", 12) },
        save("Slow Orbit v2", "Studio One", 20),
        Save {
            tempo: Some(128.0),
            missing_plugins: 1,
            tags: vec!["dark".into()],
            todos: vec![todo("Needs a second verse", false, 70), todo("Find a better kick", false, 60)],
            minutes_spent: Some(6 * 60 + 20),
            ..save("Copper Lines", "FL Studio", 70)
        },
        Save {
            tempo: Some(174.0),
            rating: 5,
            status: "Finished".into(),
            minutes_spent: Some(26 * 60),
            ..save("Paper Moons_31", "FL Studio", 200)
        },
        Save { tempo: Some(110.0), ..save("Low Tide", "Bitwig Studio", 420) },
        Save {
            tempo: Some(136.0),
            status: "Released".into(),
            rating: 5,
            favourite: true,
            ..save("Fieldwork_9", "FL Studio", 800)
        },
        // Hours spent and left for months: the Dust off card's.
        Save {
            tempo: Some(124.0),
            status: "Arranging".into(),
            minutes_spent: Some(3 * 60 + 5),
            ..save("Kite Season", "FL Studio", 500)
        },
        Save { tempo: Some(100.0), minutes_spent: Some(14 * 60), ..save("Untitled_4", "FL Studio", 1300) },
        Save { tempo: Some(85.0), minutes_spent: Some(9 * 60 + 30), ..save("Old Tapes", "FL Studio", 2200) },
    ];
    saves.sort_by_key(|s| std::cmp::Reverse(s.modified));
    // Each save its own id, as in the library (the rail's line finds a song by its save's id).
    for (id, save) in (1..).zip(&mut saves) {
        save.id = id;
    }
    let tags = vec!["dark".into(), "live set".into()];
    let mut songs = mo_app::songs::Songs::from_saves(&saves, &mo_library::SongChoices::default(), tags, 3, Some(false));
    // Off the Dust off card ("Not now") but in its own shot, so the list sits where the other shots' clicks expect it.
    let now = chrono::Local::now().naive_local();
    for row in songs.dusty(now) {
        songs.later.insert(songs.rows[row].song.key.clone(), i64::MAX);
    }
    // Bounces (made-up files, never read): two of Copper Lines, one of Slow Orbit.
    let bounce = |name: &str, days: i64| mo_app::bounces::Bounce {
        path: format!(r"C:\Music\Exports\{name}"),
        name: name.into(),
        mtime: (now - Days::days(days)).and_utc().timestamp(),
    };
    let key = |name: &str| songs.rows.iter().find(|r| r.song.name == name).map(|r| r.song.key.clone());
    let mut found = std::collections::HashMap::new();
    if let Some(k) = key("Copper Lines") {
        found.insert(k, vec![bounce("Copper Lines (final).wav", 64), bounce("Copper Lines_3.mp3", 80)]);
    }
    if let Some(k) = key("Slow Orbit") {
        found.insert(k, vec![bounce("Slow Orbit v3.wav", 12)]);
    }
    songs.set_bounces(&found);
    // Stage guesses for the board: Night Drive looks mixed, Copper Lines arranged.
    let shape =
        |seconds: f64, tracks: u32, eq: u32, compressed: u32, sends: u32, automation: u32| mo_formats::shape::Shape {
            seconds,
            bars: seconds / 60.0 * 140.0 / 4.0,
            markers: 5,
            automation,
            tracks,
            eq,
            compressed,
            sends,
            master: Vec::new(),
        };
    let mut shapes = std::collections::HashMap::new();
    for r in &songs.rows {
        let guessed = match r.song.name.as_str() {
            "Night Drive" => shape(232.0, 12, 9, 7, 2, 14),
            "Copper Lines" => shape(150.0, 6, 1, 0, 1, 0),
            _ => continue,
        };
        let save = mo_app::board::newest(&r.song);
        shapes.insert(save.id, mo_app::board::KeptShape { mtime: save.mtime, size: save.size, shape: Some(guessed) });
    }
    songs.set_shapes(&shapes);
    songs
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
const SHOW_BUTTON: (f32, f32) = (1296.0, 342.0);

/// Made-up moved folders whose link doesn't work, one of each kind the Plugins page shows with its own sign, and what
/// the last Link again said of one (by where its link is).
fn sample_broken_links() -> (Vec<mo_app::plugins::links::Broken>, std::collections::BTreeMap<String, String>) {
    use mo_app::plugins::links::{Broken, Trouble};
    let moved = |from: &str, to: &str| mo_space::folder_move::MovedFolder {
        from: from.to_owned(),
        to: to.to_owned(),
        old_copy: String::new(),
        bytes: 0,
        moved: String::new(),
        key: String::new(),
        kind: 2,
        shared_with: Vec::new(),
    };
    let elsewhere = mo_space::after_move::Elsewhere {
        was_on: r"E:\".to_owned(),
        now_on: r"F:\".to_owned(),
        to: r"F:\Plugin Data\Arcade".to_owned(),
    };
    let vital = r"C:\Users\you\Documents\Vital";
    let broken = vec![
        Broken { moved: moved(vital, r"E:\Plugin Data\Vital"), trouble: Trouble::LeadsElsewhere },
        Broken {
            moved: moved(r"C:\Users\you\Documents\Output\Arcade", r"E:\Plugin Data\Arcade"),
            trouble: Trouble::OnAnotherDrive(elsewhere),
        },
        Broken {
            moved: moved(r"C:\ProgramData\Spectrasonics\STEAM", r"E:\Plugin Data\STEAM"),
            trouble: Trouble::Replaced,
        },
        Broken {
            moved: moved(r"C:\Users\you\Documents\Native Instruments\Kontakt 7", r"E:\Plugin Data\Kontakt 7"),
            trouble: Trouble::Doubled("Native Instruments".to_owned()),
        },
        Broken {
            moved: moved(r"C:\Users\you\Documents\u-he\Hive.data", r"G:\Plugin Data\Hive.data"),
            trouble: Trouble::CopyAway { drive_away: true },
        },
    ];
    let said = r"The link was made but doesn't reach E:\Plugin Data\Vital, so the old one is back.";
    (broken, std::collections::BTreeMap::from([(mo_space::key(vital), said.to_owned())]))
}

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
                    key: mo_library::read::name_key(name).into(),
                    vendor: vendor.into(),
                    kind,
                    format: format.into(),
                    installed,
                    daw: s.daw.as_str().into(),
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
const REMOVE_OLDER_BUTTON: (f32, [f32; 2]) = (1150.0, [776.0, 794.0]);
/// The Presets page's Plugin menu: the first thing in its filter column.
const PLUGIN_CHIP: (f32, f32) = (288.0, 152.0);
/// A page's turn of the wheel: `-PAGE_DOWN` turns a page down, `PAGE_DOWN` up ([`scroll`]).
const PAGE_DOWN: f32 = 300.0;
/// On the chips of the folder strip under the Sounds list, with the tree open.
const STRIP_WHEEL: (f32, f32) = (1100.0, 830.0);

/// Where the wheel turns the song panel down in the narrowest window, and by how much: on its status line, which
/// doesn't scroll on its own, as the lists and the arrangement in it do.
const NARROW_PANEL_WHEEL: (f32, f32, f32) = (760.0, 404.0, -2.0 * PAGE_DOWN);
const OLDER_BUTTON: (f32, f32) = (1205.0, 341.0);
/// The Space page's card: the "i" beside its kind's name (BITWIG COPIES), at 1440 x 900.
const BITWIG_INFO: (f32, f32) = (1206.0, 58.0);
/// Space's Old copies after moving folders line, the page at its top, and a place on the page with nothing on it (right
/// of its name, left of a card).
const SPACE_OLD_LINE: (f32, f32) = (400.0, 493.0);
const SPACE_EMPTY: (f32, f32) = (1000.0, 60.0);
/// Where the wheel turns a page's card (Space's, or Sounds') down to its end, and by more than enough: on its parts.
const SPACE_CARD_WHEEL: (f32, f32, f32) = (1240.0, 600.0, -10.0 * PAGE_DOWN);
/// The second line on the galaxy's Swept card, after the sweep and an arrow key.
const SWEPT_LINE: (f32, f32) = (760.0, 301.0);
/// Where the tip of the line at `SWEPT_LINE` comes up, over the line below it, in pixels (x, y, width, height).
const SWEPT_TIP: (u32, u32, u32, u32) = (740, 319, 200, 42);
/// On the galaxy's Swept card, after the sweep: on its first line, where the wheel turns.
const SWEPT_CARD_WHEEL: (f32, f32) = (800.0, 260.0);
/// The galaxy left of the Swept card, in pixels (x, y, width, height): its stars move when the map zooms.
const GALAXY_LEFT: (u32, u32, u32, u32) = (470, 300, 200, 540);
/// The Swept card in the short window, too short for its lines, gone down to its oldest: the line just above that one,
/// and Clear.
const SHORT_SWEPT_ABOVE: (f32, f32) = (760.0, 469.0);
const SHORT_SWEPT_CLEAR: (f32, f32) = (903.0, 551.0);
/// How many times the sweeps go across the map in the short window, and the fewest lines on the Swept card that fill
/// it there (nine are more than it has room for).
const SHORT_SWEEP_ROWS: usize = 9;
const SHORT_SWEPT_LINES: usize = 10;
/// The Lock chip on the kit's second pad (Snare).
const SNARE_LOCK: (f32, f32) = (846.0, 194.0);
/// A point on a clip in the arrangement, from the view's top left, for the hover and zoom shots.
const ARRANGEMENT_CLIP: (f32, f32) = (480.0, 180.0);
/// Inside the first tile of the Sounds grid, and the first line of its List, at their top (x, y, width, height in
/// pixels): all one colour there, the list shows nothing.
const FIRST_TILE: (u32, u32, u32, u32) = (250, 297, 200, 70);
const FIRST_LINE: (u32, u32, u32, u32) = (250, 312, 400, 30);
/// A point on a Sounds tile to right-click, and where a right-click menu's lines start under the mouse (its card's
/// padding) and how tall each is (studio.slint's MenuItems and MenuLine).
const MENU_TILE: (f32, f32) = (560.0, 278.0);
const MENU_LINES_TOP: f32 = 4.0;
const MENU_LINE: f32 = 32.0;
/// With the card open: the waveform of the tile at [`MENU_TILE`], and the waveform on the card.
const TILE_WAVE: (f32, f32) = (560.0, 322.0);
const CARD_WAVE: (f32, f32) = (1200.0, 292.0);
/// The right edges of the Sounds filter column and its folder tree (open), halfway down.
const FILTERS_SEAM: (f32, f32) = (436.0, 500.0);
const TREE_SEAM: (f32, f32) = (732.0, 500.0);
/// The gap between the Songs list and its panel, halfway down, and the list's width there (a third of the room).
const SONGS_SEAM: (f32, f32) = (844.0, 500.0);
const SONGS_LIST_WIDTH: f32 = 380.0;
/// The left edge of the Songs list's Last saved heading (in the gap before it), the list made `SONGS_WIDER_LIST`
/// wider than its third (as far as the page lets it).
const SONGS_WIDER_LIST: f32 = 220.0;
const SONGS_SAVED_SEAM: (f32, f32) = (750.0, 130.0);
/// The seam between the Sounds list and its card (the card at its usual width), halfway down.
const CARD_SEAM: (f32, f32) = (1009.0, 500.0);
/// The path on the Sounds card, on its one line (Boom Kick's, cut short at its start).
const CARD_PATH: (f32, f32) = (1150.0, 226.0);
/// Below and right of the mouse at [`MENU_TILE`], off its tile (with the card open): a menu opened there covers it.
const MENU_OVER: (u32, u32, u32, u32) = (700, 377, 120, 80);
/// The Sounds card's Add a tag box, the card turned down to its end (with a sound's six tags over it).
const TAG_BOX: (f32, f32) = (1130.0, 467.0);
/// The Songs, Presets, MIDI and Plugins lists in the short window: left of the activity cards, which move on their own.
const SHORT_LIST: (u32, u32, u32, u32) = (230, 200, 760, 400);
/// The mouse over each of those lists (Songs' is narrow beside its panel; Plugins' starts low), for the wheel.
/// The second line of a song's Saves tab (an older save), at 1440 x 900.
const SECOND_SAVE: (f32, f32) = (1025.0, 588.0);
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
    // The MIDI parts card, further down.
    scroll(window, 900.0, 500.0, -PAGE_DOWN);
    shots.push(("settings-midi-parts.png".into(), capture(window)));
    scroll(window, 900.0, 500.0, 3.0 * PAGE_DOWN);
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
    // A scan going: one folder done, a folder just added being listened to, one on a drive that's unplugged.
    let samples: Vec<String> =
        vec!["I:\\Samples".into(), "I:\\Fl Studio Backup\\Old Samples".into(), "G:\\Splice".into()];
    let at = |h: u32, m: u32| chrono::Local::now().date_naive().and_hms_opt(h, m, 0).unwrap_or_default();
    let scanning = mo_app::sound_scan::ScanState {
        running: true,
        started: at(15, 40),
        folders: vec![
            mo_app::sound_scan::FolderScan { folder: samples[0].clone(), files: 453_741, ..Default::default() },
            mo_app::sound_scan::FolderScan {
                folder: samples[1].clone(),
                files: 3_906,
                to_listen: 3_906,
                listened: 1_280,
                unreadable: 2,
                ..Default::default()
            },
            mo_app::sound_scan::FolderScan { folder: samples[2].clone(), away: true, ..Default::default() },
        ],
        ..Default::default()
    };
    // Only the key the scan state is kept under: no file is made.
    let screens_sounds = PathBuf::from("screens-sounds.db");
    mo_app::sound_scan::show_state(&screens_sounds, scanning.clone());
    let folder_groups = |samples: Vec<String>, moved: &mo_space::folder_move::MovedFolder| {
        mo_app::settings::folder_groups(
            vec!["I:\\Fl Studio Data\\FL Studio\\Projects".into(), "K:\\Bitwig Studio\\Projects".into()],
            samples,
            vec!["L:\\New Midi".into()],
            vec![],
            vec!["C:\\Program Files\\Common Files\\VST3".into(), "C:\\Program Files\\Common Files\\CLAP".into()],
            std::slice::from_ref(moved),
        )
    };
    mo_ui::settings::show_folders(app, Ok(folder_groups(samples.clone(), &moved)));
    // Exports go into a folder chosen on another drive (an invented one).
    let exports = r"I:\DAWmate Exports";
    mo_ui::settings::show_exports(
        app,
        &mo_app::settings::exports_card(Some(exports), &Ok(PathBuf::from(exports)), true),
    );
    data.invoke_tab_picked(1);
    capture(window);
    shots.push(("settings-folders.png".into(), capture(window)));
    // The scan done: each folder says what it found and when.
    let mut scanned = scanning;
    scanned.running = false;
    scanned.ended = Some(at(15, 47));
    scanned.folders[1].listened = 3_906;
    mo_app::sound_scan::show_state(&screens_sounds, scanned);
    mo_ui::settings::show_folders(app, Ok(folder_groups(samples, &moved)));
    shots.push(("settings-folders-scanned.png".into(), capture(window)));
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
    // And one more Library folder, an old one on another drive, on the list with its ×.
    mo_ui::settings::remember(mo_app::app_settings::BITWIG_LIBRARIES_KEY, r"D:\Old Bitwig\Library");
    mo_ui::settings::show_bitwig_extension(
        app,
        mo_dawlink::scripts::ScriptState::Outdated,
        "3.19",
        vec![std::path::PathBuf::from(r"K:\Bitwig Studio\Extensions")],
    );
    let fl_program = mo_app::shell::daw_exe_key("FL Studio");
    mo_ui::settings::remember(&fl_program, r"C:\Program Files\Image-Line\FL Studio 2025\FL64.exe");
    mo_ui::settings::show_fl(
        app,
        mo_dawlink::scripts::ScriptState::Outdated,
        vec![std::path::PathBuf::from(r"I:\Fl Studio Data\FL Studio\Settings\Hardware\DAWmate")],
    );
    capture(window);
    shots.push(("settings-daws.png".into(), capture(window)));
    // Take out on FL Studio's row, asking first and naming the file that goes.
    data.set_take_out_asking("FL Studio".into());
    capture(window);
    shots.push(("settings-daws-take-out.png".into(), capture(window)));
    data.set_take_out_asking("".into());
    // Further down, the program each DAW's songs open in: FL Studio's chosen here (an invented path, set before
    // show_fl showed the tab), Studio One's chosen in the old app, Bitwig's Windows' choice.
    scroll(window, 900.0, 500.0, -4.0 * PAGE_DOWN);
    shots.push(("settings-daws-programs.png".into(), capture(window)));
    scroll(window, 900.0, 500.0, 4.0 * PAGE_DOWN);
    mo_ui::settings::remember(&fl_program, "");
    // No Bitwig Studio folder in Documents and none chosen: the card says to choose it (the 10 Nov review's X3-5).
    mo_ui::settings::remember(mo_app::app_settings::BITWIG_FOLDER_KEY, "");
    mo_ui::settings::remember(mo_app::app_settings::BITWIG_LIBRARIES_KEY, "");
    mo_ui::settings::show_bitwig_extension(app, mo_dawlink::scripts::ScriptState::NotInstalled, "3.19", Vec::new());
    capture(window);
    shots.push(("settings-daws-no-folder.png".into(), capture(window)));
    data.invoke_tab_picked(3);
    capture(window);
    shots.push(("settings-shortcuts.png".into(), capture(window)));
    // The mouse on a line's "No key" button: the line's two buttons show, and stay while the mouse is on one.
    hover(window, KEY_LINE_NO_KEY.0, KEY_LINE_NO_KEY.1);
    let pointed = capture(window);
    hover(window, -1.0, -1.0);
    shots.push(("settings-shortcuts-pointed.png".into(), pointed));
    // A search leaves only the actions with its words, under their pages.
    data.set_keys_search("bitwig".into());
    data.invoke_keys_search_edited("bitwig".into());
    let found = capture(window);
    shots.push(("settings-shortcuts-search.png".into(), found));
    data.set_keys_search("".into());
    data.invoke_keys_search_edited("".into());
    data.invoke_tab_picked(4);
    // Two steps of the beta checklist marked (nothing is written for the screenshots), then taken off again.
    data.invoke_beta_mark("start.release".into(), 0, "".into());
    data.invoke_beta_mark("start.close-clean".into(), 1, "A crash log was left".into());
    capture(window);
    shots.push(("settings-developer.png".into(), capture(window)));
    data.invoke_beta_mark("start.release".into(), 0, "".into());
    data.invoke_beta_mark("start.close-clean".into(), 1, "".into());
    data.invoke_tab_picked(0);
}

/// Settings › Developer turned down the beta checklist, then another page and back: the checklist is where it was
/// (Solomon, 24 Nov: "it goes back to the top when I leave the page"). Another tab starts at its top.
fn developer_keeps_its_place(app: &mo_ui::AppWindow, window: &MinimalSoftwareWindow) -> Result<(), String> {
    let data = app.global::<mo_ui::SettingsData>();
    start_on(app, Place::Settings);
    data.invoke_tab_picked(4);
    capture(window);
    let top = capture(window);
    scroll(window, 900.0, 500.0, -4.0 * PAGE_DOWN);
    let down = capture(window);
    let kept = data.get_developer_scroll();
    start_on(app, Place::Songs);
    capture(window);
    start_on(app, Place::Settings);
    capture(window);
    let back = capture(window);
    let other = {
        data.invoke_tab_picked(0);
        capture(window);
        data.get_developer_scroll()
    };
    data.invoke_tab_picked(4);
    capture(window);
    let again = capture(window);
    // Back to its top for whatever follows.
    scroll(window, 900.0, 500.0, 40.0 * PAGE_DOWN);
    data.invoke_tab_picked(0);
    capture(window);
    // The checklist's part of the page, clear of the activity cards bottom right, which move on their own.
    const LIST: (u32, u32, u32, u32) = (300, 150, 1000, 550);
    if same_in(&down, &top, LIST) || kept >= 0.0 {
        return Err(format!("Developer: the wheel didn't turn the checklist down (kept {kept})"));
    }
    if !same_in(&back, &down, LIST) {
        return Err("Developer: after another page the checklist was no longer where it was left".into());
    }
    if other != kept || !same_in(&again, &down, LIST) {
        return Err("Developer: after the General tab the checklist was no longer where it was left".into());
    }
    Ok(())
}

/// Settings › Developer turned to the end of the beta checklist: its last steps (the Self-check ones) end inside its
/// card, above the This build card (Solomon's F12 note of 24 Nov: "this build is covering the test results", the
/// wrapped How lines measured at one line's height). Shot as settings-developer-end.png.
fn developer_checklist_end(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    out: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let data = app.global::<mo_ui::SettingsData>();
    start_on(app, Place::Settings);
    data.invoke_tab_picked(4);
    capture(window);
    // To the page's end: the checklist's last steps, This build and Activity cards.
    scroll(window, 900.0, 500.0, -60.0 * PAGE_DOWN);
    capture(window);
    let pixels = capture(window);
    image::save_buffer(out.join("settings-developer-end.png"), &pixels, WIDTH, HEIGHT, image::ExtendedColorType::Rgb8)?;
    scroll(window, 900.0, 500.0, 80.0 * PAGE_DOWN);
    data.invoke_tab_picked(0);
    capture(window);
    Ok(())
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

/// A right click on another MIDI line while a part plays (the review of 14 Nov, MI-2): that pick for its menu starts
/// and stops nothing, in the loop or in Bitwig; its card offers to play it, and the menu's play lines play it rather
/// than stopping what plays. (The line's right press sets the flag and picks, as on every list: done here by hand.)
fn midi_right_click_while_playing(app: &mo_ui::AppWindow) -> Result<(), Box<dyn std::error::Error>> {
    let midi = app.global::<mo_ui::MidiData>();
    let keys = app.global::<mo_ui::AppKeys>();
    let menu_pick = |row| {
        keys.set_picking_for_menu(true);
        mo_ui::midi::pick_row(app, row);
        keys.set_picking_for_menu(false);
    };
    mo_ui::midi::soundfont_here(app, true);
    mo_ui::midi::pick_row(app, 0);
    mo_ui::midi::run_action(app, "app.play");
    let first = midi.get_picked().title;
    menu_pick(1);
    if !midi.get_playing() || midi.get_loop_picked() || midi.get_picked().title == first {
        return Err(
            "midi right click: a right click on the second line as the first played stopped or switched it".into()
        );
    }
    mo_ui::midi::run_action(app, "app.play");
    if !midi.get_playing() || !midi.get_loop_picked() {
        return Err("midi right click: the menu's Play or stop the loop didn't play the line right-clicked".into());
    }
    mo_ui::midi::run_action(app, "app.play");
    if midi.get_playing() {
        return Err("midi right click: Play or stop the loop didn't stop the part it plays".into());
    }
    // The same in Bitwig.
    mo_ui::midi::run_action(app, "midi.playBitwig");
    menu_pick(0);
    if midi.get_daw_playing() != "bitwig" || midi.get_daw_picked() {
        return Err("midi right click: a right click on another line stopped or switched the part in Bitwig".into());
    }
    mo_ui::midi::run_action(app, "midi.playBitwig");
    if midi.get_daw_playing() != "bitwig" || !midi.get_daw_picked() {
        return Err("midi right click: the menu's Play in Bitwig didn't play the line right-clicked".into());
    }
    mo_ui::midi::run_action(app, "midi.playBitwig");
    if midi.get_daw_playing() != "" {
        return Err("midi right click: Play in Bitwig, or stop didn't stop the part in Bitwig".into());
    }
    mo_ui::midi::soundfont_here(app, false);
    mo_ui::midi::pick_row(app, 0);
    Ok(())
}

/// A right click at a point in the window.
fn right_click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Right;
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
}

/// Drags the seam at `at` (a part's edge) `by` pixels across and lets go, after a picture of it held there. Fails
/// unless the part `name` (as `mo_app::widths` knows it) is then `by` wider (or narrower, `grows` -1) than it was, or
/// unless putting it back to its usual width, as a double-click does, gives it the width it had. `width` reads the part's width from the page.
fn drag_seam(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    at: (f32, f32),
    by: f32,
    grows: f32,
    name: &str,
    width: impl Fn() -> f32,
) -> Result<Vec<u8>, String> {
    let was = width();
    let button = slint::platform::PointerEventButton::Left;
    let from = slint::LogicalPosition::new(at.0, at.1);
    let to = slint::LogicalPosition::new(at.0 + by, at.1);
    window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: from });
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position: from, button });
    window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: to });
    capture(window);
    let held = capture(window);
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position: to, button });
    let got = width();
    if (got - was - grows * by).abs() > 1.0 {
        return Err(format!(
            "{name}: dragging its seam at {at:?} by {by} made it {got} wide, from {was}; has it moved?"
        ));
    }
    app.global::<mo_ui::Widths>().invoke_reset(name.into());
    hover(window, -1.0, -1.0);
    if width() != was {
        return Err(format!("{name}: a double-click's reset left it {} wide, not its usual {was}", width()));
    }
    Ok(held)
}

/// Two places across the Sounds card's waveform ([`CARD_WAVE`]) to drag between to pick a part.
const PART_DRAG: (f32, f32) = (1100.0, 1300.0);

/// A part picked on the Sounds card's waveform (the 1.1 plan's light sample edits): pressed and dragged across it, its
/// edges snapping to the sound's hits, and the handle under it lit to carry the part. A click outside it lets it go, as
/// Esc does.
fn part_shots(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    shots: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let data = app.global::<mo_ui::SoundsData>();
    // A thirty-second of the way apart: about 12 points on the card, so every place is within snapping reach of one.
    let hits: Vec<f64> = (1..32).map(|i| f64::from(i) / 32.0).collect();
    mo_ui::sounds::hits_for_screens(app, &hits);
    let ((from, to), y) = (PART_DRAG, CARD_WAVE.1);
    let pick = || {
        let button = slint::platform::PointerEventButton::Left;
        let (a, b) = (slint::LogicalPosition::new(from, y), slint::LogicalPosition::new(to, y));
        window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: a });
        window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position: a, button });
        window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: b });
        window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position: b, button });
    };
    pick();
    let (a, b) = (data.get_part_from() * 32.0, data.get_part_to() * 32.0);
    if data.get_part_text().is_empty() {
        return Err(format!(
            "sounds-part.png: dragging across the card's waveform at {PART_DRAG:?}, {y} picked no part"
        ));
    }
    if (a - a.round()).abs() > 1e-3 || (b - b.round()).abs() > 1e-3 || b <= a {
        return Err(format!("sounds-part.png: the part's edges ({a}, {b} thirty-seconds) didn't snap to the hits"));
    }
    hover(window, from + 20.0, y);
    shots.push(("sounds-part.png".into(), capture(window)));
    click(window, from - 40.0, y);
    if !data.get_part_text().is_empty() {
        return Err("sounds-part.png: a click outside the part didn't let it go".into());
    }
    pick();
    if !mo_ui::sounds::escape(app) || !data.get_part_text().is_empty() {
        return Err("sounds-part.png: Esc didn't let the part go".into());
    }
    hover(window, -1.0, -1.0);
    Ok(())
}

/// A left click at a point in the window.
fn click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Left;
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
}

/// Settings, Shortcuts: where the fourth line's "No key" button shows once the mouse is on the line.
const KEY_LINE_NO_KEY: (f32, f32) = (1218.0, 421.0);

/// The Sounds list's year strip: the date's name at its top ("Changed"), which opens the Years by menu, and that
/// menu's First used line (its fourth, under Made, Changed and Opened).
const YEARS_BY_NAME: (f32, f32) = (972.0, 131.0);
const YEARS_BY_FIRST_USED: (f32, f32) = (760.0, 282.0);

/// Rediscover's How far afield slider in the filter column, near its track's right end (it rests nearer its left).
const AFIELD_SLIDER: (f32, f32) = (385.0, 634.0);

/// The Sounds page's Tempo slider with no filter picked: its track's height in the window, where its higher handle
/// rests, and where 150 BPM is along it.
const TEMPO_TRACK_Y: f32 = 602.0;
const TEMPO_HIGH_END_X: f32 = 419.0;
const TEMPO_150_X: f32 = 357.0;

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

/// How wide the filter column is (studio.slint's FilterColumn).
const COLUMN_WIDTH: u32 = 208;

/// How far above the filter column's foot the lowest thing drawn in it ends, in pixels, in a picture of the window
/// `width` across. The column is found by the line down its right side: the first from the left, after the rail's own,
/// that runs unbroken from near the top of the page to near its foot. None without one.
fn column_room_below(pixels: &[u8], width: u32) -> Option<u32> {
    let height = u32::try_from(pixels.len() / 3).ok()? / width;
    let at = |x: u32, y: u32| {
        let i = ((y * width + x) * 3) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };
    let rows = 200..height.saturating_sub(100);
    if rows.is_empty() {
        return None;
    }
    let line = (COLUMN_WIDTH + 4..width / 2).find(|&x| {
        let c = at(x, rows.start);
        rows.clone().all(|y| at(x, y) == c && at(x - 1, y) != c && at(x + 1, y) != c)
    })?;
    let edge = at(line, rows.start);
    let foot = (rows.start..height).take_while(|&y| at(line, y) == edge).last()?;
    // The column's last points before its line are always empty: the page's own colour.
    let empty = at(line - 4, foot);
    let drawn = |y: u32| (line + 2 - COLUMN_WIDTH..line - 1).any(|x| at(x, y) != empty);
    let lowest = (rows.start..=foot).rev().find(|&y| drawn(y))?;
    Some(foot - lowest)
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

/// The pixels of a capture (RGB, `width` across) in the box at `at` (left, top) of `size` (across, down).
fn crop_rgb(shot: &[u8], width: u32, at: (u32, u32), size: (u32, u32)) -> Vec<u8> {
    (at.1..at.1 + size.1)
        .flat_map(|y| {
            let start = ((y * width + at.0) * 3) as usize;
            shot.get(start..start + (size.0 * 3) as usize).unwrap_or_default().iter().copied()
        })
        .collect()
}

fn slug(name: &str) -> String {
    name.to_lowercase().replace(' ', "-")
}

/// The list's own scroll bar (Solomon's F12 note of 25 Nov, 12:59: Slint's thin one was fiddly, and a drag edge got
/// in its way), on Sounds' List: lit under the mouse, a click on its track below the thumb turns a page, and the thumb
/// dragged down turns the list. Each must move the list's lines; the list goes back to its top after.
fn scroll_bar_checks(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    shots: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let lines = |shot: &[u8]| crop_rgb(shot, WIDTH, LIST_LINES.0, LIST_LINES.1);
    let button = slint::platform::PointerEventButton::Left;
    let at = |y: f32| slint::LogicalPosition::new(SCROLL_BAR_X, y);
    let data = app.global::<mo_ui::SoundsData>();
    // Until two pictures of the list in a row are the same: a slow runner can still be laying the list out (or
    // filling it) when the check starts, and a press then can land on a thumb still growing (25 Nov, #763's run).
    let settled = || {
        let mut last = capture(window);
        for _ in 0..10 {
            let now = capture(window);
            if lines(&now) == lines(&last) {
                return now;
            }
            last = now;
        }
        last
    };
    // How far the list is turned down, once it has moved from `from` (or after a few tries, unmoved).
    let turned_from = |from: f32| {
        for _ in 0..5 {
            capture(window);
            if (data.get_lines_y() - from).abs() >= 0.5 {
                break;
            }
        }
        data.get_lines_y()
    };
    // Back to the list's top by the wheel over its lines, checked by where the list says it is. Not by the Home key:
    // the list is turned to the picked sound, which the shuffle puts anywhere, and Home there left it at its end
    // (25 Nov: 2 runs in 4), where the thumb covers the point on the track this check clicks.
    let to_top = || {
        let (x, y) = LIST_LINES.0;
        let position = slint::LogicalPosition::new(x as f32 + 40.0, y as f32 + 40.0);
        for _ in 0..3 {
            window.dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y: 100_000.0,
            });
            let now = settled();
            if data.get_lines_y().abs() < 0.5 {
                return Ok(now);
            }
        }
        Err(format!("sounds scroll bar: the wheel didn't take the list to its top (turned {})", data.get_lines_y()))
    };
    to_top()?;
    hover(window, -1.0, -1.0);
    let top = settled();
    hover(window, SCROLL_BAR_X, SCROLL_BAR_TRACK_Y);
    capture(window);
    let lit = capture(window);
    if lit == top {
        return Err(format!(
            "sounds-scroll-bar.png: nothing lit at x {SCROLL_BAR_X}; has the list's scroll bar moved?"
        ));
    }
    shots.push(("sounds-scroll-bar.png".into(), lit));
    let from = data.get_lines_y();
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position: at(SCROLL_BAR_TRACK_Y), button });
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position: at(SCROLL_BAR_TRACK_Y), button });
    let paged = turned_from(from);
    if (paged - from).abs() < 0.5 {
        return Err(format!(
            "sounds scroll bar: a click on its track below the thumb (y {SCROLL_BAR_TRACK_Y}) didn't turn the list a \
             page: turned {from} before and {paged} after, {} sounds shown",
            data.get_shown()
        ));
    }
    to_top()?;
    let from = data.get_lines_y();
    window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at(SCROLL_BAR_THUMB_Y) });
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position: at(SCROLL_BAR_THUMB_Y), button });
    window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at(SCROLL_BAR_THUMB_Y + 120.0) });
    let moved = turned_from(from);
    let dragged = capture(window);
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased {
        position: at(SCROLL_BAR_THUMB_Y + 120.0),
        button,
    });
    if (moved - from).abs() < 0.5 || lines(&dragged) == lines(&top) {
        return Err(format!(
            "sounds scroll bar: dragging its thumb down didn't turn the list: turned {from} before and {moved} after"
        ));
    }
    shots.push(("sounds-scroll-bar-dragged.png".into(), dragged));
    hover(window, -1.0, -1.0);
    to_top()?;
    hover(window, -1.0, -1.0);
    Ok(())
}

/// Sounds' List with its card open: the scroll bar down the list's right edge, a point on its track near the bottom
/// (below the thumb when the list is at its top), and its thumb's top part at the list's top.
const SCROLL_BAR_X: f32 = 939.0;
const SCROLL_BAR_TRACK_Y: f32 = 860.0;
const SCROLL_BAR_THUMB_Y: f32 = 170.0;
/// The list's lines (left of the bar), for telling whether the list turned.
const LIST_LINES: ((u32, u32), (u32, u32)) = ((500, 160), (420, 700));
