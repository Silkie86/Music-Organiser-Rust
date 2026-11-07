//! The rail's "In your DAWs" watch, against the stand-in Bitwig (`mo_dawlink::bitwig::fake`): no test reaches a real
//! Bitwig, and none uses the link's own port (rule 11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::time::Duration;

use mo_app::rail_daws::{BitwigWatch, EVERY, SLOW};
use mo_dawlink::bitwig::Bitwig;
use mo_dawlink::bitwig::fake::{FakeBitwig, OnConnect};

/// A short wait, where nothing will answer: a Bitwig that doesn't is found out quickly. Where the stand-in answers, the
/// watch waits as long as the stand-in's own link does (2 seconds), so a busy computer running the tests doesn't read
/// as a busy Bitwig.
const SHORT: Duration = Duration::from_millis(300);

fn watch(bitwig: &FakeBitwig) -> BitwigWatch {
    BitwigWatch::new(bitwig.bitwig())
}

/// From now on the watch waits [`SHORT`] for a new link (one it keeps keeps its own wait).
fn hurry(watch: &mut BitwigWatch) {
    watch.bitwig.connect_timeout = SHORT;
    watch.bitwig.timeout = SHORT;
}

/// The words an ask changed the rail's line to, or none when it left the line as it was.
fn asked(watch: &mut BitwigWatch) -> Option<String> {
    watch.ask().line.map(|l| l.text)
}

#[test]
fn the_rail_follows_what_bitwig_has_open_on_one_link() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut watch = watch(&bitwig);
    let first = watch.ask();
    let line = first.line.unwrap();
    assert_eq!((line.text.as_str(), line.linked, line.project.as_str()), ("Night Drive", true, "Night Drive"));
    assert_eq!(first.wait, EVERY);
    // Nothing changed: nothing to show.
    assert_eq!(asked(&mut watch), None);
    bitwig.world().project = "Other Song".into();
    assert_eq!(asked(&mut watch).as_deref(), Some("Other Song"));
    // The link was kept and asked again: connecting sends nothing, and each ask after the first is one hello.
    assert_eq!(bitwig.world().commands(), ["hello", "hello"]);
}

#[test]
fn a_busy_bitwig_keeps_its_line_for_a_few_asks() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut watch = watch(&bitwig);
    assert_eq!(asked(&mut watch).as_deref(), Some("Night Drive"));
    // Bitwig stops answering (opening a big project, say). A new call would be hung up on, so a watch that called
    // again in the same ask would say "Not linked" now.
    {
        let mut world = bitwig.world();
        world.silent = vec!["hello".into()];
        world.on_connect = OnConnect::HangUp;
    }
    let (wait, asks_at) = (watch.bitwig, std::time::Instant::now());
    hurry(&mut watch);
    let missed = watch.ask();
    // The link it kept waited as long as it was made to (the stand-in's 2 seconds), not the shorter wait set since.
    assert!(asks_at.elapsed() >= wait.timeout, "{:?}", asks_at.elapsed());
    assert_eq!((missed.line, missed.wait), (None, EVERY));
    // The next call isn't greeted either: still busy, still the same line.
    bitwig.world().on_connect = OnConnect::Nothing;
    assert_eq!(asked(&mut watch), None);
    // The third ask in a row it misses: the rail says so.
    assert_eq!(asked(&mut watch).as_deref(), Some("Not answering"));
    assert_eq!(asked(&mut watch), None);
    // Bitwig answers again: back to what it has open.
    {
        let mut world = bitwig.world();
        world.silent.clear();
        world.on_connect = OnConnect::Greet;
    }
    watch.bitwig = wait;
    assert_eq!(asked(&mut watch).as_deref(), Some("Night Drive"));
}

#[test]
fn a_bitwig_busy_from_the_start_says_so_at_once() {
    let bitwig = FakeBitwig::start().unwrap();
    bitwig.world().on_connect = OnConnect::Nothing;
    let mut watch = watch(&bitwig);
    hurry(&mut watch);
    assert_eq!(asked(&mut watch).as_deref(), Some("Not answering"));
}

#[test]
fn a_link_that_says_no_is_kept_and_asked_once_a_minute() {
    let bitwig = FakeBitwig::start().unwrap();
    // Part of the extension didn't start: it greets with what it knows, and says no to hello.
    bitwig.world().hello.insert("error".into(), "No MIDI ports".into());
    let mut watch = watch(&bitwig);
    let first = watch.ask();
    let line = first.line.unwrap();
    assert_eq!((line.text.as_str(), line.linked), ("Linked, not working fully", false));
    assert!(line.tip.contains("Bitwig said: No MIDI ports"), "{}", line.tip);
    assert_eq!(first.wait, SLOW);
    // A new call would be hung up on: the link it has is kept, and the line stays.
    bitwig.world().on_connect = OnConnect::HangUp;
    let again = watch.ask();
    assert_eq!((again.line, again.wait), (None, SLOW));
    assert_eq!(bitwig.world().commands(), ["hello"]);
}

#[test]
fn a_link_that_says_no_later_says_so_in_its_own_words_and_is_kept() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut watch = watch(&bitwig);
    assert_eq!(asked(&mut watch).as_deref(), Some("Night Drive"));
    // The link greeted the app as one working fully, then says no to what's open (in words without a full stop).
    bitwig.world().hello.insert("error".into(), "Something went wrong".into());
    let refused = watch.ask();
    let line = refused.line.unwrap();
    assert_eq!((line.text.as_str(), line.linked, line.project.as_str()), ("Linked, not working fully", false, ""));
    assert!(line.tip.contains("didn't say what's open there. Bitwig said: Something went wrong."), "{}", line.tip);
    // Not what a link part of which didn't start is told: this one started.
    assert!(!line.tip.contains("didn't start"), "{}", line.tip);
    assert_eq!(refused.wait, SLOW);
    // A new call would be hung up on: the link it has is kept, and the line stays.
    bitwig.world().on_connect = OnConnect::HangUp;
    let again = watch.ask();
    assert_eq!((again.line, again.wait), (None, SLOW));
    assert_eq!(bitwig.world().commands(), ["hello", "hello"]);
    // A "no" without words of Bitwig's own isn't said twice.
    bitwig.world().hello.insert("error".into(), "".into());
    let line = watch.ask().line.unwrap();
    assert!(line.tip.contains("what's open there. Bitwig said no. The app"), "{}", line.tip);
}

#[test]
fn a_link_bitwig_closed_is_called_again_at_once() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut watch = watch(&bitwig);
    assert_eq!(asked(&mut watch).as_deref(), Some("Night Drive"));
    // Bitwig closes the link (it started the extension again, after an install, say), with another project open.
    {
        let mut world = bitwig.world();
        world.hang_up_on = vec!["hello".into()];
        world.project = "Other Song".into();
    }
    assert_eq!(asked(&mut watch).as_deref(), Some("Other Song"));
}

#[test]
fn nothing_listening_says_not_linked() {
    // A port nothing listens on: one the system had free, let go again. Another test's stand-in may be given it
    // meanwhile, so another is tried then.
    for _ in 0..10 {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut watch = BitwigWatch::new(Bitwig { port, connect_timeout: SHORT, timeout: SHORT, ..Bitwig::default() });
        let line = watch.ask().line.unwrap();
        if line.text == "Not linked" {
            assert!(!line.linked && line.project.is_empty());
            assert!(line.tip.contains("it's closed, or the Music Organiser controller isn't on"), "{}", line.tip);
            return;
        }
    }
    panic!("something answered on every port tried");
}
