//! Converting a song from one DAW to another (phase 5). FL Studio to Bitwig comes first, ported from the C# app's
//! `BitwigProject` and `DawProject` exports.
//!
//! The work is in two steps, so each can be tested on its own:
//! 1. [`plan`]: what the song becomes, in no DAW's terms: the tracks, what's on each one, its clips, its automation,
//!    and a note for everything that can't come across. Every choice about the song is made here.
//! 2. A writer ([`bitwig`] for Bitwig) that only fills a copy of a template the DAW saved with that plan.
//!
//! Nothing here changes the song it reads. A writer only ever writes a new file (rule 3).

pub mod bitwig;
pub mod clips;
pub mod fl_clones;
pub mod lost;
pub mod notes;
pub mod plan;
pub mod relink;
pub mod sample;
pub mod save;

pub use plan::{
    AudioClip, Device, Lane, LanePart, LanePoint, Options, Plan, Role, SampleFile, Target, Track, TrackKind, TrackRef,
    from_fl,
};
