"""POP909 -> the theory bench's labelled-notes format (one JSON object a line).

Usage: python3 pop909.py <POP909 folder (the one holding 001, 002 ...)> <out.jsonl>

POP909 (Wang et al., ISMIR 2020; MIT licence) is 909 Chinese pop songs as MIDI (MELODY, BRIDGE and PIANO tracks) with
beat, chord and key labels. The labels were made by software (keys from the audio, chords from the MIDI), so they are
a rough guide: comparing two versions of the engine on them is the reliable part. Nothing from the songs goes into
the app; the files stay outside the repository.

Each song becomes one line:
  source, id, ppq, length (ticks), beats_per_bar, first_downbeat (tick of the first labelled bar line), aligned (the
  labelled bar lines fall on the MIDI's bar grid), parts [{name, role, notes: [[start, length, key, velocity], ...]}],
  keys [[start, end, "Gb:maj"], ...], chords [[start, end, "B:maj"], ...] (Harte labels), key_labels, chord_labels.
Label times (seconds) are turned into ticks with the file's tempo map.
"""
import json
import os
import sys

import mido

ROLES = {'MELODY': 'melody', 'BRIDGE': 'melody', 'PIANO': 'accompaniment'}


def tempo_map(mid):
    """[(tick, microseconds a beat)], from every track, in order of tick."""
    changes = []
    for track in mid.tracks:
        tick = 0
        for msg in track:
            tick += msg.time
            if msg.type == 'set_tempo':
                changes.append((tick, msg.tempo))
    changes.sort()
    if not changes or changes[0][0] != 0:
        changes.insert(0, (0, 500000))
    return changes


def seconds_to_ticks(seconds, ppq, tempos):
    """The tick at a time in seconds, by the tempo map."""
    elapsed = 0.0
    for i, (tick, tempo) in enumerate(tempos):
        next_tick = tempos[i + 1][0] if i + 1 < len(tempos) else None
        span = None if next_tick is None else (next_tick - tick) * tempo / ppq / 1e6
        if span is None or elapsed + span > seconds:
            return tick + (seconds - elapsed) * 1e6 * ppq / tempo
        elapsed += span
    return 0.0


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


def labels(path, ppq, tempos):
    out = []
    with open(path, encoding='utf-8') as f:
        for line in f:
            fields = line.split()
            if len(fields) < 3:
                continue
            start = round(seconds_to_ticks(float(fields[0]), ppq, tempos))
            end = round(seconds_to_ticks(float(fields[1]), ppq, tempos))
            if end > start:
                out.append([start, end, fields[2]])
    return out


def song(folder):
    sid = os.path.basename(folder)
    mid_path = os.path.join(folder, sid + '.mid')
    if not os.path.exists(mid_path):
        return None
    mid = mido.MidiFile(mid_path)
    ppq = mid.ticks_per_beat
    tempos = tempo_map(mid)
    parts = []
    for track in mid.tracks:
        name = (track.name or '').strip().upper()
        if name in ROLES:
            notes = notes_of(track)
            if notes:
                parts.append({'name': name, 'role': ROLES[name], 'notes': notes})
    if not parts:
        return None
    beats = []
    with open(os.path.join(folder, 'beat_midi.txt'), encoding='utf-8') as f:
        for line in f:
            fields = line.split()
            if len(fields) >= 3:
                beats.append((seconds_to_ticks(float(fields[0]), ppq, tempos), float(fields[2]) > 0.5))
    downs = [i for i, (_, down) in enumerate(beats) if down]
    gaps = [b - a for a, b in zip(downs, downs[1:])]
    per_bar = max(set(gaps), key=gaps.count) if gaps else 4
    first_down = round(beats[downs[0]][0]) if downs else 0
    bar = per_bar * ppq
    down_ticks = [beats[i][0] for i in downs]
    near = sum(1 for t in down_ticks if abs((t - first_down) - round((t - first_down) / bar) * bar) <= ppq / 4)
    aligned = len(down_ticks) > 4 and near >= 0.9 * len(down_ticks)
    length = max(n[0] + n[1] for p in parts for n in p['notes'])
    return {
        'source': 'pop909',
        'id': sid,
        'ppq': ppq,
        'length': length,
        'beats_per_bar': per_bar,
        'first_downbeat': first_down,
        'aligned': aligned,
        'parts': parts,
        'keys': labels(os.path.join(folder, 'key_audio.txt'), ppq, tempos),
        'chords': labels(os.path.join(folder, 'chord_midi.txt'), ppq, tempos),
        'key_labels': 'software (from the audio)',
        'chord_labels': 'software (from the MIDI)',
    }


def main():
    root, out = sys.argv[1], sys.argv[2]
    done = skipped = 0
    with open(out, 'w', encoding='utf-8') as f:
        for name in sorted(os.listdir(root)):
            folder = os.path.join(root, name)
            if not os.path.isdir(folder):
                continue
            try:
                item = song(folder)
            except (OSError, ValueError, EOFError) as e:
                print(f'{name}: {e}', file=sys.stderr)
                item = None
            if item is None:
                skipped += 1
                continue
            f.write(json.dumps(item, separators=(',', ':')) + '\n')
            done += 1
    print(f'{done} songs written, {skipped} skipped')


if __name__ == '__main__':
    main()
