# Key from notes: mo-harmony part 2 (final design)

## Summary

The key reader is a conditional logit over the 24 keys. Each key is scored from eight named kinds of evidence (here called streams), every one counted up from the candidate tonic:

- the tune's notes;
- the notes of every other part;
- the bass on each beat, weighted by where the beat falls in the bar;
- major triads standing on the bass;
- minor triads standing on the bass;
- the first bass of a loop;
- the first bass of a song;
- the last bass of a song.

A minor base rate is added on top.

Each stream's weights are a counted log-probability table (learned from counts, so Solomon's counts may refill it under rule 5), times one trust number, plus small per-degree corrections held near 0 by a strong penalty. One experiment picks how strong that penalty is. At one end the model is pure tables; at the other it is the per-degree model in the working tree, which has the better probabilities today (8-bar log-loss 0.70 to 0.76, against 0.82 to 0.95 for tables).

How sure the reader is comes from a curve over the number of distinct bars heard, which has no jumps, plus offsets for the kind of input and the kind of span. All of it is fitted on held-out scores only.

The answer comes in grades:

- **One key.**
- **A relative pair:** "C minor or Eb major". The notes are sure; which note is home isn't.
- **A home note:** the mode is unclear.
- **No clear key:** too few notes, notes that fit no scale (a style-free test), or chances spread out.

Each grade's precision is checked on its own cases. A colour (Mixolydian or I7, blues, Dorian, harmonic minor) is read after the key and never changes it.

All counts are integers, so these hold bit for bit:

- moving the notes up or down moves the answer by the same interval;
- note order, how notes are split into parts, playing a loop twice, octave doubling, ghost notes, velocity changes and an added drum part change nothing;
- reading bar by bar and adding up equals reading the whole.

Folds are by song hash and shared by POP909 and POP909-CL. Fold 0 stays frozen and decides the merge.

Pop data hardly contains the flat 7th in major (0.7% of note time) or the natural 6th in minor (0.5%). So the Mixolydian vamp and the blues are declared expected failures on POP909-only data, not merge gates. Their fixes are planned: colour components (E9), wider tables for where home is from CC0 and CC BY chord charts (E10), and Solomon's counts.

Effort used: high.

## Inputs and types (Rust)

The reader builds on the existing `notes.rs` (`Note` with 16-bit velocity, `Part`, `Role`, `Grid`) and the exported `KEY_FINDER_VERSION`. It replaces the draft `Evidence`, but keeps its single `features()` path, which the app and the bench share. Files: `crates/mo-harmony/src/keyfind/{mod.rs, heard.rs, read.rs, answer.rs, colour.rs, explain.rs, fact.rs, weights.rs}`. `weights.rs` is generated.

```rust
pub const KEY_FINDER_VERSION: u32 = 1;      // first merged fit; +1 whenever an answer can change (rule 7)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Span { Loop, Song, Window }        // pack part or clip / whole arrangement / stretch cut from longer music
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Config { Full, MelodyOnly, BassOnly, Accompaniment }  // which streams have sound

/// Integer sufficient statistics. Exact to merge, so part 5 can add them bar by bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heard {
    pub tune: PerPitch<u64>,          // melody ticks sounding (a note counts for at most 2 bars from its start)
    pub harmony: PerPitch<u64>,       // all other pitched ticks (Harmony, Bass, Unknown parts)
    pub bass: PerPitch<u64>,          // sum of mu over beats whose anchor has this pitch class
    pub major_on_bass: PerPitch<u64>, // sum of mu where a major triad stands on the anchor, by root
    pub minor_on_bass: PerPitch<u64>,
    pub flat7_on_bass: PerPitch<u64>, // sum of mu where the anchor's minor 7th sounds too (colour only)
    pub first_bass: Option<PitchClass>, pub last_bass: Option<PitchClass>,
    bars: Vec<BarPrint>,              // distinct bar fingerprints, sorted, each with beats sounding per stream
    pub notes: u32, pub roles: RoleSet,
}
impl Heard {
    pub fn merge(&mut self, later: &Heard);   // sums; union of bars; first from self, last from later
    pub fn features(&self, key: Key, span: Span, each: impl FnMut(usize, f64)); // the one scoring path
    pub fn bars_heard(&self) -> u32;  pub fn config(&self) -> Config;  pub fn diatonic_fit(&self) -> f64;
}

pub struct KeyReader { /* beat scratch, reused between parts: no allocation per part in the indexer */ }
impl KeyReader {
    pub fn new() -> Self;
    pub fn hear(&mut self, parts: &[Part], grid: Grid) -> Option<Heard>;     // None: no pitched notes
    pub fn hear_bars(&mut self, parts: &[Part], grid: Grid) -> Vec<Heard>;   // one per bar (part 5)
    pub fn read(&mut self, parts: &[Part], grid: Grid, span: Span) -> Option<KeyReading>;
}
pub fn find_key(parts: &[Part], grid: Grid, span: Span) -> Option<KeyReading>;
pub fn find_song_key(sections: &[(&[Part], Grid)]) -> Option<KeyReading>;   // metre changes: hear, merge, read
pub fn read_heard(heard: &Heard, span: Span, model: &Model) -> KeyReading; // the bench passes fold models
pub fn explain(parts: &[Part], grid: Grid, span: Span) -> Option<Explanation>; // on demand, re-reads

pub struct Model<'a> { pub weights: &'a [f64; FEATURES], pub calibration: &'a Calibration,
                       pub thresholds: &'a Thresholds, pub not_tonal_below: f64, pub minor_shift: f64 }
pub const SHIPPED: Model<'static>;          // from weights.rs

pub struct KeyReading {
    pub chances: [f64; 24],                 // by Key::index, calibrated, sum 1
    pub answer: KeyAnswer, pub shown_chance: f64,
    pub best: Option<Key>, pub runner_up: Option<(Key, f64)>,
    pub colour: Colours, pub bars_heard: u32, pub config: Config, pub diatonic_fit: f64,
}
pub enum KeyAnswer { Key(Key), OrRelative(Key), HomeNote { tonic: PitchClass, leaning: Option<Mode> },
                     Unclear { why: Why, leaning: Option<Key> } }
pub enum Why { TooFewNotes, NotTonal, Spread }
pub struct Colours(u8);  // MIXOLYDIAN | I7 | BLUES | DORIAN | HARMONIC_MINOR | NO_SIXTH
pub enum HintSource { KeySignatureEvent, Name, Typed }
pub enum HintCheck { Agrees, InAnswer, Relative, Parallel, Fifth, Disagrees, Ignored }
impl KeyReading {
    pub fn check_hint(&self, hint: Key, from: HintSource) -> HintCheck; // a C-major signature event: Ignored
    pub fn fact(&self) -> KeyFact;
}
#[derive(Clone, Copy)] pub struct KeyFact(pub [u8; 8]);
// [0] layout 1; [1] kind 0 Key, 1 OrRelative, 2 HomeNote, 3 TooFewNotes, 4 NotTonal, 5 Spread;
// [2] best key index or 255; [3] runner-up index or 255; [4..6] shown chance u16 (x65535, big-endian);
// [6] P(best) x255; [7] colour bits. tonic(): Key and OrRelative give the likelier key's tonic,
// HomeNote gives its tonic, Unclear gives None. mode(): only Key and OrRelative.
```

`PerKey<T>` is `[T; 24]` indexed by `Key`, like `PerPitch`, so `indexing_slicing` stays denied.

**Input rules**

- **Roles.** Melody feeds the tune stream. Harmony, Unknown and Bass feed the harmony mass. Drums are never heard. The caller maps its role guesses: arp, pad and keys to Harmony; lead to Melody.
- **Pitch.** Notes come at the pitch heard: transpose applied, only notes that are played, muted notes left out. MIDI 2.0 per-note pitch is rounded to a semitone in the heard-notes layer.
- **Ghost notes.** A note is dropped when it is shorter than 1/8 of a beat and its velocity is under 1/8 of the 90th-percentile velocity of its stream (melody, or all other parts). The percentile is taken over the whole input. Velocity is used for nothing else (E7).
- **Hints are never inputs.** Key-signature events and keys in names don't enter the model; the notes alone decide. `check_hint` compares them afterwards. C-major signature events score 0.167 against reference keys (research, MIDI practice), so they are `Ignored`.
- **What the index hash must cover.** The MIDI index's note hash must cover the notes, the roles, the grid (ticks per beat, beats per bar, bar line) and the span.
- **Metre changes.** A song whose metre changes is read section by section, each section with its own `Grid`, and the `Heard` values are merged.
- **Damaged files.** The beat grid is clamped to 65,536 beats, as the draft does.

## Evidence and formulas

**Grid and metrical weight.** The beat is the `Grid` beat, so an eighth in 6/8. The metrical weight mu is a small integer that depends only on where the beat falls in its bar, never on where a stretch starts:

- 4 on a bar start;
- 2 on the middle beat of a bar of 4 or more beats, or on every third beat when the bar has 6, 9 or 12 beats (6/8, 9/8, 12/8);
- 1 everywhere else.

**For each beat j, in one pass:**

- **Ticks.** Each note adds its overlap with the beat to `tune` or `harmony`, as long as the beat starts within 2 bars of the note's start.
- **Anchor a_j (the beat's bass).** Notes count when they sound at any point in the first half of the beat. A pushed note held across the beat therefore counts on the beat, as musicians hear it.
  - If a Bass-role note sounds then, the lowest such key is the anchor.
  - Otherwise, take the lowest Harmony or Unknown key ℓ sounding then. It is the anchor if every key sounding is ℓ plus whole octaves, or if the next distinct key up is at least ℓ+5.
  - The melody is never the anchor.
  - The test only looks at intervals. There is no register cut-off, so moving the notes up or down can't change the anchor.
- **Chord set S_j.** The pitch classes of non-melody notes sounding for at least 1/4 of the beat (`overlap*4 >= beat`).
- **Bass and triad counts.** With anchor root r:
  - `bass[r] += mu`;
  - `major_on_bass[r] += mu` if r+4 and r+7 are in S_j. If r+3 sounds as well, it is heard as a #9, so C7#9 still counts as C major.
  - `minor_on_bass[r] += mu` if r+3 and r+7 are in S_j and r+4 is not;
  - `flat7_on_bass[r] += mu` if r+10 is in S_j.
- **First and last bass.** The anchor on the first bar start that has one, and on the last.
- **Bar fingerprint.** A wrapping sum of FNV-1a 64 hashes over (stream, onset minus bar start, key, length capped at 2 bars) for the notes starting in the bar. A sum doesn't depend on note order. A bar with no onsets gets no fingerprint.
  - `bars_heard` is the number of distinct fingerprints. A loop written out 4 times counts as the loop; a chord held for 4 bars counts as 1 bar.
  - The evidence sizes N_tune and N_acc are the beats sounding in each stream, summed over the first bar with each distinct fingerprint.

**Streams, with shares x_f[d] = c_f[t+d] / C_f (0 when C_f = 0) and size g_f = N_f / (N_f + 4) (E2 tests other forms):**

| # | Stream | Counts | Spans | Size | Why |
|---|---|---|---|---|---|
| 0 | Tune | `tune` | all | N_tune | The melody has its own table: rock and pop tunes are often minor over major chords (RS200) |
| 1 | Harmony | `harmony` | all | N_acc | The collection: which notes are used |
| 2 | Bass | `bass` | all | N_acc | Home is where the bass sits on strong beats |
| 3 | Major on bass | `major_on_bass` | all | N_acc | The home chord over its own root. Extensions that don't stand on their bass can't flip it (G7alt's Eb-G-Bb is not on G) |
| 4 | Minor on bass | `minor_on_bass` | all | N_acc | Same, for minor chords |
| 5 | First bass, loop | one-hot | Loop | 1 | Where a loop really starts. Provisional (see training) |
| 6 | First bass, song | one-hot | Song | 1 | In POP909-CL the first downbeat bass is the tonic in 71% of major and 74% of minor songs |
| 7 | Last bass, song | one-hot | Song | 1 | Songs end at home (72% major, 74% minor). Never used on loops, which often end on V |

A Window gets no streams 5 to 7. Cut clips start on the tonic only 36% of the time (checked on 7,860 8-bar clips).

**Feature vector.** `FEATURES = 8*24 + 1 = 193`, the same size as the draft:

- index f*24 + 12*[minor] + d has value g_f * x_f[d];
- index 192 is [minor].

**Collection and colour facts.**

- Diatonic fit D is the largest share of tune plus harmony ticks lying inside any one of the 12 diatonic collections. Chromatic gives 0.58, octatonic 0.63, whole-tone 0.67; an even blues scale gives about 0.83; diatonic music gives more than 0.9.
- The colour shares v[d] come from tune plus harmony, counted from the tonic.

**Why the exactness holds.**

- Every count is an integer, so transposed, repeated, split or reordered input gives the same integers, moved to new pitch classes where it was transposed.
- A share a/b is a correctly rounded division, so 4a/4b gives the same float as a/b.
- Scores sum over streams, then degrees, in a fixed order. Nothing reads absolute pitch.
- The softmax adds its 24 terms after sorting them in ascending order, so a rotated set adds up to the same bits.
- `Heard::merge` adds integers, so reading bar by bar and adding up is exactly the whole reading. The ghost floors come from the whole input.

## Model and decoding

**Score.**

s(k) = Σ_f Σ_d W[f][m][d] · g_f · x_f[t+d] + β·[m = minor], summed in fixed order.

**Weights.** keyfit fits W as **W[f][m][d] = w_f · L_f,m[d] + δ_f,m[d]**:

- L is a counted log-probability table;
- w_f is one trust number per stream;
- δ is a per-degree correction with its own L2 penalty λ_δ.

With λ_δ at infinity this is the pure tables model. With λ_δ = 1e-4 it is the draft's 193-weight model. Only W and β ship; L, w and λ_δ go in the `weights.rs` header.

**Chances.**

- σ = max(0.05, Σ_j τ_j·h_j(ln bars_heard) + γ_config + γ_span).
  - h_j are tent-shaped (piecewise-linear) functions with knots at 1, 2, 4, 8, 16, 32 and 64 bars; outside that range the value is clamped.
  - γ has offsets for MelodyOnly, BassOnly and Accompaniment (Full = 0), and for Loop and Song (Window = 0).
  - That is 12 numbers, with no step at any bucket edge.
- z(k) = σ·s(k) + minor_shift·[minor]. minor_shift is 0 now; it is a slot for correcting a different minor share, from counts.
- P(k) = exp(z(k) − max z) / (the 24 exps summed in sorted order).

**Ties.** If the top two chances are exactly equal:

- a relative pair gives OrRelative, naming the major key;
- a parallel pair gives HomeNote with `leaning: None`;
- anything else gives Unclear(Spread) with no leaning.

Lower ties are broken by the interval above the best key's tonic, then major first. Every one of these rules moves with the notes, so transposition stays exact.

**Graded answer and colour.** See the next section.

## Training and data (now with POP909, later with Solomon data)

**Data that may ship.**

- POP909-CL: keys, key changes and bars checked by people. MIT licence; credited to Yao, Chen, Dubnov and Berg-Kirkpatrick.
- E10 adds McGill Billboard (CC0) and ChoCo's CC BY partitions, rendered from their chord charts.
- keyfit reads a `licence` field on every item and refuses anything that isn't MIT, CC0 or CC BY. It refuses Solomon's items unless `--private-counts` is given, and then uses them for counts only.
- Measuring only, never shipped: POP909's machine labels, RS200, Lakh key events, HookTheory, CoCoPops, and ChoCo's JAAH, CASD and Mozart data (NC-SA).

**Tables L.**

- For each mode, L is the mean over training songs of each song's stream shares, counted up from its labelled tonic. A song that changes key counts once per key region.
- Smoothing: L[d] = ln((Σ shares + 2/12) / (songs + 2)).
- First and last tables come from whole songs. The loop first-bass table comes from opening clips.
- **Tables are cross-fitted.** Inside the training folds, each song's features use tables counted without its inner fold (5 inner folds). Uncross-fitted tables inflate their own trust: Design 1's quick check halved the inflation of the 144-cell bass-move table this way, and the current Rust tables run (8-bar 77.5%, log-loss 0.93) was not cross-fitted.
- The shipped tables use all songs.

**Cases.**

- **Whole one-key songs** (Song), 778 of them.
- **Windows** of 1, 2, 4, 8 and 16 bars lying wholly inside one key region: at most 16 per song per length, picked by a hash of (id, start bar).
- **Opening clips:** each song's first L bars (L = 1, 2, 4, 8, 16) from its first downbeat with notes, inside the first key region. These are Loop stand-ins.
- **Part types:** every clip also appears in four more forms: melody only, upper piano only (Accompaniment), derived bass only (piano anchors by the relative test, as a Bass part), and piano.
- **Weights:** each (span, length, part type) set has total weight 1, and whole songs have 5.
- Clips that cross a key change are scored, so the bench stays comparable with C#, but never fitted on.

**Fit.**

- `fit.rs` (Newton with Cholesky and backtracking) gets a penalty per weight (`l2: &[f64]`): λ_w = 1e-4 on w and β, λ_δ on δ.
- λ_δ is chosen from {∞, 1, 1e-1, 1e-2, 1e-3, 1e-4} by inner-fold log-loss. The rule: take the **largest λ_δ within 0.01 nats and 0.3 points of the best**. This leans towards the tables on purpose, because tables are what Solomon's counts can refill.
- No weight is dropped for being negative. Overlapping streams split credit, and the reasons are worked out from W, not from w.
- Calibration (12 numbers) is the same conditional logit on the features s(k)·h_j, s(k)·[config] and s(k)·[span]. `fit::calibration` is generalised to n terms.
- Everything is deterministic: the same data gives the same bits.

**Folds and leaks.**

- `fold = fnv1a64("pop909/" + id) % 5`, so POP909 and POP909-CL put each song in the same fold. Today keyfit uses a fixed shuffle; that changes.
- Songs whose transposition-invariant note hashes match share a fold.
- Grouping by artist is E13, if POP909's song index is fetched.
- **Fold 0 is frozen.** Every choice (features, μ, λ, λ_δ, gate, thresholds) is made on folds 1 to 4 only.
- Calibration and thresholds are always fitted on out-of-fold scores: nested, inside each outer fold.
- `keyfit --final` is the only command that touches fold 0. Each run appends a line (date, version, commit, results) to `tools/theorybench/test-runs.md`.
- Every report prints the gap between training and held-out log-loss as an alarm for too many weights.
- **Shipping:** after the go decision, everything is refitted on all 906 songs. Calibration and thresholds then come from 5-fold out-of-fold scores.

**Build order with POP909-CL only.**

1. **Bench PR:**
   - hash folds and the frozen fold;
   - span and part-type cases;
   - finders that return 24 chances;
   - the new metrics;
   - λ per weight in `fit.rs`.
2. **mo-harmony PR, on placeholder weights:**
   - `Heard` and `KeyReader`;
   - scoring, calibration, graded answers, colour, `KeyFact`, `explain`;
   - the hand cases and invariance tests.
   - Port check: λ_δ = 1e-4 with streams 5 to 7 off must land within 0.7 points of the draft's 79.8% at 8 bars.
3. **Experiments E1 to E8 and E11 to E14** on folds 1 to 4; freeze the choices.
4. **E9 and E10**, the modal fixes, gated as below.
5. **`keyfit --final`** once, then the merge PR with the report and the generated `weights.rs`.

**Experiments.**

A change is kept only if it meets all of these:

- on folds 1 to 4, paired and song-clustered, either 8-bar accuracy rises with its interval above 0, or (for shape or calibration choices only) log-loss falls by at least 0.01 with accuracy down no more than 0.3 points;
- no length, part type or whole-song set gets worse beyond its interval;
- no gate hand case breaks;
- major and minor are reported apart.

| # | Choice | Variants (start first) |
|---|---|---|
| E1 | Shape | λ_δ grid above; tables cross-fitted; NB-1 reference (w = 1, δ = 0, calibrated) |
| E2 | Evidence size | N/(N+4) beats; g = 1; n0 of 2 or 8; ln(1+N/4). Judged on the single-stream part types |
| E3 | Anchor and metre | Gap of 5 vs 7 semitones; mu 4/2/1 vs bar starts only vs flat; hypermetre (×2 on bar 1, ×1.5 on bar 3, counted from the song's or loop's own first bar, never the window's), kept only if loop units of 4 bars or more keep repeat invariance |
| E4 | Chords | On the bass only; all triads with on-bass counted twice (Design 1); none; shells (root, 3rd, 7th, no 5th); a dominant-7th-on-bass stream. Also the metamorphic flip rates |
| E5 | Loop start and tune home | Loop first-bass weight fitted on opening clips vs tied to the Song one; a tune phrase-end stream (a melody note followed by at least a beat with no melody onset); a tune-on-strong-beats stream. Judged on opening clips and melody-only clips |
| E6 | Bass moves | Off; on only if accuracy rises, not just log-loss (Design 1's check: 8-bar 80.8 to 79.8 even cross-fitted) |
| E7 | Velocity | Relative ghost gate; absolute 4096; none; √velocity weighting |
| E8 | Note weight | Ticks capped at 2 bars; presence per beat; onsets; ticks × μ |
| E9 | Colour components | Candidates of 12 tonics × {Ionian, Mixolydian, Aeolian, Dorian, Harmonic minor}. The pitch streams (0 to 2) swap degrees 10↔11 or 8↔9; each component has a log base rate from counts; a key's chance is the sum over its components. Keep if POP909-CL loses at most 0.5 points, fifth errors don't rise, and the modal cases improve |
| E10 | Home tables from charts | Billboard (CC0) and ChoCo CC BY, rendered to a bass plus block chords, feed only the bass, chords-on-bass and first/last tables (they have no voicings or melody). Mode is a silver label from the tonic chord. Gate: POP909-CL not worse, the hand cases, and RS200 tonic accuracy (measuring only) |
| E11 | Calibration | The draft's 2-number curve vs knots vs knots + config + span. Judged by ECE in each bucket |
| E12 | Thresholds | Per knot vs one; precision met with more answers shown |
| E13 | Fold grouping | By song vs by artist. If by-artist scores are more than 1 point lower, group by artist |
| E14 | Training mix | Set weights as above vs by use; one W vs a separate W for Song |

**Evidence that sets expectations.**

- On the same 9,063 8-bar clips, with accompaniment plus melody, tables gave 80.0% (log-loss 0.948) and per-degree weights 79.5% (log-loss 0.776). Accuracy came out level, and per-degree gave far better probabilities (keys2).
- Per-degree plus beat triads gave 81.4%, log-loss 0.697, 11.7% relative errors (keys3).
- The Rust draft per-degree model gave clips 45.0 / 59.8 / 72.1 / 79.8 / 83.2%, whole songs 90.7%, key-change songs 78.1%, 8-bar relative errors 13.4%, and 8-bar major / minor 85.1 / 72.6%. With its 2-number calibration, ECE per bucket was 0.02 to 0.09.
- An earlier prototype in the scratchpad (`keydesign/`) had pop-fitted weights, colour components and a hand-set loop-opening weight. I re-ran it:
  - With the base rates fitted on pop (about −2) and no opening weight:
    - Cm-Ab-Eb-Bb read Eb 0.53 / Cm 0.42;
    - the blues read F 0.45;
    - C-Bb-F-C read F 0.82.
  - With an opening weight of 0.8 to 1.0 and base rates of −0.4:
    - Cm-Ab-Eb-Bb read Cm 0.61 to 0.65;
    - the blues read C 0.55 to 0.61;
    - gospel I-I7-IV-iv read C 0.76 to 0.85;
    - C-Bb-F-C still read F 0.47 to 0.51 over C 0.38 to 0.44.
  - So the modal fixes need E10, not only E9.

**When Solomon's data arrives.**

1. **Measure first.** Run the shipped reader against C# by genre folder, role and length, with pack-clustered intervals. The C# profile is in-sample on 95 of his kits (MusicTheory.cs), so those kits are reported apart.
2. **Counts, which rule 5 allows.**
   - Re-count L pooled with POP909-CL, which is worth n0 = 200 songs.
   - minor_shift = logit(his minor share) − logit(0.427).
   - Colour base rates (E9) from counts.
   - Check the not-tonal gate's refusal rate.
   - A flatter, more chromatic gospel or jazz table weakens the note streams on its own, and shifts weight to the bass, chord and start evidence.
3. **Weights.** W, β and the calibration stay public-data fits until he answers the rule-5 question. Then:
   - refit with folds by vendor plus pack, after dedup by transposition-invariant hash (a loop shipped in 12 keys counts once);
   - weight each genre equally;
   - choose λ by leaving one pack out;
   - keep one model for every genre.
   - A change ships only if no genre gets worse.

## Calibration and the graded answer

**Calibration** is fitted and reported on nested out-of-fold scores. Log-loss is the main measure; the bench also prints:

- Brier score;
- ECE with 15 equal-mass bins and song-clustered intervals;
- reliability tables for the top key, the relative pair (P(k) + P(rel)) and the tonic (P(t major) + P(t minor)), per knot bucket and per config.

Loop calibration rests on opening clips and is provisional until Solomon's packs are measured.

**Graded answer, in order:**

1. Fewer than 4 notes, or under 1 beat sounding: `Unclear(TooFewNotes)`.
2. D < `not_tonal_below`: `Unclear(NotTonal)`.
   - Ship the 0.5th percentile of D over out-of-fold POP909-CL clips with at least 4 notes, kept within 0.70 to 0.80. Start at 0.75.
   - It must refuse at least 95% of a generated null set (whole-tone, octatonic, chromatic, random 12-note) at 2 bars or more.
   - It must pass the blues-scale riff, and later refuse under 1% of Solomon's gold parts.
3. P(k1) ≥ τ_one: `Key(k1)`.
4. If the runner-up is the relative and P(k1) + P(rel) ≥ τ_pair: `OrRelative(k1)`. The notes are sure, home isn't, and the app can still say "3 flats".
5. If P(t major) + P(t minor) ≥ τ_home: `HomeNote(t)`, leaning to the likelier mode. If steps 4 and 5 both pass, the larger sum wins.
6. Otherwise: `Unclear(Spread)`, leaning k1.

**Thresholds.** Each threshold is the smallest value on a 0.01 grid at which the song-clustered 95% upper bound of the error, **measured only on the answers of that kind**, meets its target. This guards against the winner's curse: the top answer looks surer than it is, and only its own cases show that.

- They are set at each knot, made non-increasing with length, and interpolated on ln(bars), so a part growing by one bar can't flip.
- Coverage is reported beside each threshold.

| Bars heard | 1 to 2 | 3 to 5 | 6 to 11 | 12 to 23 | 24+ and Song |
|---|---|---|---|---|---|
| Target error, Key | 15% | 10% | 10% | 8% | 5% |
| τ_one, start | 0.85 | 0.80 | 0.80 | 0.75 | 0.70 |
| τ_pair, start (truth outside the pair ≤ 10%) | 0.90 | 0.90 | 0.90 | 0.90 | 0.90 |
| τ_home, start (wrong tonic ≤ 10%) | 0.85 | 0.85 | 0.85 | 0.85 | 0.85 |

**Colour.** One rule set, read after the key, from the shares v (floors at 0.04 unless stated). A colour never changes the key.

| Key | Colour | Rule |
|---|---|---|
| Major | Blues | v[3], v[4] and v[10] all at least 0.04. A plain I7-IV7-V7 blues is "Blues" |
| Major | Mixolydian | Not Blues, v[10] ≥ 0.04 and v[10] ≥ 2·v[11]. Flagged I7 when `flat7_on_bass[t]` is at least 30% of `bass[t]` |
| Minor | Harmonic minor | v[11] ≥ 0.03 and v[11] ≥ v[10]/2 |
| Minor | Dorian | v[9] ≥ 0.04 and v[9] ≥ 2·v[8] (both minor colours together read "melodic minor") |
| Minor | No sixth | v[8] + v[9] < 0.02: Aeolian and Dorian can't be told apart |

These numbers are set by hand. HookTheory's mode labels measure them (measuring only).

## Explanations

**Exact shares.** Each stream's term is

Δ_f = σ · Σ_d (W[f][m1][d]·g_f·x_f[t1+d] − W[f][m2][d]·g_f·x_f[t2+d]).

With the minor term and minor_shift added, the terms sum exactly to ln P(k1)/P(k2). The comparison key is the runner-up for a Key, the relative for a pair, and the parallel for a home note.

**What is shown.** Up to three terms that are at least 0.3 nats and at least 10% of the margin, plus one term against. Wording always comes from counts in `Heard`, never from weight values. Chords are spelled in the key (`spell.rs`).

**Headlines**

- "C minor (86%). Other reading: Eb major (11%)."
- "C minor or Eb major (same notes, 3 flats), leaning C minor (58/34): which note is home isn't clear."
- "Home note C; no third is played, so major or minor isn't clear." / "Both thirds sound: blues."
- "No clear key; closest C minor (41%) or Ab major (22%)." / "No key: at most 67% of the notes fit any one scale." / "Too few notes."
- With 2 bars heard or fewer: "Only one bar of notes, so this is a guess."

**Reasons**

- Bass: "C is in the bass on 9 of 16 strong beats."
- Start: "The loop starts with C in the bass." / "The song starts and ends on C."
- Chords: "Cm sounds over its own root on 6 strong beats; Eb on 2."
- Notes:
  - "The chords use C D Eb F G Ab Bb, the notes of C minor and of Eb major."
  - "B natural, the leading note of C minor, sounds 6% of the time."
  - "Bb and Eb don't fit C major."
- Minor term, shown only when it decides: "A close call; minor keys are a little less common in the music it learned from."

**The other reading.** "Eb major is the other reading (34%): same notes, but the music doesn't settle on Eb."

**Colour**

- "Dorian colour: A natural for 4 beats, Ab never."
- "I7 colour: Bb over the C bass, B never."
- "Blues colour: both Eb and E, with Bb."
- "Harmonic minor: B natural over G."

**Hints and windows**

- "The file name says Cm, which agrees."
- "The file's key signature says C major (often a default); not used."
- "Cut from a longer part, so where it starts and ends isn't counted."

## Speed

- **The note pass.** One pass, no sorting. Each note touches at most 2 bars of beats.
- **Per beat** the reader keeps:
  - 2 bytes of chord set;
  - the lowest Bass key;
  - the lowest two distinct other keys;
  - two partial sums.
- **Bars.** Each bar's fingerprint is one u64. Distinct bars come from sort plus dedup.
- **Scoring.** 24 keys × 8 streams × 12 degrees is about 2,300 multiply-adds, then a sort of 24 numbers.
- **Estimate.** About 10 to 30 µs for a 16-bar, 300-note part on one core (target: under 1 ms). A 20,000-note song takes about 1 to 2 ms.
- **Allocation.** `KeyReader` reuses its scratch, so the indexer allocates nothing per part.
- **On demand only.** `KeyReading` carries no `Heard`; explanations come from `explain`, which re-reads in microseconds.
- **Threads.** `find_key` is single-threaded and `Sync`. The indexer batches with rayon at low priority, one `KeyReader` per thread (`map_init`).
- **Determinism.** No HashMap on the path; float sums in fixed order.

## API for the MIDI page and the Songs page

**MIDI page.**

- Call `KeyReader::read(parts, grid, Span::Loop)` for a pack part or clip of a whole number of bars, at most 32. Longer full-song exports use `Span::Song` (the cut-off is an open question).
- Store `reading.fact()` (8 bytes) under (note hash, `KEY_FINDER_VERSION`); recompute when the version differs.
- Show the answer, the shown chance and the colour. Show `check_hint` against name keys (`Key::parse` on name tokens) beside the answer.
- Search uses `KeyFact::tonic()`, `mode()` and the colour bits.

**Songs page.**

- `find_song_key(&[(parts, grid), ...])` reads the notes the arrangement plays, at the pitch heard, one section per metre, with drums dropped by the drum gate.
- It is read-only until the twin tests pass.
- Show the typed `musical_key` beside the answer, via `check_hint(.., Typed)`.

**Version and hashes.**

- `weights.rs` holds W, β, the calibration, the thresholds, the gate, `SHIPPED_HASH` and `HASH_VERSION`.
- A test recomputes a hash over every shipped constant (the colour constants included) and requires `SHIPPED_HASH` to match it and `HASH_VERSION == KEY_FINDER_VERSION`. `keyfit --write` sets `HASH_VERSION` to version + 1 when the hash changes, so CI fails until `mod.rs` is bumped too.
- A golden test hashes every hand case's chances, so a code change that moves an answer without a version bump also fails (rule 7).

## Bench changes and the merge rule

**Bench changes (theorybench).**

1. **Folds.** Hash folds, the frozen fold 0, dedup, optional artist grouping, and the `test-runs.md` log.
2. **Cases.** Spans, part types and opening clips. Results by start bar mod 4 and by metre.
3. **Finders.** Every finder returns 24 chances; the old finders return one-hot chances.
4. **Baselines in every run:**
   - the C# app;
   - Aarden-Essen with an out-of-fold temperature;
   - NB-1;
   - melody plus harmony only;
   - the draft per-degree model.
5. **Columns:**
   - exact, tonic, MIREX (ascending fifth only, plus both fifths);
   - error kinds (fifth up, fifth down, relative, parallel, other);
   - major and minor apart;
   - log-loss, Brier, ECE (equal-width and equal-mass);
   - reliability tables;
   - risk-coverage and AURC;
   - the graded-answer mix, each kind's precision with its upper bound, and coverage at the target.
6. **Against C#.** The song-clustered paired bootstrap decides; McNemar is printed for information. A disagreement table shows only-C#-right, only-Rust-right and both wrong, by relation.
7. **`keyfit` options:** `--design` (folds 1 to 4), `--final` (fold 0, logged), `--ablate`, `--lambda-delta`, `--sources` (licence-checked), `--write`.
8. **Other modes:**
   - `--null`: the atonal null set.
   - `--invariance`: transposition, order, part split, repeat, octave, velocity, ghost notes, drums.
   - `--metamorphic`: on POP909-CL clips, add diatonic 9ths and 13ths to labelled chords, and b9, #9 and b13 to labelled dominants; print the flip rates.
   - `--dump answers.jsonl` and `--timing` (p50 and p99 µs, one thread).
9. **On Solomon's machine (read-only, prints counts only):**
   - `theorybench twin`: library.db typed keys and C#'s detected key against Rust, clustered by folder; prints 30 disagreements for him to judge by ear.
   - `theorybench packs <folder>`: silver labels from names, deduped, clustered by pack.

**Merge rule.** It is decided on frozen fold 0 (about 181 songs), against C# on the same bench cases: clips labelled at the middle, those that cross key changes included. The 5-fold numbers are printed for context but carry selection bias, so they never overrule fold 0. The C# profile is flagged as in-sample on 335 POP909 songs.

1. **Clips of 1, 2, 4 and 8 bars:** exact accuracy above C#, with the paired 95% interval above 0. The 8-bar gain must be at least 3 points (C# is at 75.2%; the draft is at +4.6).
2. **16-bar clips:** not worse; the paired interval's low end at or above −2 points.
3. **Whole one-key songs:** not worse; the low end at or above −3 points, and the 5-fold point estimate at or above 87.8%.
4. **Key-change songs:** the 5-fold estimate no more than 5 points below 69.5%.
5. **Minor and relative keys at 8 bars:**
   - minor accuracy not below C#'s 72.9% beyond its interval, and the major/minor gap printed;
   - relative share not above C#'s 12.6% beyond its interval (the measured models give 11.7 to 13.4%).
6. **Calibration:**
   - ECE (equal-mass) at most 0.04 in every bucket for full mixes and 0.06 for single-stream configs;
   - log-loss below the calibrated Aarden-Essen in every bucket.
7. **Graded answers:** every kind meets its targets on its own cases; at C#'s coverage, precision is higher.
8. **Behaviour:**
   - every gate hand case passes;
   - invariances hold bit for bit;
   - flip rates are under 2% for diatonic extensions and under 5% for altered dominants;
   - p99 under 1 ms;
   - 1 thread and 16 give the same output;
   - a refit gives identical bits;
   - the version is bumped.

**Later, before the Songs page trusts the Rust key (Solomon's data):**

- no genre folder worse than C# beyond its pack-clustered interval;
- the precision targets met, or τ raised with a version bump;
- most of the audited disagreements judged in Rust's favour;
- agreement with name keys reported both as exact and as "name key inside the shown answer".

## Test cases

**Conventions** unless a case says otherwise:

- 4/4, ppq 480, Loop span, velocity 32768.
- Chords last one bar each. They are played as whole notes on a Harmony part, with each chord tone in MIDI 60 to 71.
- The roots are whole notes on a Bass part, in MIDI 36 to 47.

**G** means a merge gate. **T** means a target: it is tracked, expected to fail on POP909-only data, and must pass before Solomon's genres are trusted.

| # | Input | Expected |
|---|---|---|
| 1 | Cm, Ab, Eb, Bb | G: the top two are {C minor, Eb major}; not Key(Eb major); P(C minor) is higher than in cases 2 and 3. T: C minor on top. The prototype gave 0.42 without a loop-start weight and 0.61 to 0.65 with one |
| 2 | Eb, Bb, Cm, Ab | G: Eb major on top (prototype 0.67 to 0.83), C minor second; the answer is Key or OrRelative of Eb |
| 3 | Ab, Eb, Bb, Cm and Bb, Cm, Ab, Eb | G: the top two are {Eb major, C minor}; never Key(C minor); Ab major and Bb major never on top |
| 4 | Case 1 written out ×2 and ×4; case 1 as a Window | G: ×2 and ×4 give chances bit-identical to case 1. As a Window: the top two are {C minor, Eb major}, and P(C minor) is no higher than in the Loop reading |
| 5 | Case 2 plus a C-major signature event and the name "loop_Cmaj"; case 1 plus the name "Dark Keys Cm 90bpm" | G: there is no hint input, so the chances are identical by construction. check_hint: the C-major event gives Ignored; "Cmaj" against Eb gives Disagrees; "Cm" against case 1 gives Agrees, or InAnswer if the answer is a pair |
| 6 | Quick-change blues: C7 F7 C7 C7, F7 F7 C7 C7, G7 F7 C7 G7, as root-position 7ths | G: C (major or minor) in the top two; never Key(F major) or Key(F minor). T: Key(C major) or HomeNote(C), colour Blues. Prototype: F 0.45 with pop base rates; C 0.55 to 0.61 with colour components |
| 7 | Case 6 plus an eighth-note tune on C Eb F Gb G Bb (C4 to Bb4), each 4-bar phrase ending on a half-note C4 on beat 1 | G: not NotTonal; never Key(F). T: C major, colour Blues |
| 8 | Melody only, 2 bars of eighths: C4 Eb4 F4 Gb4 G4 Bb4 G4 F4, Eb4 C4 Eb4 F4 Gb4 F4 Eb4, then a half-note C4 | G: not Unclear(NotTonal) (D is about 0.8). T: tonic C on top |
| 9 | Mixolydian vamp C, Bb, F, C | T: C major on top, colour Mixolydian. A declared failure on POP909-only data: the prototype gave F 0.47 to 0.82 and C 0.06 to 0.44. E10 is the planned fix |
| 10 | Dorian: Cm9 for 2 bars (C2 under Eb4 G4 Bb4 D5), then F9 for 2 bars (F2 under A3 Eb4 G4) | G: C minor on top (prototype 0.54 to 0.78); Bb major not on top; colour Dorian |
| 11 | ii-V vamp Dm7, G7, as a 2-bar Loop | G: G major never on top. T: D minor on top with colour Dorian, or a pair; never Key(C major) |
| 12 | Dm7, G7, Cmaj7, Cmaj7 | G: C major on top (the first bass must not overrule the cadence). T: Key(C major) |
| 13 | Melody only, 4 bars, as (start beat, length, MIDI): (0,1,60) (1,.5,63) (1.5,.5,65) (2,1,67) (3,1,70) (4,1.5,67) (5.5,.5,65) (6,2,60) (8,1,60) (9,.5,63) (9.5,.5,65) (10,1,67) (11,.5,70) (11.5,.5,72) (12,1,70) (13,1,67) (14,2,60) | G: C minor on top (prototype 0.70); runner-up Eb major, F minor or G minor; colour No sixth; never a key alone but C minor. T: Eb major second; the answer C minor, or C minor or Eb major. (2026-11-01: F minor added to the runner-ups and the targets added, after three independent judges. The first version allowed G minor but not F minor with no reason given: both hold all five notes and are a fifth from C minor, and C, the long first and last note, is F minor's fifth. Four classic key profiles put F minor or G minor ahead of Eb major too. Every finder built had F minor and Eb major within 0.01 of each other for second. Eb major stays the wish, as targets.) |
| 14 | Dm7, G7alt (G2 under B3 F4 Ab4 Bb4 Eb5), Cmaj7, A7(b9 #9) (A2 under C#4 E4 G4 Bb4 C5). Twins: plain Dm7 G7 Cmaj7 A7, and rich Dm11 G13 Cmaj9 A7(b9 #9) | G: the top key is C major or D minor in all three; F major and C minor never on top; P(C major) for the altered version is at least the plain version's minus 0.15. T: C major on top. The first bass D and the A7-to-Dm7 wrap honestly point at D minor (prototype: C 0.40 to 0.58, Dm 0.14 to 0.36) |
| 15 | Minor ii-V-i: Dm7b5, G7, Cm, Cm | G: C minor on top; colour Harmonic minor |
| 16 | Bass only: C2, Ab1, Eb2, Bb1 as whole notes | G: the top two are {C minor, Eb major}; the answer is not Key. T: C minor on top |
| 17 | Bass only, quarter notes: G2 Bb2 C3 D3, Eb3 D3 C3 Bb2, A2 C3 D3 F#2, then a whole-note G2 | T: G minor on top |
| 18 | Bass only, 2 bars of eighths: C2 C2 C2 G2, repeated | G: the top key's tonic is C; the answer is HomeNote(C) or Unclear(Spread); never Key |
| 19 | Held Cm7 (C3 Eb4 G4 Bb4), Harmony only, 4 bars | G: C minor on top; bars heard = 1; not Key(Eb major); colour No sixth; the "guess" line is shown |
| 20 | The same chord inverted (Eb3 G3 Bb3 C4), no bass | G: the top two are {C minor, Eb major}; not Key. Eb3 to G3 is 4 semitones, so there is no anchor |
| 21 | C4 E4 G4 over C2, held 4 bars | G: C major on top; never Key of any other key |
| 22 | Power chord C2 G2 C3 held 4 bars; a C2 drone (Bass only) held 4 bars | G: tonic C on top; HomeNote(C) or Unclear; never Key |
| 23 | Melody only, 4 bars of eighths: whole tone (C D E F# G# A#), octatonic (C Db Eb E F# G A Bb), chromatic (60 + i mod 12) | G: Unclear(NotTonal) for all three (D = 0.67, 0.63, 0.58) |
| 24 | Am, F, C, G, and its twin C, G, Am, F | G: Am-first gives the top two {A minor, C major} and not Key(C major); C-first gives C major on top. T: A minor on top for Am-first (prototype: 0.42 without a loop-start weight, 0.61 to 0.65 with) |
| 25 | Am7, Dm7, G7, Cmaj7 | G: the top two are {C major, A minor}; P(best) < 0.9 |
| 26 | Andalusian: Am, G, F, E, played twice | G: A minor on top (prototype 0.89 to 0.96); colour Harmonic minor |
| 27 | Gospel: C, C7, F, Fm, C, C | G: C major on top; F major and F minor not on top; colour Mixolydian with the I7 flag |
| 28 | Autumn Leaves as a Song: Cm7 F7 Bbmaj7 Ebmaj7 Am7b5 D7 Gm Gm | G: G minor on top (prototype 0.52 to 0.69) |
| 29 | Rootless, no bass: Dm9 (F4 A4 C5 E5), G13 (F4 A4 B4 E5), Cmaj9 (E4 G4 B4 D5), Cmaj9. Also C9 without root (E4 G4 Bb4 D5) held 4 bars | G: in the first, the top is C major or A minor and the answer is not Key. In the second, not Key. Both explanations say there is no bass line |
| 30 | The seven notes C4 to B4 held together for 1 bar | G: the top two are {C major, A minor}; never Key |
| 31 | Three notes; no pitched notes (drums only) | G: Unclear(TooFewNotes); `find_key` returns None |
| 32 | Every case above, changed: moved by −6 to +5 semitones (all notes kept in MIDI 12 to 115); notes shuffled; a part split in two of the same role; a drum part added; harmony doubled an octave up; ghost notes added (1/32 beat long, velocity 1/16 of the others); all velocities halved (inputs use even velocities) | G: when moved, the chances rotate exactly and the answer, runner-up and colour move with them. Every other change gives bit-identical chances |
| 33 | A synthetic 64-bar song with notes held across bar lines, read bar by bar (`hear_bars`) and merged | G: equal to `hear` of the whole, every count bitwise; first bass from the start, last bass from the end |
| 34 | Damaged input: ppq 0, negative lengths, i64 extreme ticks, key 127, a 10-million-beat span, an empty part list | G: no panic; None when there are no pitched notes; otherwise chances sum to 1 within 1e-12 |

## What part 5 adds

- **Window scores.** `hear_bars` gives integer statistics for each bar. μ never depends on where a window starts, so any window's `Heard` is a merge (prefix sums for the arrays, and a union for distinct bars). Its `Window` score is the emission of a semi-Markov key path with learned durations: a home key, plus regions relative to it, plus tonicised spans labelled "tonicises X", as the local-key research recommends.
- **Chord evidence.** Part 3's soft chord roots (summed over its N best readings) become a new stream, along with cadence evidence. W is refitted and the version bumped. Nothing here is replaced.
- **The song's key.** It stays part 2's Song reading unless an ablation shows the key path does better.
- **Lifts.** Part 5 also handles lifts, which part 2's last-bass feature can't (a final lift pulls the last bass).

## What was taken from where

- **Design 3 (base):**
  - per-degree evidence over few streams;
  - integer accumulators and distinct bars;
  - μ 4/2/1 by position in the bar;
  - the diatonic-fit gate;
  - the whole leak protocol (hash folds shared with POP909, dedup, artist grouping, frozen fold, the logged `--final`, nested calibration and thresholds, the train/held-out gap alarm);
  - the winner's-curse checks;
  - metamorphic flips, damaged-input tests, minor_shift;
  - the explicit KeyFact bytes;
  - the twin and packs tools, the M+H and calibrated Aarden-Essen baselines, `--dump`;
  - the relative-melody idea.
- **Design 1:**
  - counted tables under the fit, now as W = w·L + δ;
  - cross-fitting and the NB-1 reference;
  - the relative bass test with no register cut-off;
  - triads standing on the bass;
  - exact merging and `hear_bars`;
  - check_hint with C-major signatures Ignored;
  - stream dropout;
  - thresholds interpolated on ln(bars);
  - exact reason shares and the constant hash tied to the version;
  - the gospel, Andalusian, Window-vs-Loop, octave, ghost and merge cases.
- **Design 2:**
  - the collection-then-home framing;
  - separate loop and song first-bass weights, and no last bass on loops;
  - the relative ghost gate;
  - the reusable `KeyReader` and lazy explanations;
  - the licence check in keyfit;
  - the measured colour-note shares and the predicted failures;
  - E9 and E10;
  - per-bucket temperatures, made continuous here;
  - the I7 flag and the count-based wording;
  - the null set;
  - the rootless, ii-V vamp, ii-start cadence, Autumn Leaves, root-and-fifth, cluster and blues-with-tune cases.
- **New in this synthesis:**
  - the sorted softmax sum and the rotation-safe tie rule;
  - the Gate/Target split, backed by the re-run prototype;
  - calibration from knots plus config plus span offsets;
  - deciding the merge on fold 0;
  - counting the #9 as a major third.

## Judges' points and how each was handled

| Point | Handling |
|---|---|
| No absolute pitch thresholds | Only the relative anchor test; case 32 moves every case across registers |
| Bit-exact claims must be true | Integer counts, the sorted softmax sum, the tie rule; the claims are listed with the conditions under which they hold |
| Loop repeats and held chords | Sizes and calibration length come from distinct bars, not raw beats |
| Evidence independent of window start | μ by position in the bar; hypermetre only in E3, with phase from the song's own bars; part 5 merges statistics, never scores |
| Loops reading song ends | A separate loop first-bass weight; no last bass on loops; opening clips as stand-ins; a Loop calibration offset; marked provisional |
| The modal problem | Declared up front, with prototype numbers; cases 6, 9 and 11 are targets; E9, E10 and Solomon's counts are the planned fixes |
| Per-degree vs tables | The hybrid, with λ_δ set by experiment; only identical-case comparisons are cited (keys2; the Rust tables run was not cross-fitted) |
| Calibration not flexible enough | Knots + config + span; the ECE bar is checked against that flexibility |
| Set coverage vs singleton precision | Each grade's precision is measured on its own cases |
| Style-free not-tonal gate | Diatonic fit, set as a percentile, checked on the null set; the blues riff (case 8) must pass |
| Headline numbers on the shipped features | Design 1's 90.3% included bass moves and is not cited; all numbers are re-measured |
| Altered chords | Metramorphic flips on labelled dominants before any claim |
| Colour rules agree with the cases | One rule set; a plain blues is Blues (Designs 1 and 3, over Design 2) |
| Minor base rate | Kept as β plus a minor_shift slot (Design 3, over Design 2's dropping it) |
| Folds and test fold | Hash folds, dedup, artist grouping, fold 0 frozen and logged, packs by vendor plus pack |
| Melody-only parts | Never an anchor; tune home evidence as its own streams (E5) |
| #9 voicings | A major triad counts when both thirds sound |
| Use the existing API | Part, Grid (6/8 beat as Grid defines it), Role, KEY_FINDER_VERSION |
| Merge bars consistent with data | Non-inferiority for 16 bars, songs and relative errors; decided on fold 0 (the engineering judge, over the stats judge's nested headline, because the choices use folds 1 to 4) |
| Rule-5 adaptation path | Tables refillable by counts; weights wait for his answer |
| Allocation bound, hot path | 65,536-beat clamp; 8-byte KeyFact holding the Unclear reason; explanations on demand |
| Version discipline | Constant hash plus golden-answer hash |
| Selection leaks | No dropping of negative weights; every choice inside folds 1 to 4 |
| Bass moves | Off unless accuracy rises (E6) |
| Held C triad strictness | Top key asserted, grade not (the music judge, over Design 3) |

## Open questions for Solomon

1. **Rule 5.** May the weights, corrections and the 12 calibration numbers fitted on your MIDI ship, or only counts? Is a shipped table made from public weights times counts that include yours acceptable?
2. **Short loops.** Do you want more answers or fewer wrong ones? The start is 15% error allowed at 1 to 2 bars and 5% for songs. Is "C minor or Eb major" useful to you?
3. **Name keys.** When the notes give a pair and the file or pack name says Cm, should the page show "C minor (from the name)", or always the pair?
4. **Modulating songs.** On the Songs page, is the song's key the longest one, the first, or the last?
5. **Search and naming.** Should a Dorian vamp match "C minor" and a Mixolydian vamp match "C major"? Should the label read "C minor, Dorian colour" or "C Dorian"?
6. **Blues and gospel names.** A plain I7-IV7-V7 reads "C, blues colour", and gospel I-I7 reads "C major, I7 colour". Right?
7. **Loop or song.** On the MIDI page, is a whole number of bars up to 32 a loop, and anything longer a song?
8. **Metre in FL patterns.** They carry no time signature. Should 6/8 and 12/8 be detected from the notes, taken from the project, or set per pack?
9. **Label conventions.** Do your packs name vi-IV-I-V and i-VI-III-VII loops by the major or the minor key? Could you judge 50 to 100 disagreements by ear, and label a small gold set where "A or B" and "no clear key" are allowed?
10. **Ghost notes.** Is the ghost gate (under 1/8 of the part's usual velocity and shorter than 1/8 of a beat) right for humanised pack MIDI?