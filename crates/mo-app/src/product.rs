//! The app's name and the words around it, from `product.toml` at the top of the repo, the one place they're kept
//! (DAWmate, since 15 Nov 2026; "Music Organiser" was the working title). Anything that shows the app's name to a
//! person takes it from here; names that other programs or folders know the app by (the data folder, the DAW links,
//! the MIDI port) don't.

/// What people see: the title bar, About, the logs, the installer.
pub const NAME: &str = env!("MO_PRODUCT_NAME");

/// One line on what the app is.
pub const DESCRIPTION: &str = env!("MO_PRODUCT_DESCRIPTION");

/// Who makes it.
pub const PUBLISHER: &str = env!("MO_PRODUCT_PUBLISHER");

/// The app's page.
pub const WEBSITE: &str = env!("MO_PRODUCT_WEBSITE");

/// The background helper's name, for its tray icon and its log.
pub fn helper_name() -> String {
    format!("{NAME} helper")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_words_are_there() {
        for words in [NAME, DESCRIPTION, PUBLISHER, WEBSITE] {
            assert!(!words.trim().is_empty());
        }
        assert!(WEBSITE.starts_with("https://"), "{WEBSITE}");
        assert_eq!(helper_name(), format!("{NAME} helper"));
    }
}
