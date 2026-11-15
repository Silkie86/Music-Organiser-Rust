//! Which key does what in the curve editor (Solomon, 14 Nov 2026: "make it so you can choose what modifier keys do
//! what"). The editor's three keys each have one meaning everywhere: one is free (off the grid, finely), one bends
//! (bend, smooth, the box that picks points), one makes (paint, copy, repeat, pick more points). Which of Shift, Alt
//! and Ctrl is which is chosen on Settings > Shortcuts; each key has one meaning, so choosing a key for one swaps it
//! with the one that had it.
//!
//! The editor's own rules go by the meanings ([`Keys`]'s `shift` is "the free key is held", `alt` the bend key,
//! `ctrl` the make key, as the keys are when nothing is chosen); [`KeyMap::roles`] turns the keys held into those,
//! and [`KeyMap::words`] names the chosen keys in the hint line's words.

use crate::curve_touch::Keys;

/// The setting's name in the app's settings: the keys for free, bend and make, in that order ("shift alt ctrl").
pub const SETTING: &str = "curve_editor_keys";

/// A key held with the mouse. Ctrl is Cmd on a Mac, Alt is Option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Shift,
    Alt,
    Ctrl,
}

impl Key {
    pub const ALL: [Key; 3] = [Key::Shift, Key::Alt, Key::Ctrl];

    /// The key's name, as a Mac names it when `mac`.
    pub fn name(self, mac: bool) -> &'static str {
        match (self, mac) {
            (Key::Shift, _) => "Shift",
            (Key::Alt, false) => "Alt",
            (Key::Alt, true) => "Option",
            (Key::Ctrl, false) => "Ctrl",
            (Key::Ctrl, true) => "Cmd",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Key::Shift => "shift",
            Key::Alt => "alt",
            Key::Ctrl => "ctrl",
        }
    }
}

/// What a key means in the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Free,
    Bend,
    Make,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Free, Role::Bend, Role::Make];

    /// The meaning in a word (the lamp's), and what it covers (Settings' line).
    pub fn words(self) -> (&'static str, &'static str) {
        match self {
            Role::Free => ("free", "Free: off the grid, finely, a box adds to the picked points"),
            Role::Bend => ("bend", "Bend: bend a line, smooth or sharpen, draw a box to pick points"),
            Role::Make => ("make", "Make: paint steps, copy, repeat, pick more points"),
        }
    }
}

/// Which key has which meaning: each key has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyMap {
    free: Key,
    bend: Key,
    make: Key,
}

impl Default for KeyMap {
    fn default() -> KeyMap {
        KeyMap { free: Key::Shift, bend: Key::Alt, make: Key::Ctrl }
    }
}

impl KeyMap {
    /// As the setting keeps it; the keys as they come when it's missing or isn't three different keys.
    pub fn from_setting(value: Option<&str>) -> KeyMap {
        let keys: Vec<Key> = value
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|w| Key::ALL.into_iter().find(|k| k.word().eq_ignore_ascii_case(w)))
            .collect();
        match keys[..] {
            [free, bend, make] if free != bend && bend != make && free != make => KeyMap { free, bend, make },
            _ => KeyMap::default(),
        }
    }

    /// The setting's value.
    pub fn to_setting(self) -> String {
        format!("{} {} {}", self.free.word(), self.bend.word(), self.make.word())
    }

    /// The key that has this meaning.
    pub fn key(self, role: Role) -> Key {
        match role {
            Role::Free => self.free,
            Role::Bend => self.bend,
            Role::Make => self.make,
        }
    }

    /// `key` given to `role`: the meaning that had it gets the key `role` had.
    pub fn with(self, role: Role, key: Key) -> KeyMap {
        let was = self.key(role);
        let swap = |k: Key| {
            if k == key {
                was
            } else if k == was {
                key
            } else {
                k
            }
        };
        KeyMap { free: swap(self.free), bend: swap(self.bend), make: swap(self.make) }
    }

    /// The keys held, as their meanings: `shift` says the free key is held, `alt` the bend key, `ctrl` the make key.
    pub fn roles(self, held: Keys) -> Keys {
        let down = |key: Key| match key {
            Key::Shift => held.shift,
            Key::Alt => held.alt,
            Key::Ctrl => held.ctrl,
        };
        Keys { shift: down(self.free), alt: down(self.bend), ctrl: down(self.make) }
    }

    /// The editor's words (written with the keys as they come: "Shift: off the grid", "Alt-drag", "Cmd: paint") with
    /// each key's name changed for the key that has its meaning now, as a Mac names it when `mac`. Only for the
    /// editor's hint words, where every key named is one of the three held with the mouse.
    pub fn words(self, text: &str, mac: bool) -> String {
        const NAMES: [(&str, Role); 5] = [
            ("Shift", Role::Free),
            ("Option", Role::Bend),
            ("Alt", Role::Bend),
            ("Cmd", Role::Make),
            ("Ctrl", Role::Make),
        ];
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        // A key's name starts a word and isn't the start of a longer one.
        let mut starts_word = true;
        while let Some(c) = rest.chars().next() {
            let named = NAMES.iter().find(|(name, _)| {
                starts_word
                    && rest
                        .strip_prefix(name)
                        .is_some_and(|after| !after.chars().next().is_some_and(char::is_alphanumeric))
            });
            match named {
                Some((name, role)) => {
                    out.push_str(self.key(*role).name(mac));
                    rest = rest.get(name.len()..).unwrap_or_default();
                    starts_word = false;
                }
                None => {
                    out.push(c);
                    rest = rest.get(c.len_utf8()..).unwrap_or_default();
                    starts_word = !c.is_alphanumeric();
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_is_three_different_keys_or_the_keys_as_they_come() {
        let d = KeyMap::default();
        assert_eq!(d.to_setting(), "shift alt ctrl");
        assert_eq!(KeyMap::from_setting(Some("shift alt ctrl")), d);
        let m = KeyMap::from_setting(Some("Ctrl shift ALT"));
        assert_eq!((m.key(Role::Free), m.key(Role::Bend), m.key(Role::Make)), (Key::Ctrl, Key::Shift, Key::Alt));
        assert_eq!(KeyMap::from_setting(Some(&m.to_setting())), m);
        for bad in
            [None, Some(""), Some("shift shift ctrl"), Some("shift alt"), Some("a b c"), Some("shift alt ctrl alt")]
        {
            assert_eq!(KeyMap::from_setting(bad), d, "{bad:?}");
        }
    }

    #[test]
    fn a_key_chosen_for_one_meaning_swaps_with_the_one_that_had_it() {
        let m = KeyMap::default().with(Role::Free, Key::Alt);
        assert_eq!((m.key(Role::Free), m.key(Role::Bend), m.key(Role::Make)), (Key::Alt, Key::Shift, Key::Ctrl));
        assert_eq!(m.with(Role::Free, Key::Alt), m, "the key it has already");
        // Whatever is chosen, each key keeps one meaning.
        let mut m = KeyMap::default();
        for (role, key) in [(Role::Make, Key::Shift), (Role::Bend, Key::Shift), (Role::Free, Key::Ctrl)] {
            m = m.with(role, key);
            let mut keys = Role::ALL.map(|r| m.key(r).word());
            keys.sort_unstable();
            assert_eq!(keys, ["alt", "ctrl", "shift"]);
        }
    }

    #[test]
    fn keys_held_are_their_meanings_and_the_words_name_the_chosen_keys() {
        let m = KeyMap::default().with(Role::Free, Key::Alt);
        // Alt is the free key now, Shift bends.
        let held = Keys { alt: true, ..Keys::default() };
        assert_eq!(m.roles(held), Keys { shift: true, ..Keys::default() });
        assert_eq!(KeyMap::default().roles(held), held);
        let text = "Shift: off the grid · Ctrl: paint steps · Alt-drag: pick points in a box";
        assert_eq!(m.words(text, false), "Alt: off the grid · Ctrl: paint steps · Shift-drag: pick points in a box");
        assert_eq!(KeyMap::default().words(text, false), text);
        // A Mac's names for the keys, whatever names the words came with.
        assert_eq!(
            KeyMap::default().words("Ctrl-drag copies it · Alt: bend", true),
            "Cmd-drag copies it · Option: bend"
        );
        let mac = "Option: bend it · Cmd-click: pick it with others · Shift: finely";
        assert_eq!(m.words(mac, true), "Shift: bend it · Cmd-click: pick it with others · Option: finely");
        // Only whole words that are a key's name.
        assert_eq!(m.words("Alternate Shifted · é Alt", false), "Alternate Shifted · é Shift");
    }
}
