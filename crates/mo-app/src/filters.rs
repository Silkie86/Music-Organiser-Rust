//! The filters' shared parts, the same on every page (Solomon, 4 Nov 2026: "make sure this is consistant throughout
//! the app"). A filter with only a few choices shows each one as a chip of its own, picked by a click; a longer one is
//! a chip that opens a menu that scrolls. Where it makes sense, more than one can be picked: each picked one is lit
//! (a chip) or ticked (a menu line), and the menu's chip says what's picked ("Part: Bass", "Part: Bass + 2").

/// How many choices a filter can have and still show each one as a chip of its own. There's room for this many
/// (Solomon, 4 Nov 2026: "theres alot of screen real estate so things dont have to be in as many menus"); a longer
/// list, like the plugins, goes in a menu that scrolls.
pub const CHIPS_UP_TO: usize = 10;

/// One choice as its chip or menu line shows it: its words, how many lines it would show ("" when it isn't counted),
/// and whether it's picked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    pub text: String,
    pub count: String,
    pub on: bool,
}

/// A filter as the page shows it: its choices as chips of their own (all of a short list), or in its menu (all of a
/// longer one), and what the menu's chip says is picked in it ("" when nothing is).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub chips: Vec<Shown>,
    pub more: Vec<Shown>,
    pub more_picked: String,
}

/// Shows `choices`, each with its count ("" for none), with the ones in `picked` on: each as a chip of its own when
/// there are [`CHIPS_UP_TO`] or fewer, otherwise all in the menu. A pick is matched without minding capitals (ASCII
/// ones, as the lists it filters match). The menu's chip names the first one picked, whatever order the menu is in.
pub fn show(choices: &[(String, String)], picked: &[String]) -> Filter {
    let is_picked = |text: &str| picked.iter().any(|p| p.eq_ignore_ascii_case(text));
    let mut shown: Vec<Shown> = choices
        .iter()
        .map(|(text, count)| Shown { text: text.clone(), count: count.clone(), on: is_picked(text) })
        .collect();
    // One picked that isn't a choice any more (after a new look at the songs, say) stays on show, picked, to be
    // unpicked: otherwise it would hide lines with nothing to say so.
    let counted = choices.iter().any(|(_, count)| !count.is_empty());
    for gone in picked.iter().filter(|p| !choices.iter().any(|(text, _)| text.eq_ignore_ascii_case(p))) {
        shown.push(Shown { text: gone.clone(), count: if counted { "0".into() } else { String::new() }, on: true });
    }
    // Chips or a menu by the choices alone, so a gone one doesn't turn the chips into a menu until it's unpicked.
    if choices.len() <= CHIPS_UP_TO {
        return Filter { chips: shown, ..Filter::default() };
    }
    Filter { more_picked: picked_among(&shown, picked), more: shown, chips: Vec::new() }
}

/// [`show`] with every choice in the menu, however few there are: for a place with no room for a chip each (the MIDI
/// page's parts of a song, in a narrow window). The menu's chip says what's picked, as a long list's does.
pub fn show_in_menu(choices: &[(String, String)], picked: &[String]) -> Filter {
    let mut all = show(choices, picked);
    if all.more.is_empty() {
        all.more = std::mem::take(&mut all.chips);
        all.more_picked = picked_among(&all.more, picked);
    }
    all
}

/// What a menu's chip says of the ones in `picked` that are among its lines (`shown`), in the order they were
/// picked and in the menu's own words.
fn picked_among(shown: &[Shown], picked: &[String]) -> String {
    let on: Vec<&str> = picked
        .iter()
        .filter_map(|p| shown.iter().find(|s| s.text.eq_ignore_ascii_case(p)).map(|s| s.text.as_str()))
        .collect();
    picked_words(&on)
}

/// [`show`] for a long list whose first choices are picked far more than the rest (the sounds' types, the most used
/// first): the first `up_to` as chips of their own, and the others in a menu after them, whose chip says what's
/// picked in it. A short list ([`CHIPS_UP_TO`] or fewer) is all chips, as [`show`] makes it. One picked that isn't a
/// choice any more goes in the menu, so the chips stay as they are.
pub fn show_split(choices: &[(String, String)], picked: &[String], up_to: usize) -> Filter {
    let mut all = show(choices, picked);
    if all.more.is_empty() {
        return all;
    }
    let more = all.more.split_off(up_to.min(all.more.len()));
    Filter { more_picked: picked_among(&more, picked), chips: all.more, more }
}

/// A choice where one is always picked, such as the order a list is in: each choice as a chip of its own, with the
/// picked one on, and nothing counted. It's for a short list ([`CHIPS_UP_TO`] or fewer): all of it stays on show
/// (Solomon, 4 Nov 2026: "makes sure things that are useful arent hidden away"). The page keeps the pick when the
/// picked chip is clicked again, so there's always one.
pub fn one_of<S: AsRef<str>>(choices: &[S], picked: &str) -> Filter {
    let chips = choices
        .iter()
        .map(|c| Shown { text: c.as_ref().to_owned(), count: String::new(), on: c.as_ref() == picked })
        .collect();
    // What's picked, in words, for a small drop-down that shows the choice as "Placed by: Both" (OneOfChip).
    let more_picked = choices.iter().map(AsRef::as_ref).find(|c| *c == picked).unwrap_or_default().to_owned();
    Filter { chips, more_picked, ..Filter::default() }
}

/// What a menu's chip says is picked: the one, "Bass + 2" for more, or "" for none.
pub fn picked_words(picked: &[&str]) -> String {
    match picked {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, rest @ ..] => format!("{first} + {}", rest.len()),
    }
}

/// Picks or unpicks `choice`. With `many`, it joins or leaves the ones picked; alone, it becomes the only one, or none
/// when it was the one already. Every filter picks through this, so they all pick the same way.
pub fn toggle(picked: &mut Vec<String>, choice: &str, many: bool) {
    let was = picked.iter().position(|p| p.eq_ignore_ascii_case(choice));
    match (was, many) {
        (Some(at), true) => {
            picked.remove(at);
        }
        (None, true) => picked.push(choice.to_owned()),
        (Some(_), false) => picked.clear(),
        (None, false) => *picked = vec![choice.to_owned()],
    }
}

/// [`toggle`] for a filter that keeps its picks as values, not words: `words` says each value's words, and
/// `from_words` turns words back into a value (`None` for words that aren't one of its choices).
pub fn toggle_as<T: Copy, W: AsRef<str>>(
    picked: &mut Vec<T>,
    choice: &str,
    many: bool,
    words: impl Fn(T) -> W,
    from_words: impl Fn(&str) -> Option<T>,
) {
    let mut as_words: Vec<String> = picked.iter().map(|&p| words(p).as_ref().to_owned()).collect();
    toggle(&mut as_words, choice, many);
    *picked = as_words.iter().filter_map(|w| from_words(w)).collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices(names: &[&str]) -> Vec<(String, String)> {
        names.iter().map(|n| ((*n).to_owned(), String::new())).collect()
    }

    #[test]
    fn a_short_list_is_chips_and_a_long_one_a_menu() {
        let short = show(&choices(&["Synths", "Effects"]), &["synths".to_owned()]);
        assert_eq!(short.chips.iter().map(|c| c.on).collect::<Vec<_>>(), [true, false]);
        assert!(short.more.is_empty() && short.more_picked.is_empty());
        // One more than there's room for: all in the menu.
        let plugins: Vec<(String, String)> = (0..=CHIPS_UP_TO).map(|n| (format!("Synth {n}"), String::new())).collect();
        let long = show(&plugins, &["synth 3".to_owned(), "Synth 1".to_owned()]);
        assert!(long.chips.is_empty());
        assert_eq!(long.more.iter().filter(|c| c.on).count(), 2);
        // The chip names the first one picked, in the menu's own words.
        assert_eq!(long.more_picked, "Synth 3 + 1");
        // As many as there's room for: all chips.
        assert_eq!(show(&plugins[..CHIPS_UP_TO], &[]).chips.len(), CHIPS_UP_TO);
        // One picked that's no longer a choice stays, picked, with nothing counted.
        let counted = vec![("Bass".to_owned(), "3".to_owned())];
        let kept = show(&counted, &["Vocal".to_owned()]);
        assert_eq!(kept.chips.last(), Some(&Shown { text: "Vocal".into(), count: "0".into(), on: true }));
        // A full row of chips with a gone one too stays chips, so the row doesn't turn into a menu and back.
        let full = show(&plugins[..CHIPS_UP_TO], &["Gone".to_owned()]);
        assert_eq!((full.chips.len(), full.more.len()), (CHIPS_UP_TO + 1, 0));
    }

    #[test]
    fn a_long_list_can_keep_its_first_choices_as_chips() {
        let types: Vec<(String, String)> = (0..=CHIPS_UP_TO).map(|n| (format!("Type {n}"), "1".to_owned())).collect();
        let picked = ["Type 0".to_owned(), "Gone".to_owned(), "type 9".to_owned()];
        let split = show_split(&types, &picked, 3);
        assert_eq!(
            split.chips.iter().map(|c| (c.text.as_str(), c.on)).collect::<Vec<_>>(),
            [("Type 0", true), ("Type 1", false), ("Type 2", false)]
        );
        // The rest in the menu, the one no longer a choice last; its chip says what's picked in it, not on the chips.
        assert_eq!(split.more.len(), CHIPS_UP_TO + 1 - 3 + 1);
        assert_eq!(split.more.last(), Some(&Shown { text: "Gone".into(), count: "0".into(), on: true }));
        assert_eq!(split.more_picked, "Gone + 1");
        // A short list is all chips.
        let short = show_split(&types[..CHIPS_UP_TO], &[], 3);
        assert_eq!((short.chips.len(), short.more.len()), (CHIPS_UP_TO, 0));
    }

    #[test]
    fn a_list_can_be_all_in_the_menu_however_short() {
        // Three choices, two picked (one of them no longer a choice): chips anywhere else, all in the menu here.
        let parts = vec![("Keys".to_owned(), "chords".to_owned()), ("Sub".to_owned(), "bassline".to_owned())];
        let picked = ["sub".to_owned(), "Gone".to_owned()];
        let menu = show_in_menu(&parts, &picked);
        assert!(menu.chips.is_empty());
        assert_eq!(
            menu.more.iter().map(|c| (c.text.as_str(), c.count.as_str(), c.on)).collect::<Vec<_>>(),
            [("Keys", "chords", false), ("Sub", "bassline", true), ("Gone", "0", true)]
        );
        assert_eq!(menu.more_picked, "Sub + 1");
        // The same lines as the chips would be.
        assert_eq!(menu.more, show(&parts, &picked).chips);
        // Nothing picked: the chip says nothing. A long list is as `show` makes it.
        assert_eq!(show_in_menu(&parts, &[]).more_picked, "");
        let long: Vec<(String, String)> = (0..=CHIPS_UP_TO).map(|n| (format!("Synth {n}"), String::new())).collect();
        let picked = ["Synth 3".to_owned()];
        assert_eq!(show_in_menu(&long, &picked), show(&long, &picked));
    }

    #[test]
    fn one_of_a_few_shows_every_choice_with_the_picked_one_on() {
        let order = one_of(&["Shuffled", "Name", "Newest"], "Name");
        assert_eq!(
            order.chips.iter().map(|c| (c.text.as_str(), c.count.as_str(), c.on)).collect::<Vec<_>>(),
            [("Shuffled", "", false), ("Name", "", true), ("Newest", "", false)]
        );
        assert!(order.more.is_empty());
        assert_eq!(order.more_picked, "Name");
        // Words of any kind, such as numbers made as text.
        let counts = one_of(&[30, 60].map(|n: i32| n.to_string()), "60");
        assert_eq!(counts.chips.iter().filter(|c| c.on).count(), 1);
    }

    #[test]
    fn the_menus_chip_says_what_is_picked() {
        assert_eq!(picked_words(&[]), "");
        assert_eq!(picked_words(&["Bass"]), "Bass");
        assert_eq!(picked_words(&["Bass", "Lead", "Pad"]), "Bass + 2");
    }

    #[test]
    fn picking_more_than_one_or_only_one() {
        let mut picked = Vec::new();
        toggle(&mut picked, "Bass", true);
        toggle(&mut picked, "Lead", true);
        assert_eq!(picked, ["Bass", "Lead"]);
        toggle(&mut picked, "bass", true);
        assert_eq!(picked, ["Lead"]);
        // Alone: the one picked takes the place of the others, and picking it again picks none.
        toggle(&mut picked, "Synths", false);
        assert_eq!(picked, ["Synths"]);
        toggle(&mut picked, "Synths", false);
        assert!(picked.is_empty());
        // The same for picks kept as values.
        let words = |n: u8| ["Zero", "One", "Two"][usize::from(n)];
        let from_words = |w: &str| (0..3).find(|&n| words(n) == w);
        let mut values = vec![1u8];
        toggle_as(&mut values, "Two", true, words, from_words);
        assert_eq!(values, [1, 2]);
        toggle_as(&mut values, "Zero", false, words, from_words);
        assert_eq!(values, [0]);
        toggle_as(&mut values, "Zero", false, words, from_words);
        assert!(values.is_empty());
    }
}
