//! The places on the left-hand rail. Agreed with Solomon on 2026-10-29: Songs, Sounds, Playground, Plugins and Space,
//! with Settings at the bottom. Ctrl+K jumps to anything from anywhere, so the rail stays short.

/// The `start_page` setting's value that means "open on the page I last used".
pub const LAST_PAGE_CHOICE: &str = "Last page I used";

/// One place on the rail, in the order it's shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    Songs,
    Sounds,
    Playground,
    Plugins,
    Space,
    Settings,
}

impl Place {
    /// Every place, top to bottom. Settings sits apart at the bottom of the rail.
    pub const ALL: [Place; 6] =
        [Place::Songs, Place::Sounds, Place::Playground, Place::Plugins, Place::Space, Place::Settings];

    pub fn name(self) -> &'static str {
        match self {
            Place::Songs => "Songs",
            Place::Sounds => "Sounds",
            Place::Playground => "Playground",
            Place::Plugins => "Plugins",
            Place::Space => "Space",
            Place::Settings => "Settings",
        }
    }

    /// The id of the shortcut that opens this place (see `shortcuts`).
    pub fn shortcut_id(self) -> &'static str {
        match self {
            // The C# app's ids, so keys you changed there still work here.
            Place::Songs => "app.goProjects",
            Place::Sounds => "app.goSounds",
            Place::Playground => "app.goPlayground",
            Place::Plugins => "app.goPlugins",
            Place::Space => "app.goSpace",
            Place::Settings => "app.goSettings",
        }
    }

    /// The place's position on the rail, which is also how the screens refer to it.
    pub fn index(self) -> usize {
        Place::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Option<Place> {
        Place::ALL.get(i).copied()
    }

    /// The place the app opens on, from the `start_page` setting and, for "Last page I used", `last_page`. The C#
    /// app wrote these names, so they're read as they are: its Projects and Sound Lab pages open as Songs and Sounds,
    /// and its MIDI page lives in the Playground now.
    pub fn start_place(start_page: Option<&str>, last_page: Option<&str>) -> Place {
        match start_page {
            Some(LAST_PAGE_CHOICE) => Place::from_page_name(last_page.unwrap_or_default()),
            Some(name) => Place::from_page_name(name),
            None => Place::Songs,
        }
    }

    /// A page name as the settings store it (this app's or the C# app's) to the place that shows it now.
    pub fn from_page_name(value: &str) -> Place {
        match value {
            "Sounds" | "Sound Lab" | "Samples" => Place::Sounds,
            "Playground" | "MIDI" => Place::Playground,
            "Plugins" => Place::Plugins,
            "Space" => Place::Space,
            "Settings" => Place::Settings,
            _ => Place::Songs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips() {
        for p in Place::ALL {
            assert_eq!(Place::from_index(p.index()), Some(p));
        }
        assert_eq!(Place::from_index(Place::ALL.len()), None);
    }

    #[test]
    fn old_start_pages_open_their_new_home() {
        assert_eq!(Place::start_place(Some("Projects"), None), Place::Songs);
        assert_eq!(Place::start_place(Some("Sound Lab"), None), Place::Sounds);
        assert_eq!(Place::start_place(Some("Samples"), None), Place::Sounds);
        assert_eq!(Place::start_place(Some("MIDI"), None), Place::Playground);
        assert_eq!(Place::start_place(None, None), Place::Songs);
    }

    #[test]
    fn last_page_choice_opens_the_last_page() {
        assert_eq!(Place::start_place(Some(LAST_PAGE_CHOICE), Some("Plugins")), Place::Plugins);
        assert_eq!(Place::start_place(Some(LAST_PAGE_CHOICE), None), Place::Songs);
    }
}
