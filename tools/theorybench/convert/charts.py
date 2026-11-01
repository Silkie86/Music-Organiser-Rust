"""Chord charts -> the theory bench's labelled-notes format (one JSON object a line).

Usage:
  python3 -I charts.py jams <dir of ChoCo .jams> <meta.csv> <source> <licence> <out.jsonl> [--titles <titles.txt>]
  python3 -I charts.py billboard <billboard-2.0-salami_chords> <billboard-2.0-index.csv> <out.jsonl>
      [--skip <titles.txt>]

Two chart collections, both of people's chord labels with no notes of their own:
  - The McGill Billboard charts (Burgoyne, Wild and Fujinaga, ISMIR 2011; CC0): 890 charts of US Billboard Hot 100
    songs from 1958 to 1991, with their tonic and metre. Duplicates (a song drawn twice) are dropped, and so are the
    songs whose titles are in --skip, so a set measured on its own stays unseen. Written with licence CC0, so keyfit
    may learn from them.
  - The Rolling Stone 200 rock charts (de Clercq and Temperley), as ChoCo's JAMS files (de Berardinis et al.).
    Written with the licence given on the command line: "measuring only (RS200)" keeps keyfit from learning from
    them. --titles writes their titles, for the Billboard run's --skip.

    git clone --filter=blob:none --sparse https://github.com/boomerr1/the-mcgill-billboard-project.git billboard
    cd billboard && git sparse-checkout set --no-cone '/billboard-2.0-index.csv' '/billboard-2.0-salami_chords/*'
    git clone --filter=blob:none --sparse https://github.com/smashub/choco.git choco
    cd choco && git sparse-checkout set --no-cone '/partitions/rock-corpus/choco/*'
    python3 -I charts.py jams choco/partitions/rock-corpus/choco/jams-converted \
        choco/partitions/rock-corpus/choco/meta.csv rs200 "measuring only (RS200)" rs200.jsonl --titles rs200-titles.txt
    python3 -I charts.py billboard billboard/billboard-2.0-salami_chords billboard/billboard-2.0-index.csv \
        billboard.jsonl --skip rs200-titles.txt

(The Billboard repository is an unofficial copy of McGill's release, which is CC0 too.)

Each chart is played as a bass line (the chord's bass, C2 to B2) under block chords (G3 to F#4), both struck at the
chord's start and again at every bar line it lasts over. A beat is the metre's own beat note (an eighth in 6/8 and
12/8), as pop909cl.py counts them; a Billboard bar written in another metre is stretched to the song's bar. Charts
have no tune, so keyfit hears each as its full mix, its chords alone and its bass alone.

Keys: the chart's tonic, with the mode read from the tonic chord in that key region (a silver label): minor when
chords on the tonic with a minor third last longer than those with a major third, else major; a region with no chord
on its tonic takes the song's choice for that tonic, and is left unlabelled when there is none.
"""

import csv
import json
import re
import sys
from pathlib import Path

PPQ = 480
NOTE = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
DEGREE = {1: 0, 2: 2, 3: 4, 4: 5, 5: 7, 6: 9, 7: 11, 8: 12, 9: 14, 10: 16, 11: 17, 12: 19, 13: 21}
SHORT = {
    "maj": [0, 4, 7], "min": [0, 3, 7], "dim": [0, 3, 6], "aug": [0, 4, 8],
    "maj7": [0, 4, 7, 11], "min7": [0, 3, 7, 10], "7": [0, 4, 7, 10], "dim7": [0, 3, 6, 9],
    "hdim7": [0, 3, 6, 10], "minmaj7": [0, 3, 7, 11], "maj6": [0, 4, 7, 9], "min6": [0, 3, 7, 9],
    "9": [0, 4, 7, 10, 14], "maj9": [0, 4, 7, 11, 14], "min9": [0, 3, 7, 10, 14],
    "11": [0, 4, 7, 10, 14, 17], "min11": [0, 3, 7, 10, 14, 17], "13": [0, 4, 7, 10, 14, 21],
    "maj13": [0, 4, 7, 11, 14, 21], "min13": [0, 3, 7, 10, 14, 21], "sus4": [0, 5, 7], "sus2": [0, 2, 7],
    "1": [0], "5": [0, 7], "": [0],
}


def pitch(name):
    """A note name (C, F#, Bb, Ebb) as a pitch class."""
    if not name or name[0] not in NOTE:
        raise ValueError(name)
    pc = NOTE[name[0]]
    for ch in name[1:]:
        if ch == "#":
            pc += 1
        elif ch == "b":
            pc -= 1
        else:
            raise ValueError(name)
    return pc % 12


def interval(text):
    """A Harte degree (3, b7, #11, bb13) in semitones above the root."""
    m = re.fullmatch(r"([#b]*)(\d+)", text)
    if not m or int(m.group(2)) not in DEGREE:
        raise ValueError(text)
    shift = m.group(1).count("#") - m.group(1).count("b")
    return DEGREE[int(m.group(2))] + shift


def harte(label):
    """A Harte chord as (root pc, the pcs, the bass pc, the third: 'major', 'minor' or None); None for no chord."""
    label = label.strip()
    if label in ("N", "X", "*", "&pause", ""):
        return None
    if ":" in label:
        root_text, rest = label.split(":", 1)
    else:
        m = re.fullmatch(r"([A-G][#b]*)(.*)", label)
        if not m:
            raise ValueError(label)
        root_text, rest = m.group(1), m.group(2)
        rest = rest or "maj"
    root = pitch(root_text)
    bass_text = None
    if "/" in rest:
        rest, bass_text = rest.split("/", 1)
    m = re.fullmatch(r"([^()]*)(?:\(([^)]*)\))?", rest)
    if not m:
        raise ValueError(label)
    short, extra = m.group(1), m.group(2)
    if short not in SHORT:
        raise ValueError(label)
    steps = set(SHORT[short] if (short or extra is None) else [0])
    if short == "" and extra is None:
        steps = set(SHORT["maj"])
    for item in (extra.split(",") if extra else []):
        item = item.strip()
        if not item:
            continue
        if item.startswith("*"):
            steps.discard(interval(item[1:]))
        else:
            steps.add(interval(item))
    pcs = sorted({(root + s) % 12 for s in steps})
    bass = (root + interval(bass_text)) % 12 if bass_text else root
    if bass not in pcs:
        pcs = sorted(set(pcs) | {bass})
    rel = {s % 12 for s in steps}
    third = "minor" if 3 in rel and 4 not in rel else "major" if 4 in rel else None
    return root, pcs, bass, third


def render(segments, bar_ticks, end):
    """The bass and chord parts for chord segments [(start, end, label)] in ticks, struck again at each bar line."""
    bass, chords = [], []
    for start, stop, label in segments:
        chord = harte(label)
        if chord is None or stop <= start:
            continue
        _, pcs, low, _ = chord
        strikes = [start] + [b for b in range((start // bar_ticks + 1) * bar_ticks, stop, bar_ticks)]
        for i, at in enumerate(strikes):
            until = strikes[i + 1] if i + 1 < len(strikes) else stop
            bass.append([at, until - at, 36 + low, 90])
            for pc in pcs:
                chords.append([at, until - at, 55 + (pc - 7) % 12, 70])
    bass.sort()
    chords.sort()
    return [{"name": "BASS", "role": "bass", "notes": bass}, {"name": "CHORDS", "role": "chords", "notes": chords}]


def silver_keys(regions, segments, end):
    """Key labels for tonic regions [(start, end, tonic pc)], each mode read from the chords on its tonic."""
    def thirds(lo, hi, tonic):
        major = minor = 0
        for s, e, label in segments:
            chord = harte(label)
            if chord is None or chord[0] != tonic:
                continue
            overlap = min(e, hi) - max(s, lo)
            if overlap <= 0:
                continue
            if chord[3] == "major":
                major += overlap
            elif chord[3] == "minor":
                minor += overlap
        return major, minor

    song = {}
    for _, _, tonic in regions:
        if tonic not in song:
            song[tonic] = thirds(0, end, tonic)
    names = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"]
    out = []
    for lo, hi, tonic in regions:
        if hi <= lo:
            continue
        major, minor = thirds(lo, hi, tonic)
        if major == minor == 0:
            major, minor = song[tonic]
        if major == minor == 0:
            continue
        out.append([lo, hi, names[tonic] + (":min" if minor > major else ":maj")])
    return out


def norm_title(t):
    return re.sub(r"[^a-z0-9]", "", t.lower().replace("&", "and"))


def billboard(chart_dir, index_csv, out, skip_titles):
    index = {}
    with open(index_csv, newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            if row.get("title"):
                index[int(row["id"])] = (row["title"], row["artist"])
    seen = set()
    items, dropped = [], {"duplicate": 0, "in the measuring set": 0, "no labels": 0, "unreadable": 0}
    bad_labels = {}
    for folder in sorted(Path(chart_dir).iterdir()):
        path = folder / "salami_chords.txt"
        if not path.is_file():
            continue
        num = int(folder.name)
        title, artist = index.get(num, ("", ""))
        key = (norm_title(title), norm_title(artist))
        if title and key in seen:
            dropped["duplicate"] += 1
            continue
        seen.add(key)
        if title and norm_title(title) in skip_titles:
            dropped["in the measuring set"] += 1
            continue
        try:
            item = billboard_item(path, folder.name, bad_labels)
        except ValueError as e:
            dropped["unreadable"] += 1
            print(f"  {folder.name}: {e}", file=sys.stderr)
            continue
        if item is None:
            dropped["no labels"] += 1
            continue
        item["title"], item["artist"] = title, artist
        items.append(item)
    write(items, out)
    print(f"billboard: {len(items)} songs written; left out: {dropped}")
    if bad_labels:
        print("  chords not read (played as no chord):", sorted(bad_labels.items(), key=lambda x: -x[1])[:20])


def metre_beats(text):
    """A metre (4/4, 12/8, 6/8, 3/4) as beats a bar, a beat being the metre's own beat note (an eighth in 6/8 and
    12/8), as pop909cl.py counts them."""
    num, _ = (int(x) for x in text.split("/"))
    return num


def billboard_item(path, ident, bad_labels):
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    metre = None
    tonic = None
    bars = []  # (tokens, tonic)
    for line in lines:
        if line.startswith("#"):
            m = re.match(r"#\s*(\w+):\s*(.*)", line)
            if not m:
                continue
            field, value = m.group(1), m.group(2).strip()
            if field == "metre" and metre is None:
                metre = value
            elif field == "tonic":
                try:
                    tonic = pitch(value.split(":")[0].strip())
                except ValueError:
                    tonic = None
            continue
        if "|" not in line:
            continue
        body = line.split("\t", 1)[-1]
        first, last = body.index("|"), body.rindex("|")
        inner, after = body[first:last + 1], body[last + 1:]
        line_bars = []
        for bar in inner.strip("|").split("|"):
            tokens = bar.split()
            tokens = [t for t in tokens if not re.fullmatch(r"\(\d+/\d+\)", t)]
            if tokens:
                line_bars.append(tokens)
        m = re.search(r"\bx(\d+)", after)
        times = int(m.group(1)) if m else 1
        for _ in range(times):
            bars.extend((tokens, tonic) for tokens in line_bars)
    if metre is None:
        metre = "4/4"
    beats = metre_beats(metre)
    bar_ticks = beats * PPQ
    segments = []
    regions = []
    current = None  # (label, start)
    for b, (tokens, ton) in enumerate(bars):
        start = b * bar_ticks
        if ton is not None and (not regions or regions[-1][2] != ton):
            if regions:
                regions[-1][1] = start
            regions.append([start, None, ton])
        share = bar_ticks / len(tokens)
        for i, tok in enumerate(tokens):
            at = start + round(i * share)
            if tok == ".":
                continue
            label = tok
            try:
                if harte(label) is None:
                    label = "N"
            except ValueError:
                bad_labels[tok] = bad_labels.get(tok, 0) + 1
                label = "N"
            if current is not None:
                segments.append([current[1], at, current[0]])
            current = (label, at)
    end = len(bars) * bar_ticks
    if current is not None:
        segments.append([current[1], end, current[0]])
    segments = merge(segments)
    if not regions or end == 0:
        return None
    regions[-1][1] = end
    keys = silver_keys([tuple(r) for r in regions], segments, end)
    if not keys:
        return None
    return {
        "source": "billboard",
        "id": ident,
        "ppq": PPQ,
        "length": end,
        "beats_per_bar": beats,
        "first_downbeat": 0,
        "aligned": True,
        "parts": render(segments, bar_ticks, end),
        "keys": keys,
        "chords": [s for s in segments if s[2] != "N"],
        "key_labels": "human (tonic), mode from the tonic chord",
        "chord_labels": "human",
        "licence": "CC0",
    }


def merge(segments):
    out = []
    for s in segments:
        if out and out[-1][2] == s[2] and out[-1][1] == s[0]:
            out[-1][1] = s[1]
        elif s[1] > s[0]:
            out.append(list(s))
    return out


def jams(jams_dir, meta_csv, source, licence, out, titles_out):
    meta = {}
    with open(meta_csv, newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            meta[row["id"]] = (row.get("title", ""), row.get("performers", ""))
    items, dropped, bad_labels = [], {"no labels": 0, "unreadable": 0}, {}
    titles = []
    for path in sorted(Path(jams_dir).glob("*.jams"), key=lambda p: int(re.sub(r"\D", "", p.stem) or 0)):
        d = json.loads(path.read_text(encoding="utf-8"))
        anns = {}
        for a in d["annotations"]:
            anns.setdefault(a["namespace"], a["data"])
        chords, keys_data, timesig = anns.get("chord_harte"), anns.get("key_mode"), anns.get("timesig")
        if not chords or not keys_data:
            dropped["no labels"] += 1
            continue
        num, den = 4, 4
        if timesig:
            num, den = timesig[0]["value"]["numerator"], timesig[0]["value"]["denominator"]
        beats = metre_beats(f"{num}/{den}")
        unit = PPQ  # the durations count the metre's beat note, which is the bench's beat
        bar_ticks = beats * PPQ
        first = chords[0]["time"]
        measure = int(round(first * 1000)) // 1000
        origin_units = 0.0
        segments, at = [], 0.0
        for o in chords:
            label = o["value"]
            try:
                if harte(label) is None:
                    label = "N"
            except ValueError:
                bad_labels[label] = bad_labels.get(label, 0) + 1
                label = "N"
            s = round(at * unit)
            at += o["duration"]
            segments.append([s, round(at * unit), label])
        end = round(at * unit)
        # Key regions: placed by measure.beat from the first chord's measure (keys change on bar lines in RS200).
        regions = []
        for k in keys_data:
            t = k["time"]
            m = int(round(t * 1000)) // 1000
            frac = round((t - m) * 10)
            start = (m - measure) * bar_ticks + max(frac - 1, 0) * round(unit)
            tonic = pitch(k["value"].split(":")[0])
            regions.append([max(start, 0), None, tonic])
        regions.sort()
        for i, r in enumerate(regions):
            r[1] = regions[i + 1][0] if i + 1 < len(regions) else end
        segments = merge(segments)
        keys = silver_keys([tuple(r) for r in regions], segments, end)
        if not keys:
            dropped["no labels"] += 1
            continue
        title, artist = meta.get(path.stem, (d.get("file_metadata", {}).get("title", ""), ""))
        titles.append(norm_title(title))
        items.append({
            "source": source,
            "id": path.stem,
            "ppq": PPQ,
            "length": end,
            "beats_per_bar": beats,
            "first_downbeat": 0,
            "aligned": True,
            "parts": render(segments, bar_ticks, end),
            "keys": keys,
            "chords": [s for s in segments if s[2] != "N"],
            "key_labels": "human (tonic only), mode from the tonic chord",
            "chord_labels": "human",
            "licence": licence,
            "title": title,
            "artist": artist,
        })
    write(items, out)
    if titles_out:
        Path(titles_out).write_text("\n".join(titles) + "\n", encoding="utf-8")
    print(f"{source}: {len(items)} songs written; left out: {dropped}")
    if bad_labels:
        print("  chords not read (played as no chord):", sorted(bad_labels.items(), key=lambda x: -x[1])[:20])


def write(items, out):
    with open(out, "w", encoding="utf-8") as f:
        for item in items:
            f.write(json.dumps(item, separators=(",", ":")) + "\n")


def main(argv):
    if len(argv) >= 4 and argv[0] == "billboard":
        skip = set()
        if "--skip" in argv:
            skip = set(Path(argv[argv.index("--skip") + 1]).read_text(encoding="utf-8").split())
        billboard(argv[1], argv[2], argv[3], skip)
    elif len(argv) >= 6 and argv[0] == "jams":
        titles = argv[argv.index("--titles") + 1] if "--titles" in argv else None
        jams(argv[1], argv[2], argv[3], argv[4], argv[5], titles)
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
