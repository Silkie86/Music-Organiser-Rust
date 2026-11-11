# Tunes and chords together: what was tried (2026-11-11)

What `mo_harmony::together` is for: "find chords for this melody" and "find a melody for these chords". Both ask one
thing, how well a tune sits over an accompaniment when the two play together, with the tune tried in all 12 keys.
This note says how that is measured, what the old app's way scores, and what each idea tried came to. The commands
and the result tables are in `tools/theorybench/README.md` ("Tunes and chords together").

## The measure

POP909-CL (MIT licence, 906 pop songs with a tune and a piano accompaniment written for it), the songs in 4/4 with
their bar lines on the grid, cut into stretches of 8 bars. A stretch's own tune and accompaniment were written for
each other, so:

- **Its own over another song's:** given the tune, does its own accompaniment sit better than another song's, tried
  in all 12 keys? Asked against 50 other songs' stretches, and the other way round (given the accompaniment).
- **First of them all:** is its own first of all 51? This is the hard one: among 50 pop accompaniments in their best
  keys there is nearly always one that fits about as well as the real one.
- **Over another stretch of its song:** the same key and style, other chords, so only fitting bar by bar tells them
  apart.
- **Sits best unmoved:** its own pair, tried at all 12 moves, is best with no move.

Folds as the key finder draws them: fitted on folds 2 to 4, reported on fold 1, fold 0 kept back for one last look.

## The score to beat

The C# app's matcher (`HarmonyMatch.cs`) ported as it was (`tools/theorybench/src/pairs.rs`): the two parts laid over
each other half a bar at a time, each tune note worth 1 where the accompaniment plays it in that half bar, less where
it doesn't, less again where it rubs a note that is played. On fold 1 it puts a stretch's own pair over another song's
90.4% of the time and first of 51 39.8% of the time. Its closer look on all 12 moves (it only looked closely at 3)
scores the same, so its first pick of moves cost it nothing.

## What was found

1. **Line the two up as written.** The first measure started each part from the bar of its own first note, as two
   loops would be played. A tune that starts with a bar's rest was then slid a bar against its own chords: the new
   code, set to score the way the old app does, came to about 83% where the old app's own code scored 89%, and
   matched it once the two were lined up. `LineUp::AsWritten` plays the two in the same time; `LineUp::Loops` stays for
   two loops that each start at their own bar 1.
2. **Hand-set worths lose.** The first try gave each tune note a hand-set worth by what it is over the chord read
   under it (root, third, fifth, seventh, tension, avoid note). It scored far below the old app's way. Two reasons:
   the worths were guesses, and the chord's name alone throws away what the half-bar look knows (which notes are
   really sounding, and which of them a tune note rubs).
3. **So the worths are fitted, and the old app's look is kept beside the chords.** Every tune note gives shares
   (which kind of note it is over the chord; whether the accompaniment plays it in that half bar; how far above each
   played note it is, 1 to 11 semitones), the tune as a whole gives one (how alike its balance of notes is to the
   accompaniment's), and how a tune sits is the shares times what each is worth. The worths that give the right
   answers the best chances are found by `theorybench pairs --fit`. With every number free, on the songs fitted on:

   | left out | loss a question | own first of 51: chords for a tune / a tune for chords |
   |---|---|---|
   | nothing | 1.405 | 58.4% / 48.1% |
   | what each note is over its chord | 1.583 | 46.3% / 41.8% |
   | the half-bar look (played, and the 11 distances) | 1.491 | 56.3% / 47.3% |
   | the tune as a whole beside the backing | 1.495 | 59.1% / 40.1% |

   (An earlier, larger family of numbers than the one shipped, 500 steps each, before the review's fixes below:
   the sizes are what matter.) What each
   note is over its chord is the largest piece, and it is the piece the old app didn't have. The tune as a whole
   matters most when picking a tune for chords.
4. **A note on a strong beat counts no more than any other.** The more the strong beats counted, the worse it got:
   every beat the same, loss 1.555; the bar's first beat 1, the half bar 0.9, other beats 0.8 and off the beat 0.55,
   1.587; 1, 0.8, 0.6 and 0.3, 1.715, with 1 to 2 points fewer pairs right. A note held on into the next chord counts
   in full there as well (1.542; counted as a quarter there, 1.633). So there is no beat weight at all. (These and the
   next were tried on the first family of numbers, 600 steps each.)
5. **A note counts by its length**, between a sixteenth and a whole bar (0.25 to 4 beats). A longest of 4 beats
   rather than 2 helped a little (1.572 from 1.587) and 8 hardly more (1.568); a shortest of a thirty-second (1.596)
   or an eighth (1.600) was a little worse than a sixteenth (1.587).
6. **A short note between beats isn't forgiven a clash.** A kind of its own for a note outside the chord struck
   between beats and at most half a beat long (a passing note) fitted no kinder (-0.18 to -0.37) than the same note
   anywhere else (-0.04 to -0.28), so it tells nothing apart and is out. An extra worth for a colour held long fitted
   to 0.045: nothing.
7. **Dropped because they changed nothing** (the loss moved 0.004 or less with each left out): whether the
   accompaniment plays the note under it over that chord, and how strongly; how much of the note's own time the same
   note sounds under it; a half step above a note sounding under it at that moment; a yes or no for being in the
   stretch's scale; and the played and rubbing looks counted on the beats alone. Two more are out though they helped
   pop a little: how strongly the accompaniment plays the note in the half bar and in the stretch. The second fitted
   backwards (the more the accompaniment plays a note, the less a tune note on it was worth: -0.21), which reads as
   a quirk of these songs. Leaving both out cost 1.7 points of first-of-all for a tune for chords and gave 0.6 back
   for chords for a tune (on the songs fitted on).
8. **Pop songs teach two things wrong, so the fit is held to two rules.**
   - *Colours.* Left to itself the fit marks a note the chord names beyond its triad (a 7th, 9th, 11th or 13th) far
     below its fifth, some of them below 0: pop tunes sit on roots, thirds and fifths, and POP909's piano parts name
     few colours. A tune on the 9 of Dm9 and the 13 of G13 over rootless voicings then sat better moved up a fourth
     than where it was written. Rule: a colour the chord names is worth what its fifth is (`--tie colour=fifth`).
   - *Rubs.* Left to itself the fit gives credit for a note a third, a fifth or a sixth above a played note. A B
     held over C7 (a half step above its B flat: the wrong note) then bought its clash back from sitting a major
     third above G and a fifth above E, and beat the blue third (E flat over C7, which blues and gospel live on).
     Rule: only the rubs count (a half step, a whole step, a fourth, a tritone and the sevenths above a played note);
     the consonant distances are worth nothing (`--without above3,above4,above7,above8,above9`).
   - *Notes outside the chord* are told apart by the music, not by a theory table: a note the accompaniment plays
     somewhere in the stretch ("beside", worth a little) from one it hardly plays at all ("outside", marked down).
     The theory names (tension, avoid note) were tried first and fitted backwards on pop.

   **What the two rules cost on pop, and what they buy.** With every number free the loss is 1.432 (1.518 with the
   rules). On fold 1 a stretch's own accompaniment is then first of 51 59.5% of the time (53.7% with the rules) and
   its own tune 47.4% (46.7%); its own over another song's, 94.1% and 93.0% (93.4% and 92.6%). So the rules give up
   6 points of first place for chords for a tune and under a point everywhere else. The free numbers put a colour
   the chord names at -0.12, down with the notes it doesn't name (-0.21), where its fifth is 0.52. On the hand
   cases they sit the tune on ninths and thirteenths at 0.41 where it's written and 0.88 up a fourth (0.98 and 1.00
   with the rules: up a fourth that tune is all fifths, thirds and a ninth, so both are right), and they sit the wrong
   B over C7 above the blue third (0.80 over 0.75; with the rules 0.62 under 0.67).
9. **The fit is settled and repeatable.** The loss has one lowest point, so the fit starts from nothing and one
   command gives the shipped numbers. It needs its 8,000 steps: at 3,000 the loss is there to four places but the
   two least-used numbers are still on their way (a note over no chord another 0.02, a note outside another 0.005).
   At 8,000 every number agreed to three places with a fit started from other numbers (tried before the review's
   fixes below).

## The numbers shipped (`Sitting::SHIPPED`, version 1)

| a tune note that is | worth |
|---|---|
| the chord's third (or a sus chord's 4th or 2nd) | 0.960 |
| its root | 0.732 |
| its fifth (named, or the plain fifth a voicing leaves out), or a colour it names (7, 6, 9, 11, 13) | 0.667 |
| over sound with no chord read | 0.238 |
| not in the chord, but played somewhere in the stretch | 0.060 |
| not in the chord and hardly played anywhere | -0.394 |

Added by the half bar: 0.068 when the accompaniment plays the note there; and when it doesn't, for each played note
it is above by a half step -0.592, a whole step -0.460, a fourth -0.393, a tritone -0.702, a minor seventh -0.274, a
major seventh -0.258 (each by how strongly that note is played). Added once for the tune: 0.335 times how alike its
balance of notes is to the accompaniment's.

The scale is set so the best tenth of pairs written for each other sit at 0.95 or more and the middle one of another
song's parts, at its best move, at 0.5: of pairs written for each other a quarter sit under 0.67 and half under 0.79;
of another song's parts at their best move a quarter sit over 0.58 and a tenth over 0.67. So the number sorts a list
well, but a single number is no promise: 0.7 can be either. It isn't held to 0 to 1 (a tune all on thirds reaches
about 1.3, one that clashes throughout goes well under 0), so that the best stay in order; hold it only to draw it.

## After the code review (2026-11-11)

The review of the pull request found ten things; all are fixed, each with a test, and the numbers above and in the
README are from after the fixes and a fresh fit (loss 1.5175; it was 1.5227).

- **Only the third and fifth a chord names are its own.** An A flat over a C7 with no fifth voiced counted as its
  fifth, and an F over Csus2 as its third (the top worth). Now the chord has to name them; the one note it may leave
  out is the plain fifth of a shell voicing, when it names no other fifth.
- **The best stay in order.** The number was held to 0 to 1, so a tune all on thirds and one all on roots both came
  out at exactly 1 and tied. With all the fixes in, a stretch's own accompaniment is first of 51 more often at the
  short lengths, where such ties were commonest (2 bars 13.3% from 11.8%, 4 bars 31.3% from 30.3%).
- **Two loops start from the tune's own bar lines** (`LineUp::Loops { bar_line }`), not from tick 0.
- **A damaged file can't hang or stop it:** a note that never ends is followed for 32 bars at most, and two lengths
  too long to multiply play once through the longer.
- **The half bar follows the metre:** half a bar where the beats halve, the whole bar in 3/4, never over a bar line.
  (Still fitted in 4/4 alone.)
- **The measure:** a stretch the new way can't score now loses every count (it was given "best unmoved"); no other
  stretch is drawn twice, so "first of 51" is of 51; velocities and the cutting of stretches use the bench's shared
  code. All ten together moved the 8-bar numbers by 0.3 points at most.

## Bars picked from a song (2026-11-11)

Fitting to the song being worked on (`mo_midi::fit`) showed one more thing the two-loops line-up got wrong: it took
each side's length from its notes, from the bar of the first note to the bar the last one ends in. For bars picked
from a song that isn't the length: a lead that rests in the last of four picked bars went round every three bars
against four bars of chords, slipping a bar each time, and chords that come in a bar after the first picked bar
were heard a bar early. Two additions hold the picked bars as picked, a bar of rest too (`together::tests` and
`fit::tests` have the cases):

- `Backing::over(reading, parts, from, length)`: the chords over a stretch of time the caller gives.
  `Backing::of` is the same thing over the reading's own stretch, so nothing measured above moves.
- `LineUp::LoopsFrom { start, length }`: the tune's own round given, against the backing going round.

## A first look, for searching a whole library (2026-11-11)

The close look above needs a part's notes, and a found chord part has to be read by the chord reader first (about a
quarter of a millisecond each). A library of a few hundred thousand parts can't have all of that done on every
search, so a search takes two looks: a quick first look at every part, by a little kept in memory, then the close
look at the best share only. The question was what the little has to be. `pairs --quick 200` measures it: 207
stretches of fold 1, each tried against every stretch of another song (about 1,650) both ways round, counting how
many of the close look's best ten are still there when the first look keeps only a share.

| the first look goes by | kept 0.5% | 1% | 2% | 5% | 10% | 20% |
|---|---|---|---|---|---|---|
| **a tune for chords:** each tune's notes with no timing | 1.2% | 2.3% | 3.5% | 7.3% | 13.1% | 22.9% |
| **a tune for chords:** each tune two beats at a time | 78.4% | 96.9% | **99.1%** | 99.9% | 100% | 100% |
| **chords for a tune:** each chord part's notes with no timing | 2.4% | 4.2% | 8.0% | 16.1% | 27.4% | 44.2% |
| **chords for a tune:** each chord part guessed, two beats at a time | 40.2% | 60.5% | 77.6% | 93.3% | **97.8%** | 99.4% |
| **chords for a tune:** each chord part's own chords kept, two beats at a time | 80.1% | 96.2% | 98.9% | 99.8% | 100% | 100% |
| **chords for a tune:** the old app's closer look (24 numbers a half bar) | 34.0% | 53.2% | 73.4% | 91.0% | 96.4% | 99.0% |

- **Which notes a part plays, with no timing, is no use at all**: it keeps the close look's best about as often as
  picking at random would. That is all the library keeps of each part today (`chroma`, 12 bytes). Two parts go
  together bar by bar, not on the whole.
- **Two beats at a time is enough.** `mo_harmony::slots_of` keeps how long each of the 12 pitch classes sounds in each
  two beats (12 bytes a slot, 24 a bar, about 200 a part). A slot is two quarter notes whatever the time signature, so
  a song in 6/8 is looked at in the same stretches of time as the library's parts, which are all read as 4/4; a loop
  that ends inside a slot (a bar of 6/8 is a slot and a half) is kept going round until it ends on a slot's line. No
  more than 64 slots are kept of a part: one longer than 32 bars is looked at by its first 32. For the song's chords,
  `Backing::outline` works out what each pitch class is worth in each slot by the close look's own numbers, so a tune
  made of whole slots gets exactly the close look's answer (`first_look::tests`), and real tunes land near it: keeping
  2 parts in 100 loses 1 of the close look's best hundred.
- **The other way round is harder**, since the found part is the chords and its chords aren't read yet.
  `Outline::guessed` guesses them from the slots: a pitch class played for a good part of a slot is worth what a
  chord note is, the rest sit beside it or outside by the part's own scale, and a note a semitone or so over a played
  one rubs. Each pitch class counts once however many octaves double it (without that it kept 89.7% at 5 in 100
  where it now keeps 93.3%). Keeping 10 in 100 loses 2 of the best hundred, and already beats the old app's closer
  look at every share.
- **Reading each chord part's chords ahead and keeping its own outline** would let chords for a tune keep 2 in 100
  as well. It wasn't chosen yet: those numbers change whenever the chord reader or the worths do, so every part
  would have to be read again each time. It is the next step if the close look at 10 in 100 proves slow on Solomon's
  library.
- **How fast.** On one core the first look takes about 1 microsecond a part as a tune and 3 to 6 as chords (so
  200,000 parts in about a second at the worst, before the cores share it); the close look takes 10 to 15
  microseconds as a tune once its notes are read, and about 250 as chords.

So the plan for the search: a first look at every part, then the close look at the best **2 in 100** when looking
for tunes and the best **10 in 100** when looking for chords (and never fewer than a few hundred, so a small
library gets the close look throughout).

## The search (2026-11-11)

`mo_midi::fit::Target::search` puts the two looks together over the MIDI index.

- **What's kept.** The index keeps each set of notes' slots (`slots` in `midi-index.db`, made when the notes are
  saved, 3 to 7 bytes a slot), and a search loads them all into memory once (a `SlotBook`, about 200 bytes a part).
- **The first look** goes over every part of the kind looked for, each set of notes once, on the app's background
  threads.
- **The close look** is given the best 2 in 100 (tunes) or 10 in 100 (chords), and never fewer than 300, so a small
  library gets the close look throughout. Their notes are read from the index 256 parts at a time, the likeliest
  first, so a search stopped for a newer one has already looked at the best of them.
- **A loop in every key comes up once.** A pack's loop saved in twelve keys is twelve parts with one shape: the
  search gives the copy that fits best, and of copies that fit alike the one that needs moving least.
- **How fast.** 100,000 made-up four-bar parts on 4 threads, their notes already in memory
  (`fit::tests::how_long_a_search_takes`): 0.06 s looking for tunes (2,000 close looks), 0.5 s looking for chords
  (10,000 close looks, each part's chords read first). Reading the kept parts' notes from a real index on disk is
  on top of that and isn't measured yet.

## What it doesn't know yet

- **A song that isn't in 4/4.** The library's parts are all read as 4/4, and the first look goes by their 4/4
  slots, lined up in time with the song's chords. The close look then loops a found tune by the song's own bars.
  The two can disagree about where a loop comes round in 3/4 or 6/8. Not measured.
- **The song's own parts are in the library too.** A search for a tune over a song's chords will find that song's
  own tune first. Whoever lists what's found should leave out parts that come from the song's own file.

- **How long a loop from the library is.** A part's length isn't saved with it, so a found part still goes round
  from its first note's bar to its last note's: a four-bar loop that rests in its last bar is heard as three bars.
  Rounding up to 1, 2, 4, 8 or 16 bars is the likely fix; Solomon's packs' chord and melody pairs (below) are where
  to count whether it helps before it ships.
- **Only pop was measured.** Jazz, gospel and blues are held by the two rules and nine hand cases
  (`together::tests`), not by data. A set with real jazz tunes over real voicings would settle how much the rules
  cost or win there.
- **A colour the chord doesn't name is still marked down.** A D held over a plain Cmaj7 (its 9th, a note any jazz
  player would hold there) is a note "beside" the chord that rubs the C a whole step under it and the E a seventh
  under it: with C over Am7 before it, that two-note tune sits at 0.37, where an E in its place sits at 1.22 and an F
  at -0.24. The order is right; the gap between a 9th and a third is what pop teaches. Over Cmaj9 the same D is named by
  the chord and sits as a fifth does. This is the first thing jazz data should be asked about.
- **Solomon's own packs** have chord and melody files made for each other (same folder, same name but for the
  word). Counting how often the matcher puts those together is the next real-world measure: it runs on his PC, and
  only the counts leave it (rule 5).
- **Rhythm and shape.** Nothing here asks whether the tune's rhythm suits the accompaniment's, or whether its
  phrases start and end where the chords change. Two parts in the same key over the same chords score alike however
  they feel together.
- **The key the chords are in.** The tune is judged over each chord and beside the stretch's notes, not against a
  key: a tune that fits every chord but leans on a note outside the key isn't marked down for it.
