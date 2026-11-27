//! The Edit strip along the bottom of Sounds, on the app's side (the 1.1 plan's light sample edits, mock-ups 8 and 9,
//! Solomon's "Build as shown" of 27 Nov 2026). The strip itself has no screen and is the plugin's too
//! ([`mo_app::edit_strip`]): the sound open in it, its edits and their undo, its Slices, what plays, and everything it
//! shows. This is what the Sounds page gives it ([`Host`]): the sound picked in the list, the page's player, a thread
//! of its own to read and make on (newest wins, rule 12), the cards that say how a save or a send went, and its place
//! on the screen ([`SoundEdit`]).

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use mo_app::background::{self, Newest};
pub(super) use mo_app::edit_strip::EDIT;
use mo_app::edit_strip::slices::{self, Slices, To};
use mo_app::edit_strip::{self as strip, Carried, Host, Picked, Saving, Showing, Strip, Ticked, Work};
use mo_app::shortcuts::Shortcuts;
use mo_app::{edit_copy, log, to_bitwig};
use mo_audio::AudioData;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{FEED, HERE, PAGE, did, drag_out, kit, part, play_in_memory, player};
use crate::{AppWindow, EditChip, EditMark, SliceCut, SoundEdit, SoundsData};

thread_local! {
    /// The strip: one to the window.
    static STRIP: Rc<RefCell<Strip>> = Rc::default();
    /// The thread the strip's reads and makes are done on, started at the first of them.
    static WORKER: RefCell<Option<Newest<Job>>> = const { RefCell::new(None) };
}

/// A read or a make, for the strip's thread.
struct Job {
    ask: u64,
    id: i64,
    work: Work,
    window: slint::Weak<AppWindow>,
}

/// The Sounds page as the strip's host: the window, and the strip it keeps.
struct OnPage<'a> {
    app: &'a AppWindow,
    strip: Rc<RefCell<Strip>>,
}

/// The strip's host on `app`'s Sounds page.
fn host(app: &AppWindow) -> OnPage<'_> {
    OnPage { app, strip: STRIP.with(Rc::clone) }
}

impl Host for OnPage<'_> {
    fn strip(&self) -> &RefCell<Strip> {
        &self.strip
    }

    fn picked(&self) -> Option<Picked> {
        let colour = self.app.global::<SoundsData>().get_picked().colour;
        PAGE.with(|p| {
            let p = p.borrow();
            let s = p.picked.and_then(|at| p.list.sound(at))?;
            Some(Picked {
                id: s.id,
                path: PathBuf::from(&s.path),
                seconds: s.analysis.as_ref().map_or(0.0, |a| a.seconds),
                name: s.name().to_owned(),
                colour,
                bpm: s.analysis.as_ref().and_then(|a| a.bpm).filter(|b| *b > 0.0),
            })
        })
    }

    fn card_part(&self) -> Option<(f64, f64)> {
        part::picked_seconds()
    }

    fn show(&self, showing: Showing) {
        show(&self.app.global::<SoundEdit>(), showing);
    }

    /// On the strip's thread (started at the first ask), or here for the screenshots, which have no event loop to send
    /// the answer back with.
    fn work(&self, ask: u64, id: i64, work: Work) {
        if HERE.with(|h| *h.borrow()) {
            let answer = strip::answer(work);
            strip::arrived(self, ask, id, answer);
            return;
        }
        let job = Job { ask, id, work, window: self.app.as_weak() };
        WORKER.with(|w| {
            let mut w = w.borrow_mut();
            if w.is_none() {
                match Newest::start("Edit sound", do_job) {
                    Ok(worker) => *w = Some(worker),
                    Err(e) => {
                        log::error("Sounds", &format!("Couldn't start the Edit strip's thread: {e}"));
                        return;
                    }
                }
            }
            if let Some(worker) = w.as_ref() {
                worker.ask(job);
            }
        });
    }

    fn play(&self, audio: Arc<AudioData>, from: f64, till: Option<f64>) {
        if let Some(at) = PAGE.with(|p| p.borrow().picked) {
            play_in_memory(self.app, at, audio, from, till);
        }
    }

    /// Swapped in by the player itself, so nothing is heard twice or missed: only while it still has the strip's play
    /// (by the play's number).
    fn swap(&self, audio: Arc<AudioData>, place: &dyn Fn(f64) -> f64) -> bool {
        let key = PAGE.with(|p| p.borrow().play).to_string();
        player()
            .play_from(audio, key.clone(), |now| match now {
                Some((k, at)) if k == key => Ok(place(at)),
                _ => Err(()),
            })
            .is_ok()
    }

    fn plays(&self) -> bool {
        PAGE.with(|p| {
            let p = p.borrow();
            p.play_edit && p.playing.is_some()
        })
    }

    fn picked_plays(&self) -> bool {
        PAGE.with(|p| {
            let p = p.borrow();
            p.playing.is_some_and(|(at, _)| Some(at) == p.picked)
        })
    }

    fn stop(&self) {
        super::stop(self.app);
    }

    fn played(&self) {
        if let Some(at) = PAGE.with(|p| p.borrow().picked) {
            did::played(self.app, at);
        }
    }

    fn carry(&self, what: Carried) {
        if let Some(at) = PAGE.with(|p| p.borrow().picked) {
            drag_out::carry_edit(self.app, at, what);
        }
    }

    fn let_go(&self) {
        crate::drag::let_go(self.app);
    }

    /// On a thread of its own; a card says how it went.
    fn save(&self, edit: Saving) -> bool {
        let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return false };
        let Saving { source, original, changed, edits, name } = edit;
        let job = feed.start(&format!("Saving the edit of {name}"), "");
        // The screenshots: the card goes up, and nothing is written.
        if HERE.with(|h| *h.borrow()) {
            return true;
        }
        let made = background::spawn("Save edit", move || {
            background::lower_priority();
            let saved = mo_catch::catch_crash(AssertUnwindSafe(|| {
                edit_copy::edit_to_keep(&source, &original, changed, &edits)
            }))
            .unwrap_or_else(|_| Err("Something went wrong: the log says what.".to_owned()));
            match saved {
                Ok(copy) => {
                    let file = copy.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    log::info("Sounds", &format!("Saved the edit of {name} as {}", copy.display()));
                    job.done_as(&format!("Saved the edit of {name}"), &format!("{file}, in {}", edit_copy::FOLDER));
                }
                Err(why) => {
                    log::warning("Sounds", &format!("Couldn't save the edit of {name}: {why}"));
                    job.failed(&why);
                }
            }
        });
        if let Err(e) = made {
            log::error("Sounds", &format!("Couldn't start saving the edit: {e}"));
        }
        true
    }

    /// On a thread of its own; a card says how it went, and Send comes back once it's done.
    fn send(&self, to: To, slices: Slices) -> bool {
        let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return false };
        // The screenshots: nothing is written and nothing is sent.
        if HERE.with(|h| *h.borrow()) {
            return false;
        }
        let Slices { id, source, changed, audio, what, spans, name } = slices;
        let count = to_bitwig::slices_text(spans.len());
        let job = match to {
            To::Bitwig => to_bitwig::slices_card(&feed, &name, spans.len()),
            To::Folder => feed.start(&format!("Saving {count} of {name}"), ""),
        };
        let setting = PAGE.with(|p| p.borrow().kit.drum_machine.clone());
        let weak = self.app.as_weak();
        let started = background::spawn("Slices", move || {
            background::lower_priority();
            let saved = mo_catch::catch_crash(AssertUnwindSafe(|| {
                edit_copy::slices_to_keep(&source, &audio, changed, &what, &spans)
            }))
            .unwrap_or_else(|_| Err("Something went wrong: the log says what.".to_owned()));
            let went_in = match (saved, to) {
                (Err(why), _) => {
                    log::warning("Sounds", &format!("Couldn't make the slices of {name}: {why}"));
                    job.failed(&why);
                    false
                }
                (Ok(files), To::Folder) => {
                    let folder = files.first().and_then(|f| f.parent()).map(std::path::Path::to_path_buf);
                    let named = folder
                        .as_ref()
                        .and_then(|f| f.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    log::info("Sounds", &format!("Saved {count} of {name} in {named}"));
                    job.done_as(&format!("Saved {count} of {name}"), &format!("{named}, in {}", edit_copy::FOLDER));
                    if let Some(folder) = folder {
                        crate::files::open_folder(&feed, &folder.to_string_lossy());
                    }
                    false
                }
                // A panic in it is logged as a crash and fails the card as it goes; Send still comes back.
                (Ok(files), To::Bitwig) => mo_catch::catch_crash(AssertUnwindSafe(|| {
                    to_bitwig::slices_made(&job);
                    let pads = to_bitwig::slice_pads(&files);
                    let bitwig = to_bitwig::Bitwig::default();
                    to_bitwig::send_slices(job, &bitwig, &kit::KITS_TO_BITWIG, &name, &pads, setting.as_deref())
                }))
                .unwrap_or(false),
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = weak.upgrade() {
                    slices::sent(&host(&app));
                    // Its slices went into Bitwig: that counts towards how liked the sound is, as a send of it does.
                    if went_in {
                        did::sent(&app, &[id]);
                    }
                }
            });
        });
        // Its card was taken into the thread, so it fails by itself.
        if let Err(e) = started {
            log::error("Sounds", &format!("Couldn't start sending the slices: {e}"));
            return false;
        }
        true
    }
}

/// One job on the strip's thread.
fn do_job(job: Job) {
    let Job { ask, id, work, window } = job;
    background::lower_priority();
    let answer = strip::answer(work);
    let _ = window.upgrade_in_event_loop(move |app| strip::arrived(&host(&app), ask, id, answer));
}

pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<SoundEdit>();
    let weak = app.as_weak();
    data.on_tool(move |id| {
        if let Some(app) = weak.upgrade() {
            run_action(&app, &id);
        }
    });
    let weak = app.as_weak();
    data.on_gain_stepped(move |by| {
        if let Some(app) = weak.upgrade() {
            strip::gain_stepped(&host(&app), by);
        }
    });
    let weak = app.as_weak();
    data.on_gain_reset(move || {
        if let Some(app) = weak.upgrade() {
            strip::gain_reset(&host(&app));
        }
    });
    let weak = app.as_weak();
    data.on_fade_moving(move |fade_in, at| {
        if let Some(app) = weak.upgrade() {
            strip::fade_moving(&host(&app), fade_in, f64::from(at));
        }
    });
    let weak = app.as_weak();
    data.on_fade_let_go(move |fade_in| {
        if let Some(app) = weak.upgrade() {
            strip::fade_let_go(&host(&app), fade_in);
        }
    });
    data.on_snap(|at, within| STRIP.with(|s| strip::snap(&s.borrow(), at, within)));
    data.on_wave_pressed(|| STRIP.with(|s| strip::wave_pressed(&mut s.borrow_mut())));
    let weak = app.as_weak();
    data.on_picked(move |from, to| {
        if let Some(app) = weak.upgrade() {
            strip::part_picked(&host(&app), f64::from(from), f64::from(to));
        }
    });
    let weak = app.as_weak();
    data.on_seek(move |at| {
        if let Some(app) = weak.upgrade() {
            strip::seek(&host(&app), f64::from(at));
        }
    });
    let weak = app.as_weak();
    data.on_stop(move || {
        if let Some(app) = weak.upgrade() {
            strip::stop(&host(&app));
        }
    });
    let weak = app.as_weak();
    data.on_drag_edit(move || {
        if let Some(app) = weak.upgrade() {
            strip::drag(&host(&app), false);
        }
    });
    let weak = app.as_weak();
    data.on_drag_part(move || {
        if let Some(app) = weak.upgrade() {
            strip::drag(&host(&app), true);
        }
    });
    let weak = app.as_weak();
    data.on_drag_let_go(move || {
        if let Some(app) = weak.upgrade() {
            crate::drag::let_go(&app);
        }
    });
    let weak = app.as_weak();
    data.on_hear_original(move |on| {
        if let Some(app) = weak.upgrade() {
            strip::hear_original(&host(&app), on);
        }
    });
    let weak = app.as_weak();
    data.on_zoom(move |on| {
        if let Some(app) = weak.upgrade() {
            strip::zoom(&host(&app), on);
        }
    });
    let weak = app.as_weak();
    data.on_close(move || {
        if let Some(app) = weak.upgrade() {
            close(&app);
        }
    });
    // The Slices.
    let weak = app.as_weak();
    data.on_cut_at_picked(move |choice| {
        if let Some(app) = weak.upgrade() {
            slices::cut_at_picked(&host(&app), &choice);
        }
    });
    let weak = app.as_weak();
    data.on_sensitivity_moved(move |to| {
        if let Some(app) = weak.upgrade() {
            slices::sensitivity_moved(&host(&app), to);
        }
    });
    let weak = app.as_weak();
    data.on_sensitivity_released(move |_| {
        if let Some(app) = weak.upgrade() {
            slices::end_live(&host(&app));
        }
    });
    let weak = app.as_weak();
    data.on_each_picked(move |choice| {
        if let Some(app) = weak.upgrade() {
            slices::each_picked(&host(&app), &choice);
        }
    });
    data.on_slice_down(|at, within, shift| {
        STRIP.with(|s| slices::down(&mut s.borrow_mut(), f64::from(at), f64::from(within), shift));
    });
    let weak = app.as_weak();
    data.on_slice_move(move |at, shift, far| {
        if let Some(app) = weak.upgrade() {
            slices::moved(&host(&app), f64::from(at), shift, far);
        }
    });
    let weak = app.as_weak();
    data.on_slice_up(move |cut_short| {
        if let Some(app) = weak.upgrade() {
            slices::up(&host(&app), cut_short);
        }
    });
    let weak = app.as_weak();
    data.on_slice_double(move || {
        if let Some(app) = weak.upgrade() {
            slices::double(&host(&app));
        }
    });
    data.on_slice_near(|at, within, _| STRIP.with(|s| slices::near(&s.borrow(), f64::from(at), f64::from(within))));
}

/// The strip's tips, with each tool's key from the shortcut list (rule 15).
pub(super) fn show_keys(app: &AppWindow, keys: &Shortcuts) {
    let data = app.global::<SoundEdit>();
    let key = |id: &str| SharedString::from(keys.key_of(id));
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
    STRIP.with(|s| strip::set_original_key(&mut s.borrow_mut(), keys.key_of(strip::ORIGINAL)));
}

/// Runs one of the strip's actions (its key, its button or its line on the card's menu), if it's one of them: whether
/// it did anything (`None` for any other action).
pub(super) fn run_action(app: &AppWindow, id: &str) -> Option<bool> {
    strip::run_action(&host(app), id)
}

/// Opens the strip on the picked sound (the right-click menu's Edit: the click picked it), or keeps it open on it.
/// Whether there was one to open it on.
pub(super) fn open(app: &AppWindow) -> bool {
    strip::open(&host(app))
}

/// Closes the strip. Its sound's edits are kept; what was read is let go.
pub(super) fn close(app: &AppWindow) {
    strip::close(&host(app));
}

/// Esc's steps on the page, before the card's: the slice picked goes, then the Slices; the part picked on the strip
/// goes, then the strip. Whether it did one.
pub(super) fn escape(app: &AppWindow) -> bool {
    strip::escape(&host(app))
}

/// Another sound was picked: the strip, if it's open, edits that one now (the last one's edits are kept).
pub(super) fn sound_picked(app: &AppWindow) {
    strip::sound_picked(&host(app));
}

/// The part picked on the card changed: the strip takes the new part of a sound too long to edit whole.
pub(super) fn card_part_changed(app: &AppWindow) {
    strip::card_part_changed(&host(app));
}

/// Play on the page with the strip open on the picked sound: the edit plays (the part picked, or the slice, with
/// one), or stops. Whether it was the strip's to do.
pub(super) fn toggle_play(app: &AppWindow) -> bool {
    strip::toggle_play(&host(app))
}

/// A key let go: O stops the original.
pub(super) fn key_up(app: &AppWindow, text: &str) {
    strip::key_up(&host(app), &crate::key_name(text, false, false, false, false));
}

/// The page's ticker, while the strip's play plays: `at` is where the player is (seconds into what it plays; none
/// until it has the play). The strip's waveform shows where the edit has got to. Whether it goes on: a play of the part
/// picked stops at its end.
pub(super) fn tick(app: &AppWindow, at: Option<f64>) -> bool {
    let till = PAGE.with(|p| p.borrow().play_part.map(|(_, end)| end));
    match STRIP.with(|s| strip::tick(&s.borrow(), at, till)) {
        Ticked::Waiting => true,
        Ticked::At(place) => {
            app.global::<SoundEdit>().set_played(place);
            true
        }
        Ticked::Ended => false,
    }
}

/// The strip's play started (`on`) or stopped.
pub(super) fn show_playing(app: &AppWindow, on: bool) {
    let data = app.global::<SoundEdit>();
    data.set_playing(on);
    if !on {
        data.set_played(0.0);
    }
}

/// `items` as a list for the screen.
fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    if items.is_empty() { ModelRc::default() } else { ModelRc::new(VecModel::from(items)) }
}

/// `words` as a list for the screen.
fn words(words: Vec<String>) -> ModelRc<SharedString> {
    model(words.into_iter().map(SharedString::from).collect())
}

/// Copies the strip as it is now onto the screen: all of it, each time.
fn show(data: &SoundEdit, showing: Showing) {
    let Showing {
        open,
        folder_text,
        name,
        colour,
        picked_text,
        note,
        ready,
        shape,
        points,
        view_seconds,
        view_from,
        zoomed,
        hit_marks,
        has_part,
        pick_from,
        pick_to,
        fade_in,
        fade_out,
        fade_in_shown,
        fade_out_shown,
        fade_in_curve,
        fade_out_curve,
        ruler,
        chips,
        can_undo,
        can_redo,
        has_edits,
        original,
        gain_text,
        gain_changed,
        drag_text,
        slices,
    } = showing;
    data.set_shown(open);
    data.set_folder_text(folder_text.into());
    data.set_name(name.into());
    data.set_colour(colour);
    data.set_picked_text(picked_text.into());
    data.set_note(note.into());
    data.set_ready(ready);
    data.set_shape(shape.into());
    data.set_points(i32::try_from(points).unwrap_or(i32::MAX));
    data.set_view_seconds(view_seconds);
    data.set_view_from(view_from);
    data.set_zoomed(zoomed);
    data.set_hit_marks(hit_marks.into());
    data.set_has_part(has_part);
    data.set_pick_from(pick_from);
    data.set_pick_to(pick_to);
    data.set_fade_in(fade_in);
    data.set_fade_out(fade_out);
    data.set_fade_in_shown(fade_in_shown);
    data.set_fade_out_shown(fade_out_shown);
    data.set_fade_in_curve(fade_in_curve.into());
    data.set_fade_out_curve(fade_out_curve.into());
    data.set_ruler(model(ruler.into_iter().map(|m| EditMark { at: m.at, text: m.text.into(), bar: m.bar }).collect()));
    data.set_chips(model(chips.into_iter().map(|c| EditChip { text: c.text.into(), last: c.last }).collect()));
    data.set_can_undo(can_undo);
    data.set_can_redo(can_redo);
    data.set_has_edits(has_edits);
    data.set_original(original);
    data.set_gain_text(gain_text.into());
    data.set_gain_changed(gain_changed);
    data.set_drag_text(drag_text.into());
    let slices::Showing {
        on,
        cuts,
        slice_from,
        slice_to,
        first_cut,
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
        cuts_mark,
    } = slices;
    data.set_slicing(on);
    data.set_cuts(model(
        cuts.into_iter()
            .map(|c| SliceCut { at: c.at, number: i32::try_from(c.number).unwrap_or(i32::MAX), room: c.room })
            .collect(),
    ));
    data.set_slice_from(slice_from);
    data.set_slice_to(slice_to);
    data.set_first_cut(first_cut);
    data.set_cut_at(cut_at.into());
    data.set_cut_at_choices(words(cut_at_choices));
    data.set_cut_at_hits(cut_at_hits);
    data.set_sensitivity(sensitivity);
    data.set_each(each.into());
    data.set_each_choices(words(each_choices));
    data.set_send_text(send_text.into());
    data.set_can_send(can_send);
    data.set_pads(words(pads));
    data.set_to_folder_text(to_folder_text.into());
    data.set_saved_in(saved_in.into());
    data.set_cuts_mark(cuts_mark);
}

/// For the screenshots: opens the strip on the picked sound, as if `audio` were what its file held (the made-up sounds
/// have none), makes `tools`' edits (by their ids in the shortcut list) and picks a part of it (from and to, 0 to 1 of
/// it), all at once.
pub fn for_screens(app: &AppWindow, audio: AudioData, part: Option<(f64, f64)>, tools: &[&str]) {
    strip::for_screens(&host(app), audio, part, tools);
}

/// For the screenshots: whether the strip is open, ready, and how many edits it has.
pub fn state_for_screens() -> (bool, bool, usize) {
    STRIP.with(|s| strip::state_for_screens(&s.borrow()))
}

/// For the screenshots: how many cuts the sound's Slices have (none with them off), and the slice picked.
pub fn slices_state_for_screens() -> (usize, Option<usize>) {
    STRIP.with(|s| slices::state_for_screens(&s.borrow()))
}
