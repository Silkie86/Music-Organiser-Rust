//! Plays the song's MIDI from the arrangement on the Songs page, each instrument with a General MIDI sound
//! (`mo_app::arrangement_play`), through the one player Sounds plays through ([`crate::sounds::player`]). The synth
//! makes the song as it plays (`mo_audio::gm::SongSound`), so it starts at once from wherever the playhead is,
//! however long the song is. Playing it stops what plays elsewhere, and a play there stops it.
//!
//! The playhead: Play starts from it, a click on the ruler moves it (and the song with it while it plays), and while
//! it plays the view follows it a page at a time. Stop puts it back where the play started, as a DAW does.
//!
//! The Sounds panel lists the song's instruments with the sound each plays with; a click on one opens General MIDI's
//! families and their sounds to pick another, remembered for the song. A pick while it plays is heard at once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mo_app::arrangement_play::{self as play, Sound, Voice, YourSounds};
use mo_app::{background, log};
use mo_formats::arrangement::Arrangement;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{AppWindow, ArrChoice, ArrSound, ArrangementData};

/// The keys of these plays with the player start with this.
const PLAY_KEY: &str = "arrangement:";

/// Songs with fewer notes than this have their instruments matched on the window's thread, at once; more, on a
/// thread of their own.
const QUICK_NOTES: usize = 20_000;

/// Counts the plays asked for (and stops), so a play that gets going after a newer one or a stop stays quiet (newest
/// wins, rule 12).
static ASKED: AtomicU64 = AtomicU64::new(0);

/// Counts the arrangements shown, so instruments matched for an older one are dropped.
static SHOWN: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct State {
    /// The song's key in the song library, which your sounds are kept under ("" for none: they're not kept).
    song: String,
    arrangement: Option<Arc<Arrangement>>,
    /// The song's instruments, once matched.
    voices: Option<Arc<Vec<Voice>>>,
    /// The sounds you picked for the song's instruments.
    yours: HashMap<String, Sound>,
    /// Where the playhead is, in beats, and where the play under way started.
    at: f64,
    from: f64,
    /// A play is asked for (it may still be getting going), and the player's key for it once it has it.
    playing: bool,
    key: Option<String>,
    /// Play was pressed before the instruments were matched: it plays once they are.
    waiting: bool,
    /// The panel line whose sounds are shown to pick from, and the family shown there.
    open: Option<usize>,
    family: usize,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static ROWS: Rc<VecModel<ArrSound>> = Rc::new(VecModel::default());
    static CHOICES: Rc<VecModel<ArrChoice>> = Rc::new(VecModel::default());
    static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// Play's key, from the one shortcut list (rule 15): again after a key is changed in Settings.
pub fn show_keys(app: &AppWindow, shortcuts: &mo_app::shortcuts::Shortcuts) {
    app.global::<ArrangementData>().set_play_key(shortcuts.key_of("app.play").into());
}

pub fn connect(app: &AppWindow) {
    let data = app.global::<ArrangementData>();
    ROWS.with(|rows| data.set_sounds(ModelRc::from(rows.clone())));
    CHOICES.with(|c| data.set_family_sounds(ModelRc::from(c.clone())));
    data.set_families(ModelRc::new(VecModel::from(
        play::FAMILIES.iter().map(|f| SharedString::from(*f)).collect::<Vec<_>>(),
    )));
    WINDOW.with(|w| *w.borrow_mut() = Some(app.as_weak()));
    let weak = app.as_weak();
    data.on_toggle_play(move || {
        if let Some(app) = weak.upgrade() {
            toggle_play(&app);
        }
    });
    let weak = app.as_weak();
    data.on_tick(move || {
        if let Some(app) = weak.upgrade() {
            tick(&app);
        }
    });
    let weak = app.as_weak();
    data.on_pick_line(move |i| {
        if let Some(app) = weak.upgrade() {
            pick_line(&app, usize::try_from(i).ok());
        }
    });
    let weak = app.as_weak();
    data.on_show_family(move |f| {
        if let Some(app) = weak.upgrade() {
            STATE.with(|s| s.borrow_mut().family = usize::try_from(f).unwrap_or(0).min(play::DRUMS));
            show_choices(&app);
        }
    });
    let weak = app.as_weak();
    data.on_pick_sound(move |k| {
        if let Some(app) = weak.upgrade() {
            let family = STATE.with(|s| s.borrow().family);
            if let Some(sound) = usize::try_from(k).ok().and_then(|k| play::family_sounds(family).get(k).copied()) {
                pick_sound(&app, Some(sound));
            }
        }
    });
    let weak = app.as_weak();
    data.on_matched_sound(move || {
        if let Some(app) = weak.upgrade() {
            pick_sound(&app, None);
        }
    });
}

/// The song whose arrangement is shown next: its key in the song library, which your sounds are kept under.
pub fn for_song(song: &str) {
    STATE.with(|s| s.borrow_mut().song = song.to_owned());
}

/// The arrangement on show changed (another song, or another of its views), or there's none now: what plays of the
/// one before stops, and the new one's instruments are matched.
pub fn shown(app: &AppWindow, arrangement: Option<Arc<Arrangement>>) {
    let same = STATE.with(|s| match (&s.borrow().arrangement, &arrangement) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    });
    if same {
        return;
    }
    stop(app);
    let shown = SHOWN.fetch_add(1, Ordering::SeqCst) + 1;
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.arrangement = arrangement.clone();
        s.voices = None;
        s.at = 0.0;
        s.from = 0.0;
        s.open = None;
        s.waiting = false;
        s.yours = YourSounds::from_setting(crate::settings::value(play::SOUNDS_KEY).as_deref()).of(&s.song);
    });
    let data = app.global::<ArrangementData>();
    data.set_play_note(SharedString::new());
    data.set_sound_open(-1);
    show_voices(app);
    let Some(a) = arrangement else { return };
    let notes: usize = a.lanes.iter().flat_map(|l| &l.clips).map(|c| c.notes.len()).sum();
    if notes < QUICK_NOTES {
        voices_matched(app, shown, Arc::new(play::voices(&a)));
        return;
    }
    let weak = app.as_weak();
    let started = background::spawn("Match the song's instruments to sounds", move || {
        if SHOWN.load(Ordering::SeqCst) != shown {
            return;
        }
        let voices = Arc::new(play::voices(&a));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                voices_matched(&app, shown, voices);
            }
        });
    });
    if let Err(e) = started {
        log::warning("Arrangement", &format!("Couldn't match the song's instruments to sounds: {e}"));
    }
}

/// The instruments of the arrangement shown as `shown` are matched: shown in the panel, and played if Play was
/// pressed meanwhile.
fn voices_matched(app: &AppWindow, shown: u64, voices: Arc<Vec<Voice>>) {
    if SHOWN.load(Ordering::SeqCst) != shown {
        return;
    }
    let waiting = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.voices = Some(voices);
        std::mem::take(&mut s.waiting)
    });
    show_voices(app);
    if waiting {
        start(app);
    }
}

/// The panel's lines, the Sounds chip and Play's tooltip, as the instruments and your sounds are now.
fn show_voices(app: &AppWindow) {
    let data = app.global::<ArrangementData>();
    let (rows, badge, count, open) = STATE.with(|s| {
        let s = s.borrow();
        let voices = s.voices.as_deref().map(Vec::as_slice).unwrap_or_default();
        let rows: Vec<ArrSound> = voices
            .iter()
            .map(|v| {
                let sound = play::sound_of(v, &s.yours);
                ArrSound {
                    name: v.name.as_str().into(),
                    sound: sound.name().into(),
                    tip: play::sound_tip(v, sound).into(),
                    yours: sound != v.matched,
                    colour: v.colour.map(colour).unwrap_or_default(),
                    has_colour: v.colour.is_some(),
                }
            })
            .collect();
        (rows, play::sounds_badge(voices, &s.yours), voices.len(), s.open)
    });
    ROWS.with(|model| {
        // Line by line, so the panel keeps its place as a sound is picked.
        if model.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&row) {
                    model.set_row_data(i, row);
                }
            }
        } else {
            model.set_vec(rows);
        }
    });
    data.set_sounds_badge(badge.into());
    let known = STATE.with(|s| s.borrow().voices.is_some());
    data.set_can_play(count > 0 || !known && STATE.with(|s| s.borrow().arrangement.is_some()));
    data.set_play_tip(play::play_tip(known.then_some(count)).into());
    if count == 0 {
        data.set_sounds_open(false);
    }
    data.set_sound_open(open.and_then(|o| i32::try_from(o).ok()).unwrap_or(-1));
    show_choices(app);
}

/// A track's colour (0xRRGGBB) for the panel's dot.
fn colour(rgb: u32) -> slint::Color {
    slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// The sounds of the family shown under the open line, the one it plays with lit.
fn show_choices(app: &AppWindow) {
    let (family, playing) = STATE.with(|s| {
        let s = s.borrow();
        let playing = s.open.and_then(|o| s.voices.as_ref()?.get(o).map(|v| play::sound_of(v, &s.yours)));
        (s.family, playing)
    });
    let choices: Vec<ArrChoice> = play::family_sounds(family)
        .into_iter()
        .map(|sound| ArrChoice { name: sound.name().into(), on: Some(sound) == playing })
        .collect();
    CHOICES.with(|model| model.set_vec(choices));
    app.global::<ArrangementData>().set_family_shown(i32::try_from(family).unwrap_or(0));
}

/// A click on a panel line: its sounds open under it, on the family of the sound it plays with; on the open line,
/// they close.
fn pick_line(app: &AppWindow, line: Option<usize>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.open == line {
            s.open = None;
            return;
        }
        s.open = line;
        let sound = line.and_then(|l| s.voices.as_ref()?.get(l).map(|v| play::sound_of(v, &s.yours)));
        if let Some(sound) = sound {
            s.family = play::family_of(sound);
        }
    });
    show_voices(app);
}

/// Picks `sound` for the open line's instrument (none: back to the one matched to it), keeps it for the song, and
/// while the song plays, plays on with it from where it is.
fn pick_sound(app: &AppWindow, sound: Option<Sound>) {
    let picked = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let voice = s.open.and_then(|o| s.voices.as_ref()?.get(o).cloned())?;
        // The matched one isn't kept as yours: it's what plays anyway.
        let sound = sound.filter(|sound| *sound != voice.matched);
        match sound {
            Some(sound) => s.yours.insert(voice.name.clone(), sound),
            None => s.yours.remove(&voice.name),
        };
        Some((s.song.clone(), voice.name, sound, s.playing))
    });
    let Some((song, name, sound, playing)) = picked else { return };
    if !song.is_empty() {
        let mut yours = YourSounds::from_setting(crate::settings::value(play::SOUNDS_KEY).as_deref());
        yours.pick(&song, &name, sound);
        if !crate::settings::remember(play::SOUNDS_KEY, &yours.setting()) {
            log::warning("Arrangement", "The sound picked is used until the app closes: the settings can't be kept");
        }
    }
    show_voices(app);
    if playing {
        // Heard at once: the song plays on from where it is with the new sound.
        let at = STATE.with(|s| s.borrow().at);
        let from = STATE.with(|s| s.borrow().from);
        start_at(app, at, from);
    }
}

/// Space and the Play button: plays the song's MIDI from the playhead, or stops it. False when no arrangement is
/// shown (so the key goes on).
pub fn toggle_play(app: &AppWindow) -> bool {
    let (shown, playing) = STATE.with(|s| {
        let s = s.borrow();
        (s.arrangement.is_some(), s.playing)
    });
    if !shown {
        return false;
    }
    if playing {
        stop(app);
    } else {
        start(app);
    }
    true
}

/// Plays from the playhead.
fn start(app: &AppWindow) {
    let at = STATE.with(|s| s.borrow().at);
    start_at(app, at, at);
}

/// Plays from `at` (beats), the play having started at `from` (where Stop puts the playhead back).
fn start_at(app: &AppWindow, at: f64, from: f64) {
    let data = app.global::<ArrangementData>();
    // What plays elsewhere would be cut off by it, or start over it later.
    crate::sounds::stop_for_another(app);
    crate::midi::stop_for_another(app);
    crate::curve_editor::call_off_tone();
    crate::song_player::stop_for_another(app);
    let id = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
    let ready = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.playing = true;
        s.key = None;
        s.at = at;
        s.from = from;
        let a = s.arrangement.clone()?;
        match s.voices.clone() {
            Some(voices) => Some((a, voices, s.yours.clone())),
            None => {
                s.waiting = true;
                None
            }
        }
    });
    data.set_playing(true);
    data.set_play_note(SharedString::new());
    let Some((a, voices, yours)) = ready else { return };
    if voices.is_empty() {
        stopped(app, Some("This song has no MIDI to play.".into()));
        return;
    }
    let chosen = crate::settings::value(mo_app::midi::SOUNDFONT_KEY);
    let found = crate::midi::soundfont_found();
    let weak = app.as_weak();
    let started = background::spawn("Play the song's MIDI", move || {
        let wanted = move || ASKED.load(Ordering::SeqCst) == id;
        let played = play_song(&a, &voices, &yours, at, found, chosen.as_deref(), id, &wanted);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            if !wanted() {
                return;
            }
            match played {
                Ok(key) => STATE.with(|s| s.borrow_mut().key = Some(key)),
                Err(why) => {
                    log::warning("Arrangement", &format!("The song's MIDI couldn't play: {why}"));
                    stopped(&app, Some(why));
                }
            }
        });
    });
    if let Err(e) = started {
        stopped(app, Some(format!("Couldn't start playing: {e}")));
    }
}

/// Makes the song's MIDI into sound from `at` (beats) and plays it, unless a newer play or a stop came meanwhile.
/// Gives the player's key for it. Off the window's thread: the SoundFont may be read here the first time.
#[allow(clippy::too_many_arguments)]
fn play_song(
    a: &Arrangement,
    voices: &[Voice],
    yours: &HashMap<String, Sound>,
    at: f64,
    found: Option<PathBuf>,
    chosen: Option<&str>,
    id: u64,
    wanted: &dyn Fn() -> bool,
) -> Result<String, String> {
    let path = found
        .or_else(|| mo_app::midi::soundfont_path(chosen, &mo_app::paths::data_dir()))
        .ok_or("To hear the song's MIDI, choose a General MIDI SoundFont (.sf2) on the MIDI page first.")?;
    let font = crate::midi::font_at(&path)?;
    let tempo = play::bpm(a);
    let played: Vec<(Vec<mo_audio::gm::Note>, u8, bool)> = voices
        .iter()
        .map(|v| {
            let (notes, program, drums) = v.plays(play::sound_of(v, yours));
            let notes = notes
                .iter()
                .map(|n| mo_audio::gm::Note { start: n.start, length: n.length, key: n.key, velocity: n.velocity7() })
                .collect();
            (notes, program, drums)
        })
        .collect();
    let parts: Vec<mo_audio::gm::Part> = played
        .iter()
        .map(|(notes, program, drums)| mo_audio::gm::Part {
            notes,
            ppq: mo_midi::PPQ,
            length: 0,
            tempo,
            program: *program,
            drums: *drums,
        })
        .collect();
    let song = Arc::new(mo_audio::gm::SongSound::new(&font, &parts));
    drop(parts);
    drop(played);
    let seconds = play::seconds_at(a, at);
    let stream = mo_audio::play::Stream::made(song, seconds, wanted).map_err(|e| e.to_string())?;
    let key = format!("{PLAY_KEY}{id}");
    crate::sounds::player()
        .play_stream_if(stream, key.clone(), || if wanted() { Ok(()) } else { Err(String::new()) })
        .map(|()| key)
}

/// While it plays: the playhead where the player is, the view following it. When the player has something else now
/// (a sound played another way), it's stopped.
fn tick(app: &AppWindow) {
    let (key, a) = STATE.with(|s| {
        let s = s.borrow();
        (s.key.clone(), s.arrangement.clone())
    });
    let (Some(key), Some(a)) = (key, a) else { return };
    let player = crate::sounds::player();
    if player.playing().as_deref() != Some(key.as_str()) {
        stopped(app, None);
        return;
    }
    let Some(seconds) = player.position() else { return };
    let beat = play::beat_at(&a, seconds);
    STATE.with(|s| s.borrow_mut().at = beat);
    crate::arrangement::place_playhead(app, Some(beat), true);
}

/// A press on the ruler at `beat`: the playhead goes there, and the song with it while it plays.
pub fn ruler(app: &AppWindow, beat: f64) {
    let (playing, key, a) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.at = beat;
        if !s.playing {
            s.from = beat;
        }
        (s.playing, s.key.clone(), s.arrangement.clone())
    });
    crate::arrangement::place_playhead(app, Some(beat), false);
    if !playing {
        return;
    }
    match (key, a) {
        (Some(key), Some(a)) if crate::sounds::player().playing().as_deref() == Some(key.as_str()) => {
            crate::sounds::player().seek(play::seconds_at(&a, beat));
            // Stop goes back to where you moved it to.
            STATE.with(|s| s.borrow_mut().from = beat);
        }
        // Still getting going: started again from there.
        _ => start_at(app, beat, beat),
    }
}

/// The view moved: the playhead keeps to its beat.
pub fn view_moved(app: &AppWindow) {
    let (at, shown) = STATE.with(|s| {
        let s = s.borrow();
        (s.at, s.playing || s.at > 0.0)
    });
    crate::arrangement::place_playhead(app, shown.then_some(at), false);
}

/// Stops the song's MIDI, the playhead back where the play started.
fn stop(app: &AppWindow) {
    let key = STATE.with(|s| s.borrow().key.clone());
    ASKED.fetch_add(1, Ordering::SeqCst);
    if let Some(key) = key {
        let player = crate::sounds::player();
        if player.playing().as_deref() == Some(key.as_str()) {
            player.stop();
        }
    }
    stopped(app, None);
}

/// It's no longer playing (stopped, ended, or another sound took its place): the button and playhead say so, and
/// `why` when it couldn't play.
fn stopped(app: &AppWindow, why: Option<String>) {
    let at = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let was = s.playing;
        s.playing = false;
        s.key = None;
        s.waiting = false;
        if was {
            s.at = s.from;
        }
        s.at
    });
    let data = app.global::<ArrangementData>();
    data.set_playing(false);
    data.set_play_note(why.unwrap_or_default().into());
    crate::arrangement::place_playhead(app, (at > 0.0).then_some(at), false);
}

/// The player says the sound under `key` reached its end: if it's this page's, it's stopped.
pub fn ended(key: &str) {
    let ours = STATE.with(|s| s.borrow().key.as_deref() == Some(key));
    if !ours {
        return;
    }
    WINDOW.with(|w| {
        if let Some(app) = w.borrow().as_ref().and_then(slint::Weak::upgrade) {
            stopped(&app, None);
        }
    });
}

/// Another page is about to play: the song's MIDI stops.
pub fn stop_for_another(app: &AppWindow) {
    if STATE.with(|s| s.borrow().playing) {
        stop(app);
    }
}

/// Esc: stops the song's MIDI when it plays. Whether it did.
pub fn stop_on_escape(app: &AppWindow) -> bool {
    let playing = STATE.with(|s| s.borrow().playing);
    if playing {
        stop(app);
    }
    playing
}
