//! How wide each part of a page is that you can drag wider or narrower: the side cards, the filter column, the Sounds
//! folder tree, a list's columns (Solomon, 25 Nov 2026: "all the columns/cards in the app and plugin should be
//! resizeable", each with "a defualt and a min a max" that it isn't fixed between).
//!
//! Every such part is in [`PARTS`] once, by a short name such as `sounds.card`, with its usual width and the least and
//! most it may be. The widths you chose are kept in one setting, [`KEY`], as `name=width` pairs; a part at its usual
//! width isn't written at all (rule 23: store each fact once, and nothing that's the default anyway), except a side
//! card you dragged ([`Part::is_card`]: its usual width is a way of laying out its page). A part no longer
//! in the table, or a width that isn't a number, is left out when it's read, so an old or hand-edited file can't break
//! a page.

use std::collections::BTreeMap;

/// The setting the widths are kept in, in the app's own settings file.
pub const KEY: &str = "widths";

/// A part of a page you can make wider or narrower, in logical pixels (what Slint calls `px`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    /// Its name in the setting: `<page>.<part>`.
    pub name: &'static str,
    /// Its width until you drag it, and again after a double-click on its edge. 0 where the page works it out (the
    /// Songs list, a third of the room; a song's track names, by the view's width): then the page is given 0, and its
    /// own width is used.
    pub usual: f32,
    /// The least and the most it may be: enough for what it shows, and not so much the rest of the page goes.
    pub least: f32,
    pub most: f32,
}

const fn part(name: &'static str, usual: f32, least: f32, most: f32) -> Part {
    Part { name, usual, least, most }
}

impl Part {
    /// Whether it's the card beside a page's list. A card's usual width isn't only a width: until you drag it, the
    /// page lays itself out its usual way (the list at its widest, the card taking the rest, so on a wide window the
    /// card is much wider than its usual width), and it's given 0 then. Once you've dragged it, it's as wide as you
    /// made it, even at its usual width: "dragged to 400" and "never dragged" are two facts. Before, a card let go
    /// within half a pixel of its usual width stayed that wide until the app closed, and came back much wider at the
    /// next start (the whole-app review of 26 Nov, WI-2).
    pub fn is_card(&self) -> bool {
        self.name.ends_with(".card")
    }
}

/// Every part you can drag. Their usual widths are the ones the pages had before they could be dragged.
pub const PARTS: &[Part] = &[
    // The card beside a page's list (the picked thing's). As wide as the window allows: the page keeps the list
    // at least 380 px (480 on Space) beside it, so on a wide screen a card can take most of it.
    part("sounds.card", 400.0, 320.0, 2400.0),
    part("presets.card", 400.0, 320.0, 2400.0),
    part("midi.card", 420.0, 320.0, 2400.0),
    part("plugins.card", 420.0, 320.0, 2400.0),
    part("space.card", 380.0, 320.0, 2400.0),
    // The filter column on the left of a page's list.
    part("sounds.filters", 208.0, 160.0, 360.0),
    part("presets.filters", 208.0, 160.0, 360.0),
    part("midi.filters", 208.0, 160.0, 360.0),
    part("plugins.filters", 208.0, 160.0, 360.0),
    part("songs.filters", 208.0, 160.0, 360.0),
    // The Sounds folder tree, beside the filter column.
    part("sounds.folders", 280.0, 180.0, 480.0),
    // The Sounds Edit strip along the bottom of the page: its height, dragged by its top edge.
    part("sounds.edit", 320.0, 240.0, 900.0),
    // The Songs list beside the song's panel. Its usual width is none: the page gives it a third of the room then.
    part("songs.list", 0.0, 320.0, 1200.0),
    // The Curves list beside the curve editor. None usual: the page gives it about a fifth of the room then.
    part("curves.list", 0.0, 220.0, 720.0),
    // The track names in a song's arrangement: usual 118 or 172 by the view's width, worked out by the view.
    part("songs.tracks", 0.0, 118.0, 480.0),
    // The lists' columns of fixed width, by page: the least that still shows the heading, about three times as wide
    // at most. The column that stretches (the name) gives or takes the room.
    part("sounds.type", 110.0, 60.0, 300.0),
    part("sounds.key", 52.0, 40.0, 140.0),
    part("sounds.tempo", 80.0, 60.0, 200.0),
    part("sounds.length", 56.0, 56.0, 140.0),
    part("sounds.used", 96.0, 64.0, 240.0),
    part("presets.part", 96.0, 56.0, 260.0),
    part("presets.kind", 56.0, 48.0, 160.0),
    part("presets.file", 72.0, 44.0, 180.0),
    part("midi.from", 180.0, 80.0, 480.0),
    part("midi.bars", 44.0, 44.0, 120.0),
    part("midi.files", 44.0, 44.0, 120.0),
    part("midi.key", 96.0, 48.0, 240.0),
    part("midi.saved", 96.0, 60.0, 240.0),
    part("plugins.songs", 80.0, 60.0, 200.0),
    part("plugins.last", 76.0, 76.0, 200.0),
    part("plugins.size", 72.0, 48.0, 180.0),
    part("songs.status", 150.0, 70.0, 320.0),
    part("songs.saved", 90.0, 90.0, 220.0),
    part("songs.time", 40.0, 36.0, 140.0),
];

/// The file in the data folder where the CLAP plugin keeps its parts' widths (`mo-plugin`'s `widths`), written whole
/// through [`crate::backup::write_whole`] (so it's on [`crate::backup::WRITTEN_WHOLE`]). Plugins before 27 Nov 2026
/// wrote it by way of the same name with [`crate::backup::PART_ENDING`] after it, which the start-up sweep still
/// knows.
pub const PLUGIN_FILE: &str = "plugin-widths.txt";

/// The CLAP plugin's parts you can drag. The plugin keeps their widths in a file of its own (it never writes the
/// app's settings); the app has no page for them.
pub const PLUGIN_PARTS: &[Part] = &[
    // Each view's filter column (Sounds, MIDI, Presets, Curves): one width for all.
    part("plugin.filters", 196.0, 160.0, 360.0),
    // The Sounds view's folder tree.
    part("plugin.folders", 280.0, 180.0, 480.0),
    // Chord check's list of takes.
    part("plugin.takes", 210.0, 160.0, 480.0),
    // The Sounds view's Edit strip along the bottom of the panel: its height, dragged by its top edge. Lower than
    // the app's to start with, as the panel opens smaller than the app's window.
    part("plugin.edit", 284.0, 240.0, 900.0),
];

/// The part called `name`, the app's or the plugin's, if there is one.
pub fn find(name: &str) -> Option<&'static Part> {
    PARTS.iter().chain(PLUGIN_PARTS).find(|p| p.name == name)
}

/// The widths you chose, by part: only parts in [`PARTS`], each within its limits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Widths {
    chosen: BTreeMap<&'static str, f32>,
}

impl Widths {
    /// From the setting's value (`sounds.card=520;midi.card=480`). Anything it can't use is left out.
    pub fn read(value: &str) -> Widths {
        let mut widths = Widths::default();
        for pair in value.split(';') {
            let Some((name, width)) = pair.split_once('=') else { continue };
            let (Some(part), Ok(width)) = (find(name.trim()), width.trim().parse::<f32>()) else { continue };
            widths.set(part.name, width);
        }
        widths
    }

    /// The setting's value: the parts not at their usual width, and the cards you dragged, by name (so it reads the
    /// same each time). "" when there are none.
    pub fn value(&self) -> String {
        self.chosen.iter().map(|(name, w)| format!("{name}={}", w.round())).collect::<Vec<_>>().join(";")
    }

    /// How wide `name` is, as its page is given it: the width you chose, else its usual one. 0 for a part that isn't in
    /// the table, one whose usual width the page works out (see [`Part::usual`]), or a card ([`Part::is_card`]), while
    /// you haven't chosen one.
    pub fn width(&self, name: &str) -> f32 {
        let Some(part) = find(name) else { return 0.0 };
        let usual = if part.is_card() { 0.0 } else { part.usual };
        self.chosen.get(part.name).copied().unwrap_or(usual)
    }

    /// Makes `name` `width` wide, kept within its limits, and returns the width it got: 0 for a part that isn't in
    /// the table, its width as it was for a width that isn't a number (then nothing changes). Within half a pixel of its usual width it
    /// counts as the usual width and isn't kept, except for a card ([`Part::is_card`]).
    pub fn set(&mut self, name: &str, width: f32) -> f32 {
        let Some(part) = find(name) else { return 0.0 };
        if !width.is_finite() {
            return self.width(name);
        }
        let width = width.round().clamp(part.least, part.most);
        if !part.is_card() && (width - part.usual).abs() < 0.5 {
            self.chosen.remove(part.name);
        } else {
            self.chosen.insert(part.name, width);
        }
        width
    }

    /// Puts `name` back to its usual width, and returns it.
    pub fn reset(&mut self, name: &str) -> f32 {
        self.chosen.remove(name);
        self.width(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_once_with_its_usual_width_inside_its_limits() {
        let all: Vec<&Part> = PARTS.iter().chain(PLUGIN_PARTS).collect();
        for (i, p) in all.iter().enumerate() {
            // A usual width of 0 only where the page works one out (the Songs list's third of the room): anywhere else
            // the part would be given no width at all.
            let worked_out = p.name == "songs.list" || p.name == "curves.list" || p.name == "songs.tracks";
            assert!(if worked_out { p.usual == 0.0 } else { p.least <= p.usual && p.usual <= p.most }, "{}", p.name);
            assert!(p.least <= p.most, "{}", p.name);
            assert!(p.name.contains('.') && !p.name.contains(['=', ';']), "{}", p.name);
            assert!(all[..i].iter().all(|q| q.name != p.name), "{} twice", p.name);
        }
    }

    #[test]
    fn usual_widths_are_not_kept() {
        let mut w = Widths::default();
        assert_eq!(w.value(), "");
        assert_eq!(w.width("sounds.filters"), 208.0);
        assert_eq!(w.set("sounds.filters", 240.4), 240.0);
        assert_eq!(w.set("midi.filters", 208.2), 208.0);
        assert_eq!(w.value(), "sounds.filters=240");
        assert_eq!(w.reset("sounds.filters"), 208.0);
        assert_eq!(w.value(), "");
    }

    /// The whole-app review of 26 Nov, WI-2: a card is given 0 until it's dragged (its page lays itself out its usual
    /// way then), and a card dragged to its usual width is kept as dragged, so it comes back the same after a restart;
    /// only a double-click forgets it.
    #[test]
    fn a_card_dragged_to_its_usual_width_is_kept() {
        let mut w = Widths::default();
        let cards: Vec<&str> = PARTS.iter().filter(|p| p.is_card()).map(|p| p.name).collect();
        assert_eq!(cards, ["sounds.card", "presets.card", "midi.card", "plugins.card", "space.card"]);
        assert!(PLUGIN_PARTS.iter().all(|p| !p.is_card()));
        assert_eq!(w.width("sounds.card"), 0.0);
        assert_eq!(w.set("sounds.card", 400.2), 400.0);
        assert_eq!(w.set("midi.card", 520.4), 520.0);
        assert_eq!(w.width("sounds.card"), 400.0);
        assert_eq!(w.value(), "midi.card=520;sounds.card=400");
        assert_eq!(Widths::read(&w.value()), w);
        assert_eq!(w.reset("sounds.card"), 0.0);
        assert_eq!(w.value(), "midi.card=520");
    }

    #[test]
    fn a_worked_out_width_is_none_until_dragged() {
        let mut w = Widths::default();
        assert_eq!(w.width("songs.list"), 0.0);
        assert_eq!(w.set("songs.list", 10.0), 320.0);
        assert_eq!(w.value(), "songs.list=320");
        assert_eq!(w.reset("songs.list"), 0.0);
        assert_eq!(w.value(), "");
    }

    #[test]
    fn kept_within_limits() {
        let mut w = Widths::default();
        assert_eq!(w.set("sounds.card", 10.0), 320.0);
        assert_eq!(w.set("sounds.card", 5000.0), 2400.0);
        assert_eq!(w.set("sounds.card", f32::NAN), 2400.0);
        assert_eq!(w.set("nothing.here", 500.0), 0.0);
        assert_eq!(w.width("nothing.here"), 0.0);
    }

    #[test]
    fn reads_back_what_it_wrote_and_skips_the_rest() {
        let mut w = Widths::default();
        w.set("sounds.card", 512.0);
        w.set("space.card", 700.0);
        assert_eq!(Widths::read(&w.value()), w);
        let odd = Widths::read("sounds.card=600; gone.part=300;midi.card=wide;;plugins.card=9999;presets.card");
        assert_eq!(odd.value(), "plugins.card=2400;sounds.card=600");
    }
}
