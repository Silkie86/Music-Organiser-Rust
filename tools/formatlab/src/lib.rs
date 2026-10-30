//! Format Lab: looks inside Bitwig and FL Studio files to learn how each DAW saves things. It shows everything in a
//! file named where the field registry knows the name ([`ops::tree`]), every value that differs between two saves
//! ([`ops::diff`]), everywhere a file holds a value ([`ops::find`]), and, across many projects, what the registry
//! doesn't know yet ([`census::Census`]). It only reads files, and the census keeps only ids and counts.
//!
//! A port of the C# app's Format Lab (`Core/Formats`). MO Probe and the live tests come with the DAW links (phase 5).

pub mod census;
pub mod command;
pub mod lab;
pub mod ops;
pub mod registry;

mod bitwig_lab;
mod fl_lab;
