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
90.4% of the time and first of 51 40.0% of the time. Its closer look on all 12 moves (it only looked closely at 3)
scores the same, so its first pick of moves cost it nothing.

## What was found

1. **Line the two up as written.** The first measure started each part from the bar of its own first note, as two
   loops would be played. A tune that starts with a bar's rest was then slid a bar against its own chords: the new
   code, set to score the way the old app does, came to about 83% where the old app's own code scored 89%, and
   matched it once the two were lined up. `Place::AsWritten` plays the two in the same time; `Place::Loops` stays for
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

   (An earlier, larger family of numbers than the one shipped, 500 steps each: the sizes are what matter.) What each
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

   **What the two rules cost on pop, and what they buy.** With every number free the loss is 1.437 (1.523 with the
   rules). On fold 1 a stretch's own accompaniment is then first of 51 59.7% of the time (53.8% with the rules) and
   its own tune 47.8% (46.9%); its own over another song's, 94.1% and 93.0% (93.4% and 92.6%). So the rules give up
   6 points of first place for chords for a tune and under a point everywhere else. The free numbers put a colour
   the chord names at -0.13, down with the notes it doesn't name (-0.20), where its fifth is 0.51. On the hand
   cases they sit the tune on ninths and thirteenths at 0.41 where it's written and 0.88 up a fourth (0.98 and 1.00
   with the rules: up a fourth that tune is all fifths, thirds and a ninth, so both are right), and they sit the wrong
   B over C7 above the blue third (0.80 over 0.74; with the rules 0.62 under 0.67).
9. **The fit is settled and repeatable.** The loss has one lowest point, so the fit starts from nothing and one
   command gives the shipped numbers. It needs its 8,000 steps: at 3,000 the loss is there to four places (1.5227)
   but the two least-used numbers are still on their way (a note over no chord 0.213, then 0.233; a note outside
   -0.387, then -0.392). At 8,000 every number agrees to three places with a fit started from other numbers.

## The numbers shipped (`Sitting::SHIPPED`, version 1)

| a tune note that is | worth |
|---|---|
| the chord's third (or a sus chord's 4th or 2nd) | 0.951 |
| its root | 0.726 |
| its fifth, or a colour it names (7, 6, 9, 11, 13) | 0.662 |
| over sound with no chord read | 0.233 |
| not in the chord, but played somewhere in the stretch | 0.070 |
| not in the chord and hardly played anywhere | -0.392 |

Added by the half bar: 0.075 when the accompaniment plays the note there; and when it doesn't, for each played note
it is above by a half step -0.594, a whole step -0.464, a fourth -0.414, a tritone -0.710, a minor seventh -0.287, a
major seventh -0.268 (each by how strongly that note is played). Added once for the tune: 0.336 times how alike its
balance of notes is to the accompaniment's.

The scale is set so the best tenth of pairs written for each other sit at 0.95 or more and the middle one of another
song's parts, at its best move, at 0.5: of pairs written for each other a quarter sit under 0.67 and half under 0.79;
of another song's parts at their best move a quarter sit over 0.58 and a tenth over 0.67. So the number sorts a list
well, but a single number is no promise: 0.7 can be either.

## What it doesn't know yet

- **Only pop was measured.** Jazz, gospel and blues are held by the two rules and nine hand cases
  (`together::tests`), not by data. A set with real jazz tunes over real voicings would settle how much the rules
  cost or win there.
- **A colour the chord doesn't name is still marked down.** A D held over a plain Cmaj7 (its 9th, a note any jazz
  player would hold there) is a note "beside" the chord that rubs the C a whole step under it and the E a seventh
  under it: with C over Am7 before it, that two-note tune sits at 0.37, where an E in its place sits at 1.00 and an F
  at 0. The order is right; the gap between a 9th and a third is what pop teaches. Over Cmaj9 the same D is named by
  the chord and sits as a fifth does. This is the first thing jazz data should be asked about.
- **Solomon's own packs** have chord and melody files made for each other (same folder, same name but for the
  word). Counting how often the matcher puts those together is the next real-world measure: it runs on his PC, and
  only the counts leave it (rule 5).
- **Rhythm and shape.** Nothing here asks whether the tune's rhythm suits the accompaniment's, or whether its
  phrases start and end where the chords change. Two parts in the same key over the same chords score alike however
  they feel together.
- **The key the chords are in.** The tune is judged over each chord and beside the stretch's notes, not against a
  key: a tune that fits every chord but leans on a note outside the key isn't marked down for it.
