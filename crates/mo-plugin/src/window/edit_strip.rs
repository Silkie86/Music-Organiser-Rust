//! The Edit strip along the bottom of the panel's Sounds tab, on the plugin's side (the 1.1 plan's light sample edits;
//! its look is Solomon's "Build it like this" of 27 Nov 2026, pictures 12 to 14). The strip itself has no screen and is
//! the app's too ([`mo_app::edit_strip`]): the sound open in it, its edits and their undo, what plays, and everything
//! it shows. This is what the panel gives it ([`Host`]): the sound picked in the list, the plugin's player (the edit
//! plays through the track), threads of its own to read, make and draw on (newest wins, rule 12), and its place on the
//! screen ([`SoundEdit`]).
//!
//! The strip is kept by the plugin, not by the window ([`crate::Instance::edit_strip`]): with the window closed and
//! opened again each sound's edits are where they were, for as long as the plugin stays loaded.
//!
//! What it writes is what the app's strip writes: a copy of an edit, saved or dragged out, in the export folder's
//! Edited sounds. Never the sound's own file (rule 2), and nothing in the app's settings or databases.
//!
//! Its screen is a picture the strip works out ([`screen::scene`]), drawn by the processor on a thread of its own, as
//! the curve editor's is here. Its Slices come with the plugin's next step: until then their key is faint.

use super::{Heard, Lines, Slots, take};
use crate::audition::{Played, Player};
use crate::library;
use crate::threads::{OneAtATime, background, newer};
use crate::{EditChip, EditMark, EditRoom, Panel, SliceCut, SoundEdit, Theme};
use keyboard_types::{Key, KeyboardEvent, NamedKey};
use mo_app::edit_copy::{self, Unread};
use mo_app::edit_strip::screen::{self, Colours, Room, Wave};
use mo_app::edit_strip::slices;
use mo_app::edit_strip::{self as strip, Answer, Carried, Host, Picked, Saving, Showing, Strip, Ticked, Work};
use mo_app::shapes::Colour;
use mo_app::shell;
use mo_app::shortcuts::{self, Shortcuts};
use mo_audio::AudioData;
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// In the tests nothing is read, written, carried or started: the work is done as it's asked for, on the test's own
/// thread, and a save only says where the copy would be (rule 11: tests never reach a real folder).
const HERE: bool = cfg!(test);

/// How long the drag handle glows once what it carried has landed, as the app's does.
const GLOW: Duration = Duration::from_millis(120);

/// The action Space has in the shortcut list: with the strip open on the picked sound, it plays the edit.
const PLAY: &str = "app.play";

/// The sound the strip is open on, as the list had it when the strip opened on it: the list may be searched or worked
/// out again while the strip stays on its sound.
struct On {
    picked: Picked,
    /// Its place in the whole list, where its line is looked for first.
    at: usize,
    /// Whether the library knows it as a loop (Trim silence drags a loop as it is).
    is_loop: bool,
}

/// The screen's picture, drawn: how often the screen had gone blank when it was asked for, and its pixels.
type Drawn = (u64, SharedPixelBuffer<Rgba8Pixel>);

/// What the strip's threads hand back to the window's thread, the newest of each.
#[derive(Default)]
struct Answers {
    /// A read or a make that's done: its ask, the sound's id and what came of it.
    work: Mutex<Option<(u64, (i64, Answer))>>,
    /// A save that's done: its ask, and the copy or why there's none.
    saved: Mutex<Option<(u64, Result<PathBuf, String>)>>,
    /// The screen's picture, drawn, with its number.
    picture: Mutex<Option<(u64, Drawn)>>,
    /// The shortcut list as the app's settings have it, once they're read.
    keys: Mutex<Option<Shortcuts>>,
}

/// The strip's lists as the screen has them: kept, and changed line by line, so the window keeps what it has laid out
/// of them instead of making it all again each time the strip shows itself.
#[derive(Default)]
struct Lists {
    ruler: Rc<VecModel<EditMark>>,
    chips: Rc<VecModel<EditChip>>,
    cuts: Rc<VecModel<SliceCut>>,
    cut_at_choices: Rc<VecModel<SharedString>>,
    each_choices: Rc<VecModel<SharedString>>,
    pads: Rc<VecModel<SharedString>>,
}

/// What the screen's picture is of.
#[derive(Default)]
struct Screen {
    /// The waveform the strip shows (its picture is on the screen, or on its way there): none while it has none.
    wave: Option<Arc<Wave>>,
    /// The colour of the sound's type, as the theme numbers them.
    colour: i32,
    /// Counts the times the screen went blank, so a picture asked for before that is dropped.
    shown: u64,
    /// Counts the pictures asked for, and the newest one that's up.
    pictures: u64,
    up: u64,
}

/// The panel as the strip's host: one to a window.
pub(super) struct OnPanel {
    panel: slint::Weak<Panel>,
    strip: RefCell<Strip>,
    lines: Rc<Lines>,
    player: Arc<Player>,
    slots: Arc<Slots>,
    /// What the window last heard clicked, and the sound its "Playing" line names: the window's own, shared, so the
    /// strip's play is a play like any other there (a click on its sound's line stops it, and so does Esc).
    heard: Rc<Cell<Option<(Heard, bool)>>>,
    playing: Rc<Cell<Option<u64>>>,
    /// The strip's own play, as the player numbers it, and where it stops (seconds into the edit: the end of the
    /// part it's of).
    mine: Cell<Option<u64>>,
    till: Cell<Option<f64>>,
    /// Whether the strip's play played at the last look: the strip shows itself again when that changes.
    was_playing: Cell<bool>,
    /// Where the moment heard is in the sound's own file (0 its start, 1 its end), for the play head on its line,
    /// and where the play started there: what's between the two has played.
    head: Cell<Option<(f32, f32)>>,
    /// The part of the sound's own file the strip is on, for its line.
    part: Cell<Option<(f32, f32)>>,
    on: RefCell<Option<On>>,
    answers: Arc<Answers>,
    /// The threads the strip's reads and makes, and its screen's pictures, are done on.
    worker: OneAtATime,
    drawer: OneAtATime,
    screen: RefCell<Screen>,
    lists: Lists,
    /// The shortcut list: the app's own keys until its settings are read.
    keys: RefCell<Shortcuts>,
    /// When what the drag handle carried landed: its glow goes a moment later.
    landed: Cell<Option<Instant>>,
}

impl OnPanel {
    /// The strip's host on `panel`, with the strip as the last window left it (`strip`), the list's lines, and what
    /// the window knows of what plays (`heard`, `playing`).
    pub(super) fn new(
        panel: &Panel,
        strip: Strip,
        lines: Rc<Lines>,
        player: Arc<Player>,
        slots: Arc<Slots>,
        heard: Rc<Cell<Option<(Heard, bool)>>>,
        playing: Rc<Cell<Option<u64>>>,
    ) -> Rc<OnPanel> {
        Rc::new(OnPanel {
            panel: panel.as_weak(),
            strip: RefCell::new(strip),
            lines,
            player,
            slots,
            heard,
            playing,
            mine: Cell::new(None),
            till: Cell::new(None),
            was_playing: Cell::new(false),
            head: Cell::new(None),
            part: Cell::new(None),
            on: RefCell::new(None),
            answers: Arc::default(),
            worker: OneAtATime::named("mo-plugin-edit"),
            drawer: OneAtATime::named("mo-plugin-edit-picture"),
            screen: RefCell::default(),
            lists: Lists::default(),
            keys: RefCell::default(),
            landed: Cell::new(None),
        })
    }

    /// Wires the strip's place on the panel to the strip, and has the app's shortcut list read for its keys.
    pub(super) fn fill_in(self: &Rc<Self>, panel: &Panel) {
        // What's picked on the strip's screen wears the Sounds place's colour, as in the app.
        panel.global::<Theme>().set_here(i32::try_from(mo_app::places::Place::Sounds.index()).unwrap_or(1));
        let data = panel.global::<SoundEdit>();
        data.set_can_slice(false);
        data.set_kept_while("while the plugin stays loaded".into());
        let me = Rc::clone(self);
        panel.on_edit(move || {
            strip::run_action(&*me, strip::EDIT);
        });
        let me = Rc::clone(self);
        data.on_laid_out(move |r| strip::laid_out(&*me, room(&r)));
        let me = Rc::clone(self);
        data.on_play(move || {
            strip::toggle_play(&*me);
        });
        data.on_part_text(|seconds, bpm| strip::part_text(f64::from(seconds), f64::from(bpm)).into());
        let me = Rc::clone(self);
        data.on_tool(move |id| {
            me.tool(&id);
        });
        let me = Rc::clone(self);
        data.on_gain_stepped(move |by| {
            strip::gain_stepped(&*me, by);
        });
        let me = Rc::clone(self);
        data.on_gain_reset(move || {
            strip::gain_reset(&*me);
        });
        let me = Rc::clone(self);
        data.on_fade_moving(move |fade_in, at| strip::fade_moving(&*me, fade_in, f64::from(at)));
        let me = Rc::clone(self);
        data.on_fade_let_go(move |fade_in| strip::fade_let_go(&*me, fade_in));
        let me = Rc::clone(self);
        data.on_snap(move |at, within| strip::snap(&me.strip.borrow(), at, within));
        let me = Rc::clone(self);
        data.on_wave_pressed(move || strip::wave_pressed(&mut me.strip.borrow_mut()));
        let me = Rc::clone(self);
        data.on_picked(move |from, to| strip::part_picked(&*me, f64::from(from), f64::from(to)));
        let me = Rc::clone(self);
        data.on_seek(move |at| strip::seek(&*me, f64::from(at)));
        let me = Rc::clone(self);
        data.on_stop(move || strip::stop(&*me));
        let me = Rc::clone(self);
        data.on_drag_handle(move || strip::drag_handle(&*me));
        let me = Rc::clone(self);
        data.on_drag_part(move || strip::drag(&*me, true));
        let me = Rc::clone(self);
        data.on_drag_let_go(move || me.slots.drag_let_go());
        let me = Rc::clone(self);
        data.on_hear_original(move |on| {
            strip::hear_original(&*me, on);
        });
        let me = Rc::clone(self);
        data.on_zoom(move |on| strip::zoom(&*me, on));
        let me = Rc::clone(self);
        data.on_close(move || strip::close(&*me));
        let me = Rc::clone(self);
        data.on_show_saved(move || strip::show_saved(&*me));
        self.show_keys(panel, Shortcuts::default());
        if !HERE {
            // The keys as the app's Settings have them: its settings file is read, never written, and off the
            // window's thread (the data drive may be asleep).
            let answers = Arc::clone(&self.answers);
            background("mo-plugin-edit-keys", move || {
                let setting = crate::midi::setting(&mo_app::paths::data_dir(), shortcuts::SETTING_KEY);
                *crate::threads::lock(&answers.keys) = Some(Shortcuts::from_setting(setting.as_deref()));
            });
        }
    }

    /// The strip's tips, with each tool's key from the shortcut list (rule 15); the keys are those from now on.
    fn show_keys(&self, panel: &Panel, keys: Shortcuts) {
        let data = panel.global::<SoundEdit>();
        let key = |id: &str| SharedString::from(keys.key_of(id));
        data.set_play_key(key(PLAY));
        data.set_edit_key(key(strip::EDIT));
        data.set_undo_key(key(strip::UNDO));
        data.set_redo_key(key(strip::REDO));
        data.set_save_key(key(strip::SAVE));
        data.set_original_key(key(strip::ORIGINAL));
        data.set_trim_key(key(strip::TRIM));
        data.set_fade_in_key(key(strip::FADE_IN));
        data.set_fade_out_key(key(strip::FADE_OUT));
        data.set_reverse_key(key(strip::REVERSE));
        data.set_normalise_key(key(strip::NORMALISE));
        data.set_louder_key(key(strip::LOUDER));
        data.set_quieter_key(key(strip::QUIETER));
        data.set_loop_key(key(strip::LOOP));
        data.set_slices_key(key(slices::SLICES));
        data.set_bitwig_key(key(slices::TO_BITWIG));
        strip::set_original_key(&mut self.strip.borrow_mut(), keys.key_of(strip::ORIGINAL));
        *self.keys.borrow_mut() = keys;
    }

    /// One of the strip's tools, by its id in the shortcut list (its key or its button): whether it did anything,
    /// none for an id that isn't the strip's. The Slices' own wait for the plugin's next step.
    fn tool(&self, id: &str) -> Option<bool> {
        if id == slices::SLICES || id == slices::TO_BITWIG {
            return Some(false);
        }
        strip::run_action(self, id)
    }

    /// A key on its way down that nothing in the panel used (no box is being typed in): whether the strip took it.
    /// Its keys are its own on the Sounds tab, where it is. Esc is the window's first, to stop what's heard
    /// (`sounding`: it had something to stop); with nothing to stop, it lets the part picked on the strip go, then
    /// closes the strip.
    pub(super) fn key(&self, panel: &Panel, key: &KeyboardEvent, sounding: bool) -> bool {
        if panel.get_view() != 0 {
            return false;
        }
        if key.key == Key::Named(NamedKey::Escape) {
            return !key.repeat && !sounding && self.escape();
        }
        self.key_down(&super::key_name(key), key.repeat)
    }

    /// A key on its way down, by its name as the shortcut list writes it ("Alt+T", "Space"; `repeat` while it's held
    /// down): one of the strip's does its job, once however long it's held. Whether it was the strip's to take: with
    /// the strip open its keys are its own even when one has nothing to do just then (Ctrl+Z with nothing to undo),
    /// so they never go on to move through the list.
    fn key_down(&self, name: &str, repeat: bool) -> bool {
        let (tool, play) = {
            let keys = self.keys.borrow();
            let tool = keys.page_action_for(name, shortcuts::SOUNDS_ACTIONS);
            // A page's own key comes before an app-wide one, as in the app.
            (tool, tool.is_none() && keys.action_for(name) == Some(PLAY))
        };
        let open = strip::open_on(&self.strip.borrow()).is_some();
        if play {
            return if repeat { open } else { strip::toggle_play(self) };
        }
        let Some(id) = tool.filter(|id| id.starts_with("sounds.edit")) else { return false };
        if repeat {
            return open;
        }
        self.tool(id).is_some_and(|did| did || open)
    }

    /// A key let go, by its name with no Ctrl, Shift or Alt: O stops the original.
    pub(super) fn key_up(&self, name: &str) {
        strip::key_up(self, name);
    }

    /// The window lost the keys: a key held down there is never seen let go, so the original stops as if it were.
    pub(super) fn keys_gone(&self) {
        strip::hear_original(self, false);
    }

    /// Esc with nothing playing: the part picked on the strip goes, then the strip. Whether it did one.
    fn escape(&self) -> bool {
        strip::escape(self)
    }

    /// The sound the strip is open on, by its id, and its place in the whole list as last known: its line stays
    /// picked through a search, while the new list still shows it.
    pub(super) fn on(&self) -> Option<(i64, Option<usize>)> {
        let open = strip::open_on(&self.strip.borrow())?;
        let at = self.on.borrow().as_ref().filter(|on| on.picked.id == open).map(|on| on.at);
        Some((open, at))
    }

    /// Where the strip's play is on its sound's line, while it plays and the line is the one picked: its row, how
    /// far through the sound's own file the moment heard is, and how far through it the play started.
    pub(super) fn line_head(&self, panel: &Panel) -> Option<(i32, f32, f32)> {
        if !self.plays() {
            return None;
        }
        let (at, from) = self.head.get()?;
        Some((i32::try_from(self.own_row(panel)?).ok()?, at, from))
    }

    /// What the drag handle carries is being carried out of the window now (`on`), or isn't any more.
    pub(super) fn lifted(&self, panel: &Panel, on: bool) {
        panel.global::<SoundEdit>().set_lifted(on);
    }

    /// What the drag handle carried landed: the handle glows for a moment ([`OnPanel::tick`] ends it).
    pub(super) fn landed(&self, panel: &Panel) {
        panel.global::<SoundEdit>().set_landed(true);
        self.landed.set(Some(Instant::now()));
    }

    /// The window is going: the strip, put away for the next one ([`strip::put_away`]), each sound's edits kept. None
    /// if it's in the middle of something, which it isn't as a window closes.
    pub(super) fn put_away(&self) -> Option<Strip> {
        let mut strip = std::mem::take(&mut *self.strip.try_borrow_mut().ok()?);
        strip::put_away(&mut strip);
        Some(strip)
    }

    /// The window's timer: what the strip's threads found is taken in, the strip follows the line picked in the list,
    /// and its play shows as it goes, on the strip and on its sound's line.
    pub(super) fn tick(&self, panel: &Panel) {
        if let Some(keys) = take(&self.answers.keys) {
            self.show_keys(panel, keys);
        }
        if let Some((ask, (id, answer))) = take(&self.answers.work) {
            strip::arrived(self, ask, id, answer);
        }
        if let Some((ask, saved)) = take(&self.answers.saved) {
            strip::saved(self, ask, saved);
        }
        if let Some((number, (shown, pixels))) = take(&self.answers.picture) {
            self.put_up(panel, shown, number, pixels);
        }
        // Another line picked in the list (a click, an arrow key, the DAW's knob): the strip edits that sound now.
        let open = strip::open_on(&self.strip.borrow());
        let on_row = usize::try_from(panel.get_picked()).ok().and_then(|row| self.lines.sound(row));
        if let (Some(open), Some(on_row)) = (open, on_row)
            && open != on_row
        {
            strip::sound_picked(self);
        }
        if self.plays() {
            let at = self.mine.get().and_then(|n| self.player.seconds(n));
            let ticked = strip::tick(&mut self.strip.borrow_mut(), at, self.till.get());
            match ticked {
                Ticked::Waiting => {}
                Ticked::At(now) => {
                    let data = panel.global::<SoundEdit>();
                    data.set_played_from(now.from);
                    data.set_played(now.at);
                    data.set_level_left(now.level[0]);
                    data.set_level_right(now.level[1]);
                    self.head.set(now.in_file.zip(now.from_in_file));
                }
                // A play of the part picked has come to the part's end.
                Ticked::Ended => Host::stop(self),
            }
        }
        // It started or stopped, here or anywhere in the window (Esc, a click on another line, its own end).
        if self.plays() != self.was_playing.get() {
            self.playing_changed();
        }
        // Its sound's line in the list shows the part of the file the strip is on.
        let own = self.own_row(panel);
        if let Some(row) = own
            && let Some(at) = self.lines.at(row)
            && let Some(on) = self.on.borrow_mut().as_mut()
        {
            on.at = at;
        }
        let (from, to) = self.part.get().unwrap_or_default();
        panel.set_edit_row(own.and_then(|row| i32::try_from(row).ok()).unwrap_or(-1));
        panel.set_edit_from(from);
        panel.set_edit_to(to);
        if self.landed.get().is_some_and(|at| at.elapsed() >= GLOW) {
            self.landed.set(None);
            panel.global::<SoundEdit>().set_landed(false);
        }
    }

    /// The row of the strip's sound in the list, when that's the line picked there.
    fn own_row(&self, panel: &Panel) -> Option<usize> {
        let open = strip::open_on(&self.strip.borrow())?;
        let row = usize::try_from(panel.get_picked()).ok()?;
        (self.lines.sound(row) == Some(open)).then_some(row)
    }

    /// The name of the strip's sound's file, as the "Playing" line says it.
    fn file_name(&self) -> Option<(i64, String)> {
        let on = self.on.borrow();
        let on = on.as_ref()?;
        Some((on.picked.id, library::name(&on.picked.path.to_string_lossy()).to_owned()))
    }

    /// Says `line` where the panel says what plays.
    fn say(&self, line: &str) {
        if let Some(panel) = self.panel.upgrade() {
            panel.set_playing(line.into());
        }
    }

    /// The strip's play started or stopped: the strip shows itself with it, lit from where the play starts and its
    /// meter the play's, or with nothing lit and its meter the edit's own loudest again.
    fn playing_changed(&self) {
        let plays = self.plays();
        self.was_playing.set(plays);
        if !plays {
            self.till.set(None);
            self.head.set(None);
        }
        if let Some(panel) = self.panel.upgrade() {
            panel.global::<SoundEdit>().set_playing(plays);
        }
        strip::show(self);
    }

    /// Keeps the sound the strip is open on ([`On`]) the one it is open on: taken from the line picked in the list as
    /// the strip opens on it, and let go as the strip closes.
    fn follow(&self, panel: &Panel) {
        let open = strip::open_on(&self.strip.borrow());
        let mut on = self.on.borrow_mut();
        match open {
            None => *on = None,
            Some(id) if on.as_ref().is_some_and(|on| on.picked.id == id) => {}
            Some(id) => {
                let row = usize::try_from(panel.get_picked()).ok();
                *on = row
                    .and_then(|row| self.lines.picked(row))
                    .filter(|(picked, ..)| picked.id == id)
                    .map(|(picked, at, is_loop)| On { picked, at, is_loop });
            }
        }
    }

    /// The strip shows `wave` now (none: it has no waveform to show), of a sound whose type's colour is `colour`. Its
    /// picture is drawn again only for another waveform or another colour; the one on the screen stays until the new
    /// one is there, and goes at once when there's none to show.
    fn show_wave(&self, panel: &Panel, wave: Option<Arc<Wave>>, colour: i32) {
        let blank = wave.is_none();
        {
            let mut s = self.screen.borrow_mut();
            let same = match (&s.wave, &wave) {
                (Some(old), Some(new)) => Arc::ptr_eq(old, new) && s.colour == colour,
                (None, None) => true,
                _ => false,
            };
            if same {
                return;
            }
            if blank {
                s.shown += 1;
            }
            (s.wave, s.colour) = (wave, colour);
        }
        if blank {
            panel.global::<SoundEdit>().set_wave_picture(Image::default());
        } else {
            self.redraw(panel);
        }
    }

    /// Asks for a picture of the screen as it is now, drawn on the drawing thread, the newest asked winning (rule 12).
    fn redraw(&self, panel: &Panel) {
        let (shown, number, wave, colour) = {
            let mut s = self.screen.borrow_mut();
            let Some(wave) = s.wave.clone() else { return };
            s.pictures += 1;
            (s.shown, s.pictures, wave, s.colour)
        };
        let colours = colours(panel, colour);
        if HERE {
            self.put_up(panel, shown, number, picture(&wave, &colours));
            return;
        }
        let answers = Arc::clone(&self.answers);
        self.drawer.ask(move || {
            let started = Instant::now();
            let pixels = picture(&wave, &colours);
            crate::log::took_if_slow("Edit", "Drew the strip's screen", started);
            newer(&answers.picture, number, (shown, pixels));
        });
    }

    /// A picture has been drawn (picture `number`): put on the screen, unless the screen went blank since it was asked
    /// for, or a newer one is up already.
    fn put_up(&self, panel: &Panel, shown: u64, number: u64, pixels: SharedPixelBuffer<Rgba8Pixel>) {
        {
            let mut s = self.screen.borrow_mut();
            if s.shown != shown || number <= s.up {
                return;
            }
            s.up = number;
        }
        panel.global::<SoundEdit>().set_wave_picture(Image::from_rgba8_premultiplied(pixels));
    }
}

impl Host for OnPanel {
    fn strip(&self) -> &RefCell<Strip> {
        &self.strip
    }

    /// The sound on the line picked in the list. With no line picked (the list was searched again and doesn't show the
    /// strip's sound any more) it's still the strip's own sound, so the strip stays open on it. With the Galaxy in the
    /// list's place the line picked is out of sight: the strip isn't opened from there, and one that's open stays.
    fn picked(&self) -> Option<Picked> {
        let panel = self.panel.upgrade()?;
        let open = strip::open_on(&self.strip.borrow());
        if open.is_none() && panel.get_galaxy() {
            return None;
        }
        let row = usize::try_from(panel.get_picked()).ok();
        if let Some((picked, ..)) = row.and_then(|row| self.lines.picked(row)) {
            return Some(picked);
        }
        self.on.borrow().as_ref().filter(|on| open == Some(on.picked.id)).map(|on| on.picked.clone())
    }

    /// The plugin's list has no card to pick a part on: a sound too long to edit whole isn't opened here.
    fn card_part(&self) -> Option<(f64, f64)> {
        None
    }

    fn show(&self, showing: Showing) {
        let Some(panel) = self.panel.upgrade() else { return };
        self.follow(&panel);
        let data = panel.global::<SoundEdit>();
        let Showing {
            open,
            folder_text,
            name,
            colour,
            facts,
            bpm,
            note,
            ready,
            wave,
            view_seconds,
            view_from,
            zoomed,
            has_part,
            pick_from,
            pick_to,
            pick_on_hits,
            fade_in,
            fade_out,
            fade_in_shown,
            fade_out_shown,
            fade_in_curve,
            fade_out_curve,
            fade_in_shade,
            fade_out_shade,
            fade_text,
            ruler,
            chips,
            can_undo,
            can_redo,
            has_edits,
            original,
            played,
            gain_text,
            gain_changed,
            too_loud,
            levels,
            drag_text,
            can_drag,
            save_text,
            can_save,
            saved,
            save_note,
            save_detail,
            save_failed,
            file_part,
            slices,
        } = showing;
        // The part of its own file the strip is on, for its sound's line in the list.
        self.part.set(file_part.filter(|_| open));
        // The strip's own words for a sound too long are about the app's card, where a part of it can be picked.
        let note = if note == Unread::TooLong.to_string() { too_long() } else { note };
        data.set_shown(open);
        data.set_folder_text(folder_text.into());
        data.set_name(name.into());
        data.set_colour(colour);
        data.set_facts(facts.into());
        data.set_bpm(bpm);
        data.set_note(note.into());
        data.set_ready(ready);
        self.show_wave(&panel, wave, colour);
        data.set_view_seconds(view_seconds);
        data.set_view_from(view_from);
        data.set_zoomed(zoomed);
        data.set_has_part(has_part);
        data.set_pick_from(pick_from);
        data.set_pick_to(pick_to);
        data.set_pick_from_on_hit(pick_on_hits.0);
        data.set_pick_to_on_hit(pick_on_hits.1);
        data.set_fade_in(fade_in);
        data.set_fade_out(fade_out);
        data.set_fade_in_shown(fade_in_shown);
        data.set_fade_out_shown(fade_out_shown);
        data.set_fade_in_curve(fade_in_curve.into());
        data.set_fade_out_curve(fade_out_curve.into());
        data.set_fade_in_shade(fade_in_shade.into());
        data.set_fade_out_shade(fade_out_shade.into());
        data.set_fade_text(fade_text.into());
        data.set_can_undo(can_undo);
        data.set_can_redo(can_redo);
        data.set_has_edits(has_edits);
        data.set_original(original);
        // Where the strip's play is lit from and has got to: the window's timer moves them on from here.
        let (played_from, played_to) = played.unwrap_or_default();
        data.set_played_from(played_from);
        data.set_played(played_to);
        data.set_gain_text(gain_text.into());
        data.set_gain_changed(gain_changed);
        data.set_too_loud(too_loud);
        let strip::Levels { level, hold, peak_text, over } = levels;
        data.set_level_left(level[0]);
        data.set_level_right(level[1]);
        data.set_hold_left(hold[0]);
        data.set_hold_right(hold[1]);
        data.set_meter_amber(strip::METER_AMBER);
        data.set_peak_text(peak_text.into());
        data.set_over(over);
        data.set_drag_text(drag_text.into());
        // Only where a file can be carried out of the window at all (`crate::drag`).
        data.set_can_drag(can_drag && crate::drag::CAN);
        data.set_save_text(save_text.into());
        data.set_can_save(can_save);
        data.set_saved(saved);
        data.set_save_note(save_note.into());
        data.set_save_detail(save_detail.into());
        data.set_save_failed(save_failed);
        let slices::Showing {
            on,
            cuts,
            slice_from,
            slice_to,
            first_cut,
            slice,
            picks,
            lead,
            lead_room,
            cut_at,
            cut_at_choices,
            cut_at_hits,
            sensitivity,
            each,
            each_choices,
            send_text,
            can_send,
            pads,
            to_folder_text,
            saved_in,
            sending,
            sent,
            made,
            making,
            foot_name,
            foot_facts,
            cuts_mark,
        } = slices;
        data.set_slicing(on);
        data.set_slice_from(slice_from);
        data.set_slice_to(slice_to);
        data.set_first_cut(first_cut);
        let number = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
        data.set_slice_number(number(slice));
        data.set_slice_picks(picks);
        data.set_lead_number(number(lead));
        data.set_lead_room(lead_room);
        data.set_cut_at(cut_at.into());
        data.set_cut_at_hits(cut_at_hits);
        data.set_sensitivity(sensitivity);
        data.set_each(each.into());
        data.set_send_text(send_text.into());
        data.set_can_send(can_send);
        data.set_sending(sending.is_some());
        data.set_send_progress(sending.unwrap_or(0.0));
        data.set_sent(sent);
        data.set_pads_made(number(made));
        data.set_pads_making(making);
        data.set_to_folder_text(to_folder_text.into());
        data.set_saved_in(saved_in.into());
        data.set_slice_name(foot_name.into());
        data.set_slice_facts(foot_facts.into());
        data.set_cuts_mark(cuts_mark);
        // The lists: the screen's own ones (given to it the first time), with only the lines that differ changed.
        let words = |words: Vec<String>| words.into_iter().map(SharedString::from).collect::<Vec<_>>();
        let l = &self.lists;
        data.set_ruler(ModelRc::from(l.ruler.clone()));
        data.set_chips(ModelRc::from(l.chips.clone()));
        data.set_cuts(ModelRc::from(l.cuts.clone()));
        data.set_cut_at_choices(ModelRc::from(l.cut_at_choices.clone()));
        data.set_each_choices(ModelRc::from(l.each_choices.clone()));
        data.set_pads(ModelRc::from(l.pads.clone()));
        let marks = ruler.into_iter().map(|m| EditMark { at: m.at, text: m.text.into(), bar: m.bar });
        refill(&l.ruler, marks.collect());
        refill(&l.chips, chips.into_iter().map(|c| EditChip { text: c.text.into(), last: c.last }).collect());
        let cuts = cuts.into_iter().map(|c| SliceCut { at: c.at, number: number(c.number), room: c.room });
        refill(&l.cuts, cuts.collect());
        refill(&l.cut_at_choices, words(cut_at_choices));
        refill(&l.each_choices, words(each_choices));
        refill(&l.pads, words(pads));
    }

    /// On the strip's thread, the newest asked winning; its answer comes back at the window's next tick
    /// ([`OnPanel::tick`]).
    fn work(&self, ask: u64, id: i64, work: Work) {
        if HERE {
            // The tests hand a sound over themselves (nothing is read), and an edit is made here and now.
            if matches!(work, Work::Make { .. }) {
                let answer = strip::answer(work);
                strip::arrived(self, ask, id, answer);
            }
            return;
        }
        let answers = Arc::clone(&self.answers);
        self.worker.ask(move || {
            let started = Instant::now();
            let what = match &work {
                Work::Read(source) => format!("Read {} to edit", source.path.display()),
                Work::Make { edits, .. } => format!("Made the edit ({} edits)", edits.len()),
            };
            let answer = strip::answer(work);
            match &answer {
                // The strip itself says why on its screen; the plugin's log keeps it.
                Answer::Read(Err(why)) if *why != Unread::TooLong => {
                    crate::log::line("Edit", &format!("Couldn't open the sound: {why}"));
                }
                Answer::Made(Err(why)) => crate::log::line("Edit", why),
                _ => crate::log::took_if_slow("Edit", &what, started),
            }
            newer(&answers.work, ask, (id, answer));
        });
    }

    /// Through the track, straight from memory: handed to the player here and now.
    fn play(&self, audio: Arc<AudioData>, from: f64, till: Option<f64>) {
        let Some((id, name)) = self.file_name() else { return };
        match self.player.play_shared(audio, name, from, till) {
            Played::Playing(name, n) => {
                self.heard.set(Some((Heard::Edit(id), true)));
                self.playing.set(Some(n));
                self.mine.set(Some(n));
                self.till.set(till);
                self.say(&format!("Playing {name}"));
            }
            // Not handed over: the panel says why, as for any sound that couldn't start. What was playing plays on and
            // is still what's heard; a line still being read was called off by the try (it's the newest pick no more).
            Played::Not(why) => {
                if self.heard.get().is_some_and(|(_, started)| !started) {
                    self.heard.set(None);
                }
                self.say(&why);
            }
        }
        self.playing_changed();
    }

    /// Swapped in by the player itself at the moment the strip's play has got to, so nothing is heard twice or
    /// missed: only while that play is still the one playing. The edit stops where the play was to stop, to the frame
    /// (the end of the part picked); the original, while it's held, plays on.
    fn swap(&self, audio: Arc<AudioData>, place: &dyn Fn(f64) -> f64, held: bool) -> bool {
        if !self.plays() {
            return false;
        }
        let (Some(n), Some((_, name))) = (self.mine.get(), self.file_name()) else { return false };
        let till = self.till.get().filter(|_| !held);
        match self.player.swap_shared(n, audio, name, place, till) {
            Some(Played::Playing(_, next)) => {
                self.mine.set(Some(next));
                self.playing.set(Some(next));
                true
            }
            // Not handed over (the audio thread had no room for it): what played plays on.
            Some(Played::Not(why)) => {
                crate::log::line("Edit", &format!("Couldn't swap what the strip plays: {why}"));
                false
            }
            None => false,
        }
    }

    /// The strip's play is what the window knows as heard and as playing, and the player still has it. A line
    /// picked since is what's heard from that moment: the strip's play is over for the strip (it stops nothing, and
    /// swaps nothing in), though it sounds on until the line's file is read, as one line does under the next.
    fn plays(&self) -> bool {
        matches!(self.heard.get(), Some((Heard::Edit(_), _)))
            && self.mine.get().is_some_and(|n| self.playing.get() == Some(n) && self.player.still_playing(n))
    }

    /// The strip's play, or the sound itself from a click on its line (being read still, or playing), or an edit of
    /// it the last window left playing.
    fn picked_plays(&self) -> bool {
        if self.plays() {
            return true;
        }
        let Some(picked) = self.picked() else { return false };
        match self.heard.get() {
            Some((Heard::Line(sound) | Heard::Edit(sound), started)) if sound == picked.id => {
                !started || self.playing.get().is_some_and(|n| self.player.still_playing(n))
            }
            _ => false,
        }
    }

    fn stop(&self) {
        self.player.stop();
        self.heard.set(None);
        self.playing.set(None);
        self.mine.set(None);
        self.say("");
        self.playing_changed();
    }

    /// The app counts a play towards how liked a sound is, in its own databases: the plugin writes nothing there.
    fn played(&self) {}

    /// The copy is made (or found, made before) away from the window's thread; the window carries it out once it's
    /// there, as it carries a line's file ([`super::Shown::drag`]).
    fn carry(&self, what: Carried) {
        if HERE || !crate::drag::CAN {
            return;
        }
        let Some((path, is_loop)) = self.on.borrow().as_ref().map(|on| (on.picked.path.clone(), on.is_loop)) else {
            return;
        };
        let (slots, trim) = (Arc::clone(&self.slots), self.player.trim());
        let asked = slots.ask_drag();
        let started = background("mo-plugin-drag", move || {
            let found = to_carry(&path, what, trim, is_loop, &slots);
            slots.drag_ready(asked, super::Carried::Edit, found);
        });
        if !started {
            crate::log::line("Drag", "Couldn't start making the copy to drag");
        }
    }

    fn let_go(&self) {
        self.slots.drag_let_go();
    }

    /// On a thread of its own. The strip shows how it went, and the plugin's log keeps it.
    fn save(&self, edit: Saving) -> bool {
        let Saving { ask, source, original, changed, edits, name } = edit;
        if HERE {
            // The tests: the strip shows the copy saved, under the name it would have, and nothing is written.
            let file = edit_copy::edit_file_name(&source, &edits);
            strip::saved(self, ask, Ok(PathBuf::from(edit_copy::FOLDER).join(file)));
            return true;
        }
        let answers = Arc::clone(&self.answers);
        let started = background("mo-plugin-edit-save", move || {
            let began = Instant::now();
            // Answered however it went: the strip's Save waits for this.
            let saved = mo_catch::catch_crash(AssertUnwindSafe(|| {
                edit_copy::edit_to_keep(&source, &original, changed, &edits)
            }))
            .unwrap_or_else(|_| Err("Something went wrong (see the crash note).".to_owned()));
            match &saved {
                Ok(copy) => crate::log::took("Edit", &format!("Saved the edit of {name} as {}", copy.display()), began),
                Err(why) => crate::log::line("Edit", &format!("Couldn't save the edit of {name}: {why}")),
            }
            newer(&answers.saved, ask, saved);
        });
        if !started {
            crate::log::line("Edit", "Couldn't start saving the edit");
        }
        started
    }

    /// In its folder, in Explorer: started from a thread of its own at the usual priority, as a program started
    /// takes the priority of the thread that starts it (on Linux and a Mac), and the plugin's other threads are low.
    fn reveal(&self, copy: PathBuf) {
        if HERE {
            return;
        }
        let started = std::thread::Builder::new().name("mo-plugin-reveal".into()).spawn(move || {
            let _ = mo_catch::catch_crash(AssertUnwindSafe(|| {
                let path = copy.to_string_lossy().into_owned();
                let launch = shell::reveal(&path, shell::folder_of(&path), shell::exists(&path), shell::Os::THIS);
                if let Err(e) = shell::run(&launch) {
                    crate::log::line("Edit", &format!("Couldn't show {path} in its folder: {e}"));
                }
            }));
        });
        if let Err(e) = started {
            crate::log::line("Edit", &format!("Couldn't start showing the copy in its folder: {e}"));
        }
    }

    /// The Slices come with the plugin's next step.
    fn send(&self, _: slices::To, _: slices::Slices) -> bool {
        false
    }
}

/// The file to carry out for `what` of the sound at `path`, found or made away from the window's thread: its copy as
/// edited, or the sound itself, as its line in the list drags it.
fn to_carry(path: &Path, what: Carried, trim: Option<i32>, is_loop: bool, slots: &Slots) -> Result<PathBuf, String> {
    // Looked for here: its drive may be asleep or away.
    if !path.is_file() {
        return Err(format!("Can't drag {}: the file isn't there", library::name(&path.to_string_lossy())));
    }
    match what {
        Carried::Edit { source, audio, changed, edits } => {
            mo_catch::catch_crash(AssertUnwindSafe(|| edit_copy::edit_to_keep(&source, &audio, changed, &edits)))
                .unwrap_or_else(|_| Err("Couldn't make the copy to drag (see the crash note).".to_owned()))
        }
        // A part longer than the longest that's copied goes as the whole file.
        Carried::Part(from, to) if to - from <= edit_copy::MOST_SECONDS => edit_copy::part_to_drag(path, from, to),
        Carried::Whole | Carried::Part(..) => Ok(super::sound_to_drag(path, trim, is_loop, slots)),
    }
}

/// Why a sound too long to edit whole isn't opened here, in the plugin's own words.
fn too_long() -> String {
    format!(
        "It's longer than {} minutes, too long to edit here. In the app, pick a part of it on its card to edit that part.",
        edit_copy::EDIT_MOST_SECONDS / 60.0
    )
}

/// The room the screen says it has, as the strip takes it.
fn room(r: &EditRoom) -> Room {
    Room {
        width: f64::from(r.width),
        height: f64::from(r.height),
        inset: f64::from(r.inset),
        band: f64::from(r.band),
        top: f64::from(r.top),
        bottom: f64::from(r.bottom),
        scale: f64::from(r.scale),
    }
}

/// A colour of the panel's as the picture takes it.
fn shade(c: slint::Color) -> Colour {
    Colour { r: c.red(), g: c.green(), b: c.blue(), a: c.alpha() }
}

/// The screen's colours for a sound whose type's colour is `colour`, as the panel's theme has them.
fn colours(panel: &Panel, colour: i32) -> Colours {
    let t = panel.global::<Theme>();
    let (tint, text, well) = (t.invoke_sound_type(colour), t.get_text(), t.get_well());
    Colours {
        well: u32::from(well.red()) << 16 | u32::from(well.green()) << 8 | u32::from(well.blue()),
        line: shade(t.get_stroke()),
        mark: shade(t.get_muted()),
        tint: shade(tint),
        grey: shade(t.get_muted()),
        // What has played: all but the colour of the words, with a little of the sound's own left in it.
        lit: shade(text.mix(&tint, 0.75)),
        bright: shade(text),
        over: shade(t.get_missing()),
    }
}

/// The screen's picture of `wave`, drawn by the processor. The strip works it out in pixels already, so it's drawn at
/// a pixel to a point.
fn picture(wave: &Wave, colours: &Colours) -> SharedPixelBuffer<Rgba8Pixel> {
    let pixmap = screen::scene(wave, colours).paint(1.0);
    SharedPixelBuffer::clone_from_slice(pixmap.data(), pixmap.width(), pixmap.height())
}

/// Puts `rows` in `model` in place of what's there, changing only the lines that differ, so the window keeps what it
/// has laid out of a list instead of making it all again (as the app's own `canvas::refill`).
fn refill<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    let old = model.row_count();
    let keep = old.min(rows.len());
    let mut rows = rows.into_iter();
    for (i, row) in rows.by_ref().take(keep).enumerate() {
        if model.row_data(i).as_ref() != Some(&row) {
            model.set_row_data(i, row);
        }
    }
    model.extend(rows);
    for i in (keep..old).rev() {
        model.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{draw, test_panel};
    use super::super::{key_in, key_text};
    use super::*;
    use crate::SoundLine;
    use crate::library::Library;
    use keyboard_types::{Code, KeyState, Modifiers};
    use slint::platform::WindowAdapter;
    use slint::platform::software_renderer::MinimalSoftwareWindow;

    /// The DAW's rate in these tests, and the sounds'.
    const RATE: u32 = 48_000;

    /// Two seconds of sound, as if a file held it.
    fn sound() -> AudioData {
        let frames = 2 * RATE as usize;
        let mut a = AudioData { sample_rate: RATE, channels: vec![vec![0.5; frames]], ..AudioData::default() };
        a.file_seconds = a.seconds();
        a
    }

    /// A panel with four sounds in its list (a kick, a snare, a break and a five-minute set, on rows 0 to 3) and the
    /// strip on it, wired as the window wires them, with what the window knows of what plays. Nothing is behind it: no
    /// DAW, no files.
    struct Bench {
        panel: Panel,
        window: Rc<MinimalSoftwareWindow>,
        host: Rc<OnPanel>,
        lines: Rc<Lines>,
        library: Arc<Library>,
        heard: Rc<Cell<Option<(Heard, bool)>>>,
        playing: Rc<Cell<Option<u64>>>,
        player: Arc<Player>,
    }

    impl Bench {
        fn new() -> Bench {
            let (panel, window) = test_panel();
            let mut set = mo_sounds::Sound::new(13, r"D:\Mixes\set.wav");
            set.analysis = Some(mo_sounds::SoundAnalysis { seconds: 300.0, ..Default::default() });
            let sounds = vec![
                mo_sounds::Sound::new(10, r"D:\Drums\kick.wav"),
                mo_sounds::Sound::new(11, r"D:\Drums\snare.wav"),
                mo_sounds::Sound::new(12, r"D:\Loops\break.wav"),
                set,
            ];
            let library = Arc::new(Library::new(sounds));
            let lines = Rc::new(Lines::default());
            panel.set_sounds(ModelRc::from(Rc::clone(&lines) as Rc<dyn Model<Data = SoundLine>>));
            let (heard, playing) = (Rc::new(Cell::new(None)), Rc::new(Cell::new(None)));
            let player = Arc::new(Player::default());
            let host = OnPanel::new(
                &panel,
                Strip::default(),
                Rc::clone(&lines),
                Arc::clone(&player),
                Arc::default(),
                Rc::clone(&heard),
                Rc::clone(&playing),
            );
            host.fill_in(&panel);
            let bench = Bench { panel, window, host, lines, library, heard, playing, player };
            bench.list(&[10, 11, 12, 13]);
            bench
        }

        /// The list shows these sounds, by their ids, in this order.
        fn list(&self, ids: &[i64]) {
            let places = ids.iter().map(|id| self.library.list.find(*id).expect("a sound of the bench's")).collect();
            self.lines.show(Some(Arc::clone(&self.library)), Arc::new(places));
        }

        fn data(&self) -> SoundEdit<'_> {
            self.panel.global::<SoundEdit>()
        }

        /// The line on `row` is picked (a click, an arrow, the DAW's knob), and the window's timer has run since.
        fn pick(&self, row: i32) {
            self.panel.set_picked(row);
            self.host.tick(&self.panel);
        }

        /// The strip, open on the sound picked, gets what that sound's file holds ([`sound`]): `tools`' edits are
        /// made and a part of it picked (0 to 1 of it). Opens the strip first if it's closed.
        fn read(&self, part: Option<(f64, f64)>, tools: &[&str]) {
            strip::for_screens(&*self.host, sound(), part, tools);
        }

        /// A key on its way down, as the window hands it on: to what has the keys in the panel first, then to the
        /// strip, then to the lists. `sounding`: Esc had a sound to stop.
        fn key(&self, key: Key, code: Code, modifiers: Modifiers, sounding: bool) {
            let event = KeyboardEvent { state: KeyState::Down, key, code, modifiers, ..Default::default() };
            let text = key_text(&event.key).expect("a key Slint knows");
            key_in(self.window.window(), &self.panel, &event, text, || self.host.key(&self.panel, &event, sounding));
        }

        /// A letter, with what's held.
        fn letter(&self, letter: &str, code: Code, modifiers: Modifiers) {
            self.key(Key::Character(letter.into()), code, modifiers, false);
        }

        fn chips(&self) -> Vec<String> {
            self.data().get_chips().iter().map(|chip| chip.text.to_string()).collect()
        }
    }

    #[test]
    fn the_edit_button_opens_the_strip_on_the_line_picked_and_closes_it() {
        let b = Bench::new();
        // No line picked: nothing to open it on.
        b.panel.invoke_edit();
        assert!(!b.data().get_shown() && b.host.on().is_none());
        // The snare's line is picked: the strip opens on it, and waits for its file to be read.
        b.pick(1);
        b.panel.invoke_edit();
        assert!(b.data().get_shown() && !b.data().get_ready());
        assert_eq!(b.data().get_name(), "snare");
        assert_eq!(b.host.on(), Some((11, b.library.list.find(11))));
        // Read: ready to edit, with nothing of the Slices' yet (they come with the plugin's next step).
        b.read(None, &[]);
        assert!(b.data().get_ready() && !b.data().get_can_slice() && !b.data().get_slicing());
        assert_eq!(b.data().get_kept_while(), "while the plugin stays loaded");
        // The same button closes it.
        b.panel.invoke_edit();
        assert!(!b.data().get_shown() && b.host.on().is_none());
        // With the Galaxy in the list's place the line picked is out of sight: the strip isn't opened from there.
        b.panel.set_galaxy(true);
        b.panel.invoke_edit();
        assert!(!b.data().get_shown());
    }

    #[test]
    fn the_strip_follows_the_line_picked_and_each_sounds_edits_wait_for_it() {
        let b = Bench::new();
        b.pick(0);
        b.read(None, &[strip::REVERSE]);
        assert_eq!((b.data().get_name().as_str(), b.chips().len()), ("kick", 1));
        // Another line picked: the strip is on that sound at the window's next tick, with no edits yet.
        b.pick(2);
        assert_eq!(b.data().get_name(), "break");
        assert!(!b.data().get_ready());
        b.read(None, &[]);
        assert!(b.data().get_ready() && b.chips().is_empty());
        // Back on the kick: its edit is where it was.
        b.pick(0);
        b.read(None, &[]);
        assert_eq!((b.data().get_name().as_str(), b.chips().len()), ("kick", 1));
        // The list searched again, without the kick in it: no line is picked, and the strip stays on the kick.
        b.list(&[11, 12]);
        b.pick(-1);
        assert_eq!((b.data().get_name().as_str(), b.chips().len()), ("kick", 1));
        assert_eq!(b.host.picked().map(|picked| picked.id), Some(10));
        assert_eq!(b.host.on(), Some((10, b.library.list.find(10))));
        assert_eq!(b.panel.get_edit_row(), -1);
        // A line picked in the new list: the strip goes to it.
        b.pick(1);
        assert_eq!(b.data().get_name(), "break");
    }

    #[test]
    fn a_sound_too_long_to_edit_says_so_in_the_plugins_words() {
        let b = Bench::new();
        b.pick(3);
        b.panel.invoke_edit();
        assert!(b.data().get_shown() && !b.data().get_ready());
        let note = b.data().get_note();
        assert!(note.starts_with("It's longer than 2 minutes, too long to edit here."), "{note}");
        // The app's own words are about its card, which the plugin's list doesn't have.
        assert_ne!(note, Unread::TooLong.to_string());
    }

    #[test]
    fn the_edit_plays_through_the_track_and_shows_on_the_strip_and_on_its_line() {
        let b = Bench::new();
        b.pick(0);
        b.read(None, &[]);
        // The DAW hasn't started the track's audio: the panel says so, and nothing counts as playing.
        b.data().invoke_play();
        assert_eq!(b.panel.get_playing(), "The DAW hasn't started this track's audio yet");
        assert!(!b.data().get_playing() && b.heard.get().is_none() && b.playing.get().is_none());
        // Started: Play hands the edit to the player, and it's a play like any other in the window.
        let _audio = b.player.start(RATE);
        b.data().invoke_play();
        let n = b.playing.get().expect("the edit plays");
        assert_eq!(b.heard.get(), Some((Heard::Edit(10), true)));
        assert_eq!(b.panel.get_playing(), "Playing kick.wav");
        assert!(b.data().get_playing() && b.host.plays() && b.host.picked_plays());
        // The strip is on the whole file: its line looks as usual, with the play head where the play is.
        b.host.tick(&b.panel);
        assert_eq!(b.panel.get_edit_row(), 0);
        assert_eq!((b.panel.get_edit_from(), b.panel.get_edit_to()), (0.0, 0.0));
        assert_eq!(b.host.line_head(&b.panel), Some((0, 0.0, 0.0)));
        // Play again stops it.
        b.data().invoke_play();
        assert!(!b.data().get_playing() && b.heard.get().is_none() && b.playing.get().is_none());
        assert_eq!(b.panel.get_playing(), "");
        // Stopped from anywhere else in the window (Esc, a click on its line): the strip shows it at the next tick.
        b.data().invoke_play();
        assert!(b.data().get_playing() && b.playing.get().is_some_and(|next| next > n));
        b.heard.set(None);
        b.playing.set(None);
        b.host.tick(&b.panel);
        assert!(!b.data().get_playing() && !b.host.plays());
        assert_eq!(b.host.line_head(&b.panel), None);
    }

    #[test]
    fn the_part_picked_on_the_strip_shows_on_its_sounds_line() {
        let b = Bench::new();
        b.pick(1);
        // From a quarter to a half of the snare.
        b.read(Some((0.25, 0.5)), &[]);
        b.host.tick(&b.panel);
        assert_eq!(b.panel.get_edit_row(), 1);
        assert!((b.panel.get_edit_from() - 0.25).abs() < 1e-3 && (b.panel.get_edit_to() - 0.5).abs() < 1e-3);
        // Its play starts at the part's start, and that's where the line's play head is.
        let _audio = b.player.start(RATE);
        b.data().invoke_play();
        b.host.tick(&b.panel);
        let (row, at, from) = b.host.line_head(&b.panel).expect("the part plays");
        assert!(row == 1 && (at - 0.25).abs() < 1e-3 && (from - 0.25).abs() < 1e-3, "{row} {at} {from}");
        // The strip closed: its line is as any other.
        b.panel.invoke_edit();
        b.host.tick(&b.panel);
        assert_eq!(b.panel.get_edit_row(), -1);
        assert_eq!((b.panel.get_edit_from(), b.panel.get_edit_to()), (0.0, 0.0));
    }

    #[test]
    fn the_line_is_lit_from_where_the_play_started_and_a_reversed_part_runs_back_from_there() {
        let b = Bench::new();
        let mut audio = b.player.start(RATE);
        b.pick(1);
        // The snare reversed, and a part of the edit picked, from a quarter to a half of it: that came from between a
        // half and three quarters of the file.
        b.read(Some((0.25, 0.5)), &[strip::REVERSE]);
        b.host.tick(&b.panel);
        assert!((b.panel.get_edit_from() - 0.5).abs() < 1e-3 && (b.panel.get_edit_to() - 0.75).abs() < 1e-3);
        b.data().invoke_play();
        b.host.tick(&b.panel);
        let (_, at, from) = b.host.line_head(&b.panel).expect("the part plays");
        assert!((at - 0.75).abs() < 1e-3 && (from - 0.75).abs() < 1e-3, "{at} {from}");
        // A tenth of a second on (a twentieth of the file): the play head has gone left, from where it started.
        let frames = RATE as usize / 10;
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        audio.fill(&mut l, &mut r);
        b.host.tick(&b.panel);
        let (_, at, from) = b.host.line_head(&b.panel).expect("it plays on");
        assert!((at - 0.7).abs() < 1e-3 && (from - 0.75).abs() < 1e-3, "{at} {from}");
    }

    #[test]
    fn a_line_picked_while_the_edit_plays_is_left_to_play() {
        let b = Bench::new();
        let mut audio = b.player.start(RATE);
        b.pick(0);
        b.read(None, &[]);
        b.data().invoke_play();
        let n = b.playing.get().expect("the edit plays");
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        audio.fill(&mut l, &mut r);
        assert!(b.host.plays());
        // The snare's line is clicked: the window has its file read, and it's what's heard from now on (the edit
        // sounds on until the file is there, as one line does under the next).
        b.heard.set(Some((Heard::Line(11), false)));
        b.pick(1);
        // The strip goes to the snare and stops nothing: the snare's read isn't called off, and the edit sounds on.
        assert_eq!(b.host.on().map(|(id, _)| id), Some(11));
        assert_eq!(b.heard.get(), Some((Heard::Line(11), false)));
        audio.fill(&mut l, &mut r);
        assert!(b.player.still_playing(n) && l.iter().all(|s| *s != 0.0), "nothing was told to stop");
        // For the strip its own play is over.
        assert!(!b.host.plays() && !b.data().get_playing());
    }

    #[test]
    fn an_edit_swapped_in_as_the_part_plays_still_stops_at_the_parts_end() {
        let b = Bench::new();
        let mut audio = b.player.start(RATE);
        b.pick(0);
        // The first half second of the kick's two.
        b.read(Some((0.0, 0.25)), &[]);
        b.data().invoke_play();
        let n = b.playing.get().expect("the part plays");
        let tenth = RATE as usize / 10;
        let (mut l, mut r) = (vec![0.0; tenth], vec![0.0; tenth]);
        audio.fill(&mut l, &mut r);
        // Turned down as it plays: the edit made again takes over where the play is...
        assert_eq!(strip::run_action(&*b.host, strip::QUIETER), Some(true));
        let next = b.playing.get().expect("it plays on");
        assert!(next > n && b.host.plays());
        // ...and ends at the part's end by itself, to the frame, with no tick of the window's: four tenths more.
        let (mut l, mut r) = (vec![0.0; RATE as usize], vec![0.0; RATE as usize]);
        audio.fill(&mut l, &mut r);
        assert_eq!(l.iter().filter(|s| **s != 0.0).count(), 4 * tenth);
        assert!(!b.player.still_playing(next));
    }

    #[test]
    fn a_play_that_cannot_be_handed_over_leaves_what_plays_as_it_is() {
        let b = Bench::new();
        let _audio = b.player.start(RATE);
        b.pick(0);
        b.read(None, &[]);
        // The audio thread is behind, with no room for another ask, and the break's own file is what plays.
        let mut n = 0;
        while let Played::Playing(_, number) = b.player.play_shared(Arc::new(sound()), "break.wav".into(), 0.0, None) {
            n = number;
        }
        b.heard.set(Some((Heard::Line(12), true)));
        b.playing.set(Some(n));
        // Play on the strip: the panel says why not, and the break is still what's heard.
        b.data().invoke_play();
        assert_eq!(b.panel.get_playing(), "Busy: try again");
        assert_eq!((b.heard.get(), b.playing.get()), (Some((Heard::Line(12), true)), Some(n)));
        assert!(!b.data().get_playing());
        // A line still being read was called off by the try: it's heard no more.
        b.heard.set(Some((Heard::Line(12), false)));
        b.data().invoke_play();
        assert_eq!(b.heard.get(), None);
    }

    #[test]
    fn with_the_galaxy_open_the_strips_arrow_keys_are_still_its_own() {
        let b = Bench::new();
        let steps = Rc::new(Cell::new(0));
        let stepped = Rc::clone(&steps);
        b.panel.on_sky_arrow(move |_| stepped.set(stepped.get() + 1));
        b.pick(0);
        b.read(None, &[]);
        // The Galaxy opens and takes the keys: an arrow goes from star to star.
        b.panel.set_galaxy(true);
        draw(&b.window);
        b.key(Key::Named(NamedKey::ArrowUp), Code::ArrowUp, Modifiers::empty(), false);
        assert_eq!((steps.get(), b.chips().len()), (1, 0));
        // With Alt held it's the strip's key (louder), as the shortcut list says.
        b.key(Key::Named(NamedKey::Alt), Code::AltLeft, Modifiers::ALT, false);
        b.key(Key::Named(NamedKey::ArrowUp), Code::ArrowUp, Modifiers::ALT, false);
        assert_eq!((steps.get(), b.chips().len()), (1, 1));
    }

    #[test]
    fn the_strips_keys_are_its_own_while_no_box_is_typed_in() {
        let b = Bench::new();
        let (none, alt) = (Modifiers::empty(), Modifiers::ALT);
        let ctrl = if cfg!(target_os = "macos") { Modifiers::META } else { Modifiers::CONTROL };
        draw(&b.window);
        // Closed: E opens it on the line picked, and the tools' keys aren't taken (Alt+Down goes on down the list).
        b.pick(0);
        b.key(Key::Named(NamedKey::ArrowDown), Code::ArrowDown, alt, false);
        assert_eq!(b.panel.get_picked(), 1);
        b.pick(0);
        b.letter("e", Code::KeyE, none);
        assert!(b.data().get_shown());
        b.read(None, &[]);
        // Open: Alt+R reverses, Ctrl+Z takes it back, and Alt+Down turns it down instead of moving through the list.
        b.letter("r", Code::KeyR, alt);
        assert_eq!(b.chips().len(), 1);
        b.letter("z", Code::KeyZ, ctrl);
        assert!(b.chips().is_empty());
        b.key(Key::Named(NamedKey::ArrowDown), Code::ArrowDown, alt, false);
        assert_eq!((b.panel.get_picked(), b.chips().len()), (0, 1));
        // With nothing to redo, Ctrl+Shift+Z is still the strip's. The Slices' key waits for the plugin's next step.
        b.letter("Z", Code::KeyZ, ctrl | Modifiers::SHIFT);
        b.letter("s", Code::KeyS, alt);
        assert!(!b.data().get_slicing() && b.chips().len() == 1);
        // Space plays the edit and stops it.
        let _audio = b.player.start(RATE);
        b.letter(" ", Code::Space, none);
        assert!(b.data().get_playing());
        b.letter(" ", Code::Space, none);
        assert!(!b.data().get_playing());
        // A plain arrow still moves through the list, and the strip goes with the line.
        b.key(Key::Named(NamedKey::ArrowDown), Code::ArrowDown, none, false);
        b.host.tick(&b.panel);
        assert_eq!((b.panel.get_picked(), b.data().get_name().as_str()), (1, "snare"));
        // On another tab the keys aren't the strip's.
        b.panel.set_view(1);
        b.letter("e", Code::KeyE, none);
        assert!(b.data().get_shown());
        b.panel.set_view(0);
        // E closes it.
        b.letter("e", Code::KeyE, none);
        assert!(!b.data().get_shown());
    }

    #[test]
    fn esc_lets_the_part_go_then_closes_the_strip_once_nothing_is_left_to_stop() {
        let b = Bench::new();
        let esc = |sounding: bool| b.key(Key::Named(NamedKey::Escape), Code::Escape, Modifiers::empty(), sounding);
        b.pick(0);
        b.read(Some((0.25, 0.5)), &[]);
        assert!(b.data().get_has_part());
        // It had a sound to stop: that's all it does.
        esc(true);
        assert!(b.data().get_has_part() && b.data().get_shown());
        // Then the part picked goes, then the strip.
        esc(false);
        assert!(!b.data().get_has_part() && b.data().get_shown());
        esc(false);
        assert!(!b.data().get_shown());
    }

    #[test]
    fn a_box_typed_in_keeps_its_letters_until_the_strip_is_pressed() {
        let b = Bench::new();
        let none = Modifiers::empty();
        // The tree folded to its tab: its own search box isn't there, so Tab puts the keys in Search sounds.
        b.panel.set_tree_folded(true);
        let searched = Rc::new(RefCell::new(Vec::new()));
        let typed = Rc::clone(&searched);
        b.panel.on_search(move |words| typed.borrow_mut().push(words.to_string()));
        b.pick(0);
        b.read(None, &[]);
        draw(&b.window);
        b.key(Key::Named(NamedKey::Tab), Code::Tab, none, false);
        // Typed in the search box, an e is a letter: the strip stays open.
        b.letter("e", Code::KeyE, none);
        assert_eq!(*searched.borrow(), ["e"]);
        assert!(b.data().get_shown());
        // A press on the strip gives the lists the keys (the window asks the panel for that): E is the strip's again.
        b.panel.invoke_keys_to_lists();
        b.letter("e", Code::KeyE, none);
        assert_eq!(searched.borrow().len(), 1);
        assert!(!b.data().get_shown());
    }

    #[test]
    fn the_strip_lies_along_the_bottom_and_its_screen_is_drawn() {
        let b = Bench::new();
        b.pick(0);
        // Closed, it takes no room.
        draw(&b.window);
        assert_eq!(b.panel.get_edit_height(), 0.0);
        b.read(None, &[strip::REVERSE]);
        // The screen says what room it has once it's laid out, and the strip's picture is drawn for it.
        draw(&b.window);
        slint::platform::update_timers_and_animations();
        draw(&b.window);
        let usual = mo_app::widths::find("plugin.edit").expect("the strip's part");
        assert_eq!(b.panel.get_edit_height(), usual.usual);
        assert_eq!(b.panel.get_edit_top(), 800.0 - usual.usual);
        let picture = b.data().get_wave_picture().size();
        assert!(picture.width > 600 && picture.height > 60, "{picture:?}");
        // On another tab it isn't there.
        b.panel.set_view(1);
        assert_eq!(b.panel.get_edit_height(), 0.0);
        b.panel.set_view(0);
        // A window as short as it goes: the strip gives way down to its least.
        b.window.set_size(slint::PhysicalSize::new(1280, crate::window::LEAST_HEIGHT as u32));
        assert_eq!(b.panel.get_edit_height(), usual.least);
        // Closed, its picture goes with it.
        b.panel.invoke_edit();
        let picture = b.data().get_wave_picture().size();
        assert_eq!((picture.width, picture.height), (0, 0));
    }

    #[test]
    fn a_save_says_where_the_copy_is_and_the_handle_drags_only_where_files_can_be_carried() {
        let b = Bench::new();
        b.pick(0);
        b.read(None, &[strip::REVERSE]);
        assert!(b.data().get_can_save() && !b.data().get_saved());
        assert_eq!(b.data().get_can_drag(), crate::drag::CAN);
        // Save (its key or its button): the strip shows the copy saved. In the tests nothing is written.
        b.data().invoke_tool(strip::SAVE.into());
        assert!(b.data().get_saved() && !b.data().get_save_failed());
        // Dragging does nothing here (no file is made, nothing is carried), and never panics.
        b.data().invoke_drag_handle();
        b.data().invoke_drag_let_go();
        b.data().invoke_show_saved();
    }

    #[test]
    fn the_strip_waits_for_the_next_window_with_each_sounds_edits() {
        let b = Bench::new();
        b.pick(0);
        b.read(None, &[strip::REVERSE, strip::NORMALISE]);
        assert_eq!(b.chips().len(), 2);
        // The window closes: the strip is put away, closed, for the next one.
        let kept = b.host.put_away().expect("nothing is in the middle of it");
        assert!(strip::open_on(&kept).is_none());
        // The next window's strip, on the same panel here: closed, and with the kick's edits once it's opened on it.
        let next = OnPanel::new(
            &b.panel,
            kept,
            Rc::clone(&b.lines),
            Arc::clone(&b.player),
            Arc::default(),
            Rc::clone(&b.heard),
            Rc::clone(&b.playing),
        );
        next.fill_in(&b.panel);
        strip::show(&*next);
        assert!(!b.data().get_shown());
        b.panel.invoke_edit();
        strip::for_screens(&*next, sound(), None, &[]);
        assert!(b.data().get_shown());
        assert_eq!(b.chips().len(), 2);
    }

    #[test]
    fn an_edit_left_playing_by_the_last_window_stops_at_a_press_of_play() {
        let b = Bench::new();
        let _audio = b.player.start(RATE);
        // As the last window left it: an edit of the kick plays on, as the player numbers it.
        let Played::Playing(_, n) = b.player.play_shared(Arc::new(sound()), "kick.wav".into(), 0.0, None) else {
            panic!("not handed over")
        };
        b.heard.set(Some((Heard::Edit(10), true)));
        b.playing.set(Some(n));
        b.pick(0);
        b.read(None, &[]);
        // It isn't this strip's play, but it is the picked sound's: Play stops it.
        assert!(!b.host.plays() && b.host.picked_plays());
        b.data().invoke_play();
        assert!(b.heard.get().is_none() && b.playing.get().is_none());
    }
}
