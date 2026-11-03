//! The app's links to the DAWs (spec section 3.8). For now that's the Bitwig link: the app talks to the Music
//! Organiser extension (or its fallback script) running inside Bitwig Studio, and Send to Bitwig puts a sound on a
//! Sampler, or a kit on a Drum Machine, in the project open there. The FL Studio link, watching both DAWs, the checks
//! and the live tests come later in phase 5.
//!
//! Every call here waits for Bitwig (up to half a minute for a change), so the screens make them on a background
//! thread (`mo_app::background::spawn`), never on the window's thread.

pub mod bitwig;
