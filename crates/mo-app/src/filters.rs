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
/// there are [`CHIPS_UP_TO`] or fewer, otherwise all in the menu. A pick is matched without minding capitals, as the
/// lists it filters are.
pub fn show(choices: &[(String, String)], picked: &[String]) -> Filter {
    let is_picked = |text: &str| picked.iter().any(|p| p.eq_ignore_ascii_case(text));
    let shown: Vec<Shown> = choices
        .iter()
        .map(|(text, count)| Shown { text: text.clone(), count: count.clone(), on: is_picked(text) })
        .collect();
    if shown.len() <= CHIPS_UP_TO {
        return Filter { chips: shown, ..Filter::default() };
    }
    let on: Vec<&str> = shown.iter().filter(|s| s.on).map(|s| s.text.as_str()).collect();
    Filter { more_picked: picked_words(&on), more: shown, chips: Vec::new() }
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
/// when it was the one already.
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

/// The choice at `at` in a filter shown by [`show`]: its chips first, then its menu's lines.
pub fn choice_at(filter: &Filter, at: usize) -> Option<&str> {
    filter.chips.iter().chain(&filter.more).nth(at).map(|s| s.text.as_str())
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
        let long = show(&plugins, &["Synth 1".to_owned(), "synth 3".to_owned()]);
        assert!(long.chips.is_empty());
        assert_eq!(long.more.iter().filter(|c| c.on).count(), 2);
        assert_eq!(long.more_picked, "Synth 1 + 1");
        assert_eq!(choice_at(&long, 3), Some("Synth 3"));
        assert_eq!(choice_at(&long, CHIPS_UP_TO + 1), None);
        // As many as there's room for: all chips.
        assert_eq!(show(&plugins[..CHIPS_UP_TO], &[]).chips.len(), CHIPS_UP_TO);
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
    }
}
