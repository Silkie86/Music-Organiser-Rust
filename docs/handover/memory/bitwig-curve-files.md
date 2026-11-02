---
name: bitwig-curve-files
description: Bitwig .bwcurve format (decoded 2026-11-02 from Solomon's 3 library curves) and the plan to export FL knob moves as curves
metadata:
  type: project
  modified: 2026-11-02T01:26:06.964Z
---
Solomon (2026-11-02): a curve file can be dropped onto an automation lane in Bitwig. He wants FL edit events (knob
moves) turned into curve files. Plan given: keep the automation clips; also write one .bwcurve per knob-move pattern in
a "Curves" folder next to each convert (never into his Bitwig library unless he picks); later a library of all songs.

Format (RC decode, read-only, of <Bitwig Studio folder>\Library\Curves: Filter Rhythm 001 2, Gararge 1, London City):
- Same BtWg container as .bwproject: 'BtWg' 0003 0002, body offset in 8 hex, no zip; metadata then ~5 KB space padding.
- Metadata: application_version_name, beat_length (f64, optional), branch, comment, creator, curve_category
  ('Sequence'|'Periodic'), curve_kind u8 0, revision_id/no, tags, type 'application/bitwig-curve'.
- Root class 0x127f: 0x362d u8 0, 0x3646 bool 1, 0x3601 u8 16, 0x3602 u8 (1/4/4, unknown), 0x3635 u8 (0/0/2,
  unknown), 0x3603 points list, 0x3788/0x3665/0x3666 u8 255, 0x360b name, 0x3621 creator, 0x3623 comment?,
  0x3622 category, 0x36a6 tags.
- Points class 0x127e, three f64: 0x35fd y 0..1, 0x35fe x 0..1 (sorted, normalised), 0x35ff curve -1..1 (0
  straight). Same x twice = a jump. Meanings from ranges, not yet matched to what Bitwig draws.
DROP TEST (2026-11-02 01:25 save, Basic Sequence 1 on Acid V Cutoff bar 8): Bitwig makes a normal automation clip
0xd90 (start 28, length 4, loop off, 0x2d24 clip name = curve name), points beat/value/0xba6 curve -1..1 (on the
segment's first point, not baked into extra points; no flag-0 start point needed). Lane 0x35ac gets a point at the
clip end. Likely x*length = beat, 0x35ff = 0xba6; not proven (pack curves are format 0003 0004, body unreadable,
maybe encrypted). Asked Solomon to drop his own Gararge 1 at bar 10 to prove it. See [[bitwig-automation-clips]].
Solomon 2026-11-02: the curve library could be part of the Presets page (screens thread told via coordinator).
Solomon 2026-11-02: the Curves library should also gather every automation clip from his Bitwig projects, keeping
only distinct shapes (dedupe by normalised points: x 0..1, y 0..1, bends, rounded).
Curves list rules (told Solomon + screens thread 2026-11-02): same shape = same normalised points (x,y,bend ~3 dp),
so a stretched copy counts once; skip flat/empty clips; skip the Music Organiser Exports folder (converts flood it:
sly_17 has 204 clips, 3 distinct shapes). API plan: mo_presets::curves::list -> Curve{id,name,points,length_beats,hidden,
sources}, curves.db beside library.db, .bwcurve written on demand to %LOCALAPPDATA%\MusicOrganiser\Curves.
mo-formats' Bitwig reader doesn't read automation clips (0xd90) yet; the curves scan needs that added.
Screens thread asks (2026-11-02): CurveSource gets a display name, sources newest first; scan progress hook +
cheap out_of_date(); curves::file(curve) pool-safe, returns path; rename(db,id,name)/set_hidden(db,id,bool) return
the updated Curve; save_to_bitwig returns the path written. Message screens thread when on main.
DONE 2026-11-02: curves library merged (PR #156): mo_formats::bitwig::automation::clips + mo_presets::curves
(CurvesDb in curves.db, file(), save_to_bitwig(), is_export). Screens thread told. Doc nit: Curve.points/length doc
says 'first found' but code uses newest source; fix in next PR.
