//! The app's name and the words around it, from `product.toml` at the top of the repo, the one place they're kept
//! (DAWmate, since 15 Nov 2026; "Music Organiser" was the working title). Anything that shows the app's name to a
//! person takes it from here; names that other programs or folders know the app by (the data folder, the DAW links,
//! the MIDI port) don't.

/// `product.toml`'s name as a literal, for `concat!` in this crate (the build variable is named only here).
macro_rules! name_literal {
    () => {
        env!("MO_PRODUCT_NAME")
    };
}
pub(crate) use name_literal;

/// What people see: the title bar, About, the logs, the installer.
pub const NAME: &str = name_literal!();

/// One line on what the app is.
pub const DESCRIPTION: &str = env!("MO_PRODUCT_DESCRIPTION");

/// Who makes it.
pub const PUBLISHER: &str = env!("MO_PRODUCT_PUBLISHER");

/// The app's page.
pub const WEBSITE: &str = env!("MO_PRODUCT_WEBSITE");

/// The background helper's name, for its tray icon and its log.
pub const HELPER_NAME: &str = concat!(name_literal!(), " helper");
