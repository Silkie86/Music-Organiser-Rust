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
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};

const WIDTH: u32 = 1440;
const HEIGHT: u32 = 900;

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
    mo_ui::songs::show(&app, sample_songs());
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
    mo_ui::sounds::show(&app, sample_sounds(), None, Some("1"));
    mo_ui::sounds::index_known(&app, std::sync::Arc::new(sample_fingerprints()));
    // Once to lay the page out (the tiles' width tells the page how many fit in a row), then for real.
    capture(&window);

    let mut shots = Vec::new();
    for place in Place::ALL {
        app.set_place(place.index() as i32);
        shots.push((format!("{}-graphite.png", slug(place.name())), capture(&window)));
    }
    // Space's Backup section, filled with made-up folders.
    app.set_place(Place::Space.index() as i32);
    mo_ui::backup::show_lines(&app, mo_ui::backup::sample_lines());
    shots.push(("space-backup.png".into(), capture(&window)));
    app.set_place(Place::Songs.index() as i32);
    for theme in THEMES.iter().skip(1) {
        mo_ui::apply_look(&app, &Look::from_settings(Some(theme.name), None));
        shots.push((format!("songs-{}.png", slug(theme.name)), capture(&window)));
    }

    mo_ui::apply_look(&app, &graphite);
    shots.push(("sounds-empty.png".into(), sounds_empty));
    sounds_shots(&app, &window, &graphite, &mut shots);

    // The song panel, on the first song, on Overview and on Plugins.
    for tab in [Tab::Overview, Tab::Plugins, Tab::Saves] {
        mo_ui::songs::show_picked(&app, 0, sample_detail(), tab);
        mo_ui::arrangement::show_preview(&app, mo_ui::arrangement::sample_preview());
        shots.push((format!("song-panel-{}.png", slug(tab.name())), capture(&window)));
    }
    // The arrangement: the mouse on a clip, zoomed in about the mouse, the Bitwig view, and in a light theme.
    app.global::<mo_ui::SongsData>().set_tab(Tab::Overview.index() as i32);
    let arrangement = app.global::<mo_ui::ArrangementData>();
    arrangement.invoke_moved(ARRANGEMENT_CLIP.0, ARRANGEMENT_CLIP.1);
    shots.push(("arrangement-hover.png".into(), capture(&window)));
    arrangement.invoke_wheel(ARRANGEMENT_CLIP.0, 0.0, 4.0, true, false);
    shots.push(("arrangement-zoomed.png".into(), capture(&window)));
    arrangement.invoke_left();
    arrangement.invoke_double_clicked();
    arrangement.invoke_pick_view(1);
    shots.push(("arrangement-bitwig-view.png".into(), capture(&window)));
    mo_ui::apply_look(&app, &Look::from_settings(Some("Paper"), None));
    mo_ui::arrangement::show_preview(&app, mo_ui::arrangement::sample_preview());
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
    let going = feed.start("Reading new FL saves", "");
    going.progress(0.4, Some("212 of 540"));
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
    mo_ui::songs::show_picked(&app, 0, sample_detail(), Tab::Overview);
    mo_ui::arrangement::show_preview(&app, mo_ui::arrangement::sample_preview());
    for (name, x, y) in [
        ("tip-rail.png", 60.0, 125.0),
        ("tip-button.png", SHOW_BUTTON.0, SHOW_BUTTON.1),
        ("tip-date.png", 530.0, 222.0),
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

/// The Sounds page: a sound picked and playing in its panel, a filter on, the List and History views, and the tiles in
/// a light theme.
fn sounds_shots(
    app: &mo_ui::AppWindow,
    window: &MinimalSoftwareWindow,
    graphite: &Look,
    shots: &mut Vec<(String, Vec<u8>)>,
) {
    app.set_place(Place::Sounds.index() as i32);
    let data = app.global::<mo_ui::SoundsData>();
    // The first tile's liked bar, saying why.
    hover(window, 350.0, 330.0);
    shots.push(("sounds-liked-tip.png".into(), capture_after(window, Duration::from_millis(750))));
    hover(window, -1.0, -1.0);
    // Autoplay is on, so picking a sound plays it.
    data.invoke_select(6);
    data.set_played(0.35);
    shots.push(("sounds-panel.png".into(), capture(window)));
    // Two types at once, with the Clear types chip.
    data.invoke_pick_type(0);
    data.invoke_pick_type(1);
    shots.push(("sounds-types.png".into(), capture(window)));
    data.invoke_pick_type(1);
    data.invoke_pick("key".into(), "A minor".into());
    data.invoke_toggle("mix-well".into());
    shots.push(("sounds-filtered.png".into(), capture(window)));
    data.invoke_clear();
    data.invoke_pick_view(1);
    shots.push(("sounds-list.png".into(), capture(window)));
    data.invoke_pick_view(3);
    shots.push(("sounds-history.png".into(), capture(window)));
    data.invoke_close_panel();
    data.invoke_pick_view(2);
    data.invoke_select(3);
    capture(window);
    shots.push(("sounds-galaxy.png".into(), capture(window)));
    data.invoke_pick_view(4);
    shots.push(("sounds-rediscover.png".into(), capture(window)));
    data.invoke_pick_view(5);
    data.invoke_kit_roll();
    data.invoke_pad_lock(0);
    shots.push(("sounds-kits.png".into(), capture(window)));
    data.invoke_pick_view(0);
    mo_ui::apply_look(app, &Look::from_settings(Some("Paper"), None));
    capture(window);
    shots.push(("sounds-paper.png".into(), capture(window)));
    mo_ui::apply_look(app, graphite);
    app.set_place(Place::Songs.index() as i32);
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
            let waveform = (0..160)
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
            Sound {
                id: i as i64 + 1,
                path: format!(r"D:\Samples\{folder}\{name}.wav"),
                error: None,
                analysis: Some(SoundAnalysis {
                    seconds,
                    is_loop,
                    bpm,
                    key_tonic: key.map(|k| k.0),
                    key_minor: key.is_some_and(|k| k.1),
                    waveform,
                    ..SoundAnalysis::default()
                }),
                sound_type: if ty.is_empty() { mo_sounds::types::UNKNOWN.into() } else { ty.into() },
                type_source: if i % 5 == 0 { "learned".into() } else { "name".into() },
                type_confidence: 0.9,
                user_type: None,
                name_type: None,
                tags: if i % 4 == 0 { vec!["Dark".into()] } else { Vec::new() },
                favourite: i % 6 == 1,
                hidden: false,
                rating: 0,
                plays: 0,
                added: day(1 + (i as u32 % 28), 12),
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
            ..save("Glass Harbour", "Bitwig Studio", 4)
        },
        save("Glass Harbor", "Bitwig Studio", 300),
        Save { tempo: Some(92.5), musical_key: "D major".into(), rating: 3, ..save("Slow Orbit v3", "Studio One", 12) },
        save("Slow Orbit v2", "Studio One", 20),
        Save { tempo: Some(128.0), missing_plugins: 1, ..save("Copper Lines", "FL Studio", 70) },
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

/// Where the song panel's Show in folder and Older saves buttons are in the shots. If the panel's layout moves them,
/// the tooltip and menu shots come out the same as the plain panel, and mo-screens stops with an error.
const SHOW_BUTTON: (f32, f32) = (1000.0, 328.0);
const OLDER_BUTTON: (f32, f32) = (865.0, 328.0);
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

/// A left click at a point in the window.
fn click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Left;
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
}

/// Lets hover and slide-in animations finish, then draws the window.
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
