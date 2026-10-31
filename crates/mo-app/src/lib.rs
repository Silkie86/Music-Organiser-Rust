//! Screen logic and state for Music Organiser, kept free of any UI framework so every rule here can be tested
//! without opening a window. The Slint screens in `mo-ui` only show what this crate decides.

pub mod activity;
pub mod background;
pub mod log;
pub mod note;
pub mod paths;
pub mod places;
pub mod shortcuts;
pub mod song_panel;
pub mod songs;
pub mod themes;
pub mod version;
