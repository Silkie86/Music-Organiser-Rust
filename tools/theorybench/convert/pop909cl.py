"""POP909-CL -> the theory bench's labelled-notes format (one JSON object a line).

Usage: python3 -I pop909cl.py <POP909-CL clone (the folder holding POP909_processed)> <out.jsonl>

POP909-CL (Yao, Chen, Dubnov and Berg-Kirkpatrick, ICASSP 2026; MIT licence) is POP909 with its labels checked by
people: expert-reviewed chords, corrected bar lines and time signatures, and key changes that were missing. It keeps
POP909's MELODY, BRIDGE and PIANO tracks, so the bench hears the same notes as with POP909, against better labels.
Nothing from the songs goes into the app; the files stay outside the repository.

    git clone --filter=blob:none --sparse https://github.com/AndyWeasley2004/POP909-CL-Dataset.git pop909cl
    cd pop909cl && git sparse-checkout set --no-cone '/POP909_processed/*' '/POP909_chord_labels/*'

Each song becomes one line, as pop909.py writes it. The differences:
  - Bar lines start at tick 0 (the release moves every file by whole bars so the first note is in the first bar).
  - A beat is the time signature's own beat note: a quarter in 4/4 and 3/4, an eighth in 6/8 and 12/8.
  - Keys come from the key signature events (the corrected keys, with their changes), chords from
    POP909_chord_labels (the people's chord track, named by triad and seventh templates, with its bass).
  - The CHORDS track holds the answers, so it is never one of the parts.
  - aligned is false for a song whose time signature changes or that the release flags, as one bar length can't
    describe its bar lines.
"""
import csv
import json
import os
import sys

import mido

ROLES = {'MELODY': 'melody', 'BRIDGE': 'melody', 'PIANO': 'accompaniment'}

# POP909-CL's chord qualities as Harte shorthands (or degree lists where Harte has no shorthand).
QUALITIES = {
    'M': 'maj',
    'm': 'min',
    'M7': 'maj7',
    'm7': 'min7',
    'D7': '7',
    'mM7': 'minmaj7',
    'sus2': 'sus2',
    'sus4': 'sus4',
    'o': 'dim',
    'o7': 'dim7',
    '/o7': 'hdim7',
    '+': 'aug',
    '+7': '(1,3,#5,b7)',
}

NOTES = {'C': 0, 'D': 2, 'E': 4, 'F': 5, 'G': 7, 'A': 9, 'B': 11}

# The bass as a degree above the root, as Harte writes it.
BASS_DEGREES = ['1', 'b2', '2', 'b3', '3', '4', 'b5', '5', '#5', '6', 'b7', '7']


def pitch_class(name):
    pc = NOTES[name[0].upper()]
    for c in name[1:]:
        pc += 1 if c == '#' else -1 if c == 'b' else 0
    return pc % 12


def harte(root, quality, bass):
    if root == 'N' or quality == 'N':
        return 'N'
    shorthand = QUALITIES.get(quality)
    if shorthand is None:
        raise ValueError(f'unknown chord quality {quality!r}')
    label = f'{root}:{shorthand}'
    if bass and bass != 'N':
        degree = BASS_DEGREES[(pitch_class(bass) - pitch_class(root)) % 12]
        if degree != '1':
            label += '/' + degree
    return label


def key_label(key):
    """mido's key signature ("Gb", "Gm", "C#m") as the bench writes keys ("Gb:maj", "G:min")."""
    if key.endswith('m'):
        return key[:-1] + ':min'
    return key + ':maj'


def notes_of(track):
    """[[start, length, key, velocity]] from note on/off pairs (a repeated key ends the one before it)."""
    tick = 0
    held = {}
    notes = []
    for msg in track:
        tick += msg.time
        if msg.type == 'note_on' and msg.velocity > 0:
            key = (msg.channel, msg.note)
            if key in held:
                start, vel = held.pop(key)
                if tick > start:
                    notes.append([start, tick - start, msg.note, vel])
            held[key] = (tick, msg.velocity)
        elif msg.type in ('note_off', 'note_on'):
            key = (msg.channel, msg.note)
            if key in held:
                start, vel = held.pop(key)
                if tick > start:
                    notes.append([start, tick - start, msg.note, vel])
    notes.sort()
    return notes


def metas(mid, kind):
    """[(tick, message)] of one kind of meta event, from every track, in order of tick."""
    out = []
    for track in mid.tracks:
        tick = 0
        for msg in track:
            tick += msg.time
            if msg.type == kind:
                out.append((tick, msg))
    out.sort(key=lambda t: t[0])
    return out


def regions(points, end):
    """[(tick, label)] in order -> [[start, end, label]], each lasting until the next (the last until `end`)."""
    out = []
    for i, (tick, label) in enumerate(points):
        stop = points[i + 1][0] if i + 1 < len(points) else end
        if stop > tick:
            out.append([tick, stop, label])
    return out


def song(root, sid, flagged):
    mid = mido.MidiFile(os.path.join(root, 'POP909_processed', sid + '.mid'))
    quarter = mid.ticks_per_beat
    parts = []
    for track in mid.tracks:
        name = (track.name or '').strip().upper()
        if name in ROLES:
            notes = notes_of(track)
            if notes:
                parts.append({'name': name, 'role': ROLES[name], 'notes': notes})
    if not parts:
        return None
    length = max(n[0] + n[1] for p in parts for n in p['notes'])

    meters = metas(mid, 'time_signature')
    numerator, denominator = (meters[0][1].numerator, meters[0][1].denominator) if meters else (4, 4)
    meter_changes = len({(m.numerator, m.denominator) for _, m in meters}) > 1
    beat = quarter * 4 // denominator

    keys = []
    for tick, msg in metas(mid, 'key_signature'):
        label = key_label(msg.key)
        if keys and keys[-1][1] == label:
            continue
        if keys and keys[-1][0] == tick:
            keys[-1] = (tick, label)
        else:
            keys.append((tick, label))

    chords = []
    with open(os.path.join(root, 'POP909_chord_labels', sid + '.csv'), encoding='utf-8') as f:
        for row in csv.DictReader(f):
            chords.append((round(float(row['offset_qb']) * quarter), harte(row['root'], row['quality'], row['bass'])))
    # The chord track's last chord lasts to the end of the music.
    chord_end = max(length, chords[-1][0] + 1) if chords else length

    return {
        'source': 'pop909cl',
        'id': sid,
        'ppq': beat,
        'length': length,
        'beats_per_bar': numerator,
        'first_downbeat': 0,
        'aligned': not meter_changes and not flagged,
        'parts': parts,
        'keys': regions(keys, length),
        'chords': regions(chords, chord_end),
        'key_labels': 'human (POP909 keys corrected by the POP909-CL annotators)',
        'chord_labels': 'human (the POP909-CL chord track, named by triad and seventh templates)',
    }


def main():
    root, out = sys.argv[1], sys.argv[2]
    flagged = set()
    with open(os.path.join(root, 'POP909_processed', 'metadata.csv'), encoding='utf-8') as f:
        released = []
        for row in csv.DictReader(f):
            if row['status'] == 'released':
                released.append(row['piece_id'])
                if row['flags'].strip():
                    flagged.add(row['piece_id'])
    done = skipped = 0
    with open(out, 'w', encoding='utf-8') as f:
        for sid in released:
            try:
                item = song(root, sid, sid in flagged)
            except (OSError, ValueError, KeyError, EOFError) as e:
                print(f'{sid}: {e}', file=sys.stderr)
                item = None
            if item is None:
                skipped += 1
                continue
            f.write(json.dumps(item, separators=(',', ':')) + '\n')
            done += 1
    print(f'{done} songs written, {skipped} skipped')


if __name__ == '__main__':
    main()
