# Recipe saves

Real saves made by hand by Solomon on 9 November 2026 in FL Studio 25.2, Bitwig Studio and Fender Studio (Studio
One), following a step-by-step recipe. They are the only real DAW files in the repo (rule 4 allows these and nothing
else, Solomon 2026-11-07): they hold only the DAWs' own built-in instruments and effects and three made-up tones
(`MO Tone A/B/C.wav`, sine tones made for this). No factory samples, no third-party plugins, no music of his.

`tests/recipe_saves.rs` says what each save should read as. Its `#[ignore]` tests are the known gaps.

## What was changed

Before committing, the user and PC names in the saves were blanked with stand-ins of the same length, so every
offset in the files stays the same (the readers were checked to read every save exactly as before):

| In the save | Now |
| --- | --- |
| Windows user name in paths | `motester` |
| PC name | `MO-TEST-MACHINE` |
| audio interface names | `Test Audio`, `Test Input` |
| FL's user name field | `motst` |

## The steps

FL Studio (`fl/`): 01 empty at 140; 02 FL Keys with C D E G at 128; 03 volume automation full to half over bar 1;
04 MO Tone A at bar 2; 05 mixer insert 2 with Fruity Reeverb 2, insert 3 muted; 06 a second arrangement "Second";
07 first track muted; 08 MO Tone B as a sampler channel; 10 to 12 time stretch (Resample at 200%, e3 generic,
Stretch pro, e2 generic, e3 mono, pitch +300 cents); 13 reversed; 14 trimmed, Smooth declicking, half volume (made
from 13, so still reversed); 14b clip fades; 14c sampler IN/OUT; 15 trimmed; 16 3/4 with marker "Verse".
`MO Recipe Keys.fst` is an FL Keys preset.

Bitwig (`bitwig/`): 01 empty; 02 Polysynth notes (saved as 16ths); 03 volume, pan and tempo automation; 04 tone at
bar 2; 05 Drum Machine with the three tones; 06 group and a deactivated audio track; 09 Stretch HD, 09b Stretch,
09c Elastique Pro, 10 Repitch; 11 transpose +3; 12 gain -6 dB with fades; 13 reversed; 14 trimmed; 15 looped;
16 3/4 with cue marker. Steps 14 to 16 were made after 13, so they are reversed too. Also a Polysynth preset and a
multisample of the three tones.

Fender Studio (`studio-one/`): 01 empty; 02 Mai Tai notes; 03 volume, pan and tempo automation (the pan's last point
is a 16th after bar 3); 04 tone at bar 2; 05 Impact XT pads; 06 audio track muted, Pro EQ, bus; 06b audio track
disabled; 08 Timestretch Sound at 100, 08b Drums, 08c Solo, 08d Tape, 08e Follow, 08f Speedup 2; 09 to 12 carry
on from 08f (back at 128): transpose +3, gain with fades, trimmed, 3/4 with marker on beat 4. `Media/` holds
the tone the songs point at.

## Known gaps (what the readers miss)

- Bitwig: deactivated tracks, arranger automation, time signature and cue markers. Stretch mode, transpose, gain
  and clip fades are read but the app doesn't draw or play them yet.
- FL: stretch mode, pitch, clip fades, declicking, sampler IN/OUT, the second arrangement; Stretch pro, e2 and e3
  clips read a whole-file length millions of beats long.
- Fender: automation, transpose, gain and fades, disabled tracks (read as just muted), bus routing; Timestretch
  events read as if they played at the song's speed.
