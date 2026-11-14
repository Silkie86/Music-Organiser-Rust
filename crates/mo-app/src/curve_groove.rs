//! Swing from the song: Bitwig's Groove (its swing for the whole song, as the extension's watch feed gives it) as a
//! swing the curve editor can give a curve, so a curve lines up with notes the Groove moves. For the plugin's
//! Curves tab (the plan's step 8: "swing read from Bitwig's Groove through the extension").
//!
//! How Bitwig's shuffle amount maps to how far a note moves is a guess, not measured in a Bitwig: its 100 % is taken
//! as triplet swing (every second note a third of a step late), and less in a straight line down to none.

use crate::curve_paint::swing_words;
use mo_dawlink::bitwig::watch::State;
use mo_dawlink::bitwig::{GROOVE_NEEDS, Groove, GrooveRate, Live};

/// The least shuffle that counts as swing: below it there's nothing to hear.
const LEAST: f64 = 0.005;

/// A swing to give a curve: every second line of a grid `step` beats apart moves by `amount` of a step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swing {
    pub step: f64,
    pub amount: f64,
}

/// How far a Groove's shuffle (0 to 1) moves every second note, of a step: its 100 % as triplet swing.
pub fn amount(shuffle: f64) -> f64 {
    if !shuffle.is_finite() {
        return 0.0;
    }
    shuffle.clamp(0.0, 1.0) / 3.0
}

/// The swing of a Groove: none while it's off or its shuffle is at nothing.
pub fn swing(groove: Groove) -> Option<Swing> {
    let step = match groove.rate {
        GrooveRate::Eighth => 0.5,
        GrooveRate::Sixteenth => 0.25,
    };
    (groove.on && groove.shuffle >= LEAST).then_some(Swing { step, amount: amount(groove.shuffle) })
}

/// The song's swing as Bitwig shows it now, with words for the button's tip; or why there's none to give, in words
/// for the same place.
pub fn of(live: &Live) -> Result<(Swing, String), String> {
    match &live.state {
        State::Watching => {}
        State::TooOld(answering) => {
            return Err(format!(
                "Bitwig answers as {answering}: the song's swing needs extension {GROOVE_NEEDS} or newer. Install it \
                 again from the app's Settings > DAWs."
            ));
        }
        State::Refused(why) => return Err(format!("Bitwig wouldn't say the song's swing: {why}")),
        _ => {
            return Err(
                "Waiting for Bitwig's Music Organiser extension to answer: the song's swing comes through it.".into()
            );
        }
    }
    if !live.sections.iter().any(|s| s == "groove") {
        return Err(format!(
            "The song's swing needs extension {GROOVE_NEEDS} or newer: install it again from the app's Settings > DAWs."
        ));
    }
    let Some(groove) = live.groove else {
        return Err(
            "Bitwig hasn't said its Groove yet, or its shuffle rate is one the app doesn't know: it knows 1/8 \
                    and 1/16."
                .into(),
        );
    };
    let rate = match groove.rate {
        GrooveRate::Eighth => "1/8",
        GrooveRate::Sixteenth => "1/16",
    };
    match swing(groove) {
        Some(s) => Ok((
            s,
            format!(
                "Swing the curve as the song's Groove in Bitwig: every second {rate} moves, {} (Bitwig's shuffle \
                 {:.0}%). The highlighted part, or the whole curve; Undo takes it back.",
                swing_words(s.amount),
                groove.shuffle * 100.0
            ),
        )),
        None if !groove.on => {
            Err("The Groove is off in Bitwig: the song plays straight. Turn it on to use its swing.".into())
        }
        None => Err("The Groove's shuffle is at 0 in Bitwig: there's no swing to give.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(groove: Option<Groove>) -> Live {
        Live { state: State::Watching, sections: vec!["transport".into(), "groove".into()], groove, ..Live::default() }
    }

    #[test]
    fn a_groove_thats_on_gives_its_swing_on_its_rates_grid() {
        let g = Groove { on: true, shuffle: 1.0, rate: GrooveRate::Sixteenth };
        let (s, says) = of(&live(Some(g))).expect("on, with shuffle");
        assert!((s.step - 0.25).abs() < 1e-12 && (s.amount - 1.0 / 3.0).abs() < 1e-12, "100 % is triplet swing");
        assert!(says.contains("1/16") && says.contains("triplet") && says.contains("100%"), "{says}");
        let half = Groove { on: true, shuffle: 0.5, rate: GrooveRate::Eighth };
        let (s, says) = of(&live(Some(half))).expect("on, with shuffle");
        assert!((s.step - 0.5).abs() < 1e-12 && (s.amount - 1.0 / 6.0).abs() < 1e-12);
        assert!(says.contains("1/8") && says.contains("swing 58%"), "{says}");
        // Whatever comes: never past triplet swing, never early.
        assert!((amount(7.0) - 1.0 / 3.0).abs() < 1e-12 && amount(f64::NAN) == 0.0 && amount(-1.0) == 0.0);
    }

    #[test]
    fn no_swing_says_why() {
        let off = Groove { on: false, shuffle: 0.5, rate: GrooveRate::Sixteenth };
        assert!(of(&live(Some(off))).unwrap_err().contains("off in Bitwig"));
        let none = Groove { on: true, shuffle: 0.0, rate: GrooveRate::Sixteenth };
        assert!(of(&live(Some(none))).unwrap_err().contains("shuffle is at 0"));
        assert!(of(&live(None)).unwrap_err().contains("1/8 and 1/16"));
        // An older extension's feed has no Groove in it.
        let old = Live { state: State::Watching, sections: vec!["transport".into()], ..Live::default() };
        assert!(of(&old).unwrap_err().contains(GROOVE_NEEDS));
        let older = Live { state: State::TooOld("the Music Organiser extension 3.20".into()), ..Live::default() };
        let why = of(&older).unwrap_err();
        assert!(why.contains("3.20") && why.contains(GROOVE_NEEDS), "{why}");
        assert!(of(&Live::default()).unwrap_err().contains("Waiting for Bitwig"));
        assert!(of(&Live { state: State::NoBitwig, ..Live::default() }).unwrap_err().contains("Waiting for Bitwig"));
    }
}
