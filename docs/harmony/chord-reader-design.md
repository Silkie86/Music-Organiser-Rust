# Chords from notes: mo-harmony part 3, the chord reader (final design)

Effort used: xhigh for the three designs and the merge, high for the three judges. The answers get stored in the MIDI index, it needs file-format-level exactness, and it is Solomon's main complaint about the C# app.

## Summary

The reader works in three steps.

1. **Hear.** This step needs no chord. Notes go onto a metre-aware grid of units in three streams: bass, harmony and tune. Each unit keeps whole-number tick counts as prefix sums. The step also records:
   - whether the bass is held or moving;
   - which line notes are shaped like passing, neighbour, approach or anticipation notes;
   - which chords were pushed an eighth early.
2. **Decode.** A semi-Markov search scores whole stretches of time.
   - A first pass with no context keeps the best 8 labels per stretch.
   - An exact first-order pass then adds a chord-to-chord term.
   - Forward-backward gives every reading a chance that carries any doubt about the boundaries.
   - A label is a root plus one of 16 core chords. The seventh is part of the label because it decides which tensions are legal.
3. **Name.** Labels are turned into what Solomon reads:
   - extensions, from notes that are voiced or held, never from passing notes;
   - the bass kind: root, inversion, slash, moving or none;
   - a seventh heard only in the bass, hybrid 9sus4 names, pedals, and the names of symmetric chords;
   - spelling in the key.

Design 1 is the base, as all three judges ranked it. The grafts and fixes are:

- The bass decides only when it is held. A walking bass leans towards the root, weighted by where each note falls in the bar, and never prints a slash.
- The bass is found in a keys part by gaps between notes only, never by absolute register, so moving the notes moves the answer exactly.
- Rootless readings are always offered. Over a bass they win on the bass. With no bass they are listed and never given as a sure Chord.
- Readings that use the same notes are graded together (SameNotes): C6 or Am7; the four roots of a dim7.
- The weights learn only from POP909-CL (MIT) and from voicing **set A** of a new renderer of the CC0 Billboard charts. Rootless, upper-structure, quartal, arpeggio and two-handed voicings (**set B**) are measured only, so the merge evidence is never the renderer marking its own homework.
- The key reading is used for spelling only.

## Inputs and types (Rust)

The caller maps arp, pad and keys parts to `Role::Harmony`, lead to `Melody`, drums to `Drums` (left out) and anything else to `Unknown` (heard as harmony). `Role` gains no new variant. The Songs page calls `read` once per metre section: `Span::Song` when the song has one section, otherwise `Span::Window` for each section. Chord names found in a file name are checked against the reading, never used as evidence.

```rust
// crates/mo-harmony/src/chordread/{mod,hear,lines,push,cores,candidates,score,decode,name,answer,explain,fact,version,weights,cases}.rs
pub const CHORD_READER_VERSION: u32 = 1;   // first fitted weights; +1 whenever the same notes could read differently

pub struct ChordContext<'a> {
    pub span: Span,                  // keyfind::Span: Loop wraps round; Song; Window
    pub key: Option<&'a KeyReading>, // spelling only, never evidence
    pub pins: &'a [Pin],             // Chord check answers and user fixes: a control, not evidence
    pub readings: u8,                // readings kept per span, 1..=6 (index 2, Chord check 6)
}
pub struct Pin { pub start: i64, pub end: i64, pub chord: Option<Chord> } // None pins only the boundaries

pub struct ChordReader { /* units, prefix sums, lines, lattice; reused, no allocation per read once warm */ }
impl ChordReader {
    pub fn new() -> ChordReader;
    pub fn read(&mut self, parts: &[Part], grid: Grid, cx: &ChordContext) -> Option<ChordReading>; // None: nothing pitched
    pub fn explain(&mut self, parts: &[Part], grid: Grid, cx: &ChordContext,
                   reading: &ChordReading, span: usize) -> Option<Explanation>;
}
pub fn read_chords(parts: &[Part], grid: Grid, cx: &ChordContext) -> Option<ChordReading>;

pub struct ChordReading {
    pub spans: Vec<ChordSpan>, pub texture: Texture, pub unit: Unit,
    pub start: i64, pub end: i64, pub notes: u32, pub truncated: bool,
}
pub enum Texture { Full, Accompaniment, HarmonyOnly, SingleLine } // tune+others; keys+bass; keys only; one line
pub enum Unit { HalfBeat, Beat }                                   // Beat in compound metre

pub struct ChordSpan {
    pub start: i64, pub end: i64,  // ticks, on unit starts
    pub heard_start: i64,          // first onset of a pushed chord (== start when not pushed)
    pub change: f64,               // chance a chord starts at `start` (forward-backward)
    pub answer: ChordAnswer,
    pub readings: Vec<Reading>,    // best first, distinct written names
    pub other: f64,                // chance of none of them
    pub flags: SpanFlags,          // PUSHED, PEDAL, WALKING, NO_BASS, SPLIT
}
pub enum ChordAnswer { Chord, SameNotes, Implied, Unclear, NoChord(NoChord) }
pub enum NoChord { Rest, TooFewNotes }

pub struct Reading {
    pub label: Label,          // what was decoded
    pub chord: Chord,          // what is written: root, named notes, bass (chord.rs names it)
    pub also: Option<Chord>,   // the same notes written the other way: F/G beside G9sus4, D7/C beside D/C
    pub roots: PcSet,          // a dim7's four roots, an augmented triad's three; otherwise the root alone
    pub bass: BassKind,
    pub rootless: bool,        // the root sounds in no part
    pub doubtful: PcSet,       // tensions in the gate's middle band, left out of `chord`
    pub chance: f64,           // calibrated: root, core class and bass kind right
    pub group: u8,             // same-notes group within the span
}
pub enum BassKind { Root, Inversion, Slash, Moving, None }
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Label { pub root: PitchClass, pub core: Core }
#[repr(u8)] pub enum Core { Major, Minor, Dominant7, Major7, Minor7, Major6, Minor6, MinorMajor7,
    HalfDiminished, Diminished7, Diminished, Augmented, Sus4, Sus2, Dominant7Sus4, Power } // one nibble

pub struct Explanation { pub notes: Vec<NoteWhy>, pub terms: Vec<(Term, f64, f64)>, pub bass_from: BassFrom }
pub struct NoteWhy { pub part: usize, pub note: usize, pub does: NoteDoes }
pub enum NoteDoes { ChordTone(Degree), Extension(Degree), Doubtful(Degree), Passing, Neighbour, Approach,
    Walking, Suspension, Anticipation, Appoggiatura, HeldOver, TuneOver, Clash, Bass }
pub enum BassFrom { BassPart, LowestKeys, NoBass }
```

The key finder's ghost-note rule moves into a shared helper, `notes::is_ghost`. A note is a ghost when it is shorter than an eighth of a beat and played at under an eighth of its stream's usual velocity. A test checks that every key reading, and `KEY_FINDER_VERSION`, stay unchanged. The 16-bit velocity is used only by this gate, so MIDI 2.0 resolution is never cut.

## Finding the chord spans

### Hearing (no chord needed)

**Felt beats, units and levels**

- **Simple metre.** A felt beat F is one grid beat. A unit is half a beat: unit k starts at tick `bar_line + ⌊k·beat/2⌋`.
- **Compound metre** (6, 9 or 12 beats a bar, so the beat is an eighth). A unit is one grid beat, and F is 3 units. 6/4 is read as compound (open question 7).
- **Levels and weights μ:**
  - Bar: μ 4.
  - HalfBar: μ 3. This is the middle felt beat when the bar has an even number of them (beat 3 in 4/4, the second dotted quarter in 6/8).
  - Beat: μ 2. Any other felt beat.
  - Off: μ 1.
- Chords change only on unit starts. Swung and triplet off-beats land in the second unit. Nothing assumes 4/4.

**Snap.** An onset within ⌊U/4⌋ ticks before a unit start moves onto it. This absorbs humanised timing.

**Streams**

- **B (bass):** the Bass parts.
  - When the input has no Bass part, the lowest Harmony or Unknown note sounding in a unit joins B for that unit if two things hold: another harmony note sounds in the unit, and the next note up is at least `BASS_GAP` = 7 semitones above it.
  - Only gaps count, never register. This takes shells (D3 under C4), left-hand roots and the Kenny Barron C3–G3. It never takes a B-form bottom (B2 under E3) or a quartal bottom (D3 under G3).
- **M (melody):** the Melody parts.
- **H (harmony):** everything else that is pitched.
- Bass-stream notes never enter a name except as the bass.

**Broken parts.** A Harmony or Unknown part counts as broken when:

- no two of its notes overlap by more than a unit; and
- it has at least 2 onsets per felt beat.

Each note of a broken part then counts as sounding to the end of the felt beat it was struck in (the pedal a player holds). This is detected from the notes, so roles stay unchanged and an arpeggio stays in H.

**Overhang.** The part of a note that rings on past a Bar or HalfBar boundary is "overhang" when:

- the note sounded at least F before the boundary; and
- it ends within F after it.

Overhang is legato and pedal blur. It is left out of every mass except T4, and the explanation calls it HeldOver.

**Masses (integers)**

- a_X(u, p) is the longest overlap, in ticks, of any one note of pitch class p in stream X with unit u. Octave doubles count once.
- Prefix sums over the 36 (stream, pitch class) pairs give any span's m_X(p) in 36 subtractions.
- Per unit, the reader also keeps a mask of notes struck there and a mask of notes carried in (struck earlier, still sounding).

**Presence.** For span s:

- S = the ticks of units in s where some H or B note sounds. Silence inside a span is free.
- b = S / F, the number of felt beats.
- pres(p) = min(1, 4·(m_H + m_B + ½·m_M*)(p) / max(S, 1)), where m_M* counts only tune notes held at least F.
- Both sides are in ticks, so the result is the same at any ppq.
- A pitch class sounding a quarter of the span is fully present. The heard set is P = {p : pres(p) ≥ ½}.

**Bass texture of a span**

- β is the bass pitch class with the most bass ticks.
- **Held:** β has at least 75% of the bass ticks. Octave jumps and repeats count.
- **Moving:** otherwise.
- **None:** no bass ticks.

**Lines and shapes.** The lines are the B line, the top line of each Melody part, the top line of H, and each broken part. Inner voices of block chords are never treated as lines, because they have no separate voices.

Each line note may carry one shape:

- **Passing:** steps of 1–2 semitones in and out, in the same direction.
- **Neighbour:** a step out and back.
- **Approach** (bass only): a semitone or tone into the next onset, which falls on a stronger level.
- **Anticipation:** struck on a weaker level, the same pitch class as the next onset on a stronger level, and lasting until it.
- **Suspension:** carried in, then resolving down by step.
- **Appoggiatura:** a leap in and a step out the other way, on a strong position.

A shaped note must be no longer than its anchors and have a lower μ than at least one of them. The reader stores (note, shape, pc, anchors as a PcSet, ticks).

The half of the test that depends on the chord runs per label y as a bitmask: the note is figuration under y when pc ∉ core(y) and anchors ⊆ tones(y). A note of a moving bass on a Beat or Off unit that is not a core tone of y is **Walking** under y, whatever its interval.

**Pushes.** Take a felt-beat start u. Let E be the H and B notes struck in the unit before u and held at least one unit past u. u is a push target when both hold:

- E has at least 2 pitch classes, making up at least half of the H pitch classes sounding just after u;
- the bass at u is in E, is struck at u, or is absent.

So the common gospel and R&B push, where the keys come an eighth early and the bass lands on the one, is moved. A sixteenth-early push lands in the same unit, so it is covered too.

A push target gives u a second boundary state, in which E counts from u. The push is a scored choice (E8), not a rule. Melody notes never move. A lone early note stays where it is and is judged as an anticipation.

**Loops.** A `Loop` is read with its last bar copied in front, and only its own range is reported. A chord struck in the loop's last unit and held to the loop end counts as held past tick 0, so it can be pushed to the start.

**Reading cap.** At most 16,384 units are read. When the notes lie further apart than that (only damaged files do), the busiest stretch is read, by the key finder's rule, and `truncated` is set.

**As built (hearing PR, part A).** These details differ from the text above or fill in what it left open. Each is tested.

- **Broken parts** are decided per part and felt beat, not for the whole part, so one keys part can play arpeggios in one passage and held chords in another. Notes struck within a quarter unit of each other are one strike. Two tests are added: at least half the strikes are single notes (a part repeating block chords in eighths never overlaps, but is not broken), and the part holds nothing through the felt beat except the pitch class of its lowest note (an oom-pah left hand under a held right-hand chord is not broken; an Alberti figure over a held bass note is).
- **The gap bass** is looked for in every bar where no bass part plays (a bass note struck there, or sounding a quarter of a unit there), not only when the input has no bass part: a song whose bass comes in at the chorus still gets its verse's left-hand roots. It must sound at least a quarter of its unit (a legato tail is never the bass) and must not be a broken note. "The next note up" is the next one of another pitch class: a note with only its own octaves above it has no chord to stand under, so C3 with C4 has no bass, as C3 alone has none; such a note stays the bass only when it was the unit before's, as a held bass does while an arpeggio passes its octave. Its octave doubles in the chords go to B with it, so doubling any notes an octave up changes nothing.
- **Sounding.** A note counts as sounding in a unit, for the masks and for S, only when it sounds at least a quarter of the unit.
- **Loops** run from the bar line at or before the first note to the bar line after where the last note is played. A note played just before the loop's end (within the snap) is heard on the loop's start, as the loop goes round. Only a note's own end makes it wrap: a broken chord's pedal doesn't, so an arpeggio is never a push. A loop longer than the reading cap is read as a song.
- **Broken parts at the end.** In the music's last felt beat, a note held to the end of the music counts as held through the felt beat.

**As built (hearing PR, part B).** Lines, shapes and pushes. Each point is tested.

- **Loops held on.** When a note is held on past the bar line after the last note struck, to within a felt beat of a later bar line, the loop runs to that bar line: a 4-bar loop of two 2-bar chords is 4 bars, not 3. A note ringing on to nowhere near a bar line (a release past the loop's end) is cut off at the bar line after the last onset, as before.
- **Held through.** A note is held through a felt beat when it's struck at its start and sounds to its end, each to within a quarter unit (a chord let go of 10 ticks early is still held), for both broken-part tests.
- **Broken notes** are every note struck in a broken felt beat except one held through it (a bass under the arpeggio, even a quarter-note bass that ends with the beat), not only the notes the pedal lengthens, so all of an arpeggio's notes are on its broken line. A broken note is held at least to the felt beat's end.
- **Lines.** The bass line is the lowest note of each strike of the bass parts and of the gap bass. A tune part's line and the chords' top line take the highest note struck in each strike, unless a higher note struck before still sounds more than a quarter unit into it (an inner voice); a held-over end doesn't count, and a loop's copy of a note struck before its last bar counts as struck before. The chords' top line leaves out broken notes and the bass: the gap bass, and its octaves with no chord note of another pitch class struck below them (a left hand's octave; a C5 on top of a right-hand chord is the chord's top). A chord part's broken line takes the highest broken note of each strike (a bass struck with it isn't one); its other strikes stay on the line as notes the broken notes move from or to, but are shaped only on the chords' top line, so no note has two shapes. Two notes are next to each other when the later starts at most a felt beat after the earlier ends as played; a loop's last note goes on to its first.
- **Where a note is struck.** A note struck less than a quarter unit after a unit's start is on it, for its strength (late notes aren't snapped, but they are a player's timing too). Between unit starts is weaker than any unit start.
- **Shapes,** the first that fits, in this order. Each line note has at most one.
  - **Anticipation:** struck at most a felt beat before the next stronger unit start, held into it, its pitch class struck there, and not sounding in the unit before the note is struck: a chord note played again comes early to nothing, so a 9 held on top is never explained away. Shaped until that unit start. It's tried before a suspension, as a note whose pitch class is struck where it's held sounds with it there.
  - **Suspension** (not the bass): held at least a unit into a felt beat at least as strong as where it's struck (the strongest it reaches, the first of equals), then a step down to the line's next note, which starts at or after that beat and is at least as long as the part held. Shaped from that beat. So a note prepared on a bar line and held over the next one is a suspension, and one held longer than the note it falls to (a sus4 chord's 4th) is the music's own.
  - **Approach** (the bass only): a step into a stronger next note, at most a felt beat long and no longer than it; not a step along a scale (both steps the same way, one a whole tone), which is a passing note.
  - **Passing** and **Neighbour:** at most a felt beat long, no longer than either note around it, and weaker than one of them.
  - **Appoggiatura** (not the bass): a leap of at least a minor third in and a step out the other way, at most a felt beat long, on a felt beat stronger than the note it resolves to and no longer than it.
- **Pushes** are as designed, with E's notes struck there (in a loop, a copy struck in the loop's last bar counts, and one held to the loop's start is held past it), and the unit before u between felt beats (on a grid of one tick a beat every unit is a felt beat, so nothing is pushed). Each push lists its notes, so the reader can count E from u.

### Decoding

- **Boundaries:**
  - unit starts with an H or B onset (after pushes);
  - bar starts;
  - push targets, which have two states.

  Melody onsets count only when the input is a single line. A span runs from one boundary to a later one, at most 2 bars long.
- **Pass A (zero-order).** For each span s and candidate label y: ψ(s, y) = Σ_k w_k·φ_k(s, y), every feature below except P3. Keep the best K = 8 labels, plus the best rootless label and N (no chord).
- **Pass B (first-order, exact on the pruned lattice):**

  δ(j, y) = max over i and y′ of [δ(i, y′) + P3(y′ → y)] + ψ([i, j), y).

  y = y′ is allowed only when the previous span has the full cap length, so a held chord is never counted twice.
- **Forward-backward** runs over the same lattice in a fixed order. It gives segment marginals μ(s, y), unit posteriors π(u, y) = Σ over s containing u of μ(s, y), and the change chance at u. The sharpness σ (see Calibration) multiplies every score first.
- **Output.** The Viterbi path is reported, with equal neighbours merged.
- **Rests.** A bar or more with no H, B or M sound is forced to N and answered NoChord(Rest).
- **Inside-span split.** This applies only after decoding, and only when all of these hold:
  - the bass splits into two parts, each held at least 2 felt beats;
  - the second part starts on a Bar or HalfBar unit;
  - the first part is the root;
  - the second is the label's 3rd or a seventh (10 or 11 semitones up), never the 5th;
  - the second steps (1–2 semitones) into the next span's first bass.

  The span is then cut, and the second half is written as an inversion or slash. So C C/E | F and C C/B | Am come out, while a root-fifth two-feel stays "C".
- **Pins.** Pinned labels or boundaries are forced in both the Viterbi search and forward-backward. They are a control only, with no promise of better accuracy elsewhere.

**As built (decode PR).** `chordread/decode.rs`, with the changes it needed in `score.rs` and `weights.rs`. Each point is tested (`decode_tests.rs`, `score_tests.rs`).

- **Where a chord may change.** Where a chord or bass note is struck, each bar line, and each felt beat a chord may be pushed to (read both ways, plain and pushed); the edges of rests and pins, and inside them only every two bars from where they start. The tune's notes are never places (single-line input still waits).
- **Pushes.** A pushed chord's sound is its own from the unit it's struck in, a whole unit early, and the span before ends a unit early (the evidence PR's push_out). So pushing a chord and changing early weigh the same notes, and only where the chord starts tells them apart (E8 against E7). Each span reports whether it's pushed and its units of sound.
- **Pass A** keeps the best 8, the best rootless reading, N, and the readings that may be held on into the span from before it, whatever their rank (held on, a reading scores without the start terms pass A ranks by). N is offered on a span that sounds (the tune alone too), where the music starts, and next to a rest's or a pin's edge; anywhere else a silence shorter than a rest belongs to the chords around it, held on over it.
- **A chord held on.** The same reading again may follow only a span as long as a span from its start may be: two bars on, or the last place before that (a chord struck off the bar line may have no place exactly two bars on, and without this it couldn't be held past two bars). Never from a place a chord is pushed to: that's the same way as holding it on from the plain place, and would be counted twice. Held on, it scores only what counts for each beat (`SpanFacts::held_score`): where it starts (E1 to E8) was counted where it started. Scored as a new start, a long chord could change on its own: C E G A over C held 8 bars read C6, then Am7.
- **Forward-backward** runs in log space in a fixed order, with σ = 1 until the calibration. It gives each reading's chance on any units (`Decoder::chances`) and the chance a chord changes at each unit (`Decoder::change`, 1 where a song starts); rounding never takes a chance above 1.
- **Rests.** A bar or more with nothing sounding in the chords, the bass or the tune, outside the pins. The music is heard from the bar line before its first note, so the silence before it is under a bar: N, but no rest. A rest longer than two bars is reported as one.
- **Pins** are clipped to what's heard, and one that overlaps an earlier one (by where it starts) is left out. A pinned chord is the only reading on its stretch. A pin without a chord keeps pass A's readings ranked over its whole stretch and reads one of them all through. Every way changes chord at a rest's or a pin's edges (any reading may follow any other there, even the same, and the chance a chord changes there is 1), and the report never merges spans across them, so a pin is reported as given. A silence between two pins, or next to one, may read N.
- **The report** is the best way, equal neighbours merged, clipped to the units reported (a loop's copied bar left out), each span with its readings' chances averaged over its units of sound, best first (ties from the lowest note up, N last) and the chance a chord changes where it starts. The same notes moved read the same, moved, to the last bit, and the same music at another resolution, in another order, with drums or played softer reads the same.
- **The pair term** (P3) reads ln (12 f_motion(Δroot | g, g′)): how much likelier the root's move is than one picked at random between chords of the same two groups (`Tables::motion_prior`); 0 for a move into or out of N, and for the same label. Two other forms were tried on the hand cases read whole. The joint ln f_motion, as written in the table above, counts how common the chords are again on top of P1, and broke passing dim7s. The plain conditional ln f_motion(Δroot | g, g′) is below 0 for every change, so it acts as a second cost of changing: it broke G7sus4 to G7 inside a bar (case 31) and other changes in the middle of a bar.
- **The pair term's trust** is 0.25. With the other weights moved about around the hand weights (T15 0.5 to 1.5, R2 0.5 to 0.9, B2 0.3 to 0.7, the four start costs ±0.5, E3 −3 to −1: 243 points, `show_the_grid`), every case reads right at 171 points with a trust of 0.25, 162 with 0.5, 144 with 0 or 0.1 and 117 with 0.75; in a box half that size, at all 243 with any trust from 0 to 0.5. From 1, case 33 fails.
- **Walking bass notes** (evidence). A bass note no longer than a felt beat, under chords that sound there and strike nothing new (held through it, or struck again on the same notes they last sounded, at most a bar before), is a walking note: it isn't the root struck at a change (E1), and a span starting where only walking notes are struck starts where nothing is (E3). Without this, a walking bass read a new chord on each note, and so did keys struck in sixteenths over a bass in eighths.
- **E1 counts only when the bass moves to the root**: not when the root is the bass note sounding last before, at most a bar before. So a chord struck again over the same bass, or a loop coming round, isn't a fresh start. A pushed chord's bass, or a bass note struck with chords struck anew, always counts. Only a bass note struck in the span's first felt beat counts: a bass entering later doesn't say where the chord starts (over a tune alone, C read a bar before its bass came in).
- **The hand weights**, changed for the whole read (reasons in `weights.rs`): T15 2 → 1, R2 0.4 → 0.7, B2 0.8 → 0.5, E3 −1 → −2, E4 to E7 0, −0.2, −0.5, −1 → −1, −1.2, −1.5, −2, t_core 0.2 → 0.11 (t_ten stays 0.06; chosen again with `show_the_trusts`, every case still passing a step of 0.01 away), t_pair 0 → 0.25.
- **The hand cases read whole.** The cases only a whole read decides (4, 5, 6, 27, 28, 29, 35, 37 to 45, 51, 52, 54) read as written, and every hand case reads whole as it does one stretch at a time. New cases h1 to h6 hold long chords: C6 held 8 bars, C6, Am7 and Cdim7 struck again each bar, Dm7 held with no bass, and a dense gospel part (Cmaj9, Am11, Dm9, G13 struck in sixteenths over a bass in eighths): each one chord as long as it lasts. The rootless gates with no bass (cases 12, 13 and 20 among the best two or three) are still not met at any trust in the pair term: those readings are kept, and they wait for the namer and Chord check.
- **Checked against the slow way.** The fast way (Viterbi and forward-backward) agrees with adding up every way one by one, to 1e-9: ln Z, each reading's chance on its span, the chance a chord changes at each unit, and the best way. On six cases (pushes, a chord held over a push, a rest, pins, a gap between pins) and on 198 pieces of small random music with random pins. 397 pieces of random music with random pins (odd grids and loops too) all read whole: the spans tile what's reported, the chances add up to 1, and every pin kept is read as given.
- **Known, left as they are.** A stretch where only the tune plays weighs nothing: b counts the chord and bass ticks only (S, as designed), so the tune's shares there count for nothing and the chord before is held on over it, or N is read; a chord is never read from the tune alone (single-line input waits). At the first unit reported of a loop, after its copied bar: a chord pushed there has its sound from its units on (its strike is in the copied bar), and the change chance is low where the chord goes on round from the loop's end.
- **Speed** (release, one core). 63 bars of a ii–V–I in four keys with a walking bass, a tune and pushes: 2,003 spans, about 50 readings scored on each, read whole in 45 to 50 ms, three quarters of it scoring the readings. A dense 16-bar part takes about 30 ms. That is over the budget below (3 ms for 16 bars): scoring a reading costs about a third of a microsecond, not 50 operations. A speed PR comes before the MIDI page reads with it.

**As built (speed PR).** `score.rs`, `decode.rs`, `candidates.rs`, `cores.rs` and `pitch.rs`; nothing the reader decides changed. Main and this PR timed side by side on the same machine (release, one core of a 2.1 GHz cloud Xeon), 50 reads each, two runs:

| Music | main, middle | main, 98 in 100 | speed PR, middle | speed PR, 98 in 100 |
|---|---|---|---|---|
| 16 dense bars (Cmaj9, Am11, Dm9, G13 in sixteenths over a bass in eighths) | 30 to 32 ms | 35 to 36 ms | 8.7 to 8.9 ms | 9.2 to 10.1 ms |
| 64 bars of ii–V–I with a walking bass, a tune and pushes | 46 to 52 ms | 59 to 61 ms | 14 to 17 ms | 22 to 24 ms |

- **Scoring** (`SpanFacts::score_all`). What depends only on the span is worked out once for it (the evidence that counts once, how far each note is from sounding, which tensions are heard, the wholes each share is of); what depends only on the root once for each root (T15, R1 to R4, L1, B7, E1); and what depends only on the core and the held bass's interval once for the weights (`Scorer`: B1 to B6, P1, and P2's cells, worked out again only when the weights or tables change). The ticks of any set of notes are three lookups in a table of the sums of every set of four neighbouring notes (`SetSums`), so a label's own notes, its tensions and the rest (all less those two) cost three lookups each, not a loop over twelve notes. Each label's notes are worked out when the app is built (`LabelNotes`). The shaped notes are grouped by note and the notes they hang on, so each label tests each kind once, and only where any note passes by.
- **The slow way stays.** `SpanFacts::features` then `score` is the reference, and what an explanation will show. A test checks the quick way agrees with it to 1e-9 on over 200,000 labels (the samples and random music, every span, the candidates and a fifth of every other label, on the counted and the even tables), and that scoring all at once and one at a time agree to the last bit.
- **Pass A** keeps the best eight without sorting all fifty: one that scores no more than the eighth so far is turned away at once, and ties keep the candidates' order, as the ranking did.
- **Forward-backward** adds the masses up without a logarithm for each pair of readings: each place's masses are taken over the highest, multiplied by e to the sharpness times the pair term (from a table), and added; one logarithm is taken for each reading. A sum too small to keep its bits (under 1e-280) or past the largest float, and every sum where a move's factor is too far from 1 (e to more than 30 either way: a trust in the pair term of about 8 or more on the counted tables), is added up in logarithms instead, as before; otherwise a way whose mass is too small for a float could be lost while its move makes it matter most.
- **Candidates.** The escape is found among the cores each heard root doesn't let in, with the same set sums; and a set of cores lists its cores by their bits.
- **Code review** (high effort): ten points, eight fixed (the guard above, one count by kind shared by the slow and the quick scoring, one escape search, one rule for each sum and its fallback, the span's part worked out once for pins and silence too, the build failing if a core's tension is one of its own notes, and dead state dropped). Two were left: the exact sums' fallback for ticks near the top of a u64 stays, so the quick scoring agrees with the evidence on any input; and grouping the shaped notes searches only the kinds already met (a few dozen at most), not every note.
- **Checked against main.** On 300 pieces of music (the hand cases, the decode tests' music, the song, the dense part and about 200 pieces of random music, some with pins), every chord read, every span and every reading kept is the same as main's. The scores and chances move in the last bits only, at most 1.5 parts in a million million: whole-number sums are the same to the bit, but the float evidence is added up in another order. In 3 of about 17,000 spans of random music, two readings that score the same up to rounding swap places in the kept list (Cdim7 and F#dim7, the same four notes; B♭ and F#), which changes nothing read. The same notes moved, at another resolution or in another order still read the same to the last bit.
- **Not yet in the budget.** The budget (3 ms for 99 reads in 100 of 16 dense bars, one core) isn't met on this machine: about 10 ms. A fast desktop core may be twice as quick, which still isn't enough. A read of the dense part is about 104 million instructions (main: about 330 million, counted the same way). About a third is scoring the labels (some 80,000 labels on 1,928 spans, about 430 instructions each, ten of them divisions: each share stays its whole ticks divided once, so the same music at another resolution scores the same to the last bit); about a seventh is the span facts; a tenth the decode itself; a twelfth e to the x; and a seventh the candidates and keeping the best between them. Next, before the MIDI page reads with it: skip the labels that can't reach the best eight with a cheap bound that never turns away one that could, work out the strong bass from running sums, and score each place's spans on several threads (rayon, at low priority; 1 and 16 threads must give the same bits).

## Candidates and evidence (formulas)

**Cores (intervals above the root):**

| Core | Intervals | Core | Intervals |
|---|---|---|---|
| Major | 0 4 7 | MinorMajor7 | 0 3 7 11 |
| Minor | 0 3 7 | HalfDiminished | 0 3 6 10 |
| Dominant7 | 0 4 7 10 | Diminished7 | 0 3 6 9 |
| Major7 | 0 4 7 11 | Diminished | 0 3 6 |
| Minor7 | 0 3 7 10 | Augmented | 0 4 8 |
| Major6 | 0 4 7 9 | Sus4 | 0 5 7 |
| Minor6 | 0 3 7 9 | Sus2 | 0 2 7 |
| Dominant7Sus4 | 0 5 7 10 | Power | 0 7 |

**`CLASS[core][interval]`** (const) is Core, Tension or Other. An interval is a Tension when `chord::role` on the core's own intervals says Tension. It is Other in every other case (Avoid, or a seventh or sixth that the core doesn't have).

This settles every objection to a flat list of families:

- **7#9.** Under Dominant7 the b3 is a Tension (#9). Under Minor7 a major 3rd is Other, so both thirds with a b7 cannot read as minor.
- **Am6 against F#m7b5.** These are two labels on two roots.
- **11 chords.** C9sus4 is Dominant7Sus4 with a 9.
- **Legal tensions.** A b9 is legal on a dominant and Other on a maj7.

**Candidates per span.** Two const tables, `VALID[4096]` and `VALID_ROOTLESS[4096]`, map the heard set read from a root to a 16-bit mask of valid cores. Both are tested over all 4,096 sets.

- **Played root:** the root is in P, and so are the core's identity tones:
  - the third for Major and Minor;
  - the third and the 7th or 6th for 4-note cores;
  - the b3 and b5 (plus the 7th type) for the diminished family;
  - the 3rd and #5 for Augmented;
  - the 4th or 2nd (plus the b7) for sus cores;
  - the 5th for Power.
- **Rootless:** the root is not in P. The core is one of Dominant7, Minor7, Major7, Major6, Minor6 or Dominant7Sus4. Its third (or 4th) and its 7th (or 6th) are in P, plus at least one of its tensions, and at least 3 pitch classes sound. This covers A and B voicings, the dim7's rootless 7(b9) readings and tritone pairs. *(Built 2026-11-01: HalfDiminished was added, with its b5 required, because the sweep's rootless m7b5(9) voicings had no route; and table-level recall on POP909-CL is 91% piano only, 96% with the tune, mostly open fifths: see experiments/chord-reader-experiments.md, E0.)*
- **Escape:** the best label by T1 among those that fail validity, so that a real chord with no third (C G Bb D) keeps a route. Candidate recall must stay at least 99%.

**As built (candidates PR).** Measured with `theorybench candidates` (experiments E0 again). Each point is tested.

- **Open fifths.** A root heard with its fifth and neither third (an open fifth, or a sus shape such as 1 4 5) also tries the major and the minor triad. This was the biggest gain on POP909-CL (+6.2 points).
- **The escape** is one label, on a root in P, among the labels no other way lets in (open fifths included): the one whose own notes have the most presence ticks (twice the chords' and bass's ticks, plus the held tune's), not the chords' mass alone, so a chord with no third over a bass part (bass C, keys G B♭ D) escapes as C7 rather than Gm7. Ties go to the root nearest above the lowest chord or bass note sounding, then to core order, so the choice moves with the music. The list of candidates runs in that same order (by root from the lowest note up, then by core, the escape in its place), so whatever later breaks ties by the list's order moves with the music too.
- **Recall** (labelled ends moved by up to a unit, as the fit counts a reading right): POP909-CL 98.89% (accompaniment), 99.07% (with the tune), 95.97% (the upper part with no bass); 99.99% to 100% on every `voice.py` set with a bass part. With the labelled ends exactly: 96.99% and 97.43%. A moved stretch is kept to two bars, the longest the reader scores. Labels too short to place (both ends on the same unit start) are left out and counted: 20 on POP909-CL, under 0.01% of the time.
- **Not taken** (measured): rootless triads (they let wrong roots in), sevenths not played, and every tune note in P.
- **Hearing additions:** `presence_ticks` (what presence counts, before it's capped), `lowest` (the lowest chord or bass note sounding a quarter of a unit, or where none does the lowest note of any stream sounding at all, for tie-breaks), `heard_in` (the notes heard, from presence ticks already worked out) and `nearest_unit` (lines labels and pins up with the units).

**Notation for the features**

- C, X and O are the core, tension and other pitch classes of y on root r.
- h(p) = m_H(p)/Σm_H, and h_m(p) is the same share in the tune.
- "struck" means struck in s (or pushed into it). "carried" means struck earlier, overhang included.
- v(u) = μ(u) × the bass ticks in unit u, and V = Σ_u v(u).
- Features marked "once" fire once per span. All the others are multiplied by b, so evidence adds up over time and cutting a held chord in two gains nothing.

| # | Feature | Sign | Why |
|---|---|---|---|
| T1 | Σ_{C} h | + | notes explained (HarmAn) |
| T2 | Σ_{X} h, struck, not figuration | + ≤ T1 | tensions are good notes, but never outweigh a 3rd |
| T3 | Σ_{O} h, struck, not figuration | − | wrong notes count against the label |
| T4 | Σ_{O} h, carried | −, \|T4\| ≤ \|T3\| | held-over smear costs less |
| T5 | Σ h, figuration under y | − small, \|T5\| ≤ \|T3\| | explained notes are not free |
| T6–T8 | Σ_{C} h_m; Σ_{X} h_m not figuration; Σ_{O} h_m not figuration | +, +, − | the tune counts after its shape tests |
| T9 | 1 − pres(3rd or sus tone) | − | the third names the chord |
| T10 | 1 − pres(7th or 6th), 4-note cores | − | no maj7 where a triad sounds |
| T11 | 1 − pres(b5, #5, bb7), dim, m7b5, dim7, aug | − | tells m7b5 from m7 |
| T12 | 1 − pres(P5) | −, \|T12\| ≤ \|T9\| | shells leave the 5th out |
| R1–R4 | root heard in H or held tune / only in B / nowhere while a bass sounds / nowhere and no bass | free, free, −, − (R3 ≤ R4) | rootless is normal over a bass, and only a guess without one |
| T15 | Σ_p h(p)·ρ(p−r); ρ: unison 1, P5 .5, M3 .3, m7 .2, M2 .1 | + | Parncutt and HarmAn root weight |
| L1 | no bass, and the lowest H note at s's start is r | + small | E G B D alone leans Em7; never makes a slash |
| B1–B6 | held β is y's root / 3rd / 5th / 7th or 6th / tension / Other | B1 ≥ B2..B4 ≥ 0 ≥ B5 ≥ B6 | a held bass decides C6/Am7, Dm9/Fmaj7, inversion against slash |
| B7 | moving: Σ_u v(u)·[bass pc = r] / V | + | the downbeat leans the root more than beat 2 (FiloBass: 67.9% of bass notes at jazz changes are roots) |
| B8 | moving: the same for other core tones | +, ≤ B7 | |
| B9 | moving: the same for non-core notes on Bar or HalfBar, or held more than F | − | an unexplained strong bass note |
| E1 | once: first bass onset in s is r | + | the root at the change |
| E2 | once: share of y's sounding core tones struck in the first unit | + | chords change where they are struck |
| E3 | once: no H or B onset in the first unit | − | no change inside held notes |
| E4–E7 | once: level of s's start: Bar, HalfBar, Beat, Off | ≤ 0, E4 ≥ E5 ≥ E6 ≥ E7 | metre (a semi-CRF lost about 6 points without it) |
| E8 | once: s starts at a push | − | a push is a scored choice |
| P1 | t_core·ln f_core(c) | t ≥ 0 | template prior (HarmAn's tie-break) |
| P2 | t_ten·Σ over legal tensions τ of c: ln f_ten(τ\|c) if τ is heard (pres_H ≥ ½, not figuration, not carried), else ln(1 − f_ten(τ\|c)) | t ≥ 0 | a 9 on maj7 is common, a b13 on m7 rare, so Cmaj9/E can beat Em7(b13) |
| P3 | once, pass B: t_pair·ln f_motion(Δroot, g(c) \| g(c′)), with g ∈ {dominant, dim/aug, other} | t ≥ 0 | dim7 rises a semitone, V goes to I, tritone subs fall a semitone |
| N | w_N for the N label | free | no chord |

That is 39 numbers: 35 weights, 3 trusts and w_N. Each signed or ordered weight is written as a base plus a non-negative step, so no explanation can ever show a chord tone counting against its chord.

**As built (evidence PR).** `chordread/score.rs` and `chordread/weights.rs`. Each point is tested (`score_tests.rs`).

- **Counting.** b = S/F, with S the ticks in which a chord or bass note sounds. Every share is whole ticks over whole ticks, divided once at the end, so the same notes moved give every label on its new root the same evidence and the same ranking, to the last bit. The same holds at another resolution and with the notes in another order, and every piece of evidence stays in its range on any span.
- **T1 to T5** are shares of the chords' mass. A note that doesn't belong counts as struck (T3) when its pitch class is struck in the chords in the span, or pushed into it, and as carried (T4) otherwise. T4 also takes what the chords hold over a bar line or the middle of a bar on notes that don't belong, as a share of the larger of the chords' mass and all that is held over. A tension that is only carried in counts neither way.
- **Passing notes** are tested per label: a shaped note passes by under a label when it isn't one of the label's own notes and the notes it hangs on are (an anticipation or an approach needs only the first, since it hangs on the chord after). Only the chords' top line, broken chords and the tune pass by. The bass's shapes don't: its notes count by where they fall (B7 to B9), so B9 has no exemption for them. Passing ticks come out of T2 to T4, T7 and T8; the chords' go to T5, and the tune's cost nothing. A shaped note's held-over end counts with what the chords hold over, so a suspension held over the bar line passes by (T5) instead of costing as smear (T4); a tune note's held-over end counts nowhere, as the tune's masses have it. T4 and T5 each add a share of the chords' mass and a share of what they hold over, so each lies between 0 and 2.
- **T9 to T12** read pres (twice the presence ticks over S, capped at 1), taking the most present of the intervals asked for. A 7sus4's 2 may stand in for its 4th; a sus4 triad's may not. T11 is the fifth that says which chord it is (`Core::fifth`: the ♭5 of dim, m7b5 and dim7, the ♯5 of aug, and a power chord's fifth, which no shell leaves out); T12 is the plain fifth of every other core that has one.
- **Where the root is.** R1 is the root heard in the chords or the held tune, R2 the root heard only with the bass. T15 is the chords' ticks weighted by the root weights (in tenths: unison 10, fifth 5, major third 3, minor seventh 2, whole tone 1) over ten times the chords' mass. L1, with no bass, is the share of the span's time in which the lowest chord note sounding is the root (not only where it starts), so it adds up over time like the rest. R1 and R2 are kept at 0 or more (a 27th rule), so an explanation never shows the root being heard counting against the label; w_N takes up the offset the design left them free for.
- **The bass.** B3 counts the core's fifth, plain or altered (its notes less the root, third and seventh). B7 to B9 weight each unit by its level. B9 counts notes that aren't the label's own (its tensions too, as the design's "non-core" says) on units starting on a bar line or the middle of a bar, or from a bass note held longer than a felt beat. Which notes are the bass is the hearing's one rule (`Hearing::is_bass_in`, as the masses count it): a bass part's note, or a chord note, not broken, on the gap bass's pitch class; a held-over end never is.
- **Where the span starts.** E1 takes the first bass note struck in the span; E2 the share of the label's heard notes struck in its first unit (chords or bass); E3 fires when nothing in the chords or bass is struck there.
- **Pushes.** Only a push into the span's first unit is used: the pushed chord's notes count as struck where the span starts, and its bass (a bass note, or a chord note on the gap bass before it) becomes the first bass note struck. What the stretch before loses (push_out), and single-line input, wait for the decode PR.
- **The priors** read `Tables`: even (every core 1/16, every tension half the time) until the counted-tables PR, so for now they change nothing. A share is read between a millionth and 1 less a millionth, so a core never counted (or a share that isn't a number) costs a lot but never an infinite amount, and a trust of 0 always leaves the prior out. A tension is heard for P2 when it is struck in the chords in the span and its ticks not passing by are at least an eighth of S (pres_H ≥ ½).
- **Ties** keep the candidates' order (from the lowest note up), so they move with the music: C E♭ G♭ A ranks Cdim7, E♭dim7, F♯dim7, Adim7.
- **The hand weights** (`HAND`; the decode PR changed some, see "As built (decode PR)") keep all 27 rules `Weights::check` lists, and each rule broken on its own is named by the check. T1 3, T2 1, T3 −3, T4 −1, T5 −0.5; the tune 1, 0.4, −1; T9 −2, T10 −1, T11 −1.5, T12 −0.1; T15 2; R1 0, R2 0.4, R3 −0.3, R4 −1; L1 0.4; B1 to B6 0.9, 0.8, 0.2, 0.5, −0.05, −0.1; B7 to B9 1, 0.5, −0.5; E1 2, E2 0.5, E3 −1; E4 to E8 0, −0.2, −0.5, −1, −0.3; the three trusts 0; w_N −0.5.
- **Why R2 is above R1.** Case 2 (C E G over G, read C) and case 33 (G C F over D, read Dm11) are mirror images. Gsus4 over G (the E its 13th, its fifth missing) against C is shaped just like Dm11 over D (the G its 11th, its fifth missing) against G7sus4. On the notes alone, every piece of evidence weighs the two pairs the same way, except that Gsus4's root is played in the chords while Dm11's is only in the bass (and T15, which leans towards G7sus4). So no weights on the notes alone read both right; a root heard only in the bass, leaning a little towards the label, does.
- **Hand cases, one stretch at a time.** The tests read each case's stretch on its own (pass A, no neighbours; on even tables until the counted tables PR). Most must come first. Cases 21 and 22 with no bass, 23, 33 with no bass and 50 must come in the top two, and the rootless readings of cases 12 (every bar), 13 (G7) and 20 (D♭7) must be kept (among the best eight, or the best rootless). Those have no bass, and some reading on a played root explains as much of the notes whatever the weights: on B E F A (case 13), Dm6 counts three of the notes as its own and G7 two, and G7's fifth is missing. They wait for the pair term: the counted tables don't bring them up (counted tables PR, below), though case 20's G7alt is now kept.

**As built (counted tables PR).** `theorybench tables` counts the tables and writes `chordread/tables.rs` (`COUNTED`, and the sources in `COUNTED_FROM`). Each point is tested; the measurements are experiments E1.

- **What is counted.** The labels alone, in beats, on folds 1 to 4 (fold 0 kept back, drawn exactly as keyfit draws it on the same files, measured twins left out first) of the pieces whose licence lets numbers ship: POP909-CL (727 pieces) and Billboard (571). Each source is scaled to count as much as any other, and half a beat is added to each cell. The tables are counted now, ahead of chordfit, which still fits the trusts.
- **A source counts only for what its labels can say.** Each core is counted from the sources whose labels name it: f_core(c) = (beats + ½) / (beats of those sources + 8), then the 16 shares are scaled to add up to 1 (where every source names every core, nothing changes). POP909-CL's labels can't name a 6, m6, 7sus4 or power chord, so those come from Billboard alone; counted from both, they came out at half (code review). f_ten = (beats naming the tension + ½) / (beats of the core + 1), from the sources whose labels name a 9 on at least 1% of their time (Billboard 5.2%, POP909-CL 0%), so one stray label can't switch a source on. A Harte "11" with no ♯9 is read as the 9sus4 (its third dropped), as voice.py plays it.
- **The numbers.** Major 52.4% of the labelled time, minor 23.9%, m7 6.6%, 7 6.3%, maj7 2.4%, 7sus4 2.0%, sus4 and power chords 1.7%, 6 1.1%, the rest at 1% or under (dim7 0.02%). A 9 is named on 68% of 7sus4s, 23% of 6s, 20% of maj7s, 9.4% of m7s and 6.2% of 7s; a ♭13 on an m7 0.04%. Labels name tensions far less often than players play them, so P2 makes every tension cost a little.
- **The trusts** (hand-set until chordfit): t_core 0.2 and t_ten 0.06, a beat, where the hand cases' smallest lead is largest on a 0.01 grid (0.26 a beat; 0.10 trusting neither; the test `show_the_trusts` prints the grid). The two balance each other: the commonness prior alone lifts C6 over F♯m7b5 on C E F♯ A with no bass, and the tension prior holds it back (labels almost never name a ♯11 on a 6). The pair term's trust stays 0 until f_motion is counted (decode PR). (The decode PR chose them again: t_core 0.11, t_ten 0.06; see "As built (decode PR)".)
- **The hand cases** are read on the counted tables with these trusts, as the reader will read. Every gate holds, and case 20's G7alt is kept too (8th). The rootless gates (cases 12, 13 and 20 in the top two or three) are met at no pair of trusts up to 0.5 each (at best G7 comes 6th): they wait for the pair term.
- **`EVEN`** gives every core the same P1 and costs ln 2 for each tension a core can carry, heard or not, so at a trust above 0 it isn't neutral (cores with more tensions cost more). Tests that need the priors out set their trusts to 0.

**As built (pair table PR).** `theorybench tables` also counts f_motion, and `Tables::pair_prior` reads it. Each point is tested.

- **The groups (g).** Dominant: 7 and 7sus4. Dim/aug: the cores whose altered fifth says which chord they are (dim, m7b5, dim7, +). Other: every other core. The design named the groups but not which cores go where; m7b5 goes with the diminished chords because its ♭5 is what `Core::fifth` and T11 read.
- **What is counted.** A change of chord is a label read as another root or core than the one before (C to C/E, or G7 to G9, is no change); no chord, a label that can't be read, or one that lasts no time, breaks the line. Counted in changes, on the same folds as the other tables, from the sources whose labels name every core (code review: POP909-CL can't name a 6, m6, 7sus4 or power chord, so it reads C to C6 as no change and G7sus4 to C as G7 or Gsus4 to C), each source scaled to count as much as any other, half a change added to each cell; each group's 36 shares add up to 1.
- **A held chord is no move.** `pair_prior` gives 0 for the same label again (a chord held past the 2-bar cap), as the table counts only changes.
- **The numbers** (Billboard's 53,491 changes; shares of each group's changes, to a chord of the group named). Of 6,232 from a dominant, 42.4% go up a fourth to another chord and 8.7% up a fourth to another dominant; the tritone sub falling a semitone is rare in this music (0.3%). Of 331 from a diminished or augmented chord, 15.0% rise a semitone to another chord and 8.5% fall one, against 1.6% of other chords' changes rising a semitone. Of 46,928 from other chords, 21.4% go up a fourth to another such chord, 20.2% a fifth and 14.4% a tone.
- **The trust stays 0** until the decode PR, which uses it and chooses it on the hand cases read whole (0.25; see "As built (decode PR)").

## Extensions, roots, inversions and slash chords

**The extension gate (v0, set by hand, in three bands).** It applies to each Tension t of the decoded label y. The note must not be figuration under y, not from B and not overhang. The signals are:

- **V:** struck in H in a unit where at least 2 of y's core tones sound (voiced with the chord).
- **Hh:** sounds in H for at least half of S.
- **Ar:** in a broken part, struck in at least 2 felt beats of s.
- **Mt:** a tune note struck on a Beat or stronger, held at least F, and not an Avoid tone of the named family.

The bands:

- **Named:** Hh, or Ar, or V with at least ¼ of S in H, or Mt held at least ½ of S.
- **Doubtful** (goes into `doubtful`, left out of the name): V alone, or Mt held less than ½ of S.
- **Out:** everything else. The explanation calls the note Passing, a short colour, or a Clash.

An Other note held in H that would be Named also enters the name, because the name shows what the voicing plays. An Other note in the tune never does: a held F over Cmaj7 is a Clash.

So the walking D E F G under F A C never makes Dm11 (the G is in the bass), and a passing D in the tune never makes Cmaj9. Gate v1, a logistic model on the same signals with fixed signs, is fitted on set A's truth tags and kept only if experiment C6 measures it better on set B. Under v1, `doubtful` holds the tensions whose gate chance is between 0.3 and 0.7.

**How the root is chosen**

1. **Held bass.** B1–B6 compete with the notes.
   - F A C E over a held D: Dm9 explains every note with its root in the bass (B1, R2). Fmaj7 puts its 13 in the bass (B5). So Dm9 wins.
   - C6 against Am7, and Am6 against F#m7b5, are settled the same way.
2. **Moving bass.** B7, B8 and E1 lean towards the root on strong beats, weighted by μ. Notes shaped as approach or walking notes never name anything.
3. **No bass.** R1 against R4, T15, L1, P1–P3. F A C E alone reads Fmaj7 first, with Dm9 listed, unless the pair term lifts Dm9 in a ii–V–I.
4. **Symmetric chords.** The dim7 labels on all four roots merge into one reading (their chances add), and so do an augmented triad's three. The name follows the first of these that applies:
   - the held bass, if it is one of the notes;
   - for a dim7, the note a semitone under the next span's root; for an augmented triad, the note a fifth above it;
   - for a dim7, the root of the chord on both sides of it (a common-tone dim7);
   - the lowest note.

   The other roots go in `roots`.
5. **Exact ties.** Broken by score, then by the root's interval above the lowest sounding note, then by core order. Every rule moves with the notes.

**Bass kind**

- Held β = root: **Root**.
- Held β ≠ root and one of the named chord's notes: **Inversion** (C/E, C/G, G7/F, Cmaj9/E).
- Held β not one of its notes: **Slash** (D/C).
- Moving bass: **Moving**, and a slash is never written. The inside-span split above is the one exception.
- No bass: **None**.

**Seventh heard only in the bass.** When the label has a seventh (Dominant7, Major7, Minor7, MinorMajor7), the held bass is that seventh, the seventh's presence in H and M is under ½, and the root, 3rd and 5th make a complete triad above it, the name is the triad as a slash, with the seventh chord as `also`:

- D/C, also D7/C;
- C/B, also Cmaj7/B;
- Am/G, also Am7/G.

Without this rule every decoder prefers D7/C. With the seventh voiced above, it stays G7/F.

**Hybrids.** When the bass is not a chord tone, read the same notes over β. If β's 4th and b7 sound and no 3rd does, the name is β 7sus4, 9sus4 or 13sus4, and the slash form becomes `also`:

- F/G is G9sus4;
- Bb/C is C9sus4;
- Ab/Bb is Bb9sus4;
- Fm9/Bb is Bb13sus4.

D/C, G/C and E/C have no 4th over the bass, so they stay slash chords.

**Pedal.** When a held β lies under at least 2 merged spans with different labels, and is not a chord tone in at least one of them, those spans are flagged PEDAL, keep their slash names, and the hybrid rule is off. So C | F/C | G/C | C gets the flag, while the common-tone C | Cdim7 | C and the line cliché Cm, Cm(maj7), Cm7, Cm6 don't (C is the root every time).

**As built (namer PR).** `chordread/name.rs` (`Namer`, `Naming`, `NamedSpan`, `Reading`, `BassKind`, `SpanFlags`), with `Shaped::passes_under` (in `hear/lines.rs`, shared with the evidence) and `Hearing::strength_at`. Each point is tested (`name_tests.rs`). The decode reads the same to the last bit: the fingerprint of the 300 pieces is unchanged.

- **The gate** is v0 as above, on every note that isn't one of the reading's own (a Tension or an Other). Figuration is a shaped note that only passes by under the reading (`Shaped::passes_under`: it isn't one of the reading's notes, and the notes it hangs on are). The bass's notes never count, and what the chords hold over a bar line counts in neither Hh nor Mt. Hh counts each tick once however many notes sound it, less only the ticks where the notes sounding it pass by (the masses count the longest note of each pitch class in each unit instead). Mt adds up the time of the tune notes that count, once where they overlap, and only for a tension the chord can carry (not Avoid). "Struck on a Beat or stronger" means struck in the first quarter of a unit at that level. An Other enters the name only from the chords (Hh, Ar, or V with a quarter of S), never from the tune, and `doubtful` lists tensions only.
- **The notes named** are the reading's notes heard (P) and the notes the gate names. The written chord's bass is the held β, else the root. A reading is `rootless` when its root isn't heard and something else is.
- **Seventh in the bass** also covers a triad read over its own major or minor seventh, since the decode often reads C, not Cmaj7, for C E G over a held B: C/B, also Cmaj7/B. "Under ½ in H and M" is the seventh not heard when only the chords and the tune's held notes count; the triad must be heard the same way.
- **Hybrids** apply when the bass is Slash and the part isn't on a pedal. The sus chord is the same notes with β as the root, and chord.rs names it (7sus4, 9sus4, 13sus4, 7sus4(b9)).
- **Pedal.** In a run of parts over one held β, where the labels change and β isn't a note of at least one of them, each part whose label isn't rooted on β is flagged. A dim7 or augmented label is rooted on any of its notes, so C | Cdim7 | C and Cm, Cm(maj7), Cm7, Cm6 aren't flagged.
- **Symmetric chords** are decided by the chord as written (`Core::of` its notes), so a dim7 label with only three of its notes heard is a diminished triad and stays on its root. The roots are tried in the order above. When none of its notes is in the chords, it's the first of its notes going up from the lowest note heard. A new root must keep the core. `roots` is worked out from the chord as written.
- **Split** as above: at a HalfBar or Bar unit, each half's bass Held and sounding two felt beats or more. "A seventh" is the label's own (C7 C7/Bb | F/A, but C7 is never cut where the bass plays B), or either one under a major or minor triad (C C/B | Am). A diminished seventh or an augmented triad is never cut, because any of its notes is its root. Each half takes its own chances (`Decoder::chances` over its units) and its own change chance, and both are flagged SPLIT. The halves keep the span's neighbours for naming a symmetric chord: the chord after the first half is the same chord, not where it goes.
- **Readings.** Every label the decode gave a chance on the span is named. Readings written the same (root, notes and bass) are one: their chances add, and the decoded one (else the likeliest) gives the label and the doubtful notes. The decoded reading comes first, even with no chance left (a pin), then the others by chance. The sort is stable, so equal chances keep the decode's order from the lowest note up. There are at most `most` readings. Same-notes groups are keyed by the notes named and the held bass.
- **Two edge cases.** A chord pushed to the first unit reported is named from where it's struck, in the loop's copied bar. This is a loop's last chord struck before its end and held into its start (case 46's target is now met). Over silence, a reading is written in full, as the decode read it, and isn't rootless. That covers a pinned chord and any other reading the decode gave a chance on a silent stretch.
- **Hand cases.** `the_hand_cases_are_named_as_a_player_writes_them` checks the gates of cases 1–11, 14–19, 21–45, 47–55 and h6, and the targets already met (8 Bb13sus4 also Fm9/Bb, 25 C#dim7 first, 48a Cmaj9). `show_the_targets` prints the other targets and the gates the decode doesn't meet yet: 3 of 15 met (36 Dm7, 39 Cmaj7, 46). The rest are all at the decode, work for the chord fit:
  - 12: bars 1–2 are one span, Fmaj7(#11);
  - 13: G13 comes sixth;
  - 20: G7alt and Db13 aren't in the top three;
  - 15e: F#7/C (0.78) over C7(b9,#11) (0.21), and chord.rs writes the latter C7alt (question 3 above);
  - 21 and 22 T: Am6 and C6 before the dominant;
  - 23 and 50 T: Em7(b13) before Cmaj9/E;
  - 41 T: the arpeggio is cut at beat 4 (C, then G5);
  - 56 T: Eb/Fb, the chord.rs spelling fix (question 2 above).

  Case 29 is spelled D#dim7 until names are spelled by the key.
- **Rules that hold for any music** (`keeps_the_rules`). They're checked on 300 pieces of random music with random pins, the decode tests' music, the hand cases and the song, with at most 1, 6 and 40 readings:
  - the spans tile what's reported, and a span is cut only inside a decoded span;
  - a rest has no readings, and the decoded label comes first;
  - chances are 0–1, sorted after the first, adding up to at most 1 with no chord;
  - no two readings are written the same;
  - a name holds only notes that sound, and its root unless it's rootless;
  - the bass kind fits the chord, and is the same kind on every reading;
  - same-notes groups go by the notes and the held bass.

  The same music moved −6 to +5, at 96 and 960 ticks a beat, shuffled, with drums or quieter is named the same to the last bit (roots, notes, bass, chances). A Namer used again names the same. Damaged notes (ticks near the i64 limits, keys over 127, broken grids) cause no panic.
- **Speed.** Naming takes about 0.5 ms for the 64-bar song and 0.2 ms for the 16 dense bars (release, one core, the middle of 50; about 3% of reading them). It names every reading the decode gave a chance, about 35 a span. What a note may count for whatever the reading is worked out once a part, so each reading only checks what depends on it: which notes pass by under it, and which of its notes sound with each note.
- **Code review** (high effort): ten points, eight fixed. These were a split only on the chord's own seventh, never a symmetric chord; Hh counting each tick once; the speed above; one rule for the notes played (`played_in`, shared with the evidence); one timing allowance; `BassTexture::held`; and each push looked up once. Two were left. A chord pushed into a loop's first unit is still named from where it's struck, while the decode reports its sound from the unit it's pushed to: the decode scored it with the struck unit and its chances are the span's own units'. The namer also keeps its own time-ordered copy of the shaped notes, because the hearing's list is in order of lines (as documented and tested).
- **Left for the API PR** (done there: see "As built (reader API PR)"). Showing a pin's own chord as given (the `Pin.chord` above): today a pinned label is named from the notes like any reading, so Db7 pinned over C E G is written C#dim(maj7)/B#, rootless. Also spelling by the key, and the answer (Chord, SameNotes, Implied, Unclear, NoChord) with `other`.

## Naming policy

- **The C# look by default.** Names come from `Chord::new(root, notes, bass)` and `quality_of`: maj7, m7, m7b5, dim7, +, sus4, 7alt, 9sus4, 6/9 and add9, with tensions in brackets.
- **Only heard notes.** A name holds the core tones heard, plus Named tensions and Named held Other notes. It never claims an unheard note. The root is left out of the notes when it isn't heard; the name still shows it.
- **Rootless** is said in the explanation ("Dm9, rootless: F A C E, root from the bass part") and in the `rootless` field, not in the name.
- **Spelling.** `spell::spell_in_key` with the key reading's leaning key. Each root is spelled by `root_in_key` with the next chord: Db9 before C, C#dim7 before Dm, Ebdim7 falling to Dm. Other notes are spelled by degree, so the 7th of Db9 is Cb.
- **`also`** carries the other way of writing the same notes, so a later "slash form first" view needs no new reading.
- **Facts store notes, not names**, so any naming change applies at display time, with no re-index.
- **Proposed chord.rs changes** (each needs Solomon's yes and a reviewed update of the 2,048-name, `Chord::parse` round-trip and C# twin tests):
  1. In a dominant with no P5, six semitones is #11 when a natural 9 or 13 sounds. Today D over a C7 shell prints C13(b5), and C E Bb D F# prints C9(b5); both were checked on 2026-11-01 with a scratch build against `main`. C7(b5) stays for the bare shell.
  2. A bass that is only a bass is spelled readably: Eb/E. Today it prints Eb/Fb (checked).
  3. 7alt policy. Today C E Bb D# G# prints C7alt, and so does a Gb triad over a C7 shell (C E Bb Db Gb), which a jazz player writes C7(b9,#11). Kept as it is until Solomon answers.

## Model, training and data

**What is learned**

- The 39 numbers above.
- **Counted tables:**
  - f_core: 16 cells;
  - f_ten: about 60 legal (core, tension) cells;
  - f_motion: 3 × 12 × 3 = 108 cells.

  They are counted from POP909-CL and Billboard *labels* (each source weighted equally, half a count added to each cell), so the renderer has no part in them.
- Calibration: 8 numbers. Thresholds: 8 numbers.
- Gate v1, only if C6 keeps it: 6 numbers.

**Data that may ship**

- **POP909-CL** (MIT, 906 songs). Checked: 85.8% of labelled segments are plain maj or min, 9,347 segments carry an inversion or slash bass, and sus4 and sus2 appear but no 6 chords. It is read four ways, every input made from the piano's own notes (never from the chord track):
  1. the piano as one Harmony part;
  2. piano, melody and bridge;
  3. the piano with its 7-semitone-gap bass line moved to a Bass part;
  4. the upper piano, with that bass line removed and no Bass part. This is real, non-rendered material for R4 and L1 (root not played, no bass).
- **McGill Billboard charts** (CC0, 712 songs, already without the RS200 titles through the conversion's `--skip`), played by **`convert/voice.py`**.
  - voice.py is written apart from chord.rs, in Python with its own degree tables, so the two can't share a mistake.
  - Recipes use only the label's degrees. The fifth may be left out. The root is left out only in rootless recipes, for labels with a 7th and a 9 or 13.
  - Harte "11" (236 segments) is played as 9sus4, and either name is accepted when scoring. C12 tries leaving these segments out.
  - **Set A** (may be learned from, at weight 0.25): close root position, the chart's own inversions, drop 2, shells (1-3-7 and 1-7-3) with the label's tensions on top, and a gospel octave bass under a close right hand.
  - **Set B** (measured only, never fitted, in any fold): rootless A and B, upper-structure triads, quartal and So What, arpeggios and Alberti, two-handed spreads with doubles, and gospel clusters.
  - **Bass textures,** chosen by a hash of (song, section): held root; root-fifth two-feel; walking (the chart root on 2 changes in 3, otherwise the 3rd or 5th, with half-step approaches about half the time, using FiloBass's 47.7% as guidance only); pedal; the chart's inversions; no bass.
  - An optional tune has tagged passing and neighbour notes, and holds a tension only when the label has it. A fifth of the changes are pushed an eighth.
  - Every note is tagged: chord tone, extension, passing, neighbour, approach, walking or push.
- **Measuring only:**
  - RS200, played the same way (by project decision);
  - FiloBass (its licence was not read);
  - WJazzD (ODbL);
  - jazznet (MIT; measured first);
  - Solomon's packs and Chord check answers: counts only (rule 5).

**Folds.** fold = fnv1a64(normalised song id) % 5. This is keyfit's function, so `pop909/001` is shared by POP909 and POP909-CL. Renders take their chart's fold, and transposed copies share a fold. Fold 0 is frozen, and each `--final` look at it is logged in `test-runs.md`.

**Supervision**

- Gold boundaries are snapped to units.
- Labels are accept sets:
  - POP909-CL maj accepts {Major, Major7, Major6} and min accepts {Minor, Minor7, Minor6}. These are lenient: the labels are mostly triads while the piano often adds sevenths.
  - sus4 accepts {Sus4, Dominant7Sus4}. Seventh labels must match exactly.
  - Set A labels are exact.
  - N and labels that can't be mapped are masked out.

**Objective**

maximise Σ_d ω_d (ln Z_d^gold − ln Z_d) − Σ_k λ_k (w_k − w0_k)²

- Z sums over every segmentation and label of the pruned lattice. Z^gold sums over those whose labels are in the accept sets and whose boundaries are within one unit of the gold.
- The gradient is the expected features under the constrained forward-backward minus those under the unconstrained one.
- The fit is projected L-BFGS, starting from hand-set weights w0 that already pass every gate.
- λ is chosen by inner-fold likelihood.
- Gold labels are always kept among the candidates while training.
- Per-song sums are added in a fixed order, so a refit gives identical bits.
- `chordfit` refuses to write weights that break a fit gate.

**Set by hand** (all hashed): the units, levels and μ; the snap; the push test; the 75% held share; BASS_GAP 7; the presence factor 4; the overhang rule; the broken-part rule; the figuration shapes; ρ; the 2-bar cap; K = 8; the one-bar rest; gate v0's bands; the split rule; the validity tables; the naming rules.

**Experiments** run on folds 1–4. A change is kept by the key finder's rule: the gain's interval is above 0 (or the log-loss falls by at least 0.01), no set gets worse beyond its interval, and no gate breaks.

| # | What is tried |
|---|---|
| C1 | eighth units against beat units |
| C2 | span cap of 1, 2 or 4 bars |
| C3 | no pair term; K = 4, 8 or 16 |
| C4 | one bass weight set against the held/moving split |
| C5 | figuration off, outer lines only, or with voice separation |
| C6 | gate v0 against v1 |
| C7 | push as a scored choice, the C# unpush rule, or none |
| C8 | presence factor 2, 4 or 8 |
| C9 | velocity weighting |
| C10 | counted tables against free weights |
| C11 | an isotonic map on top of σ |
| C12 | training mix: set A weight 0, .25, .5 or 1; the Harte "11" handling |
| C13 | a key-context feature, with the version tied to the key finder's |
| C14 | no rootless candidates |
| C15 | BASS_GAP 5, 6 or 7 |
| C16 | held share 0.6 against 0.75 |
| C17 | the broken-part rule off |
| C18 | the escape label off |

**As built (chordfit PR).** `theorybench chordfit` (tools/theorybench/README.md, "Fitting the chord reader") and mo-harmony's `ChordFitter` (`chordread/fitter.rs`), which adds up every way and the ways the labels allow on the decode's own lattice (checked against adding up every way one by one on 192 small pieces). Nothing the reader reads changed: the fingerprint of the 300 pieces is the same. Each point is tested (`fitter_tests.rs`, and the tests in `chordfit.rs` and `ruled.rs`).

- The objective as above, with one λ for every number (`--lambda`, 1e-3), not yet chosen by inner folds. The loss is per bar, each reading weighed.
- Projected quasi-Newton in place of L-BFGS: a damped BFGS curvature started from the slopes' outer products, each step's quadratic solved within `chordread::RULES` (an accelerated projected gradient, the rules projected by Dykstra), then a line search. Every point read keeps the rules.
- The accept sets as planned, with three more: a dominant 11 accepts the 9sus4 `voice.py` plays it as, a dim7 its four roots and an augmented triad its three.
- POP909-CL is heard four ways (piano, full, split, upper), each reading weighed 1, so set A weighs a quarter of a song.
- A labelled change with no place within a unit where the reader may change chord moves to the nearest place it can (0.6% of the ends), so no song is dropped.
- σ, the thresholds, the experiments, `--final` and `--write` aren't built yet.
- **The first fit** (folds 2–4, fold 1 measured; README, "The chord reader's first fit"): names gain on every set (POP909-CL's piano 77.1% against 73.3% by hand and 75.7% for the C# app; Billboard set B 87.1% against 79.2%), and inversions read in root position fall (set B 7.3% against 19.6%). But 27 hand-case gates break (Cmaj9 as Em7, C6 as Am7, the alt voicings as triads, passing dim7s as dominants) and extensions lose (set A's held chords' F1 84.4% to 59.5%; the sweep's macro F1 61.1% to 52.1%): the loss sees a label's root and core, never its tensions. Not shipped. Next: the fit keeps every gate (on a stretch read whole, a gate is a margin between two readings' scores, a straight-line rule in the numbers, which the projection keeps), then C12.

**As built (gates PR).** The hand cases are public (`chordread::cases`, behind the `hand-cases` feature, which `theorybench` turns on), so `chordfit` reads the same cases the tests do, with any numbers: one stretch at a time, read whole and named. After a fit every gate is read with the start numbers and the fitted ones, and the report and the `--out` file say which break; weights that break one don't ship. The cases the design holds out (G: 6, 15, 32, 34, 35) and target 15e are neither learned from nor counted there: the tests check them. Nothing the reader reads changed: the fingerprint of the 300 pieces is the same. Each point is tested (`cases_tests.rs`, and the tests in `chordfit.rs` and `ruled.rs`).

- `--hand μ` adds μ times the hand cases' mean −ln P(labels) to the loss per bar. Each music the cases read whole is one piece, labelled where its gates say what it must read; a gate that only asks for readings to be kept is checked, not learned from.
- `--hold n` keeps each case's labels at a chance of at least 0.5 (or its chance at the start, if less) as a straight-line rule on the numbers, kept by the same projection as the signs and orders. For at most n more rounds every rule is drawn again where the fit got to, and the cases whose gates still break are asked for 0.75, then 0.875 (never more than at the start). The rounds stop when every gate holds, when nothing more can be asked, or when the rules can't all hold together (B1's floor is a bound of its own, so that check sees it).
- A round stops when the loss falls by less than a hundred-thousandth of itself three steps running.
- **The three fits** (README, "The hand cases in the fit"): plain breaks 58 of the 229 gates looked at, `--hand 1` 3, and `--hand 1 --hold 3` none. Learning from the hand cases keeps T2 (tensions) up (3.23 beside T1's 4.98; plain 0.38), so the extensions come back (set A's macro F1 62.8%: hand 56.4%, plain 47.9%) while roots, chord kinds and inversions keep most of the plain fit's gains (POP909-CL's piano named 76.3%: hand 73.3%, plain 77.1%). It scores at or above the hand-set numbers on every set, and every hand case, held-out ones too, holds read whole and one stretch at a time. Not shipped yet: a PR of its own, with the version bump and a fit run again on what's merged then.


## Calibration, the graded answer and alternatives for Chord check

**Sharpness**

σ = max(0.05, Σ_i κ_i·tent_i(bars heard) + offset(texture))

- The tent has knots at 1, 2, 4, 8 and 16 bars. There are 3 texture offsets, and Full is 0. That makes 8 numbers, as in the key finder.
- σ multiplies every score before forward-backward. So every listed reading comes from one calibrated distribution, with boundary doubt included and an explicit `other` mass.
- **Fitting.** σ is fitted by NLL on cross-fitted predictions from folds 1–4 (each fold scored with weights fitted on the other three). Fold 0 only verifies it.
- **The event** is defined once: the root, the core class (third: major, minor, sus or none; seventh: none, 6, b7, maj7 or bb7; the altered fifth) and the bass kind are right. It is exact on renders and judged through the accept sets on POP909-CL, where it is reported as lenient. Extension doubt is shown through `doubtful`, never mixed into the chance.
- **Checks:** ECE (15 equal-mass bins), NLL, Brier, and the reliability of the top 1, top 2 and top 3 readings.

**Span chances.** p(y) is the time-weighted mean of π(u, y) over the span. Labels with the same written name add up, and dim7 and augmented roots merge. A same-notes group is the set of readings with the same heard named notes and bass.

**Graded answer**, checked in this order:

1. **NoChord(Rest):** silence of a bar or more.
2. **NoChord(TooFewNotes):** |P| < 2.
3. **Implied:** the texture is SingleLine (tune only, bass only, or a mono lead). It leans to the top reading and is never a Chord answer.
4. **Chord:** q1 ≥ τ_one(texture).
5. **SameNotes:** the top reading's whole same-notes group adds up to at least τ_same(texture). Examples: C6 or Am7; C#dim7 (four roots) or A7(b9).
6. **Unclear:** otherwise, leaning to the top reading.

**Thresholds.** answerfit sets each τ as the smallest value on a 0.01 grid whose song-clustered 95% upper error bound meets its target:

- Chord: the event is wrong at most 10% of the time.
- SameNotes: the truth is outside the group at most 10% of the time.

No τ goes below 0.5, and τ = 1 means that kind is never given.

**Chord check.** `choices(span, 3)` gives:

- the top reading;
- the best member of its group on another root or bass (C6 → Am7), or, when the group has none, the doubtful-tension variant (Cmaj9 ↔ Cmaj7);
- the best reading outside the group.

Each guess is played with its own root in the bass.

**Rounds** (in mo-app):

- 30–50 items: 50% disagreements (Rust against C#, and between Rust variants), 30% at random (to catch errors both share), and 20% repeats at least 3 weeks later (to measure Solomon's agreement with himself).
- The choices are shuffled. The C# reading is among them where its index has one. "Type my own", "none" and "unsure" are always there.
- Answers are stored on his PC: note hash, ticks, grid, the choices shown and where each came from, the answer, the reader version and the round.
- Accuracy uses inverse-probability weights for the oversampled disagreements.
- They are for measuring only, unless he allows the 8 calibration numbers to be refitted on them. A 50-name audit of pack names goes through the same screen.

## Explanations

`explain` hears the notes again on demand (microseconds), so readings carry no reason data. It returns:

- what each note does (`NoteDoes`);
- where the bass came from;
- the six biggest feature terms for the best and runner-up readings, in plain words built from counts.

Examples:

- "Dm9: D held in the bass part. F A C E in the keys: a rootless voicing (3, 5, 7, 9). As Fmaj7, the D would be a bass outside the chord."
- "Dm7 over a walking bass: D on the bar line, E2 a passing note (D-E-F), G2 a walking note. No slash under a moving bass."
- "C13(#11): a D triad over a C7 shell (an upper structure)."
- "C6 or Am7: the same four notes; the bass C makes it C6."
- "C#dim7: named from the bass, a half step under the Dm7 it leads to; also Edim7, Gdim7, Bbdim7."
- "Pushed an eighth: struck on the and of 4." "F5 in the tune clashes with Cmaj7; not part of the chord."

**As built (explain PR).** `chordread/explain.rs` (`ChordExplainer`, `Explanation`, `NoteWhy`, `NoteDoes`, `BassFrom`, `Term`, `TermWhy`, `TERMS_SHOWN`), with `ChordReading::before` and `Decoding::before`, and `decode::kept_pins` (the decode's pin rules, now shared by the decode and the explainer). Each point is tested (`explain_tests.rs`, and the chord before a loop's first in `reader_tests.rs`). Nothing the reader reads changed: the fingerprint of the 300 pieces and the readings' hash are the same. The plain words came in the next PR (see "As built (words PR)").

- **Its own type.** `ChordExplainer::explain(parts, grid, cx, reading, span)` explains one stretch, and `explain_all` every stretch with a reading, hearing the notes once. It's kept apart from `ChordReader`, so a reader kept for reading many parts holds none of its working space. It's given the same parts, grid and context (pins too) the reading was read from, and returns None when the reading isn't of them as far as can be told (its start, end, unit, grid, loop and how many notes; and the pins: a chord starts where each pin kept starts and ends, the stretches given a chord are exactly the pins with one, each written as given, and two stretches side by side read as the same chord are one chord cut in two or have a pin's edge between them), for a rest, and for a stretch that isn't there.
- **Speed.** 64 dense bars (4,608 notes): the read takes 50.6 ms, explaining one stretch 0.87 ms, and every stretch 1.40 ms. Not microseconds, as guessed above, but far under a read.
- **What each note does** (`NoteDoes`) is told from the first reading as written, in this order: held over; the bass (the held bass or any note the written chord sounds; else passing under the chord, walking, or a clash); then in the chords and the tune, one of the chord's own notes or a tension it names, a note doubling the bass written under the chord (the C of D/C, which is the bass), a note of the reading the name leaves out, a passing shape, doubtful, colour (in the chords) or the tune over the chord (in the tune, only on a tension the chord can carry there), and else a clash. `ChordTone`, `Extension`, `Doubtful`, `Colour`, `TuneOver` and `Bass` carry the note's degree, and each note says what it's heard as (bass, tune or chords).
- **Where the bass comes from** (`BassFrom`): the bass part, the lowest keys, or no bass. A bass part's note only held over into the stretch isn't the bass part playing there.
- **The evidence** is a `TermWhy` for each feature and one for the pair term, with both readings' values and scores, the biggest lead first; terms with no lead are left out, and the first six are the ones to show. The totals are each reading's score on the stretch alone, so they needn't be the scores that ranked them.
- **A chord held on.** A reading that's the chord before (for a loop's first stretch, `ChordReading::before`: the chord the decode reads over the copy of the loop's end, just before its first unit) is that chord held on, and gets none of the evidence about where a chord starts, as the decode scores it. The decode holds a chord on only from where the chord before ends, never from a chord pushed in early, so where the stretch's chord is pushed, a reading held on is weighed on the stretch's own units. Where a pin starts or ends, every reading starts a chord. All of this is checked against the decode's own scores, on the hand cases, the tests' music and random music.
- **Left as an approximation.** Where the decode's other way is a longer chord over both stretches (one span), a runner-up held on is scored as held on: a long span's score can't be split into a score for each stretch.

**As built (words PR).** `chordread/words.rs` (`Words`, `Explanation::words`), with three things the words needed from the explanation: `NoteWhy::anchors` (what a note that passes by hangs on), `Explanation::held_on` (whether each reading is the chord before held on) and `Explanation::held_bass` (the bass each one's evidence weighs). Each point is tested (`words_tests.rs`, and the new fields in `explain_tests.rs`). Nothing the reader reads changed: the fingerprint of the 300 pieces and the readings' hash are the same.

- **Three parts.** `explanation.words(reading, chart, key)` tells:
  - **the chord**, in one sentence: its name as the chart writes it (with "Unclear, most likely", "Too few notes for a chord, most likely", "the same notes" or "implied: only one line plays" where the answer says so, and its other names when it has more than one root), what the bass is to it, and the notes above the bass by their degrees, with a rootless voicing named;
  - **the notes**: how it's played (given, pushed, one chord written in two, the chord before going on, over a pedal), then a line for each kind of note that isn't the chord's own (rung on from before, early to the next chord, walking, a clash, the tune over it, passing colour, doubtful, and the shaped ones: held over and stepping down, leaping in and stepping, passing between two notes, stepping away and back, leading in by a step);
  - **why**: a sentence for each kind of evidence among the six shown (the bass; the root; the shares of what the chords, the tune and a moving bass play; a missing third, sixth, seventh or fifth; where it starts; how common the chord and its tensions are; how the root moves), in the order the evidence leads, each telling the first reading and the runner-up.
- **Only what was weighed.** Every sentence comes from a value of the evidence (`TermWhy`) or from what a note does (`NoteDoes`), so the words never say more than the reader knew. Shares are rounded but never to all or none ("all of it" only for all, "under 1%" for a little). T4 and T5 add a share of what the chords hold over to a share of their sound, so they aren't a share of anything (they can pass 1): they're told as "a little" (under 0.1), "some" (under 0.5) or "much". A 7sus4's missing third is about its 4th or the 2 standing in for it, whichever is heard more, so both are named ("G7sus4's 4th, C, and the 2 that may stand in for it, A, are at most partly heard"). The counted tables are told as what the songs counted show ("of the chords in the songs counted, 52% are major chords and 1.7% sus4 chords").
- **Read and written.** The decode reads a label (a root and a core); the namer writes the chord from the notes played. A name on the label's root that's the same kind of chord (Dm9 for Dm7) or the label's chord with notes it doesn't sound left out (G for G7, C5 for C, Bdim for Bm7b5) is the chord read. Where the two differ otherwise (D7 written D/C, its seventh heard as a bass under it; Edim7 written C#dim7; D7sus4 written Dm11/G), a line says so ("The reader reads it as D7; it's written D/C from the notes it plays."; for a runner-up, "The runner-up, Dm11/G, is read as D7sus4."), and the why tells the evidence by the chord read, as it was weighed. A note of the chord read that the name leaves out is told as such ("C is read as Cmaj7, whose seventh, B, isn't heard").
- **Spelling.** Each chord as the chart spells it in the key (`ChordReading::names`, worked out once for all of a reading's explanations; the runner-up and the chords either side are spelled the same way), and each note by its degree from the written root (Bb as the b7 of C, A# as the 3 of F#). No double sharps or flats (but a diminished seventh's bb7), no octaves, plain ASCII (b and #).
- **Examples** (the hand cases):
  - "Dm9: D in the bass is its root, and the chords play F, A, C and E, its b3, 5, b7 and 9 (a rootless voicing over the root in the bass)." Why: "D in the bass is the root of Dm9 and would be a tension of Fmaj7/D (its 13)." "Of what the chords play, 75% is Dm9's own notes and 25% tensions it can carry; for Fmaj7/D, all of it is its own notes."
  - "Dm7: the bass moves under it through D and F, its root and b3 (so no slash is written), and the chords play F, A and C, its b3, 5 and b7."
  - "C#dim7 (also Edim7, Gdim7 or A#dim7): C# in the bass is its root, and the chords play E, G and Bb, its b3, b5 and bb7." Why: "The bass moves to C#dim7's root, C#, as it starts."
  - "With the stretch before it's one chord, Cmaj7, written in two where the bass moves to B, its seventh: no new chord starts."
  - "Pushed: struck half a beat early, on the and of 4." Any amount that isn't a half, a third or a quarter of a beat is "a little", "less than half a beat" or "less than a beat", struck "between beat 4 and the bar line".
  - "Dm9 to G13 to Cmaj9 is a more common way for the root to move than Dm9 to Fmaj7(#11)/G to Cmaj9."
- **Tested by rules** on every hand case, 200 rounds of random music with random pins, and the tests' own music: every line is plain ASCII, starts with a capital and ends with one full stop, with nothing left over from making it (doubled spaces or commas, braces, NaN); the chord line starts with the chord as the chart spells it and says what the answer says (unclear, implied, too few notes, rootless); there's a line for each way it's played and at least one for each kind of note; and one for each kind of evidence shown, after the runner-up's chord read where it's written as another. The same notes at 96 and 960 ticks a beat are told the same, and in a key the chords and notes are spelled by it (F# A# C# in B major, Gb Bb Db in D flat major).
- **Left for later.** An upper structure ("C13(#11): a D triad over a C7 shell") isn't told: nothing in the reader marks one yet.

## Speed

Estimates for a dense 16-bar 4/4 part (to be measured):

- 128 units and up to about 2,000 spans.
- Pass A scores about 30 labels per span at about 50 operations each. Per-span arrays are built once, and others are found by subtraction.
- Pass B and forward-backward take about 2,000 × 8 × 8 × 2 ≈ 2.6×10⁵ steps.
- Expected: 0.2–0.6 ms typically, 2 ms at most.
- Budget: p99 ≤ 3 ms for a 16-bar part on one core, measured with `--timing` on a dense gospel sixteenth-note part, not on pads. A 200-bar song should take ≤ 30 ms.
- Measured (speed PR, release, one core of a 2.1 GHz cloud Xeon): the dense 16-bar part has 128 units and 1,928 spans with about 42 candidates on each. It reads in about 9 ms in the middle of 50 reads, and 10 ms or less 98 times in 100. A 64-bar song reads in 14 to 17 ms, so a 200-bar one would take about 50. Both are still over the budget: see "As built (speed PR)" for what's left.

Other rules:

- No hash-keyed cache in v1. If C3 shows a need, a cache keyed by the exact 36 integers may come later.
- Scratch is reused; there is no HashMap on the path, and float sums run in a fixed order.
- 1 and 16 threads give identical output.
- rayon runs only in the indexer, at low priority, one `ChordReader` per thread.

## API and the compact fact for the MIDI page (version constant)

- `ChordReading::fact() -> ChordFact`.
- `summary() -> ChordSummary`.
- `names(key) -> Vec<Spelled>` through `spell_in_key`.
- `choices(span, n)`.
- `check_names(&[Chord]) -> Vec<NameCheck>` returns `Agrees`, `Listed` or `Disagrees`. Names come from `Chord::parse` and are never evidence.

**`ChordFact(Vec<u8>)`** is a 6-byte header followed by 10 bytes per span. A 16-bar part takes about 100–330 bytes.

Header: [layout 1, unit code, total units u16, span count u16].

| Bytes | What |
|---|---|
| 0–1 | start unit (BE). A span ends where the next starts; the last at the header's total |
| 2 | root nibble, core nibble (0xF = N) |
| 3 | bass kind (3 bits), pedal (1), bass interval (4) |
| 4–5 | named intervals above the root (12 bits), rootless, pushed, doubtful, walking |
| 6 | answer kind (3 bits: Chord, SameNotes, Implied, Unclear, Rest, TooFewNotes), runner-up in the same-notes group (1) |
| 7 | chance × 255 |
| 8 | runner-up root and core (0xFF none) |
| 9 | runner-up chance × 255 |

The fact stores notes, not names. So the page can show any answer without reading the notes again, and naming fixes need no re-index.

**`ChordSummary([u8; 8])`**, for search:

- a progression hash that ignores transposition (u32);
- the cores present (u16);
- extension bits (b9, 9, #9, 11, #11, b13, 13, alt);
- flags (slash, inversion, rootless, pedal, push, walking, hybrid, unclear for over 25% of the time).

Both are stored under (note hash, roles, grid, span, `CHORD_READER_VERSION`), and refreshed when the version differs.

**Versioning, as the key finder does it:**

- `numbers_hash` covers every weight, table, threshold and hand-set number.
- `weights.rs` keeps `SHIPPED_HASH` and `HASH_VERSION`.
- A golden hash of every hand case's readings is kept per version. So a code change that moves an answer fails CI until the version is bumped (rule 7).
- Until a version passes its twin tests, the MIDI page shows Rust chords beside the C# ones, and the databases stay read-only.

**As built (reader API PR).** `chordread/reader.rs` (`ChordReader`, `read_chords`, `ChordReading`, `ChordSpan`, `ChordContext`, `Pin`), `chordread/answer.rs` (`ChordAnswer`, `Texture`, `Thresholds`, `grade`) and `chordread/version.rs`. Each point is tested (`reader_tests.rs`). The decode reads the same to the last bit, but for one change the code review asked for (a chord given over a loop's last bar is now given for its copied bar by the decode itself): of the 300 pieces fingerprinted, only the 10 random loops with a chord pinned over their last bar read differently (checked piece by piece), and everything the reader's test music reads is the same as before the review.

- **One call.** `ChordReader::read(parts, grid, &ChordContext)` hears, decodes, names and grades. A reader kept and used again reads the same, without asking for memory each time. `ChordContext` holds the span (loop, song or stretch), the pins, and how many readings each span keeps (1 to 6: the MIDI page's index keeps 2, Chord check 6). The key was left out of it: it's never evidence, so it's given only where names are spelled (`names(key)`).
- **Every reading is named and graded, then the first few are kept**, so no answer depends on how many are kept (tested keeping 0 to 255). `other` is 1 less the readings kept.
- **Pins are in the music's own ticks**, each end moved to the nearest unit start within the music; the decode leaves out empty ones and those over one before them. A chord given is written as given, never cut in two, and graded one chord: Db7 given for a ii V I's second bar is Db7/G over the held G, and Am7/C given over a tune keeps its bass. A chord with no core pins only its ends. The decode says which spans are pinned and the chord given for each (`DecodedSpan::pinned`, `given`), so the namer needs no list of pins. In a loop, the decode gives a chord given over its last bar for the copy of that bar heard in front too, so the first chord follows it and a chord starts for sure at the first unit (as at any pin's end); a pin of only its ends isn't copied (the copy would be read from that bar's notes alone), nor is one where a pin is given in the copied bar already. A pin before the loop's start is left out.
- **The answer** follows the design's order, with a chord given checked second, after a rest. The texture is the piece's: Full (a tune and others), Accompaniment (chords and a bass part), HarmonyOnly (chords alone, in any part) or SingleLine (one part's notes, a tune's, a bass's or keys', when they're one line). A note isn't on its own when it's in a broken chord, when a note of another pitch class is struck with it (within a player's timing), or when one struck before it still sounds more than half a unit after it starts; an octave is one note; and the notes are one line when at most one in ten isn't on its own. So an arpeggio, stabs however short, and a triad in a tune or bass part are chords, while a double stop at the end of a long tune isn't. A step-by-step lead in eighths in a chords part is heard as an arpeggio, as the hearing hears two strikes a felt beat in a chords part. The same notes need two readings or more in the group. A threshold of 1 or more is never met (a chance can round up to 1), and one under a half counts as a half.
- **Hand-set thresholds** until answerfit: one chord at 0.8 and the same notes at 0.9 (Full, Accompaniment and HarmonyOnly), never for SingleLine. From the hand cases: keys alone that read more than one way (13 and 20 at 0.63, 22 with no bass at 0.75, 33 with no bass at 0.76) must not be one chord, and the chords the cases want as one chord read 0.98 or more. With these, C | F in eighths (40, 0.73) and the Alberti C (42) are the same notes leaning to C, and the walks (38) and the dense part (h6) are unclear.
- **Same-notes groups are broad**, because the namer writes only the notes heard: C E G's group holds Gsus4(add6) and Em(addb13) at 0.1% or less. So Chord check offers a reading besides the first only at 1% or more (`CHOICE_FLOOR`): C6 over C offers Am7/C; Dm9 offers Dm7 (its Fmaj7/D is 0.4%); Dm7 over a walk offers F6, then F. The choices are worked out each time they're shown, so the floor isn't in the version's hash.
- **What the chart shows.** A span's first reading, when its answer gives a chord and the decode reads a chord there (`ChordSpan::label`), or when one line alone implies it: a tune alone is never read as a chord for sure (C E G E alone is no chord at 0.77 and C at 0.08), and hand case 58 wants C implied. Elsewhere a stretch the decode reads as no chord shows none, though its answer is unclear and Chord check still offers its readings.
- **Spelling.** `names(key)` spells the chart in the key, each root by the next chord shown (round a loop, the first comes after the last), or as charts do with no key: 29 is Em7 | Ebdim7 | Dm7 in C major (D#dim7 with no key), and 55 is Db9, never C#9. `spell(&chart, span, chord, key)` spells any other chord at a span the same way, from the chart worked out once.
- **`check_names`** matches the names to the chords shown in order, as many as can be and then as many agreeing as can be (an alignment, not a first-come match, so a name that only matches late doesn't use the chart up), and round a loop from whichever chord matches best (the loop's own order on a tie). A name agrees when the chord shown is it (its root among the reading's roots, any of a dim7's four; the same core, tensions may differ; a slash must match), is listed when another kept reading is (so Chord check, keeping 6, can list more than the index, keeping 2), and disagrees otherwise. 59 agrees three times, "Bb13 Ebmaj9 Fm9" agrees three times round its loop, and the chords a reading shows always agree with it.
- **Version 1** is the first reader, on the hand-set weights. `numbers_hash` covers the weights, the tables, the thresholds and the hand-set numbers (the sharpness, the priors' floor, the readings kept, the longest span, the most units, the longest bar, the bass's gap, the most readings); the two that only choose how the decode adds its sums up are left out, as they move a chance in its last bits at most. A hash of what every hand case, the tests' music, the song and music made for each kind of answer, pin and texture read is kept for version 1, with chances to six places, over `numbers_hash` too (so writing a new SHIPPED_HASH alone can't pass both checks); CI checks it on Windows, macOS and Linux.
- **Code review (xhigh), 15 points.** Fixed: stabs shorter than a player's timing were taken for one line; a tune or bass part alone was one line whatever it played; a stretch the decode reads as no chord showed a chord; a pin of only its ends was copied into a loop's copied bar; `check_names` matched first come and never went round a loop; the namer found pins by their place in a list it was given apart; the version checks could be passed by writing a new hash alone; `spell` could overflow and worked out the whole chart for one chord; the loop copy was in the reader, not the decode; the reader sorted pins as the decode does; and the core of a written chord was worked out in three places (now `Core::of_chord`). Kept, by design: a chord given over a loop's last bar makes a chord start for sure at its first unit (that's what a pin's end means), a tune alone shows the chord it implies though the decode reads none there (hand case 58), and a lead in eighths in a chords part is an arpeggio (the docs say so now).
- **Left for later.** A part tagged as the tune that plays block chords is still heard as a tune (its notes count half, and never as what sounds), so the decode reads little there: a C triad held in a tune part is graded unclear, with none shown. The MIDI page's roles mostly keep chords out of tune parts (a part with a third or more of its notes in chords is chords, unless it's named a melody); hearing such a part as chords would change the hearing, so it waits for a PR of its own.

**As built (fact PR).** `chordread/fact.rs` (`ChordFact`, `FactReading`, `FactSpan`, `RunnerUp`, `ChordSummary`, `Extensions`, `SummaryFlags`). Each point is tested (`fact_tests.rs`). Nothing the reader reads changed: the fingerprint of the 300 pieces is the same.

- **The fact** has a 22-byte header and 11 bytes a stretch (the design said 6 and 10), so a 16-bar part takes about 110 to 380 bytes. The header adds the tick the reading starts at and the grid it was read on (ticks a beat, beats a bar), so the page can put each chord in the music's own ticks (`FactReading::tick`) without hearing the notes again or being handed the grid; the reading keeps its grid too (`ChordReading::grid`). With the unit it keeps the texture, and whether it was read as a loop or left notes out. A stretch keeps the root it's written on beside the root and core the decode read, as the two can differ: a diminished seventh read on E is written on C# before Dm, an augmented triad on the root the next chord needs, and F over G is written G9sus4. It also keeps the bass in semitones above the written root, whether the same notes are written another way too (G9sus4, also F/G), whether the decode read a chord there (so a fact's chart shows the same chords as the reading's: `FactSpan::shown`), and whether the chord was given. The layout, byte by byte, is in `ChordFact`'s docs. It keeps two readings a stretch, as the page's index keeps (`ChordContext::new`); a fact made from a reading that keeps one has no runner-ups.
- **Read back**, a fact is the reading it was made from, its chances to the nearest 255th: the same stretches, answers, flags, first readings and runners-up, the same grid, the same chart in any key (`FactReading::names`), the same summary, and each stretch's ticks. Tested on the hand cases, the tests' music, and 300 random pieces with random pins on odd grids.
- **A damaged fact reads as none**: another layout, a bit no layout-1 fact sets, more units than a reader hears, a unit the grid isn't read on, the wrong length or count, stretches out of order, a root or bass that isn't a pitch class, a code no fact has, a rest with a reading, or a stretch with no reading (or no runner-up) that keeps something of one. 20,000 damaged or random byte strings never panic, and those that read are readings the page can show. A chord with no notes is still read, as a real reading might have one.
- **The summary** is worked out from the chords the chart shows, as they're written, and is the same from a reading or from its fact read back. The progression is a hash of each chord's root above the first one's and its core. A chord held over stretches side by side, or played again after stretches with no chord, counts once. So Dm7 G7 Cmaj7 is the same number in any key, with tensions (Dm9 G13 Cmaj9) and over other basses (Dm7/C G7/B Cmaj7/E), while Dm7 Db7 Cmaj7, Dm7 G7 C and G7 Cmaj7 Dm7 are other numbers; 0 means no chord is shown, and never anything else. A loop counts from the chord it starts on, once round: a chord held over its seam counts once (the loop C F G C is C F G, as the same loop started a bar before, C C F G, is), and a loop that plays the same chords over again is the loop played once (C F C F is C F, so an 8-bar loop that plays a 4-bar one twice has its number). The cores are the chords' as written (Cmaj9 is a major seventh). A 7alt chord counts its altered ninths, eleventh and thirteenth as well as alt; a 6 and a sus chord's 2 and 4 aren't tensions. Hybrid is a sus chord on its bass that's also written as a slash chord (G9sus4, also F/G); slash and inversion are how the bass stands under the chord written; unclear is more than a quarter of the time the music plays (rests left out). The tensions and bass flags are tested on chords given (written as given, whatever the numbers read), so a later fit can't move those tests.
- **The version's hash** covers the fact and the summary of every piece it reads too, so a change to either fails CI until the version is bumped. Version 1 still hasn't shipped, so its hash was replaced, not bumped.
- **Code review (high), 10 points.** Fixed: a loop's chord held over its seam counted twice, so the same loop started a bar earlier had another number; the fact needed the grid handed in to place its chords, with nothing to say a wrong one was given (it keeps its own now, and its unit must be the grid's); the codes for basses and answers fell back to 0 for a kind they didn't list (now the compiler checks every kind has one); more units than a reader hears read as a fact; hybrid was worked out from how the namer happens to write a triad over its seventh (now it's a sus chord as well); the chart's spelling built a list it didn't need; and the docs now say the fact needs two readings a stretch, that the fact's 255ths are a second rounding the version's hash meets (coarser, so less likely to tip than the six places), and that the tests' copies of the layout's numbers are copies on purpose. Kept, by design: a chord played again after a rest counts once (the progression is the chord changes, and a rest isn't one), and a chord with no notes is read (a real reading might have one, and refusing it would lose a real fact). Left for Solomon: whether a loop's number should be the same whichever chord it starts on (open question 9).

## Bench changes and the merge rule

**theorybench**

- **`chords`** gains:
  - `--engine rust` (runs in-process);
  - `--inputs piano,full,bass,upper`;
  - `--roles off` (all parts Unknown, merged like the C# input);
  - `--voicings A|B|all`;
  - `--oracle-cuts`, which scores naming on the human boundaries apart from boundary finding;
  - `--per-segment` beside the time-weighted scores;
  - `--sweep`: every quality × recipe × 12 roots × texture, alone and inside a ii–V–I;
  - `--dump/--against`, `--timing`, `--invariance` and `--metamorphic` (added held 9ths and 13ths must not move the root; added passing notes must not change the name).
- **`chordfit`** works like keyfit, with `--design`, `--final` and `--write`.
- **The `cs/` helper** also writes the C# candidate list, so the C# app's top-k can be scored.

**New scores.** All are paired with C# and use a bootstrap clustered by song (or chart). The mir_eval levels all stay.

- **Exact:** the named pitch-class set and the bass right, per chord event and per time.
- **Per degree:** precision, recall and F1 for 6, b9, 9, #9, 11, #11, b13 and 13, with the altered tones pooled. Unconditional (not only on right-root spans), with the support printed, plus a macro average.
- **Same-notes root:** by pair (C6/Am7, m6/m7b5/9, Fmaj7/Dm9, Em7/Cmaj9, Fmaj7(#11)/G13, hybrid/slash, dim7, aug, 13/alt) and by bass texture.
- **Bass kind:** a 5 × 5 confusion table, the false-slash rate over moving basses, and the missed-inversion rate.
- **Rootless root:** with and without a bass part.
- **False extensions** from planted figuration. This is a regression check only, since the renderer's shapes resemble the reader's.
- **Lists:** gold in the top 1, 2 or 3, and MRR.
- **Calibration:** ECE, NLL, Brier, top-k reliability, risk against coverage, AURC.
- **Rates printed every run:**
  - candidate recall (gold among candidates: at least 99% of POP909-CL time, every miss listed);
  - how often the gold label was pruned;
  - how often the priors decide a span (the top reading changes without P1–P3);
  - the leave-one-voicing-family-out gap.

**Merge rule.** Decided once, on frozen fold 0, against C# on the same notes.

1. **Pre-registered primary scores** (set B fold 0). Each must rise with its interval above 0:
   - exact notes+bass, per event;
   - same-notes root;
   - extension macro-F1.
2. **Not worse:**
   - **POP909-CL** (accompaniment input, and piano with melody):
     - root, majmin+bass and bass: low end ≥ −0.5 points (C#: 88.0, 85.3, 92.8, as measured after bench PR 2; the first draft had 80.5 for majmin+bass). Root also with roles off.
     - sevenths and tetrads: ≥ −1.0. The labels are mostly template triads, so these are never must-rise gates.
     - segmentation ≥ 93.0, with changes a bar within 10% of the labels' 1.35.
   - **Set B:**
     - altered tones pooled: F1 low end ≥ −2.0, printed with a wide-interval warning (about 165 segments);
     - bass-kind accuracy ≥ C#;
     - walking false-slash rate ≤ 5% and below C#;
     - rootless root with a bass part ≥ 90%;
     - same-notes root with a held bass ≥ 95%.
   - **RS200 renders:** root and majmin+bass, low end ≥ −1.0.
3. **Calibration:** ECE ≤ 0.05 per texture on Chord answers, and every grade meets its target.
4. **Behaviour:**
   - every gate and held-out gate holds;
   - invariances hold bit for bit, including at ppq 96, 480 and 960;
   - 1 and 16 threads give the same output, and a refit gives the same bits;
   - p99 ≤ 3 ms;
   - candidate recall ≥ 99%;
   - the version is bumped.
5. **Later, before the MIDI page prefers Rust over C#:**
   - at least 300 Chord check answers;
   - Rust's top 1 beats C# with a pack-clustered paired interval above 0;
   - no genre folder worse beyond its interval (with the test's power printed);
   - extensions and same-notes root up;
   - his self-agreement printed;
   - the counted tables refilled from his pack counts, if he says yes.

**As built (bench `--engine rust` PR).** `theorybench chords --engine rust` runs the reader in the bench, in-process, on the notes the C# helper heard (the parts `--part` gives, each with its dataset's role), read whole as a song, loop or window, and keeps three readings a stretch. `--roles off` hears every part as one with no role (the C# input); `--oracle-cuts` pins each labelled chord's ends with no chord given; `--timing` reads on one low-priority thread and prints ms a piece and per 16 bars. Each stretch is scored by the name the chart shows, read back with `Chord::parse` exactly as the C# app's names are (a name stands for the plain fifth, the 9 under an 11 or 13 and the 11 of a minor 13, which count only where the label has them), and so are its other readings, the lists. `--inputs`, `--voicings` (one file a set), `--against`, `--invariance` and `--metamorphic` aren't built: the files and `--by` do their work for now. Measured 2026-11-02 on hand-set weights, before chordfit (tools/theorybench/README.md, "mo-harmony beside the C# app"):
- Played like music (voice.py sets A and B, RS200, the sweep), the chord named whole rises 10 to 19 points over the C# app (Billboard A 84.8% against 71.7%, B 80.9% against 62.9%), the extensions' macro F1 17 to 20 points, and inversions read in root position fall from about 76% to 20–37%. Walking bass 78.3% (38.9%), pedal 72.2% (42.9%). Block chords: Billboard +2.3, RS200 +0.8.
- POP909-CL piano part: notes+bass 74.0% against 75.9%, majmin+bass −1.8, root +1.5, segmentation 92.6 (93.9). About half the loss is at the chords' edges, much of the rest open fifths read as power chords (labels there never name one). Piano and tune with roles: +14.0; roles off: −9.7.
- Where chordfit starts: a root-fifth two-feel (the fifth taken as the root for the bar's second half: C over G as Gsus4(add6); sweep two-feel 49.3% against 65.0%), arpeggios read piece by piece (a 7 chord is a triad until its seventh comes), maj7(#5), m11/m7(11), maj13 and 9sus4(13) in the sweep, false b13s on block chords, and rootless roots (still almost never right: pass B's pair term).
- Speed: a POP909-CL piano part reads in 7.6 to 7.7 ms per 16 bars in the middle, 13.6 to 14.8 ms 99 times in 100 (two runs; budget 3 ms); block chords 0.3 and 1.5 ms. Speed step 2 comes before the MIDI page.
- Merge-rule gates measured here (not yet on fold 0): set B and RS200 pass all but rootless roots; POP909-CL passes the root but not majmin+bass, bass, sevenths or segmentation.

## Hand cases

Defaults: 4/4, 480 ticks a beat, Loop, velocity 32768, whole notes, C4 = 60. b: is a Bass part, k: keys (a Harmony part), t: a tune (a Melody part). "|" is a bar line, and (1–2) means beats 1–2.

- **G**: a gate. CI checks it and chordfit refuses weights that break it.
- **G\***: a held-out gate. CI only; chordfit never sees it.
- **T**: a target.

| # | Notes | Expected |
|---|---|---|
| 1 | b C2; k C4 E4 G4 | G: C, Chord, Root |
| 2 | b E2 (also E2+E3) / b G1 under k C4 E4 G4; b F2 + k G3 B3 D4 F4 | G: C/E, C/G, G7/F, all Inversion; never Em(b6) |
| 3 | b C2 + k D4 F#4 A4 | G: D/C, Slash, also D7/C; no C-rooted reading on top |
| 4 | b C2 (1–2) B1 (3–4) \| A1; k C4 E4 G4 \| A3 C4 E4 | G: C C/B \| Am; C/B Slash, also Cmaj7/B |
| 5 | b C2 (1–2) G2 (3–4) \| F2; k C4 E4 G4 \| F3 A3 C4 | G: C \| F; never C/G |
| 6 | b C2 (1–2) E2 (3–4) \| F2 (1–2) F#2 (3–4) \| G2; k C4 E4 G4 \| C4 F4 A4 (1–2) C4 Eb4 A4 (3–4) \| C4 E4 G4 | G\*: C C/E \| F F#dim7 \| C/G |
| 7 | b G1 + k F3 A3 C4 (also +D4); b C2 + k Bb3 D4 F4; b Bb1 + k Ab3 C4 Eb4 | G: G9sus4 (also F/G); C9sus4 (also Bb/C); Bb9sus4 (also Ab/Bb) |
| 8 | b Bb1 + k F3 Ab3 C4 Eb4 G4 | G: root Bb. T: Bb13sus4, also Fm9/Bb |
| 9 | b C2 held 4 bars; k C4 E4 G4 \| C4 F4 A4 \| B3 D4 G4 \| C4 E4 G4 | G: C \| F/C \| G/C \| C, PEDAL on bars 2–3; no sus or add names |
| 10 | b D2 \| G2 \| C2 \| C2; k F3 A3 C4 E4 \| F3 A3 B3 E4 \| E3 G3 B3 D4 ×2 | G: Dm9 \| G13 \| Cmaj9 (one 2-bar span), Chord, Root; never Fmaj7/D or Fmaj13/D; reason says "root from the bass part" |
| 11 | as 10, k C3 E3 F3 A3 \| B2 E3 F3 A3 \| B2 D3 E3 G3 | G: the same; the lowest keys note is never the bass |
| 12 | no bass; k F4 A4 C5 E5 \| F4 A4 B4 E5 \| E4 G4 B4 D5 ×2 | G: bar 1 top two {Fmaj7, Dm9}; bar 2 top three ⊇ {G13, Fmaj7(#11)}; bars 3–4 top two {Em7, Cmaj9}; no rootless reading is a Chord answer. T: Dm9 \| G13 \| Cmaj9 |
| 13 | no bass; k B2 E3 F3 A3 | G: G13 in the top three; no reading has a B bass; not Chord |
| 14 | no bass; k D3 C4 F4 \| G2 B3 F4 \| C3 B3 E4 | G: Dm7 \| G7 \| Cmaj7, Root (bass by the gap) |
| 15 | b C2 + k E3 Bb3 plus (a) D4 F#4 A4 (b) Eb4 G4 Bb4 (c) A4 C#5 E5 (d) Ab4 C5 Eb5 (e) Gb4 Bb4 Db5 | G\*: root C, Dominant7, notes as played: (b) C7(#9), (c) C13(b9), (d) C7alt; tensions (a) {9, #11, 13}, (e) {b9, #11}. T: (a) C13(#11) after the fix; (e) Solomon's choice |
| 16 | b C2 + k E3 Bb3 Eb4; b E2 + k G#3 D4 G4; k alone E3 G#3 D4 G4 | G: C7(#9); E7(#9); E7(#9); no minor reading in the top two |
| 17 | b C2 + k E3 Bb3 D#4 G#4 | G: root C, tensions {#9, #5}, C7alt |
| 18 | b G2 + k B3 F4 Ab4 Bb4 Eb5 | G: G7alt |
| 19 | k F3 B3 Eb4 Bb4 over b Db2; over b G1 | G: Db13; G7alt |
| 20 | the same keys, no bass | G: G7alt and Db13 in the top three; not Chord. T: they are the top two |
| 21 | k C4 E4 F#4 A4 over b A1 / F#2 / D2 / none | G: Am6; F#m7b5; D9; with none, Am6 and F#m7b5 among the readings, not Chord. T: none, then B7 (b B1 + k D#4 F#4 A4): F#m7b5 first |
| 22 | k C4 E4 G4 A4 over b C2 / A1 / F1 / none | G: C6 (Am7/C listed); Am7, Root (C6/A listed); Fmaj9; with none, top two {C6, Am7}, SameNotes or Unclear. T: none, then D7 (b D2 + k C4 F#4 A4): Am7 first |
| 23 | b E2 + k G3 B3 C4 D4 | G: top two {Cmaj9/E, Em7(b13)}. T: Cmaj9/E first |
| 24 | b C#2 + k E3 G3 Bb3 \| b D2 + k F3 A3 C4 | G: C#dim7 \| Dm7 |
| 25 | no bass; k E4 G4 Bb4 C#5 \| D4 F4 A4 | G: the dim7 reading named C#dim7 (roots C# E G Bb); A7(b9) among the readings; the group ≥ 0.9. T: C#dim7 first |
| 26 | b C2 + k E3 G3 Bb3 Db4 | G: C7(b9) |
| 27 | b C2 + k C4 E4 G4 (1–2); b C#2 + k E4 G4 Bb4 (3–4) \| b D2 + k F4 A4 C5 | G: C C#dim7 \| Dm7, change at beat 3 |
| 28 | b C2 held 2 bars; k C4 E4 G4 (1–2) C4 Eb4 F#4 A4 (3–4) \| C4 E4 G4 | G: C Cdim7 \| C; no slash, no PEDAL |
| 29 | key C major; b E2 \| Eb2 \| D2; k G3 B3 D4 \| Gb3 A3 C4 \| F3 A3 C4 | G: Em7 \| Ebdim7 \| Dm7 (spelled Eb) |
| 30 | b C2 + k E3 G#3 C4; then no bass, k E4 G#4 C5 \| b F2 + k F4 A4 C5 | G: C+; C+ (roots C E G#) \| F |
| 31 | b G1 held; k G3 C4 D4 F4 (1–2), G3 B3 D4 F4 (3–4) | G: G7sus4 G7, change at beat 3; never G11 |
| 32 | b C2 + k Bb3 D4 F4 G4; b D2 + k F3 A3 C4 E4 G4; b C2 + k Bb3 Db4 F4 G4 | G\*: C9sus4; Dm11; C7sus4(b9) |
| 33 | b E2 + k E3 A3 D4 G4 B4; b D2 + k G3 C4 F4; no bass, k E3 A3 D4 G4 | G: Em11; Dm11; top two {A7sus4, Em11}, not Chord |
| 34 | no bass, k C3 G3 D4 Eb4 Bb4 F5; b C2 + k D4 E4 G4 B4 | G\*: Cm11 (C3 is the bass); Cmaj9 |
| 35 | b C2 held 4 bars; k Eb4 G4 held, top C5 \| B4 \| Bb4 \| A4 | G\*: Cm \| Cm(maj7) \| Cm7 \| Cm6; no PEDAL |
| 36 | b D2 E2 F2 G2 in quarters; k F3 A3 C4; a 1-bar loop | G: no 9, 11 or 13; no slash; Moving; E2 Passing, G2 Walking. T: Dm7 |
| 37 | b D2 E2 F2 G2 \| G2; k F3 A3 C4 \| F3 A3 B3 E4 | G: Dm7 \| G13; no 11; G2 on beat 4 an Anticipation |
| 38 | b D2 F2 A2 Ab2 \| G2 B2 D3 Db3 \| C3 E3 G3 B2 \| C3 D3 E3 G2; k F3 C4 \| F3 B3 \| E3 B3 ×2 | G: Dm7 \| G7 \| Cmaj7 (bars 3–4 one span); no slash; Ab2 and Db3 Approach |
| 39 | b C2 E2 G2 Ab2 \| A2; k E4 G4 B4 \| C4 E4 G4 | G: no slash; Ab2 Approach; bar 2 Am7. T: bar 1 Cmaj7 |
| 40 | k only, eighths C3 E3 G3 C4 E4 C4 G3 E3 \| F3 A3 C4 F4 A4 F4 C4 A3 | G: C \| F |
| 41 | k only, eighths C4 D4 E4 G4 E4 D4 C4 G3; and C4 E4 G4 D5 ×2 | G: C, both D4 Passing. T: Cadd9 |
| 42 | k only, Alberti C3 G3 E3 G3 ×2 | G: C; no slash; bass kind Root or None |
| 43 | b C2 \| A1; k C4 E4 G4 B4 from 0 to 1680, then A3 C4 E4 G4 struck at 1680 and held to 3840 | G: Cmaj7 \| Am7 at 1920, heard_start 1680, PUSHED (bass on the beat) |
| 44 | b C2 + k C4 E4 G4 from 0 to 1680; b F2 + k F3 A3 C4 struck at 1680 and held | G: C \| F at 1920, PUSHED |
| 45 | b C2 \| A1, k C4 E4 G4 \| A3 C4 E4 on the beat; t A5 struck at 1680, held into bar 2 | G: change at 1920; A5 Anticipation; nothing PUSHED |
| 46 | 2-bar loop: silent from 0 to 960; b G2 + k F4 B4 D5 from 960 to 3600; b F2 + k E4 A4 C5 struck at 3600, held to 3840 | T: Fmaj7 from tick 0, PUSHED from the loop end \| G7 from 960 |
| 47 | b G2 + k G3 B3 D4; t D5 (1–3), E5 an eighth on 4, D5 on 4.5 | G: G; E5 Neighbour; never G6 |
| 48 | b C2 + k E3 G3 B3; t (a) D5 held (b) F5 held (c) eighths C5 D5 E5 from beat 1 | G: (b) Cmaj7, F5 a Clash, never Csus4, C(add11) or F-rooted; (c) Cmaj7, D5 Passing. T: (a) Cmaj9 |
| 49 | k only C2 G2 C3 + E4 G4 B4 D5 | G: Cmaj9, bass C |
| 50 | k only E2 E3 + C4 G4 B4 D5 | G: Cmaj9/E in the top two. T: first |
| 51 | b C2 \| F2; k C4 E4 G4 B4 from 0 to 2160; F4 A4 C5 E5 from 1920 | G: Cmaj7 \| Fmaj7 at 1920; no #11; B4 HeldOver |
| 52 | b C2, then F2 at 1920; k C4 E4 G4 from 0 to 2400; F3 A3 C4 from 1920 | G: C \| F at 1920; F with no 7th or 9th |
| 53 | 3/4: b C2 + C4 E4 G4 \| b G2 + F4 B4 D5. 12/8 (240 ticks, 12 beats): C \| G7; C, then G7 from 1440; b Ab2 + Ab3 C4 Eb4 (eighths 1–6), b Bb2 + Ab3 Db4 F4 (7–12), and a twin with the second chord struck on eighth 6 | G: change at 1440; at 2880; at 1440; Ab \| Bbm7 at eighth 7 (the twin PUSHED) |
| 54 | b C2 \| G1 \| A1 \| F1; k C4 E4 G4 \| B3 D4 G4 \| C4 E4 A4 \| C4 F4 A4; t quarter-note steps | G: C \| G \| Am \| F, Chord; tune steps Passing |
| 55 | key C major; b D2 + F3 A3 C4 \| Db2 + F3 B3 Eb4 \| C2 + E3 G3 B3 | G: Dm7 \| Db9 \| Cmaj7; spelled Db9, never C#9 |
| 56 | b E2 + k Eb4 G4 Bb4 | G: no reading names an unplayed note. T: Eb/E, Slash, after the spelling fix |
| 57 | every case moved −6 to +5 (notes kept within 12–115), shuffled, keys doubled an octave up, a drum part added, velocities halved, ghost notes added, ppq 96, 480 and 960 | G: when moved, roots, bass, chances and grades move exactly (only the spelling may change); otherwise bit-identical |
| 58 | t only C4 E4 G4 E4; no parts; one note C4; all twelve C4 to B4 held; damaged input (ppq 0, negative lengths, ticks near the i64 limits, key 127, a 10-million-beat span) | G: Implied leaning to C; None; NoChord(TooFewNotes); Unclear; no panic, and each span's chances add to at most 1 |
| 59 | the notes of Fm9 \| Bb13 \| Ebmaj9, with and without the name "Fm9 Bb13 Ebmaj9.mid" | G: bit-identical readings; `check_names` gives Agrees three times |

## Build order

1. **Bench PR 1 (no app change).**
   - The new scores, `--oracle-cuts`, `--per-segment` and `--roles off`.
   - The C# candidate lists.
   - The C# numbers on POP909-CL under the new scores.
2. **Bench PR 2.** voice.py with sets A and B, textures, tags, pushes and `--sweep`. Then the C# numbers on the renders: Solomon's complaint, measured before any new code.
3. **chord.rs PR.** `Core`, `CLASS`, `VALID` and `VALID_ROOTLESS`, tested over all 4,096 sets. Plus the #11 and Eb/E fixes, once Solomon says yes, with the twin tests updated after review.
4. **Hearing PR.** Units, streams, the bass gap, broken parts, overhang, lines and shapes, pushes and loop wrap, prefix sums. Invariance and ppq tests. The shared ghost helper, with the key finder's version unchanged.
5. **Reader PR, on hand-set weights that pass every gate.**
   - Candidates, scoring, both passes and forward-backward.
   - The namer: gate v0, seventh in the bass, hybrids, pedal, split, symmetric chords.
   - Grades, `ChordFact` and `ChordSummary`, `explain`, the version hashes and the hand cases.
6. **chordfit PR.**
   - The semi-CRF fit, tables, σ and thresholds.
   - Experiments C1–C18 on folds 1–4.
   - One `--final` run, then `weights.rs` as `CHORD_READER_VERSION` 1.
7. **mo-app.** The MIDI page index (Rust beside C#, read-only), then Chord check, then the Songs page.

## What was taken from where

| Source | Taken |
|---|---|
| Design 1 (base) | semi-Markov scorer; the 16 cores and CLASS; held/moving bass split with no slash on a moving bass; base-plus-step signs; joint semi-CRF fit with accept sets and gold forced among the candidates; two passes with exact forward-backward and `other` mass; the push test that lets the bass land on the beat; the tension prior; the hybrid and pedal rules; check_hint; ChordSummary; metamorphic tests; most hand cases |
| Design 2 | guide-tone validity table `VALID[4096]`; real upper-piano POP909-CL material for the root-not-played weights; notes stored instead of names; the Eb/E fix; dim7 and augmented naming by resolution; the inside-span split (restricted); loop-end push; overhang; chordfit's refusal to write weights that break a gate; no new Role; hand cases (upper-structure family, m6/m7b5/D9, Fmaj9, common-tone and falling dim7, line cliché, Kenny Barron, 11 chords, gospel walk-up, tritone F B Eb Bb) |
| Design 3 | renderer written apart from chord.rs, with set A trained on and set B measured only; planted figuration tags; `--oracle-cuts`; candidate recall; the sweep; μ-weighted moving bass; the Walking class; broken-part presence; fold hygiene; Chord check protocol; pack audit; `Explanation::terms`; hand cases two-feel and G7/F |
| Fact-check | bass first only when held; shape tests split into a chord-free half and a per-candidate half; shapes on lines only; first pass without context; pins as a control only |

## Judges' points and how each was handled

| Point (judges) | Handled |
|---|---|
| Seventh only in the bass → D/C, not D7/C (music, eng.) | Rule in Extensions…; case 3, case 4 |
| Push when the bass lands on the beat (all) | Design 1's test kept; case 43 |
| Bass in keys by gaps only; 7 semitones; B-form never a bass (all) | `BASS_GAP` 7, relative; cases 11, 13, 57 (ppq too) |
| One-line keys sent into the bass (eng.) | Bass needs another H note sounding; broken parts stay in H; cases 40–42 |
| Slash inside a span only for a held bass or a walk-up to the 3rd or 7th; never a two-feel (music) | Split rule; cases 4, 5, 6 |
| μ-weighted moving bass (all) | B7–B9 use v(u) |
| SameNotes by the whole group (music, stats) | Grade 5; dim7 and aug roots merged; case 25 |
| Tension prior (music) | P2 |
| Held tune tensions; avoid notes never (music) | Mt signal; case 48 (target, open question 3) |
| Circularity; set B; leave-one-family-out gap (all) | Set B measured only; gap printed |
| Selection leak (stats) | Merge on fold 0 only; set B folds 1–4 steer experiments |
| POP909-CL sevenths not-worse (all) | Merge rule 2 |
| Key never evidence (music, eng.) | `key` used only for spelling; C13 is an experiment only |
| Presence units; ppq invariance (all) | Ticks over ticks; case 57 |
| Calibrate every listed reading; `other` mass; boundary doubt; event defined once; cross-fitted (stats) | σ applied before forward-backward; event defined; folds 1–4 fit, fold 0 checks |
| Per-degree scores unconditional, macro average; altered own check (stats) | Bench scores; merge rule |
| Per-segment scores (stats) | `--per-segment` |
| Pre-registered primaries (stats) | Merge rule 1 |
| Roles-off ablation; C# candidate lists (stats) | `--roles off`; cs helper |
| No label leakage; dedup (stats) | Inputs from piano notes only; RS200 titles excluded |
| Pop priors; refill from counts (stats) | Prior-decides rate; refill before trust |
| Small refit on Chord check only (stats) | At most the 8 σ numbers, with his yes |
| Held-out hand cases (stats) | G\* block |
| Hash-keyed cache (eng.) | Dropped |
| `doubtful` with a boolean gate (eng.) | Three bands in v0 |
| No Role::Arp (eng.) | Broken texture read from the notes |
| Fact contents (eng.) | Ends, pushed, kind and reason, runner-up, intervals |
| Songs page metre (eng.) | One read per section |
| Rootless without a bass: gate both, order a target (all) | Cases 12, 20, 22, 33 |
| Relative ties (eng.) | Tie order relative to the lowest note and the next root |
| chord.rs changes reviewed and approved (all) | Naming policy; open question 2 |
| Speed on a dense part (eng.) | `--timing` gospel part |
| D1 case 19 wrong (music) | Replaced by F B Eb Bb (cases 19, 20) |
| D1 case 28 reason (music, stats) | G2 is Walking (case 36) |
| D1 case 21 text (music, eng.) | Am7 is Root (case 22) |
| D1 case 32 unreachable (eng.) | Stream rule fixed; case 41 target |
| D2 case 6(d) arguable (music) | Notes gated; name a target (case 15e) |
| D2 pedal on the line cliché and common-tone dim (stats) | Pedal needs a non-chord-tone bass; cases 28, 35 |
| D2 case 19(b) Eb/E (eng.) | Target (case 56) |
| D3 H16, H31, H37(b), H3/H7, H20t, H34(a) (all) | Cases 25, 42, 33, 12, 22, 48 rewritten |
| Walking Dm7 arguable as a gate (eng.) | Root a target alone (36); gated in a ii–V (37) |
| Validity hard or soft (music hard, stats soft) | Hard for rootless labels and for identity tones, with one escape label so recall holds. This keeps wrong-root guesses out without losing real no-third chords |
| Register test (stats liked) against gap (music, eng.) | Gap 7: invariance is a gate, and 7 already rejects B-form and quartal bottoms; C15 measures 5 and 6 |

## Open questions for Solomon

1. **Rule 5 for chords.** Rule 5 already lets counts from your packs into the counted tables (core, tension and pair counts): should they be refilled from your packs? And may the 8 calibration numbers be fitted on your Chord check answers? That second one is learning, so it would need an exception like the key finder's. *Default: neither until you say yes. Your key-finder exception isn't assumed to carry over.*
2. **Naming fixes.** Should it write C13(#11) and C9(#11) in place of C13(b5) and C9(b5), and Eb/E in place of Eb/Fb? *Default: today's names until you say yes.*
3. **Melody notes in the name.** Should a held tune note that is a legal tension go into the name (Cmaj7 with a held D on top reads Cmaj9)? *Default: yes when held at least half the span, shorter ones flagged doubtful; an avoid note never.*
4. **7alt.** Show C7#9#5 as C7alt or C7(#9,#5)? Show a Gb triad over C7 as C7alt or C7(b9,#11)? *Default: the C# look (7alt).*
5. **Hybrids and slashes.** Should Fm9/Bb and Ab/Bb also show as Bb13sus4 and Bb9sus4 first, and a triad over its seventh as D/C rather than D7/C? *Default: yes, with the other name listed.*
6. **Rootless voicings with no bass and no context.** Is the played root first (Fmaj7, with Dm9 listed and never a sure Dm9) right? *Default: yes.*
7. **6/4.** Read it as compound (two felt beats a bar)? *Default: yes.*
8. **Chord check rounds.** 3 choices plus "type my own", with the C# reading as one choice, 30–50 items a round? *Default: as described.*
9. **A loop's progression in search.** Should a loop's progression number be the same whichever of its chords it starts on (Am F C G and F C G Am as one loop)? *Default: no. A loop counts from the chord it starts on, so those are two progressions; a chord held over the loop's seam counts once, and a loop played twice over is the loop played once.*