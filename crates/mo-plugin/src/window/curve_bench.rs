//! The curve editor in the panel's Curves tab: the app's own (`mo_app::curve_card`, shown by the app's `CurveBench`),
//! so a curve is edited here as it is there (Solomon's plan, step 8: "the same editor in the plugin's Curves tab").
//! Its picture is worked out and drawn on a background thread, newest ask first; Hear plays the tone through the
//! track, at the song's speed when the DAW says it.

use super::{Carried, Slots};
use crate::audition::{MadeLoop, Played, Player};
use crate::threads::{background, lock};
use crate::{ClipTile, CurveEditorData, EditorDot, MenuItem, Panel, RulerMark, Theme};
use mo_app::background::Newest;
use mo_app::curve_card::{self, Card, Heard};
use mo_app::curve_hear::{self, Hear};
use mo_app::curve_picture::{self, Colours};
use mo_app::curve_touch::Keys;
use mo_app::presets::Offer;
use mo_app::shapes::{self, Colour};
use mo_presets::curves::Curve;
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long an answer about a tone asked for is waited for: one that a newer sound overtook is never answered.
const TONE_WAIT: Duration = Duration::from_secs(5);

/// A part action's short name on its button (the menu's own words are its tip).
fn short(offer: &Offer) -> &'static str {
    use mo_app::presets::curve_act as act;
    match offer.id {
        act::REVERSE => "Reverse",
        act::FLIP => "Flip",
        act::STEPS => "Steps",
        act::KEEP_CLIP if offer.text.starts_with("Forget") => "Forget clip",
        act::KEEP_CLIP => "Keep as clip",
        act::CUT_TIME => "Cut time out",
        act::HIGHLIGHT_ALL => "Highlight all",
        _ => "Do",
    }
}

/// A picture to work out: what the card asks for, the colours, and pixels to a point.
struct Ask {
    of: curve_card::Ask,
    colours: Colours,
    scale: f64,
}

/// A picture worked out, of which curve (as the card counts them), with the ruler's marks and where the curve ends.
struct Drawn {
    shown: u64,
    pixels: SharedPixelBuffer<Rgba8Pixel>,
    ruler: Vec<(f32, String, bool)>,
    end_x: f32,
}

impl Drawn {
    fn new(ask: &Ask) -> Drawn {
        let of = &ask.of;
        let scale = shapes::usable_scale(ask.scale);
        let scene = curve_picture::picture(&of.shape, of.highlight, of.grid, of.frame, scale, &ask.colours);
        let pixmap = scene.paint(scale);
        let (ruler, end_x) = curve_card::ruler(of.shape.length(), of.frame);
        Drawn {
            shown: of.shown,
            pixels: SharedPixelBuffer::clone_from_slice(pixmap.data(), pixmap.width(), pixmap.height()),
            ruler,
            end_x,
        }
    }
}

/// A colour of the panel's as the picture takes it.
fn colour(c: slint::Color) -> Colour {
    Colour { r: c.red(), g: c.green(), b: c.blue(), a: c.alpha() }
}

pub(super) struct Bench {
    card: RefCell<Card>,
    /// The curve open in the editor: its place in the list as read, and the curve as found.
    open: RefCell<Option<(usize, Curve)>>,
    drawer: RefCell<Option<Newest<Ask>>>,
    drawn: Arc<Mutex<Option<Drawn>>>,
    /// The part's actions as the buttons last showed them.
    offers: RefCell<Vec<Offer>>,
    player: Arc<Player>,
    /// How asking for the tone went, once that's known; and since when one is asked for and not answered yet.
    played: Arc<Mutex<Option<Played>>>,
    making: Cell<Option<Instant>>,
    /// The tone playing, as the player numbers it, and what it was made from.
    tone: Cell<Option<u64>>,
    heard: RefCell<Option<Heard>>,
}

impl Bench {
    pub(super) fn new(player: Arc<Player>) -> Rc<Bench> {
        Rc::new(Bench {
            card: RefCell::default(),
            open: RefCell::new(None),
            drawer: RefCell::new(None),
            drawn: Arc::default(),
            offers: RefCell::default(),
            player,
            played: Arc::default(),
            making: Cell::new(None),
            tone: Cell::new(None),
            heard: RefCell::new(None),
        })
    }

    /// Wires the editor's bench to the card, and starts its drawing thread.
    pub(super) fn fill_in(self: &Rc<Self>, panel: &Panel, slots: &Arc<Slots>) {
        let data = panel.global::<CurveEditorData>();
        data.set_mac(cfg!(target_os = "macos"));
        self.show_grid(panel);
        data.set_metronome(self.card.borrow().hear().click);
        // What the mouse did, and then what to show of it.
        let then = |what: fn(&mut Card, (i32, f64, f64, Keys)) -> bool| {
            let (me, weak) = (Rc::clone(self), panel.as_weak());
            move |told: (i32, f64, f64, Keys)| {
                let drawn = what(&mut me.card.borrow_mut(), told);
                if let Some(panel) = weak.upgrade() {
                    if drawn {
                        me.redraw(&panel);
                    }
                    me.show(&panel, drawn);
                }
            }
        };
        let mouse = then(|c, (kind, x, y, keys)| c.mouse(kind, x, y, keys));
        data.on_mouse(move |kind, x, y, shift, alt, ctrl| {
            mouse((kind, f64::from(x), f64::from(y), Keys { shift, alt, ctrl }));
        });
        let ruler = then(|c, (kind, x, _, keys)| c.ruler(kind, x, keys));
        data.on_ruler_mouse(move |kind, x, shift| ruler((kind, f64::from(x), 0.0, Keys { shift, ..Keys::default() })));
        let strip = then(|c, (kind, x, _, keys)| c.strip(kind, x, keys));
        data.on_strip(move |kind, x, shift, alt, ctrl| strip((kind, f64::from(x), 0.0, Keys { shift, alt, ctrl })));
        let wheel = then(|c, (along, x, by, _)| c.wheel(x, by, along != 0));
        data.on_wheel(move |x, by, along| wheel((i32::from(along), f64::from(x), f64::from(by), Keys::default())));
        let laid_out = then(|c, (_, width, height, _)| {
            c.laid_out(width, height);
            true
        });
        data.on_laid_out(move |width, height| laid_out((0, f64::from(width), f64::from(height), Keys::default())));
        let undo = then(|c, (back, ..)| c.step_back(back != 0));
        let redo = undo.clone();
        data.on_undo(move || undo((1, 0.0, 0.0, Keys::default())));
        data.on_redo(move || redo((0, 0.0, 0.0, Keys::default())));
        let (me, weak) = (Rc::clone(self), panel.as_weak());
        data.on_grid_picked(move |chip| {
            me.card.borrow_mut().pick_grid(chip);
            if let Some(panel) = weak.upgrade() {
                me.show_grid(&panel);
                me.redraw(&panel);
                me.show(&panel, true);
            }
        });
        let (me, weak) = (Rc::clone(self), panel.as_weak());
        data.on_menu_chosen(move |id| {
            me.card.borrow_mut().act(&id);
            if let Some(panel) = weak.upgrade() {
                // Shown again either way: a drag in hand may have been put back.
                me.redraw(&panel);
                me.show(&panel, true);
            }
        });
        let (me, weak) = (Rc::clone(self), panel.as_weak());
        data.on_metronome_toggled(move || {
            me.card.borrow_mut().set_hear(None, None);
            if let Some(panel) = weak.upgrade() {
                panel.global::<CurveEditorData>().set_metronome(me.card.borrow().hear().click);
            }
        });
        let (me, weak) = (Rc::clone(self), panel.as_weak());
        data.on_hear(move || {
            let Some(panel) = weak.upgrade() else { return };
            if me.tone.get().is_some() || me.making.get().is_some() {
                me.stop(&panel);
            } else {
                me.play(&panel);
            }
        });
        let (me, weak) = (Rc::clone(self), panel.as_weak());
        panel.on_curve_back(move || {
            let Some(panel) = weak.upgrade() else { return };
            me.stop(&panel);
            panel.set_curve_editing(false);
            panel.set_curve_said(SharedString::new());
        });
        let (me, weak, slots) = (Rc::clone(self), panel.as_weak(), Arc::clone(slots));
        panel.on_curve_edit_drag_out(move || {
            let Some((at, found)) = me.open.borrow().clone() else { return };
            if let Some(panel) = weak.upgrade() {
                panel.set_curve_said(SharedString::new());
            }
            // As it is now: with the edits, or as found when there are none.
            let curve = match me.card.borrow().edited() {
                Some((points, length)) => crate::curves::edited(&found, points, length),
                None => found,
            };
            let slots = Arc::clone(&slots);
            background("mo-plugin-drag", move || {
                let file = crate::curves::drag_file(&curve, &std::env::temp_dir());
                *lock(&slots.drag) = Some((Carried::Curve(at), file));
            });
        });
        let drawn = Arc::clone(&self.drawn);
        match Newest::start("mo-plugin-curve-picture", move |ask: Ask| *lock(&drawn) = Some(Drawn::new(&ask))) {
            Ok(drawer) => *self.drawer.borrow_mut() = Some(drawer),
            Err(e) => crate::log::line("Curves", &format!("Couldn't start drawing the curve editor: {e}")),
        }
    }

    /// Opens `curve` (at place `at` of the list) in the editor, in the list's place. Picked again, a curve is as it
    /// was left.
    pub(super) fn open(&self, panel: &Panel, at: usize, curve: &Curve) {
        let data = panel.global::<CurveEditorData>();
        self.stop(panel);
        data.set_name(curve.name.as_str().into());
        if self.card.borrow_mut().open(&curve.id, &curve.name, &curve.points, curve.length_beats) {
            data.set_picture(Image::default());
            data.set_ruler(ModelRc::default());
            data.set_end_x(-1.0);
        }
        *self.open.borrow_mut() = Some((at, curve.clone()));
        panel.set_curve_said(SharedString::new());
        panel.set_curve_editing(true);
        self.redraw(panel);
        self.show(panel, true);
    }

    /// The curves were read again: the one open is found among them again (it may have another place), or the
    /// editor closes when it's gone.
    pub(super) fn read_again(&self, panel: &Panel, curves: &[Curve]) {
        let Some((_, was)) = self.open.borrow().clone() else { return };
        match curves.iter().enumerate().find(|(_, c)| c.id == was.id) {
            // As it's found now: its name may be new.
            Some((at, now)) => {
                panel.global::<CurveEditorData>().set_name(now.name.as_str().into());
                self.card.borrow_mut().open(&now.id, &now.name, &now.points, now.length_beats);
                *self.open.borrow_mut() = Some((at, now.clone()));
                self.show(panel, false);
            }
            None => {
                self.stop(panel);
                self.card.borrow_mut().close();
                *self.open.borrow_mut() = None;
                panel.set_curve_editing(false);
                self.show(panel, true);
            }
        }
    }

    /// The grid's chips as the grid is now.
    fn show_grid(&self, panel: &Panel) {
        let (step, triplet, dotted) = self.card.borrow().grid_chips();
        let data = panel.global::<CurveEditorData>();
        data.set_grid_step(step);
        data.set_grid_triplet(triplet);
        data.set_grid_dotted(dotted);
    }

    /// Shows what lies over the picture as the mouse and the edits have it now; and, when the curve or its highlight
    /// may have changed (`curve`), the part's actions (working them out tries each on the curve: not on every move
    /// of the mouse).
    fn show(&self, panel: &Panel, curve: bool) {
        let data = panel.global::<CurveEditorData>();
        let now = self.card.borrow_mut().showing(cfg!(target_os = "macos"));
        let changed = now.changed;
        mo_app::show_curve_card!(data, now, EditorDot, ClipTile);
        data.set_changed(changed);
        if !curve {
            return;
        }
        let offers = self.card.borrow().part_offers();
        if *self.offers.borrow() != offers {
            let lines: Vec<MenuItem> = offers
                .iter()
                .map(|o| MenuItem {
                    what: short(o).into(),
                    key: o.text.as_str().into(),
                    id: o.id.into(),
                    enabled: o.enabled,
                })
                .collect();
            data.set_part_menu(ModelRc::new(VecModel::from(lines)));
            *self.offers.borrow_mut() = offers;
        }
    }

    /// Asks for a picture of the curve as it is now.
    fn redraw(&self, panel: &Panel) {
        let Some(of) = self.card.borrow_mut().ask() else { return };
        let t = panel.global::<Theme>();
        let well = t.get_well();
        let colours = Colours {
            well: u32::from(well.red()) << 16 | u32::from(well.green()) << 8 | u32::from(well.blue()),
            stroke: colour(t.get_stroke()),
            text: colour(t.get_text()),
            muted: colour(t.get_muted()),
            accent: colour(t.get_accent()),
        };
        let ask = Ask { of, colours, scale: f64::from(panel.window().scale_factor()) };
        if let Some(drawer) = self.drawer.borrow().as_ref() {
            drawer.ask(ask);
        }
    }

    /// Hear: the curve as the volume of a tone, round and round through the track; at the song's speed (and in step
    /// with it) when the DAW says what that is, else at 120.
    fn play(&self, panel: &Panel) {
        let Some((points, length, hear)) = self.card.borrow().to_hear() else { return };
        let song = self.player.tempo().filter(|bpm| (curve_hear::BPM_LOW..=curve_hear::BPM_HIGH).contains(bpm));
        let hear = Hear { bpm: song.unwrap_or(hear.bpm), ..hear };
        *self.heard.borrow_mut() = Some((points.clone(), length, hear));
        self.making.set(Some(Instant::now()));
        panel.global::<CurveEditorData>().set_playing(true);
        let in_step = song.map(|bpm| (curve_hear::heard_beats(length), bpm));
        let make = Box::new(move || Ok((MadeLoop::Sound(curve_hear::tone(&points, length, hear)), in_step)));
        let played = Arc::clone(&self.played);
        self.player.play_made("Curve".into(), make, move |how| *lock(&played) = Some(how));
    }

    /// Stops the tone, if it's this editor's and may still be playing (or is being made): never another sound that
    /// has taken its place since.
    fn stop(&self, panel: &Panel) {
        let ours = self.tone.take().is_some_and(|n| self.player.still_playing(n));
        if self.making.take().is_some() || ours {
            self.player.stop();
        }
        *self.heard.borrow_mut() = None;
        let data = panel.global::<CurveEditorData>();
        data.set_playing(false);
        data.set_playhead(-1.0);
    }

    /// Takes in a picture worked out, and moves the play line. A curve edited while its tone plays has the tone made
    /// again once the mouse lets go (Solomon, 14 Nov 2026: what's heard follows the edits).
    pub(super) fn tick(&self, panel: &Panel) {
        let data = panel.global::<CurveEditorData>();
        if let Some(drawn) = lock(&self.drawn).take()
            && drawn.shown == self.card.borrow().shown()
        {
            data.set_picture(Image::from_rgba8_premultiplied(drawn.pixels));
            let marks: Vec<RulerMark> =
                drawn.ruler.into_iter().map(|(x, text, bar)| RulerMark { x, text: text.into(), bar }).collect();
            data.set_ruler(ModelRc::new(VecModel::from(marks)));
            data.set_end_x(drawn.end_x);
        }
        if let Some(how) = lock(&self.played).take()
            && self.making.take().is_some()
        {
            match how {
                Played::Playing(_, n) => {
                    self.tone.set(Some(n));
                    data.set_playing(true);
                }
                Played::Not(why) => {
                    self.stop(panel);
                    panel.set_curve_said(why.into());
                }
            }
        }
        // A tone being made: the one before it ends as this one starts, which isn't a stop. One never answered (a
        // newer sound overtook it) is given up on, with nothing stopped.
        if let Some(since) = self.making.get() {
            if since.elapsed() <= TONE_WAIT {
                return;
            }
            self.making.set(None);
            if self.tone.get().is_none() {
                *self.heard.borrow_mut() = None;
                data.set_playing(false);
                data.set_playhead(-1.0);
            }
        }
        let Some(n) = self.tone.get() else { return };
        if !self.player.still_playing(n) {
            // It ended, or another sound took its place.
            self.tone.set(None);
            *self.heard.borrow_mut() = None;
            data.set_playing(false);
            data.set_playhead(-1.0);
            return;
        }
        let card = self.card.borrow();
        if let (Some(place), Some((_, length, _))) = (self.player.place(n), self.heard.borrow().as_ref()) {
            let frame = card.frame();
            let beat = f64::from(place) * curve_hear::heard_beats(*length);
            let view = frame.view;
            let in_sight = frame.fits() && beat >= view.start - 1e-9 && beat <= view.start + view.span + 1e-9;
            data.set_playhead(if in_sight { frame.x(beat) as f32 } else { -1.0 });
        }
        // Nothing is made again while the mouse holds something: the curve changes with every move.
        if card.in_hand() {
            return;
        }
        // Edited, or the metronome turned over, since the tone was made: made again, once nothing is in hand.
        let stale = match (card.to_hear(), self.heard.borrow().as_ref()) {
            (Some((points, length, hear)), Some(heard)) => {
                !curve_card::same_heard(&(points, length, Hear { bpm: heard.2.bpm, ..hear }), heard)
            }
            _ => false,
        };
        drop(card);
        if stale {
            self.play(panel);
        }
    }
}
