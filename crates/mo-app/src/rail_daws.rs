//! The rail's "In your DAWs" (spec 3.1): what Bitwig has open now, from a link to it kept open and asked again every
//! few seconds, on a low-priority thread of its own (mo-ui `daws`), never the window's (rule 12). The rail said
//! "Nothing open" whatever was open (the whole-app review of 6 Nov, X3-9). FL Studio gets its line once its link is
//! built (phase 5); until then the rail says nothing about it rather than something untrue.
//!
//! As the C# app's watch did, a Bitwig that stops answering for a moment (opening a big project, say) keeps its line
//! for a few asks before the rail says so, and a link that answers "no" stays open: only a link that closed, broke or
//! ran out of time is let go. A link part of whose extension didn't start can't answer `hello` until Bitwig starts it
//! again (which closes the connection), and each "no" goes in the link's report in Bitwig, so it's asked once a minute;
//! so is a link that greeted the app as one working fully and then said no.

use std::time::Duration;

use mo_dawlink::bitwig::{Bitwig, Error, Hello, Link};

/// How long the watch waits between asks.
pub const EVERY: Duration = Duration::from_secs(5);
/// How long it waits while the link can't say what's open (part of the extension didn't start), and after a bug in
/// an ask.
pub const SLOW: Duration = Duration::from_secs(60);
/// A Bitwig that takes the call but doesn't answer keeps its line until it has missed this many asks in a row.
pub const MISSES: u32 = 3;

/// Where each tooltip points for more.
const MORE: &str = "Settings \u{203a} DAWs says more about the link.";

/// One line under the rail's "In your DAWs": the DAW, what it has open (or that it isn't linked), whether its link
/// answers, and the tooltip.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RailDaw {
    pub daw: &'static str,
    pub text: String,
    pub linked: bool,
    pub tip: String,
    /// The project open there, by the name the DAW shows ("" for none, or a new one not saved yet): a click shows the
    /// song on Songs when it's one of yours.
    pub project: String,
}

/// Bitwig's line in the rail, from what the link said last ([`Link::refresh`]): the project open there, by the name
/// Bitwig shows. Only reads, as `settings::bitwig_status` does.
pub fn bitwig_rail(checked: &Result<Hello, Error>) -> RailDaw {
    const AGAIN: &str = "The app asks again by itself every few seconds.";
    match checked {
        Ok(hello) if hello.error.is_none() && hello.project.is_empty() => {
            line("A new project", true, format!("A new project, not saved yet, is open in Bitwig now. {MORE}"))
        }
        Ok(hello) if hello.error.is_none() => {
            let unsaved = if hello.modified { ", with changes not saved yet" } else { "" };
            RailDaw {
                project: hello.project.clone(),
                ..line(
                    &hello.project,
                    true,
                    format!(
                        "Open in Bitwig now{unsaved}. A click shows it on Songs, or Settings \u{203a} DAWs when it \
                         isn't one of your songs."
                    ),
                )
            }
        }
        Ok(hello) => line(
            "Linked, not working fully",
            false,
            format!(
                "Part of the Music Organiser extension didn't start in Bitwig, so the app can't see what's open there \
                 or send anything to it. Restarting Bitwig may help. {} {MORE}",
                said(hello.error.as_deref().unwrap_or_default())
            ),
        ),
        Err(Error::NotRunning) => line(
            "Not linked",
            false,
            "Bitwig isn't answering: it's closed, or the Music Organiser controller isn't on in it. Settings \u{203a} \
             DAWs says how to link it."
                .to_owned(),
        ),
        Err(Error::TimedOut) => line(
            "Not answering",
            false,
            format!(
                "Bitwig hasn't answered the app for a while: it may be busy (opening a big project, say). {AGAIN} \
                 {MORE}"
            ),
        ),
        Err(Error::NotTheLink { port }) => line(
            "Not answering",
            false,
            format!(
                "Something took the app's call on port {port} but didn't answer as the Music Organiser extension does: \
                 Bitwig may be busy (opening a big project, say), or another program uses the port. {AGAIN} {MORE}"
            ),
        ),
        Err(e) => line("Not linked", false, format!("{e} {MORE}")),
    }
}

/// Bitwig's line when its link, open and greeted as one working fully, then said no to what's open, or answered in a
/// way the app doesn't know (`why`). It's asked again once a minute ([`SLOW`]).
fn refused_rail(why: &Error) -> RailDaw {
    let why = match why {
        Error::Bitwig(words) => said(words),
        other => other.to_string(),
    };
    line(
        "Linked, not working fully",
        false,
        format!(
            "Bitwig's link answered the app, but didn't say what's open there. {why} The app asks again in a minute. \
             {MORE}"
        ),
    )
}

/// One line under the rail's "In your DAWs", for Bitwig.
fn line(text: &str, linked: bool, tip: String) -> RailDaw {
    RailDaw { daw: "Bitwig", text: text.to_owned(), linked, tip, ..RailDaw::default() }
}

/// "Bitwig said: …", ending in a full stop; words that already start with Bitwig (the link's own "Bitwig said no." when
/// Bitwig gave none) as they are.
fn said(words: &str) -> String {
    let words = words.trim();
    let words = if words.ends_with(['.', '!', '?']) { words.to_owned() } else { format!("{words}.") };
    if words.starts_with("Bitwig ") { words } else { format!("Bitwig said: {words}") }
}

/// Keeps up with what Bitwig has open, for the rail, one ask at a time ([`BitwigWatch::ask`]).
pub struct BitwigWatch {
    /// How to reach Bitwig (its port, and how long to wait): for a new link; one kept keeps its own.
    pub bitwig: Bitwig,
    /// The link, kept open between asks: none before the first, and once it closed, broke or ran out of time.
    link: Option<Link>,
    /// Asks in a row that Bitwig took but didn't answer.
    missed: u32,
    /// The line the rail shows now.
    shown: Option<RailDaw>,
}

/// What an ask found: the rail's new line, when it changed, and how long to wait before the next ask.
#[derive(Debug)]
pub struct Asked {
    pub line: Option<RailDaw>,
    pub wait: Duration,
}

/// What the link said, or why it didn't.
enum Answer {
    Said(Hello),
    /// The link is still open, but said no to what's open, or answered in a way the app doesn't know: why.
    Refused(Error),
    /// Bitwig took the call but didn't answer in time: busy, perhaps.
    Busy(Error),
    NotLinked(Error),
}

impl BitwigWatch {
    pub fn new(bitwig: Bitwig) -> BitwigWatch {
        BitwigWatch { bitwig, link: None, missed: 0, shown: None }
    }

    /// Asks Bitwig what it has open: on the kept link, or a new one when there's none. Waits as long as the link's own
    /// time limits at most (a few seconds), so never on the window's thread.
    pub fn ask(&mut self) -> Asked {
        let (line, wait) = match self.answer() {
            Answer::Said(hello) => {
                self.missed = 0;
                let wait = if hello.error.is_some() { SLOW } else { EVERY };
                (bitwig_rail(&Ok(hello)), wait)
            }
            // Each "no" goes in the link's report in Bitwig, so it's asked once a minute.
            Answer::Refused(why) => {
                self.missed = 0;
                (refused_rail(&why), SLOW)
            }
            Answer::Busy(why) => {
                self.missed += 1;
                // Busy for a moment: the line stays as it is (unless there's none yet).
                if self.missed < MISSES && self.shown.is_some() {
                    return Asked { line: None, wait: EVERY };
                }
                (bitwig_rail(&Err(why)), EVERY)
            }
            Answer::NotLinked(why) => {
                self.missed = 0;
                (bitwig_rail(&Err(why)), EVERY)
            }
        };
        let changed = self.shown.as_ref() != Some(&line);
        if changed {
            self.shown = Some(line.clone());
        }
        Asked { line: changed.then_some(line), wait }
    }

    fn answer(&mut self) -> Answer {
        if let Some(link) = self.link.as_mut().filter(|l| l.is_open()) {
            match link.refresh().cloned() {
                Ok(hello) => return Answer::Said(hello),
                // A "no", or an answer not laid out as the link's: the link is still open, so it's kept. One that
                // greeted the app with what didn't start, and hasn't said what's open since (it says no each time),
                // keeps the line its greeting gave.
                Err(Error::Bitwig(_) | Error::Unexpected) if link.hello().error.is_some() => {
                    return Answer::Said(link.hello().clone());
                }
                Err(why @ (Error::Bitwig(_) | Error::Unexpected)) => return Answer::Refused(why),
                // Out of time, and the link closed the connection: Bitwig may be busy, so it connects afresh next
                // time, not now (a new connection's greeting gets less time, so it would read as something else
                // answering).
                Err(why @ Error::TimedOut) => {
                    self.link = None;
                    return Answer::Busy(why);
                }
                // Bitwig closed it, or it broke: connect afresh now, to see whether Bitwig is still there.
                Err(_) => {}
            }
        }
        self.link = None;
        match self.bitwig.connect() {
            Ok(link) => {
                let hello = link.hello().clone();
                self.link = Some(link);
                Answer::Said(hello)
            }
            // Something took the call but didn't greet the app in time: Bitwig busy, most likely.
            Err(why @ Error::NotTheLink { .. }) => Answer::Busy(why),
            Err(why) => Answer::NotLinked(why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_says_what_bitwig_has_open() {
        let hello = Hello { link: 3, project: "Night drive".into(), changes: true, ..Hello::default() };
        let open = bitwig_rail(&Ok(hello.clone()));
        assert_eq!((open.daw, open.text.as_str(), open.linked), ("Bitwig", "Night drive", true));
        assert_eq!(open.project, "Night drive");
        assert!(open.tip.starts_with("Open in Bitwig now. A click shows it on Songs"), "{}", open.tip);
        let unsaved = bitwig_rail(&Ok(Hello { modified: true, ..hello.clone() }));
        assert!(unsaved.tip.starts_with("Open in Bitwig now, with changes not saved yet."), "{}", unsaved.tip);
        // A new project has nothing to show on Songs.
        let new = bitwig_rail(&Ok(Hello { modified: true, project: String::new(), ..hello.clone() }));
        assert_eq!((new.text.as_str(), new.linked, new.project.as_str()), ("A new project", true, ""));
        assert!(new.tip.starts_with("A new project, not saved yet, is open in Bitwig now."), "{}", new.tip);
        // Linked but not sending is still what's open; part of the link not started isn't, and the rail sends nothing.
        assert!(bitwig_rail(&Ok(Hello { changes: false, ..hello.clone() })).linked);
        let broken = bitwig_rail(&Ok(Hello { error: Some("no MIDI".into()), ..hello }));
        assert_eq!(
            (broken.text.as_str(), broken.linked, broken.project.as_str()),
            ("Linked, not working fully", false, "")
        );
        assert!(broken.tip.contains("Bitwig said: no MIDI."), "{}", broken.tip);
        assert!(!broken.tip.contains("didn't send"), "{}", broken.tip);
        let off = bitwig_rail(&Err(Error::NotRunning));
        assert_eq!((off.text.as_str(), off.linked), ("Not linked", false));
        assert!(off.tip.contains("closed, or the Music Organiser controller isn't on"), "{}", off.tip);
        assert_eq!(bitwig_rail(&Err(Error::Closed)).text, "Not linked");
        // Busy: the rail asks again by itself, so it doesn't say to try again.
        for busy in [Error::TimedOut, Error::NotTheLink { port: 19791 }] {
            let line = bitwig_rail(&Err(busy));
            assert_eq!((line.text.as_str(), line.linked), ("Not answering", false));
            assert!(line.tip.contains("asks again by itself"), "{}", line.tip);
            assert!(!line.tip.contains("try again"), "{}", line.tip);
        }
    }
}
