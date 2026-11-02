# What the chord bench found about the C# app (2026-11-01)

Measured with `theorybench chords` after PR #108 (charts played by `convert/voice.py`). The full numbers are in the
repo's `tools/theorybench/README.md` ("The C# baseline on charts played as music"); the score files and the commands
that made them are in `harmony/bench/chords-played/` (`run.sh`).

How the C# app is scored: by the name it shows, read back with `Chord::parse`. A name's plain fifth, the 9 under an
11 or 13, and the 11 of a minor 13 count only where the label has them. 7alt is never a right name (it can only be
right as an event).

## The headline

| Set | notes+bass | name | extensions (macro F1) |
|---|---|---|---|
| Billboard as block chords | 94.2% | 90.8% | 72.8% |
| Billboard played, set A | 77.6% | 71.7% | 33.6% |
| Billboard played, set B (measuring only) | 67.3% | 62.9% | 26.2% |
| The sweep, the chords swept | 59.4% | 53.6% | 57.3% |

## What the new reader has to do better (each is a target in the design's hand cases)

1. **Passing notes named as tensions.** Played like music, only 12% of the C# app's 11ths and 14% of its 13ths are in
   the chord. They are the tune's neighbour and passing notes and the walking bass's passing and approach notes. The
   design's gate (only voiced or held notes enter a name, never figuration) is aimed straight at this.
2. **Walking bass (38.9%) and pedals (42.9%).** Under a pedal it takes the pedal as the root; over a tonic pedal it
   never names the V. The design's held/moving bass split and its pedal rule are aimed at these.
3. **Inversions read in root position** 64% to 78% of their time.
4. **#11 written as b5** when no fifth sounds (C13(b5) for C13(#11)). mo-harmony's own namer does the same today, so
   this needs Solomon's answer to open question 2 before the reader can do better.
5. **7alt** hides which alterations are played (open question 4).
6. **Cm(add9) read as Ebmaj13/C**: the right notes from the wrong root. A held bass should settle it (B1 to B6).
7. **Power chords: its changes come a bar late**, so C5 then F5 reads as Csus4.

## The C# numbers the merge rule compares with

The rule itself is in the design ("Bench changes and the merge rule"); it is judged once, on fold 0. These are the
C# app's numbers on all of each set, for scale:

- **Must rise (set B):** notes+bass per chord 67.0%, extensions macro F1 26.2%, and the same-notes root.
- **Must not be worse (POP909-CL, piano parts):** mir_eval root 88.0%, majmin+bass 85.3% and bass 92.8%; sevenths
  82.4%. (The design wrote 80.5 for majmin+bass, from before the bench's fixes in #107; 85.3 is the number now.)
- **Set B, also not worse:** altered tones pooled F1 32.2%; bass kind; a walking bass's false-slash rate.
