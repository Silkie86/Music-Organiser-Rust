"""Plays chord charts the way a pianist and a bass player might, for the chord reader's bench.

Usage:
  python3 -I voice.py charts <charts.jsonl> <out.jsonl> --set A|B
  python3 -I voice.py sweep <out.jsonl>
  python3 -I voice.py check <rendered.jsonl>

charts: reads a bench file that convert/charts.py wrote (the McGill Billboard charts, CC0, or RS200) and writes the
same songs played again: new keys, bass and tune parts, with chord labels that say exactly what is played. A pedal
under a chord makes it a slash chord; a held root or a walking bass takes the chart's inversion away; with no bass
part, the lowest note the keys play is the bass. The keys play one recipe a section (8 bars) and the bass one texture
a section, each picked by a hash of the song and the section, so a song mixes them as an arrangement would.

  Set A (may be learned from, at a quarter weight): close root position, the chart's own inversions (the bass at the
  bottom of the keys), drop 2, shells (1-3-7 and 1-7-3) with the tensions on top, and a gospel octave bass under a
  close right hand.
  Set B (measured only, never fitted): rootless A and B, upper-structure triads, quartal and So What voicings,
  arpeggios and Alberti, two-handed spreads with doubles, and gospel clusters. A recipe that doesn't fit a chord (a
  rootless voicing needs a seventh and a 9 or 13; a broken chord needs room for its notes) gives way to the section's
  second recipe, one that fits any chord (a spread, cluster, arpeggio or Alberti).

  Bass textures: a held root; a root-fifth two-feel; walking (the root on 2 changes in 3, otherwise the third or
  fifth, with half-step approaches about half the time); a pedal on the key's tonic; the chart's own inversions; no
  bass.

A tune plays in about half the sections, with passing and neighbour notes between its chord notes; it holds a tension
only when the label has it. A fifth of the chord changes are pushed an eighth: the keys come an eighth early and the bass
lands on the beat (the label stays on the beat). Harte's 11 (a major third under an 11th) is played as a 9sus4 and
labelled so. The fifth may be left out (with two tensions or more, in shells, and where a 13 takes its place), and a
rootless recipe leaves out the root; the label keeps both.

Every note is tagged: chord (a chord note, the labelled bass included), ext (one of the label's tensions), passing,
neighbour, approach, walking (a chord note in a walking line) or push (a chord note struck early). Each part's "tags"
run beside its "notes". The item's "render" gives each labelled chord's tags (set, recipe, texture, tune, push, the
chart's own label and the quality played), for the bench's --by; "voicings" names the set.

sweep: every chord quality in SWEEP x every recipe x every bass texture, on all 12 roots (round the circle of
fourths), alone (two bars each) and as the I of a ii-V-I, measured only. Here "inversions" puts each chord note in the
bass in turn, root by root, and a pedal is the I's root under the whole ii-V-I.

check: plays each labelled chord's notes back against its label and prints what doesn't hold (charts and sweep run it
on what they write): every chord, ext, walking and push note is in the label; every label note sounds but a fifth that
may be left out and a rootless recipe's root; the tune's passing and neighbour notes are not in the label; under a
held root, a pedal or the chart's inversions the bass plays the label's bass, below the keys; with no bass, the lowest
key is the label's bass (but under a rootless recipe, labelled in root position as the root is implied).

Written apart from mo-harmony's chord.rs, with its own degree tables (and apart from charts.py's), so the reader and
the music it's measured on can't share a mistake (chord-reader design, "Data that may ship").
"""

import json
import re
import sys
from collections import Counter, namedtuple


# Choices: a hash of what's being chosen, so every run on every computer makes the same ones.


def fnv(*parts):
    """FNV-1a (64 bits) of the parts joined by "|", then mixed so the low bits change as much as the high ones."""
    h = 0xCBF29CE484222325
    for byte in "|".join(str(p) for p in parts).encode("utf-8"):
        h = ((h ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    h ^= h >> 33
    h = (h * 0xFF51AFD7ED558CCD) & 0xFFFFFFFFFFFFFFFF
    h ^= h >> 33
    return h


def pick(options, *key):
    return options[fnv(*key) % len(options)]


# Labels: Harte's syntax, read with this file's own tables.

LETTER = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
NAMES = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"]
# A degree's semitones above the root, before its sharps and flats.
NUMBER = {1: 0, 2: 2, 3: 4, 4: 5, 5: 7, 6: 9, 7: 11, 8: 12, 9: 14, 10: 16, 11: 17, 12: 19, 13: 21}
# Harte's shorthands as degrees. A 13 has no 11 (as charts.py plays them and the bench reads them).
SHORTHAND = {
    "maj": "1 3 5", "min": "1 b3 5", "dim": "1 b3 b5", "aug": "1 3 #5",
    "maj7": "1 3 5 7", "min7": "1 b3 5 b7", "7": "1 3 5 b7", "dim7": "1 b3 b5 bb7", "hdim7": "1 b3 b5 b7",
    "minmaj7": "1 b3 5 7", "maj6": "1 3 5 6", "min6": "1 b3 5 6",
    "9": "1 3 5 b7 9", "maj9": "1 3 5 7 9", "min9": "1 b3 5 b7 9",
    "11": "1 3 5 b7 9 11", "min11": "1 b3 5 b7 9 11",
    "13": "1 3 5 b7 9 13", "maj13": "1 3 5 7 9 13", "min13": "1 b3 5 b7 9 13",
    "sus4": "1 4 5", "sus2": "1 2 5", "1": "1", "5": "1 5",
}
# The name of a bass note that isn't one of the chord's own, by its semitones above the root.
OTHER_BASS = {1: "b2", 2: "2", 3: "b3", 4: "3", 5: "4", 6: "b5", 7: "5", 8: "b6", 9: "6", 10: "b7", 11: "7"}


class Degree(namedtuple("Degree", "number shift")):
    """One of a chord's degrees: 3 is (3, 0), b7 is (7, -1), #11 is (11, 1)."""

    __slots__ = ()

    @property
    def semis(self):
        return NUMBER[self.number] + self.shift

    def text(self):
        return ("#" * self.shift if self.shift > 0 else "b" * -self.shift) + str(self.number)


ROOT = Degree(1, 0)
FIFTH = Degree(5, 0)


def degree(text):
    """A Harte degree (3, b7, #11, bb7) as a Degree; an 8th, 10th and 12th are the root, third and fifth again."""
    m = re.fullmatch(r"([#b]*)(\d+)", text.strip())
    if not m or int(m.group(2)) not in NUMBER:
        raise ValueError(text)
    shift = m.group(1).count("#") - m.group(1).count("b")
    number = int(m.group(2))
    return Degree({8: 1, 10: 3, 12: 5}.get(number, number), shift)


def pitch(name):
    """A note name (C, F#, Bb, Ebb) as a pitch class."""
    if not name or name[0] not in LETTER:
        raise ValueError(name)
    pc = LETTER[name[0]]
    for ch in name[1:]:
        if ch == "#":
            pc += 1
        elif ch == "b":
            pc -= 1
        else:
            raise ValueError(name)
    return pc % 12


# A chord: its root's pitch class, its degrees (a frozenset of Degree) and its bass (ROOT in root position).
Chord = namedtuple("Chord", "root degrees bass")


def read(label):
    """A Harte label as a Chord; None for no chord (N or X)."""
    text = label.strip()
    if text in ("N", "X", ""):
        return None
    if ":" in text:
        root_text, rest = text.split(":", 1)
    else:
        m = re.fullmatch(r"([A-G][#b]*)(.*)", text)
        if not m:
            raise ValueError(label)
        root_text, rest = m.group(1), m.group(2)
    root = pitch(root_text)
    bass = ROOT
    if "/" in rest:
        rest, bass_text = rest.split("/", 1)
        bass = degree(bass_text)
    m = re.fullmatch(r"([^()]*)(?:\(([^)]*)\))?", rest)
    if not m:
        raise ValueError(label)
    short, extra = m.group(1), m.group(2)
    if short == "" and extra is None:
        short = "maj"
    if short and short not in SHORTHAND:
        raise ValueError(label)
    # A label made only of degrees in brackets still has its root, unless it says *1.
    degrees = {degree(t) for t in SHORTHAND[short].split()} if short else {ROOT}
    for item in (extra or "").split(","):
        item = item.strip()
        if item.startswith("*"):
            degrees.discard(degree(item[1:]))
        elif item:
            degrees.add(degree(item))
    return Chord(root, frozenset(degrees), bass)


def pc(chord, d):
    return (chord.root + d.semis) % 12


def bass_pc(chord):
    return pc(chord, chord.bass)


def label_notes(chord):
    """The pitch classes a label names: its degrees on its root, and its bass."""
    return {pc(chord, d) for d in chord.degrees} | {bass_pc(chord)}


Roles = namedtuple("Roles", "root thirds fifths sevenths tensions")


def roles(chord):
    """A chord's degrees by the part each plays: the root; the third (or a sus chord's 4 or 2); the fifth; the seventh
    (or the 6 of a sixth chord with no seventh); and the tensions over them (9ths, 11ths, 13ths, and a 2, 4 or 6 added
    to a chord that has a third or seventh already), lowest first."""
    ds = set(chord.degrees)
    root = ROOT if ROOT in ds else None
    thirds = sorted(d for d in ds if d.number == 3)
    if not thirds:
        thirds = [d for d in (Degree(4, 0), Degree(2, 0)) if d in ds][:1]
    fifths = sorted(d for d in ds if d.number == 5)
    sevenths = sorted(d for d in ds if d.number == 7)
    if not sevenths and Degree(6, 0) in ds:
        sevenths = [Degree(6, 0)]
    tensions = sorted(ds - {root} - set(thirds) - set(fifths) - set(sevenths), key=height)
    return Roles(root, thirds, fifths, sevenths, tensions)


def height(d):
    """Where a tension sits over the chord: a 2, 4 or 6 added to a chord counts as a 9th, 11th or 13th."""
    return {2: 14, 4: 17, 6: 21}.get(d.number, NUMBER[d.number]) + d.shift


def tag(r, d):
    return "ext" if d in r.tensions else "chord"


def tag_pc(chord, r, p):
    """A key's tag by its pitch class: one of the label's tensions is "ext"; a chord note or the bass is "chord"."""
    tones = {pc(chord, d) for d in [r.root] + r.thirds + r.fifths + r.sevenths if d}
    return "ext" if p not in tones and p in {pc(chord, d) for d in r.tensions} else "chord"


def as_played(chord):
    """Harte's 11 (a major third and seventh under an 11th) played as a 9sus4: the third would clash with the 11th a
    ninth above it. The label then says sus4."""
    ds = set(chord.degrees)
    if {Degree(3, 0), Degree(7, -1), Degree(11, 0)} <= ds:
        return chord._replace(degrees=frozenset((ds - {Degree(3, 0), Degree(11, 0)}) | {Degree(4, 0)}))
    return chord


def with_bass(chord, p):
    """The chord over bass note p: its own degree when the chord has that note (a 9th named as a 2, an 11th as a 4, a
    13th as a 6), else the plain interval."""
    if p == chord.root:
        return chord._replace(bass=ROOT)
    for d in sorted(chord.degrees):
        if pc(chord, d) == p:
            return chord._replace(bass=Degree({9: 2, 11: 4, 13: 6}.get(d.number, d.number), d.shift))
    return chord._replace(bass=degree(OTHER_BASS[(p - chord.root) % 12]))


def quality_text(degrees):
    """A chord's degrees written as Harte's shorthand with the rest in brackets ("sus4(b7,9)")."""
    for name in ("sus4", "sus2", "maj", "min"):
        base = {degree(t) for t in SHORTHAND[name].split()}
        if base <= degrees:
            extra = sorted(degrees - base, key=height)
            return name + ("(" + ",".join(d.text() for d in extra) + ")" if extra else "")
    return "(" + ",".join(d.text() for d in sorted(degrees)) + ")"


def spell(chord, chart_text, chart):
    """The label for what's played: the chart's own text while the notes are the chart's, with the bass played."""
    head = chart_text.strip().split("/", 1)[0]
    if chord.degrees != chart.degrees:
        root_text = head.split(":", 1)[0] if ":" in head else re.match(r"[A-G][#b]*", head).group(0)
        head = root_text + ":" + quality_text(set(chord.degrees))
    elif ":" not in head:
        m = re.match(r"([A-G][#b]*)(.*)", head)
        head = m.group(1) + ":" + (m.group(2) or "maj")
    return head if chord.bass == ROOT else head + "/" + chord.bass.text()


def quality_of(label):
    """The quality a label names, without its root and bass ("min7", "sus4(b7,9)")."""
    head = label.split("/", 1)[0]
    return head.split(":", 1)[1] if ":" in head else "maj"


# Voicings. A recipe gives a chord's keys bottom to top, each (MIDI key, tag), and how they're played ("block" struck
# together, "broken" one an eighth), or None when it doesn't fit the chord.

KEYS_FLOOR = 45  # A2: the keys stay above every bass note
BASS_LOW, BASS_HIGH = 28, 43  # E1 to G2
HELD_LOW = 31  # a held, pedal or two-feel bass note is G1 to F#2
TUNE_LOW, TUNE_HIGH = 67, 84  # G4 to C6


def above(p, floor):
    """The lowest MIDI key at or above floor with pitch class p."""
    return floor + (p - floor) % 12


def stack(chord, r, degrees, floor):
    """Each degree on the next key up from the one before (close position), the first at or above floor."""
    notes, at = [], floor
    for d in degrees:
        m = above(pc(chord, d), at)
        notes.append((m, tag(r, d)))
        at = m + 1
    return notes


def keep_fifth(r):
    """A plain fifth is left out of a chord with two tensions or more."""
    return not (FIFTH in r.fifths and len(r.tensions) >= 2)


def ordered(r, root=True, fifth=True):
    """The degrees as a close voicing stacks them: root, third, fifth, seventh, then the tensions, lowest first; a
    plain fifth only when `fifth`."""
    return ([r.root] if root and r.root else []) + r.thirds + [f for f in r.fifths if fifth or f != FIFTH] + \
        r.sevenths + r.tensions


def r_close(chord, r, key):
    ds = ordered(r, fifth=keep_fifth(r))
    return (stack(chord, r, ds, 48), "block") if ds else None


def r_inverted(chord, r, key):
    """The chart's own inversion: the label's bass at the bottom of the keys, the rest in close position over it."""
    b = bass_pc(chord)
    if b == chord.root or not r.root:
        return r_close(chord, r, key)
    low = above(b, 48)
    rest = [d for d in ordered(r, fifth=keep_fifth(r)) if pc(chord, d) != b]
    return [(low, tag_pc(chord, r, b))] + stack(chord, r, rest, low + 1), "block"


def r_drop2(chord, r, key):
    """Root, third, fifth and seventh (a triad doubles its root) in close position with one of them on top, the second
    from the top dropped an octave; tensions over the top."""
    if not r.root or not r.thirds or not (r.fifths or r.sevenths):
        return None
    voices = [r.root, r.thirds[0]] + r.fifths[:1] + r.sevenths[:1]
    if len(voices) == 3:
        voices.append(r.root)
    turn = fnv(*key, "drop2") % 4
    order = voices[turn:] + voices[:turn]
    keys = [m for m, _ in stack(chord, r, order, 55)]
    keys[-2] -= 12
    pairs = sorted(zip(keys, order))
    while pairs[0][0] < KEYS_FLOOR:
        pairs = [(m + 12, d) for m, d in pairs]
    rest = [d for d in ordered(r) if d not in voices]
    return [(m, tag(r, d)) for m, d in pairs] + stack(chord, r, rest, pairs[-1][0] + 1), "block"


def r_shell(chord, r, key):
    """Root, third and seventh (1-3-7 or 1-7-3; a triad's fifth stands in for the seventh) in the left hand, the
    tensions (and an altered fifth) on top. A plain fifth is left out."""
    if not r.root or not r.thirds or not (r.sevenths or r.fifths):
        return None
    third, top = r.thirds[0], (r.sevenths or r.fifths)[0]
    low = above(chord.root, KEYS_FLOOR)
    middle = [third, top] if fnv(*key, "shell") % 2 == 0 else [top, third]
    notes = [(low, "chord")] + stack(chord, r, middle, low + 1)
    rest = [d for d in ordered(r, fifth=False) if d not in (r.root, third, top)]
    return notes + stack(chord, r, rest, notes[-1][0] + 1), "block"


def r_gospel(chord, r, key):
    """The bass in octaves in the left hand, the rest of the chord in close position in the right (from the third, or
    the seventh), the root among them when the bass isn't the root."""
    if not r.root:
        return None
    b = bass_pc(chord)
    low = above(b, KEYS_FLOOR)
    lh = [(low, tag_pc(chord, r, b)), (low + 12, tag_pc(chord, r, b))]
    rh = [d for d in r.thirds + [f for f in r.fifths if keep_fifth(r) or f != FIFTH] + r.sevenths + r.tensions
          + [r.root] if pc(chord, d) != b]
    if r.sevenths and r.sevenths[0] in rh and fnv(*key, "gospel") % 2:
        at = rh.index(r.sevenths[0])
        rh = rh[at:] + rh[:at]
    return lh + stack(chord, r, rh, max(60, low + 13)), "block"


def r_rootless(chord, r, key, form):
    """Bill Evans's rootless voicings, for a chord with a seventh and a 9 or 13: A is 3-5-7-9, B is 7-9-3-5 (a dominant's
    13 in place of the 5); the other tensions on top. The root is left out; the label keeps it."""
    if not r.thirds or not r.sevenths or r.sevenths[0].number != 7:
        return None
    nines = [d for d in r.tensions if d.number in (2, 9)]
    thirteens = [d for d in r.tensions if d.number in (6, 13)]
    if not nines and not thirteens:
        return None
    third, seventh = r.thirds[0], r.sevenths[0]
    if thirteens and third == Degree(3, 0) and seventh == Degree(7, -1):
        middle = thirteens[:1]
    else:
        middle = r.fifths[:1]
    core = [third] + middle + [seventh] + nines[:1] if form == "a" else [seventh] + nines[:1] + [third] + middle
    rest = [d for d in ordered(r, root=False, fifth=False) if d not in core]
    notes = stack(chord, r, core, 50)
    return notes + stack(chord, r, rest, notes[-1][0] + 1), "block"


def triads_in(chord, r):
    """The major and minor triads among a chord's notes that hold at least one of its tensions, most tensions first."""
    by_pc = {}
    for d in sorted(chord.degrees):
        by_pc.setdefault(pc(chord, d), d)
    found = []
    for t in sorted(by_pc):
        for q in (3, 4):
            ds = [by_pc.get(p) for p in (t, (t + q) % 12, (t + 7) % 12)]
            if all(ds):
                n = sum(d in r.tensions for d in ds)
                if n:
                    found.append((n, ds))
    best = max((n for n, _ in found), default=0)
    return [ds for n, ds in found if n == best]


def r_upper(chord, r, key):
    """An upper-structure triad: the third and seventh in the left hand, a triad holding the tensions over them, the
    rest on top. The root is left out."""
    if not r.thirds or not r.sevenths or r.sevenths[0].number != 7 or not r.tensions:
        return None
    triads = triads_in(chord, r)
    if not triads:
        return None
    tri = triads[fnv(*key, "upper") % len(triads)]
    turn = fnv(*key, "upper turn") % 3
    tri = tri[turn:] + tri[:turn]
    third, seventh = r.thirds[0], r.sevenths[0]
    shell = stack(chord, r, [third, seventh], 48)
    top = stack(chord, r, tri, max(60, shell[-1][0] + 1))
    rest = [d for d in ordered(r, root=False, fifth=False) if d not in [third, seventh] + tri]
    return shell + top + stack(chord, r, rest, top[-1][0] + 1), "block"


def r_quartal(chord, r, key, so_what=False):
    """The longest stack of fourths among the chord's notes (three or more), or So What's (three fourths and a major
    third); the rest of the chord on top, the root only if the stack has it."""
    names = {pc(chord, d) for d in chord.degrees}
    chains = []
    for start in sorted(names):
        chain = [start]
        while len(chain) < 5 and (chain[-1] + 5) % 12 in names and (chain[-1] + 5) % 12 not in chain:
            chain.append((chain[-1] + 5) % 12)
        chains.append(chain)
    if so_what:
        fits = [c[:4] + [(c[3] + 4) % 12] for c in chains if len(c) >= 4 and (c[3] + 4) % 12 in names]
    else:
        fits = [c for c in chains if len(c) >= 3]
    if not fits:
        return None
    longest = max(len(c) for c in fits)
    fits = [c for c in fits if len(c) == longest]
    chain = fits[fnv(*key, "quartal") % len(fits)]
    notes, at = [], 50
    for p in chain:
        m = above(p, at)
        notes.append((m, tag_pc(chord, r, p)))
        at = m + 1
    rest = [d for d in ordered(r, root=False, fifth=False) if pc(chord, d) not in chain]
    return notes + stack(chord, r, rest, notes[-1][0] + 1), "block"


def r_arpeggio(chord, r, key):
    """The close voicing broken: up and down again, an eighth a note."""
    got = r_close(chord, r, key)
    if not got or len(got[0]) < 2:
        return None
    notes = got[0]
    return notes + notes[-2:0:-1], "broken"


def r_alberti(chord, r, key):
    """Alberti's broken chord: bottom, top, a middle note, top, the next middle note, top..."""
    got = r_close(chord, r, key)
    if not got or len(got[0]) < 3:
        return None
    notes = got[0]
    low, top, middle = notes[0], notes[-1], notes[1:-1]
    return [low, top] + [x for m in middle for x in (m, top)], "broken"


def r_spread(chord, r, key):
    """Two hands: the root and its fifth (or seventh) in the left, the rest of the chord in the right, the root again on
    top."""
    if not r.root:
        return None
    partner = r.fifths[:1] if r.fifths and (not r.sevenths or fnv(*key, "spread") % 2 == 0) else r.sevenths[:1]
    low = above(chord.root, KEYS_FLOOR)
    lh = [(low, "chord")] + stack(chord, r, partner, low + 1)
    rest = [d for d in ordered(r, root=False, fifth=keep_fifth(r)) if d not in partner]
    rh = stack(chord, r, rest, max(60, lh[-1][0] + 3))
    return lh + rh + [(above(chord.root, (rh or lh)[-1][0] + 1), "chord")], "block"


def r_cluster(chord, r, key):
    """A gospel cluster: the bass in octaves in the left hand, the rest of the chord packed as close as it goes in the
    right."""
    if not r.root:
        return None
    b = bass_pc(chord)
    low = above(b, KEYS_FLOOR)
    lh = [(low, tag_pc(chord, r, b)), (low + 12, tag_pc(chord, r, b))]
    ps = sorted({pc(chord, d) for d in ordered(r, fifth=keep_fifth(r))} - {b})
    if not ps:
        return None
    best = min((ps[i:] + ps[:i] for i in range(len(ps))), key=lambda rot: ((rot[-1] - rot[0]) % 12, rot))
    notes, at = [], max(58, low + 13)
    for p in best:
        m = above(p, at)
        notes.append((m, tag_pc(chord, r, p)))
        at = m + 1
    return lh + notes, "block"


RECIPES = {
    "close": r_close,
    "inverted": r_inverted,
    "drop2": r_drop2,
    "shell": r_shell,
    "gospel": r_gospel,
    "rootless-a": lambda c, r, k: r_rootless(c, r, k, "a"),
    "rootless-b": lambda c, r, k: r_rootless(c, r, k, "b"),
    "upper": r_upper,
    "quartal": r_quartal,
    "so-what": lambda c, r, k: r_quartal(c, r, k, so_what=True),
    "arpeggio": r_arpeggio,
    "alberti": r_alberti,
    "spread": r_spread,
    "cluster": r_cluster,
}
SET_A = ["close", "inverted", "drop2", "shell", "gospel"]
SET_B = ["rootless-a", "rootless-b", "upper", "quartal", "so-what", "arpeggio", "alberti", "spread", "cluster"]
FALLBACK = {"A": ("close",), "B": ("spread", "close")}
# Set B recipes that fit any chord: a section's second choice, for the chords its first doesn't fit.
ANY_B = ["spread", "cluster", "arpeggio", "alberti"]
# Recipes that leave the root out (it sounds only if a stack happens to hold it).
ROOTLESS = {"rootless-a", "rootless-b", "upper", "quartal", "so-what"}
TEXTURES = ["held", "two-feel", "walking", "pedal", "inversions", "none"]


def voice(chord, recipe, fallback, key, room):
    """The recipe used (the first of recipe and its fallbacks that fits the chord, and that has room to break the
    chord when it's broken: room eighths), its keys and how they're played."""
    r = roles(chord)
    for name in (recipe,) + tuple(fallback):
        got = RECIPES[name](chord, r, key)
        if got and (got[1] == "block" or len(got[0]) <= room):
            return name, got[0], got[1]
    raise ValueError(f"nothing plays {chord}")


# Playing a piece.


def grid(s, e, step, origin):
    """s, then every multiple of step (counted from origin) after s and before e."""
    first = origin + ((s - origin) // step + 1) * step
    return [s] + list(range(first, e, step))


def felt_beats(beats_per_bar):
    """How many of the metre's beats make one felt beat: three in 6/8, 9/8 and 12/8, else one."""
    return 3 if beats_per_bar >= 6 and beats_per_bar % 3 == 0 else 1


def scale(chord, r):
    """The notes a line may step through over a chord: its own, and for the steps it doesn't name, those of the mode
    it suggests (major's, a dominant's or minor's seventh, a diminished fifth's flat sixth)."""
    def first(numbers, default):
        for n in numbers:
            ds = sorted(d for d in chord.degrees if d.number == n)
            if ds:
                return ds[0].semis % 12
        return default
    minor = Degree(3, -1) in chord.degrees
    dominant = Degree(3, 0) in chord.degrees and Degree(7, -1) in chord.degrees
    flat5 = Degree(5, -1) in chord.degrees
    steps = {0, first((2, 9), 2), first((3,), 3 if minor else 4), first((4, 11), 5), first((5,), 7),
             first((6, 13), 8 if flat5 else 9), first((7,), 10 if minor or dominant or r.thirds[:1] != [Degree(3, 0)]
                                                      else 11)}
    return {(chord.root + s) % 12 for s in steps} | label_notes(chord)


def nearest(options, ideal, avoid, prefer):
    """The option nearest ideal (none of avoid), a preferred one first when two are as near."""
    return min((m for m in options if m not in avoid), key=lambda m: (abs(m - ideal), m not in prefer, m))


def walk(chord, first, times, end, nxt, key):
    """A walking line over one chord, a note a beat: first on its first beat; over four beats or more, out to a chord
    note a fourth to a sixth away by the middle; and on the last beat an approach to the next chord's first note nxt
    (half a step off about half the time, else a step below) when one follows. Between, it steps through the chord's
    scale. Each (key, tag)."""
    names = label_notes(chord)
    n = len(times)
    if n == 1:
        return [(first, "walking")]
    options = [m for m in range(BASS_LOW, BASS_HIGH + 1) if m % 12 in scale(chord, roles(chord))]
    tones = {m for m in options if m % 12 in names}
    marks = {0: (first, "walking")}
    if n >= 4:
        up = sorted(m for m in tones if 5 <= m - first <= 9)
        down = sorted(m for m in tones if 5 <= first - m <= 9)
        far = up or down
        if far:
            marks[n // 2] = (far[fnv(*key, "far") % len(far)], "walking")
    if nxt is not None:
        if fnv(*key, "approach") % 2 == 0:
            ways = [nxt + 1, nxt - 1] if fnv(*key, "approach side") % 2 else [nxt - 1, nxt + 1]
        else:
            ways = [nxt - 2, nxt + 2]
        last = next((m for m in ways if BASS_LOW <= m <= BASS_HIGH), ways[0] + 12 if ways[0] < BASS_LOW else ways[0] - 12)
        marks[n - 1] = (last, "approach")
    notes, prev = [], first
    points = sorted(marks)
    for k in range(n):
        if k in marks:
            notes.append(marks[k])
            prev = marks[k][0]
            continue
        later = [p for p in points if p > k]
        avoid = {prev}
        if later:
            goal_at = later[0]
            goal = marks[goal_at][0]
            ideal = prev + (goal - prev) / (goal_at - k + 1)
            if goal_at == k + 1:
                avoid.add(goal)
        else:
            ideal = prev + (first - prev) / (n - k + 1)
        m = nearest(options, ideal, avoid, tones)
        notes.append((m, "walking" if m % 12 in names else "passing"))
        prev = m
    return notes


def play_tune(chord, r, times, end, prev, eighth, key):
    """The tune over one chord: on each beat one of the chord's notes (or the label's tensions) near the last, held or
    moved on; some beats add a passing note to the next one a third away, or step to a neighbour and back. Returns the
    notes (start, length, key, tag) and the last key."""
    names = label_notes(chord)
    tension_pcs = {pc(chord, d) for d in r.tensions} - {pc(chord, d) for d in [r.root] + r.thirds + r.fifths +
                                                         r.sevenths if d}
    targets = [m for m in range(TUNE_LOW, TUNE_HIGH + 1) if m % 12 in {pc(chord, d) for d in chord.degrees}]
    steps = scale(chord, r) - names
    kind = lambda m: "ext" if m % 12 in tension_pcs else "chord"

    def choose(m, k):
        near = sorted(targets, key=lambda x: (abs(x - m), x))[:3]
        return near[fnv(*key, k, "tune") % len(near)]

    notes, m = [], choose(prev, -1)
    for k, t in enumerate(times):
        until = times[k + 1] if k + 1 < len(times) else end
        style = fnv(*key, k, "style") % 6
        if style == 4 and k + 1 < len(times) and until - t >= 2 * eighth:
            # A passing note to a chord note a third away.
            ways = [(x, b) for x in targets if 3 <= abs(x - m) <= 4
                    for b in range(min(x, m) + 1, max(x, m)) if b % 12 in steps]
            if ways:
                x, b = ways[fnv(*key, k, "passing") % len(ways)]
                notes += [(t, until - t - eighth, m, kind(m)), (until - eighth, eighth, b, "passing")]
                m = x
                continue
        if style == 5 and until - t >= 2 * eighth:
            # A neighbour a step away and back.
            ways = [b for b in (m + 1, m + 2, m - 1, m - 2) if b % 12 in steps]
            if ways:
                notes += [(t, until - t - eighth, m, kind(m)), (until - eighth, eighth, ways[0], "neighbour")]
                continue
        notes.append((t, until - t, m, kind(m)))
        if style >= 2:
            m = choose(m, k)
    # A note held over the beat is one note.
    merged = []
    for n in notes:
        if merged and merged[-1][2] == n[2] and merged[-1][3] == n[3] and merged[-1][0] + merged[-1][1] == n[0]:
            merged[-1] = (merged[-1][0], merged[-1][1] + n[1], n[2], n[3])
        else:
            merged.append(n)
    return merged, m


Row = namedtuple("Row", "start end chart chord recipe style keys texture tune")


def play(item, plan, which, pushes):
    """Plays a piece's labelled chords; plan(i, start, label) says each one's texture, recipe and tune, and a pedal or
    a bass note when there's one. Returns the item with the new parts, labels and render tags."""
    ppq = int(item["ppq"])
    bar = int(item.get("beats_per_bar", 4)) * ppq
    origin = int(item.get("first_downbeat", 0))
    beat = felt_beats(int(item.get("beats_per_bar", 4))) * ppq
    # An eighth note: half a beat, or in 6/8, 9/8 and 12/8 the metre's own beat.
    eighth = ppq // 2 if beat == ppq else ppq
    song = f"{item['source']}/{item['id']}"
    segs = [(int(s), int(e), label) for s, e, label in sorted(item["chords"]) if e > s and read(label) is not None]

    rows = []
    for i, (s, e, label) in enumerate(segs):
        p = plan(i, s, label)
        chart = read(label)
        chord = as_played(chart)
        texture = p["texture"]
        if texture in ("held", "two-feel", "walking"):
            chord = with_bass(chord, chord.root)
        elif texture == "pedal":
            chord = with_bass(chord, p["pedal"])
        elif "bass" in p:
            chord = with_bass(chord, p["bass"])
        fallback = ((p["second"],) if "second" in p else ()) + FALLBACK[RECIPE_SET[p["recipe"]]]
        used, keys, style = voice(chord, p["recipe"], fallback, (song, i), (e - s) // eighth)
        if texture == "none":
            # No bass part: the lowest key is the bass, but under a rootless recipe the root is implied.
            chord = with_bass(chord, chord.root if used in ROOTLESS else min(m for m, _ in keys) % 12)
        rows.append(Row(s, e, label, chord, used, style, keys, texture, p["tune"]))

    keys_part, bass_part, tune_part = [], [], []
    firsts = {}
    for i, row in enumerate(rows):
        if row.texture == "walking":
            r = roles(row.chord)
            others = r.thirds[:1] + r.fifths[:1]
            d = pick(others, song, i, "walk which") if others and fnv(song, i, "walk first") % 3 == 2 else ROOT
            firsts[i] = above(pc(row.chord, d), HELD_LOW)
    tune_prev = 76
    for i, row in enumerate(rows):
        s, e, chord = row.start, row.end, row.chord
        r = roles(chord)
        strikes = grid(s, e, bar, origin)
        ends = strikes[1:] + [e]
        if row.style == "block":
            for t, until in zip(strikes, ends):
                keys_part += [[t, until - t, m, 70, g, i] for m, g in row.keys]
        else:
            for k, t in enumerate(range(s, e, eighth)):
                m, g = row.keys[k % len(row.keys)]
                keys_part.append([t, min(eighth, e - t), m, 70, g, i])
        if row.texture in ("held", "pedal", "inversions"):
            m = above(bass_pc(chord), HELD_LOW)
            bass_part += [[t, until - t, m, 90, "chord", i] for t, until in zip(strikes, ends)]
        elif row.texture == "two-feel":
            beats = int(item.get("beats_per_bar", 4))
            times = grid(s, e, bar // 2 if beats % 2 == 0 else bar, origin)
            low = above(chord.root, HELD_LOW)
            fifth = pc(chord, r.fifths[0]) if r.fifths else chord.root
            high = low - (chord.root - fifth) % 12
            high = high + 12 if high < BASS_LOW else high
            for t, until in zip(times, times[1:] + [e]):
                m = low if t == s or (t - origin) % bar == 0 else high
                bass_part.append([t, until - t, m, 90, "chord", i])
        elif row.texture == "walking":
            times = grid(s, e, beat, origin)
            nxt = None
            if i + 1 < len(rows) and rows[i + 1].start == e:
                nxt = firsts.get(i + 1, above(bass_pc(rows[i + 1].chord), HELD_LOW))
            line = walk(chord, firsts[i], times, e, nxt, (song, i))
            for (m, g), t, until in zip(line, times, times[1:] + [e]):
                bass_part.append([t, until - t, m, 90, g, i])
        if row.tune:
            notes, tune_prev = play_tune(chord, r, grid(s, e, beat, origin), e, tune_prev, eighth, (song, i))
            tune_part += [[t, length, m, 88, g, i] for t, length, m, g in notes]

    pushed = set()
    if pushes:
        for i in range(1, len(rows)):
            s, prev = rows[i].start, rows[i - 1]
            if prev.end != s or (s - origin) % beat or prev.end - prev.start < beat:
                continue
            if rows[i].style != "block" or prev.style != "block" or fnv(song, i, "push") % 5:
                continue
            early = s - eighth
            for n in keys_part:
                if n[5] < i and n[0] + n[1] > early:
                    # The chord before stops where this one comes in (a key struck after that point never sounds).
                    n[1] = max(0, early - n[0])
                elif n[5] == i and n[0] == s:
                    n[0], n[1], n[4] = early, n[1] + eighth, "push"
            pushed.add(i)

    def part(name, role, notes):
        notes = sorted((n for n in notes if n[1] > 0), key=lambda n: (n[0], n[2]))
        return {"name": name, "role": role, "notes": [n[:4] for n in notes], "tags": [n[4] for n in notes]}

    out = {k: v for k, v in item.items() if k not in ("parts", "chords", "render")}
    out["parts"] = [p for p in (part("KEYS", "chords", keys_part), part("BASS", "bass", bass_part),
                                part("TUNE", "melody", tune_part)) if p["notes"]]
    out["chords"] = []
    out["render"] = []
    for i, row in enumerate(rows):
        label = spell(row.chord, row.chart, read(row.chart))
        out["chords"].append([row.start, row.end, label])
        tags = {"set": RECIPE_SET[row.recipe], "recipe": row.recipe, "texture": row.texture,
                "tune": "yes" if row.tune else "no", "push": "yes" if i in pushed else "no", "chart": row.chart,
                "quality": quality_of(label)}
        tags.update(plan(i, row.start, row.chart).get("tags", {}))
        out["render"].append([row.start, row.end, tags])
    last = max((n[0] + n[1] for p in out["parts"] for n in p["notes"]), default=0)
    out["length"] = max(int(item.get("length", 0)), last)
    return out


RECIPE_SET = {**{name: "A" for name in SET_A}, **{name: "B" for name in SET_B}}


def tonic_at(item, tick):
    for s, e, key in item.get("keys", []):
        if s <= tick < e:
            try:
                return pitch(key.split(":", 1)[0])
            except ValueError:
                return None
    return None


def chart_plan(item, which):
    """Each section's (8 bars') texture, recipe and tune, by a hash of the song and the section."""
    recipes = SET_A if which == "A" else SET_B
    song = f"{item['source']}/{item['id']}"
    bar = int(item.get("beats_per_bar", 4)) * int(item["ppq"])
    origin = int(item.get("first_downbeat", 0))
    span = 8 * bar
    firsts = {}
    for s, _, label in sorted(item["chords"]):
        chord = read(label)
        if chord is not None:
            firsts.setdefault((int(s) - origin) // span, chord.root)

    def plan(i, s, label):
        section = (s - origin) // span
        tonic = tonic_at(item, origin + section * span)
        return {
            "texture": pick(TEXTURES, song, section, "texture"),
            "recipe": pick(recipes, song, section, "recipe"),
            "tune": fnv(song, section, "tune") % 2 == 0,
            **({"second": pick(ANY_B, song, section, "second")} if which == "B" else {}),
            "pedal": tonic if tonic is not None else firsts.get(section, 0),
        }
    return plan


# The sweep.

SWEEP = [
    "maj", "min", "dim", "aug", "sus2", "sus4", "5", "maj6", "min6", "7", "maj7", "min7", "hdim7", "dim7", "minmaj7",
    "maj(9)", "min(9)", "maj6(9)", "min6(9)", "9", "maj9", "min9", "11", "min11", "13", "maj13", "min13",
    "7(b9)", "7(#9)", "7(#11)", "7(b13)", "7(b9,b13)", "7(#9,b13)", "9(#11)", "13(#11)", "13(b9)",
    "maj7(#11)", "maj9(#11)", "aug(b7)", "aug(7)", "sus4(b7)", "sus4(b7,9)", "sus4(b7,9,13)", "min7(11)", "hdim7(9)",
]
FOURTHS = [(5 * k) % 12 for k in range(12)]  # C F Bb Eb Ab Db Gb B E A D G


def sweep_item(quality, recipe, texture, context):
    """One recipe and texture on a quality over all 12 roots; None when the recipe doesn't fit the quality."""
    target = as_played(read("C:" + quality))
    if RECIPES[recipe](target, roles(target), ("sweep",)) is None:
        return None
    if texture == "pedal" and context == "alone":
        return None
    ppq, bar = 480, 1920
    chords, plans = [], []
    minor = Degree(3, -1) in target.degrees
    r = roles(target)
    tones = [d for d in [r.root] + r.thirds[:1] + r.fifths[:1] + r.sevenths[:1] if d]
    for k, root in enumerate(FOURTHS):
        name = f"{NAMES[root]}:{quality}"
        bass = (root + tones[k % len(tones)].semis) % 12
        target_plan = {"texture": texture, "recipe": recipe, "tune": False, "pedal": root,
                       "tags": {"quality": quality, "context": context}}
        if texture == "inversions":
            target_plan["bass"] = bass
        if context == "alone":
            chords.append([k * 2 * bar, (k + 1) * 2 * bar, name])
            plans.append(target_plan)
            continue
        base = k * 4 * bar
        ii, v = ("hdim7", "7(b9)") if minor else ("min7", "7")
        side = "held" if texture == "inversions" else texture
        chords += [[base, base + bar, f"{NAMES[(root + 2) % 12]}:{ii}"],
                   [base + bar, base + 2 * bar, f"{NAMES[(root + 7) % 12]}:{v}"],
                   [base + 2 * bar, base + 4 * bar, name]]
        for q in ("(ii)", "(V)"):
            plans.append({"texture": side, "recipe": recipe, "tune": False, "pedal": root,
                          "tags": {"quality": q, "context": context}})
        plans.append(target_plan)
    item = {
        "source": "sweep", "id": f"{quality} {recipe} {texture} {context}", "ppq": ppq, "length": chords[-1][1],
        "beats_per_bar": 4, "first_downbeat": 0, "aligned": True, "keys": [], "chords": chords,
        "key_labels": "", "chord_labels": "voice.py sweep (labels say what is played)",
        "licence": "measuring only (voice.py sweep)",
    }
    return play(item, lambda i, s, label: plans[i], RECIPE_SET[recipe], pushes=False)


def sweep(out_path):
    count, problems = 0, []
    with open(out_path, "w", encoding="utf-8") as out:
        for quality in SWEEP:
            for recipe in SET_A + SET_B:
                for texture in TEXTURES:
                    for context in ("alone", "ii-V-I"):
                        item = sweep_item(quality, recipe, texture, context)
                        if item is None:
                            continue
                        problems += [f"{item['id']}: {p}" for p in check(item)]
                        out.write(json.dumps(item, separators=(",", ":")) + "\n")
                        count += 1
    print(f"sweep: {count} pieces written ({len(SWEEP)} qualities x the recipes that fit x {len(TEXTURES)} textures, "
          "alone and in a ii-V-I)")
    report(problems)


def charts(in_path, out_path, which):
    count, time = 0, Counter()
    problems = []
    with open(in_path, encoding="utf-8") as src, open(out_path, "w", encoding="utf-8") as out:
        for line in src:
            if not line.strip():
                continue
            item = json.loads(line)
            if not item.get("chords"):
                continue
            played = play(item, chart_plan(item, which), which, pushes=True)
            played["voicings"] = which
            played["chord_labels"] = (item.get("chord_labels") or "human") + \
                f", voiced by voice.py set {which} (labels say what is played)"
            if which == "B":
                played["licence"] = "measuring only (voice.py set B)"
            problems += [f"{item['source']}/{item['id']}: {p}" for p in check(played)]
            for s, e, tags in played["render"]:
                for name in ("recipe", "texture"):
                    time[(name, tags[name])] += (e - s) / int(item["ppq"])
                time[("push", tags["push"])] += 1
            out.write(json.dumps(played, separators=(",", ":")) + "\n")
            count += 1
    print(f"set {which}: {count} pieces written")
    for name in ("recipe", "texture"):
        total = sum(v for (n, _), v in time.items() if n == name)
        shares = sorted(((v, k) for (n, k), v in time.items() if n == name), reverse=True)
        print(f"  {name}: " + ", ".join(f"{k} {100 * v / total:.1f}%" for v, k in shares))
    changes = time[("push", "yes")] + time[("push", "no")]
    print(f"  pushed: {100 * time[('push', 'yes')] / max(1, changes):.1f}% of the chords")
    report(problems)


# Checking what was written.


def owners(item):
    """Each note's labelled chord: the one it starts in, or for a pushed note the one it was struck early for."""
    starts = [s for s, _, _ in item["chords"]]
    ends = [e for _, e, _ in item["chords"]]
    found = {}
    for part in item["parts"]:
        for (t, length, m, _), g in zip(part["notes"], part.get("tags", [])):
            if g == "push":
                i = next((k for k, s in enumerate(starts) if s > t), None)
            else:
                i = next((k for k, (s, e) in enumerate(zip(starts, ends)) if s <= t < e), None)
            if i is not None:
                found.setdefault(i, []).append((part["role"], t, m, g))
    return found


def check(item):
    """What doesn't hold in a rendered piece: each a line of text (none when all is well)."""
    problems = []
    found = owners(item)
    for i, ((s, e, label), (_, _, tags)) in enumerate(zip(item["chords"], item["render"])):
        chord = read(label)
        r = roles(chord)
        names = label_notes(chord)
        notes = found.get(i, [])
        played = [(role, t, m, g) for role, t, m, g in notes if role != "melody"]
        chordish = {m % 12 for _, _, m, g in played if g in ("chord", "ext", "walking", "push")}
        if not chordish <= names:
            problems.append(f"{label} at {s}: plays {sorted(chordish - names)}, not in the label")
        need = set(names)
        if FIFTH in chord.degrees:
            need.discard(pc(chord, FIFTH))
        if tags["recipe"] in ROOTLESS:
            need.discard(chord.root)
        heard = {m % 12 for _, _, m, _ in played}
        if not need <= heard:
            problems.append(f"{label} at {s} ({tags['recipe']}): {sorted(need - heard)} never sounds")
        tension_pcs = {pc(chord, d) for d in r.tensions}
        for role, t, m, g in played:
            if g == "ext" and m % 12 not in tension_pcs:
                problems.append(f"{label} at {s}: key {m} tagged ext but not a tension")
        for role, t, m, g in notes:
            if role == "melody" and (g in ("chord", "ext")) != (m % 12 in names):
                problems.append(f"{label} at {s}: tune key {m} tagged {g}")
        bass = [m for role, t, m, g in played if role == "bass"]
        keys = [m for role, t, m, g in played if role == "chords"]
        texture = tags["texture"]
        if texture in ("held", "two-feel", "walking") and chord.bass != ROOT:
            problems.append(f"{label} at {s}: a {texture} bass with a bass other than the root")
        if texture in ("held", "pedal", "inversions"):
            if not bass or any(m % 12 != bass_pc(chord) for m in bass):
                problems.append(f"{label} at {s}: the {texture} bass doesn't play the label's bass")
            elif keys and max(bass) >= min(keys):
                problems.append(f"{label} at {s}: the bass isn't below the keys")
        if texture == "none":
            if bass:
                problems.append(f"{label} at {s}: a bass part where there should be none")
            first = [m for role, t, m, g in played if role == "chords" and t == min(x[1] for x in played)]
            want = chord.root if tags["recipe"] in ROOTLESS else min(first) % 12 if first else None
            if want is not None and bass_pc(chord) != want:
                problems.append(f"{label} at {s}: with no bass, the label's bass isn't the lowest key")
    return problems


def report(problems):
    if problems:
        print(f"  {len(problems)} problems, the first: " + "; ".join(problems[:5]))
    else:
        print("  every labelled chord plays as labelled")


def check_file(path):
    problems, count = [], 0
    with open(path, encoding="utf-8") as src:
        for line in src:
            if line.strip():
                item = json.loads(line)
                problems += [f"{item['source']}/{item['id']}: {p}" for p in check(item)]
                count += 1
    print(f"{count} pieces checked")
    report(problems)
    return 1 if problems else 0


def main(argv):
    if len(argv) == 5 and argv[0] == "charts" and argv[3] == "--set" and argv[4] in ("A", "B"):
        charts(argv[1], argv[2], argv[4])
        return 0
    if len(argv) == 2 and argv[0] == "sweep":
        sweep(argv[1])
        return 0
    if len(argv) == 2 and argv[0] == "check":
        return check_file(argv[1])
    print(__doc__.split("\n\n")[1], file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
