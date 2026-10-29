//! The app's looks: nine themes and an accent colour, copied from the C# app so the `appearance_theme` and
//! `appearance_accent` settings mean the same thing in both. Graphite is the default (Solomon's pick).

/// The settings that choose the look. Same names as the C# app.
pub const THEME_KEY: &str = "appearance_theme";
pub const ACCENT_KEY: &str = "appearance_accent";

/// The font family a theme uses for its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeFont {
    /// IBM Plex Sans, with Plex Mono for numbers and dates.
    Plex,
    /// Space Grotesk, with Space Mono for numbers and dates (Retro, Synthwave).
    Space,
    /// IBM Plex Mono for everything (Terminal).
    Mono,
}

impl ThemeFont {
    /// The family for ordinary text.
    pub fn text_family(self) -> &'static str {
        match self {
            ThemeFont::Plex => "IBM Plex Sans",
            ThemeFont::Space => "Space Grotesk",
            ThemeFont::Mono => "IBM Plex Mono",
        }
    }

    /// The family for numbers, dates and times.
    pub fn number_family(self) -> &'static str {
        match self {
            ThemeFont::Plex | ThemeFont::Mono => "IBM Plex Mono",
            ThemeFont::Space => "Space Mono",
        }
    }
}

/// One look for the whole app. Colours are 0xRRGGBB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub description: &'static str,
    pub background: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub surface_raised: u32,
    pub stroke: u32,
    pub hover: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    /// Text on accent-coloured buttons.
    pub accent_text: u32,
    pub font: ThemeFont,
    /// Dark text on a light background.
    pub light: bool,
    /// Square corners, like hardware.
    pub square: bool,
}

#[allow(clippy::too_many_arguments)]
const fn theme(
    name: &'static str,
    description: &'static str,
    c: [u32; 11],
    font: ThemeFont,
    light: bool,
    square: bool,
) -> Theme {
    Theme {
        name,
        description,
        background: c[0],
        sidebar: c[1],
        surface: c[2],
        surface_raised: c[3],
        stroke: c[4],
        hover: c[5],
        text: c[6],
        muted: c[7],
        faint: c[8],
        accent: c[9],
        accent_text: c[10],
        font,
        light,
        square,
    }
}

use ThemeFont::{Mono, Plex, Space};

/// Every theme; the first is the default.
pub const THEMES: [Theme; 9] = [
    theme(
        "Graphite",
        "Neutral greys with a mint accent: no colour cast at all. The app's own look.",
        [0x121212, 0x171717, 0x1D1D1D, 0x252525, 0x323232, 0x2A2A2A, 0xF0F0F0, 0xA3A3A3, 0x6E6E6E, 0x3DDC97, 0x06180F],
        Plex,
        false,
        false,
    ),
    theme(
        "Studio",
        "Dark grey with a violet accent.",
        [0x101114, 0x15161A, 0x1A1B20, 0x212228, 0x2A2C33, 0x24252C, 0xECEDF0, 0x9A9DA7, 0x676A74, 0x8B7CF6, 0xFFFFFF],
        Plex,
        false,
        false,
    ),
    theme(
        "Retro",
        "Warm brown and cream with amber, like old studio gear. Typewriter titles.",
        [0x17130F, 0x1E1914, 0x241E18, 0x2D261E, 0x3B3227, 0x332A21, 0xF3E7D0, 0xB9A98D, 0x7F725D, 0xF2A03D, 0x1A1208],
        Space,
        false,
        false,
    ),
    theme(
        "Midnight",
        "Deep blue-black with a cool blue accent, easy on the eyes at night.",
        [0x0B0F1C, 0x0F1424, 0x131A2D, 0x1A223A, 0x252F4A, 0x1C2541, 0xE6ECF8, 0x93A0BF, 0x5E6A88, 0x4FB3FF, 0x06121F],
        Plex,
        false,
        false,
    ),
    theme(
        "Paper",
        "Light: warm paper and ink with a vermilion accent, like a printed track sheet. For daytime.",
        [0xF4F1EA, 0xEAE5D9, 0xFBF9F4, 0xFFFFFF, 0xDCD4C3, 0xECE6DA, 0x1F1B16, 0x5F574B, 0x958B7B, 0xD9480F, 0xFFFFFF],
        Plex,
        true,
        false,
    ),
    theme(
        "Hardware",
        "Square corners, black panels and signal orange, like a drum machine's front.",
        [0x161616, 0x0F0F0F, 0x1E1E1E, 0x272727, 0x383838, 0x2C2C2C, 0xEDEDED, 0x9C9C9C, 0x666666, 0xFF6A13, 0x140800],
        Plex,
        false,
        true,
    ),
    theme(
        "Terminal",
        "Green phosphor on black, mono type everywhere, square corners. An 80s computer screen.",
        [0x040804, 0x060C06, 0x0A120A, 0x0F1A0F, 0x1D331F, 0x122214, 0xB6FFC0, 0x63B871, 0x3A6E45, 0x39FF7A, 0x021006],
        Mono,
        false,
        true,
    ),
    theme(
        "Synthwave",
        "Deep purple night with hot pink: neon, but still calm enough to work in.",
        [0x140A22, 0x1A0D2C, 0x1F1036, 0x2A1647, 0x3D2263, 0x2F1A50, 0xF6EBFF, 0xB99CD8, 0x7B5F9B, 0xFF3EA5, 0xFFFFFF],
        Space,
        false,
        false,
    ),
    theme(
        "High contrast",
        "Pure black, white text and bright yellow, as clear as possible.",
        [0x000000, 0x000000, 0x0A0A0A, 0x141414, 0x7A7A7A, 0x262626, 0xFFFFFF, 0xE0E0E0, 0xB0B0B0, 0xFFD400, 0x000000],
        Plex,
        false,
        true,
    ),
];

/// An accent to pick. `None` is the theme's own.
pub const ACCENTS: [(&str, Option<u32>); 7] = [
    ("Theme's own", None),
    ("Violet", Some(0x8B7CF6)),
    ("Blue", Some(0x4FB3FF)),
    ("Mint", Some(0x3DDC97)),
    ("Amber", Some(0xF2A03D)),
    ("Coral", Some(0xF2766B)),
    ("Pink", Some(0xF28CB1)),
];

/// The look in use: a theme with the accent picked on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub theme: Theme,
    pub accent: u32,
    pub accent_text: u32,
}

impl Look {
    /// The look from the two settings as the C# app saves them: the theme's name and the accent as "#RRGGBB" (empty
    /// for the theme's own). Anything unknown falls back to Graphite and the theme's own accent.
    pub fn from_settings(theme: Option<&str>, accent: Option<&str>) -> Look {
        let theme = theme.and_then(theme_named).unwrap_or(THEMES[0]);
        match accent.and_then(parse_hex) {
            Some(a) => Look { theme, accent: a, accent_text: if is_light(a) { 0x14110D } else { 0xFFFFFF } },
            None => Look { theme, accent: theme.accent, accent_text: theme.accent_text },
        }
    }
}

pub fn theme_named(name: &str) -> Option<Theme> {
    THEMES.iter().copied().find(|t| t.name == name)
}

/// "#RRGGBB" (or "#AARRGGBB", alpha ignored) to 0xRRGGBB.
pub fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    if !(h.len() == 6 || h.len() == 8) {
        return None;
    }
    u32::from_str_radix(&h[h.len() - 6..], 16).ok()
}

/// Whether a colour is light enough that white text on it is hard to read (same sum as the C# app).
pub fn is_light(rgb: u32) -> bool {
    let (r, g, b) = ((rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255);
    0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64 > 150.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphite_is_the_default() {
        let look = Look::from_settings(None, None);
        assert_eq!(look.theme.name, "Graphite");
        assert_eq!(look.accent, 0x3DDC97);
        assert_eq!(Look::from_settings(Some("No such theme"), Some("junk")).theme.name, "Graphite");
    }

    #[test]
    fn a_picked_accent_wins_with_readable_text() {
        let amber = Look::from_settings(Some("Midnight"), Some("#F2A03D"));
        assert_eq!(amber.theme.name, "Midnight");
        assert_eq!(amber.accent, 0xF2A03D);
        assert_eq!(amber.accent_text, 0x14110D);
        let violet = Look::from_settings(None, Some("#8B7CF6"));
        assert_eq!(violet.accent_text, 0xFFFFFF);
        assert_eq!(Look::from_settings(None, Some("")).accent, 0x3DDC97);
    }

    #[test]
    fn theme_names_are_unique() {
        for (i, t) in THEMES.iter().enumerate() {
            assert!(THEMES.iter().skip(i + 1).all(|u| u.name != t.name));
        }
    }
}
