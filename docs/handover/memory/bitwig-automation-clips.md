---
name: bitwig-automation-clips
description: How Bitwig 6.1.6 saves an automation clip on a setting's lane (for FL edit events), decoded 2026-11-01
metadata:
  type: project
---
Bitwig automation CLIP (Solomon drew one 2026-11-01 18:50, decoded by RC): sits on the setting's automation lane
0x23d (track 0x4ba / per-user 0x168>0x4e6), not inside the note clip. Lane 0x2d8c timeline 0xd91 > 0x21f items: clip
class 0xd90 {0x2af start, 0x26 length, 0x10f8 muted, 0x2ae9 holder 0xebc > 0x10e9 content 0x3c > 0x48e points 0x6e >
0x21f points 0x108 (0x2af beat from clip start?, 0x28f value 0..1, 0x359e flag (first point 0), 0xba6 curve), 0x2adb
loop 0x267}; lane 0x2aec 0xe6c > 0x10e6 lists each clip content; 0x35ac holds the value after the clip. Done in PR #131 (merged
2026-11-01): one 0xd90 per FL pattern placement for edit events, copied from the template's clip. CONFIRMED (second clip at beat 16, 18:55): point beats count from the clip's start; each clip has its own
content 0x3c written in full (0x2aec 0x10e6 lists one ref per clip); loop off, range 0+length. Each clip begins with a
flag-0 point at beat 0 (value 0.1 in both, meaning unknown) then flag-1 points. Lane 0x35ac holds one point per clip end
(song beat, held value; first flag 0, rest 1). Curves 0xba6 all ~0 (none bent yet).
Part of [[bitwig-automation-lanes]].
Solomon's template: %LOCALAPPDATA%\MusicOrganiser\Bitwig template\Music Organiser template.bwproject has 2 automation
clips (Serum 2 Filter 1 Freq, Mixer Volume). Not yet checked live: converted sly_15 opened in Bitwig.
Bitwig shows a lane's clips only up to the first one that starts before the one ahead of it (sly_17, 2026-11-02): the
writer's clips_in_order sorts them and splits overlaps (PR #149). FL 7/8 VST wrapper (version 4) layout is in
tools/formatlab/registry/fl-studio.json (wrapper 53); before #149 such plugins (sly_7's LegacyCell) read as missing.
Verified 2026-11-02 on Solomon's PC: sly_7 and sly_17 convert fine (Solomon: "both work"). Possible follow-ups, not
asked for: FL 7/8 plugins have no vendor/path (could fill from scanned_plugins); edit-event lanes are named
"setting N (edit events)" rather than the plugin's parameter names (Encoder1...).
