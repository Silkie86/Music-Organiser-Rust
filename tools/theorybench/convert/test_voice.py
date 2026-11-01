"""Tests for voice.py: python3 -I test_voice.py (from any folder; nothing outside this one is read)."""

import copy
import importlib.util
import json
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


voice = load("voice")
charts = load("charts")

def keys_of(notes):
    return [voice.NAMES[m % 12] + str(m // 12 - 1) for m, _ in notes]


def piece(chords, keys=None, beats=4):
    """A small chart in the bench's format: chords [(bars, label)], one after another."""
    ppq, out, at = 480, [], 0
    for bars, label in chords:
        end = at + round(bars * beats * ppq)
        out.append([at, end, label])
        at = end
    return {"source": "test", "id": "piece", "ppq": ppq, "length": at, "beats_per_bar": beats, "first_downbeat": 0,
            "aligned": True, "parts": [], "keys": keys or [[0, at, "C:maj"]], "chords": out, "licence": "CC0"}


def fixed(texture, recipe, tune=False, **more):
    return lambda i, s, label: dict({"texture": texture, "recipe": recipe, "tune": tune, "pedal": 0}, **more)


class Labels(unittest.TestCase):
    def test_labels_name_the_notes_charts_py_names(self):
        # Two readers written apart: every shorthand, with brackets, omissions and basses.
        labels = [f"{n}:{q}" for n in ("C", "F#", "Bb") for q in voice.SHORTHAND if q]
        labels += ["C:maj(9)", "C:min(9)/b3", "C:7(b9,#11)", "C:maj7(*5)", "C:(1,4,5,b7,9)", "Eb:hdim7/b7",
                   "C:maj/7", "C:maj/b7", "G:13(#11)/3", "A:min7/4", "D:sus4(b7,9)", "C", "Ab/3", "C:7/#4"]
        labels += [f"C:{q}" for q in voice.SWEEP]
        for label in labels:
            root, pcs, bass, _ = charts.harte(label)
            chord = voice.read(label)
            self.assertEqual(voice.label_notes(chord), set(pcs), label)
            self.assertEqual((chord.root, voice.bass_pc(chord)), (root, bass), label)

    def test_harte_11_is_played_and_labelled_as_a_9sus4(self):
        chart = voice.read("G:11")
        played = voice.as_played(chart)
        self.assertEqual(voice.spell(played, "G:11", chart), "G:sus4(b7,9)")
        self.assertEqual(voice.as_played(voice.read("G:min11")), voice.read("G:min11"))

    def test_a_bass_is_named_by_its_degree(self):
        c7 = voice.read("C:7")
        self.assertEqual(voice.spell(voice.with_bass(c7, 4), "C:7", c7), "C:7/3")
        self.assertEqual(voice.spell(voice.with_bass(c7, 2), "C:7", c7), "C:7/2")
        self.assertEqual(voice.spell(voice.with_bass(c7, 0), "C:7/b7", voice.read("C:7/b7")), "C:7")
        nine = voice.read("C:9")
        self.assertEqual(voice.spell(voice.with_bass(nine, 2), "C:9", nine), "C:9/2")


class Recipes(unittest.TestCase):
    def notes(self, label, recipe, key=("t",)):
        chord = voice.read(label)
        got = voice.RECIPES[recipe](chord, voice.roles(chord), key)
        return None if got is None else keys_of(got[0])

    def test_close_and_the_charts_inversion(self):
        self.assertEqual(self.notes("C:maj7", "close"), ["C3", "E3", "G3", "B3"])
        self.assertEqual(self.notes("C:maj7/3", "inverted"), ["E3", "C4", "G4", "B4"])
        # Two tensions: the fifth is left out.
        self.assertEqual(self.notes("C:13(#11)", "close"), ["C3", "E3", "Bb3", "D4", "Gb4", "A4"])

    def test_rootless_voicings_need_a_seventh_and_a_9_or_13(self):
        self.assertEqual(self.notes("C:9", "rootless-a"), ["E3", "G3", "Bb3", "D4"])
        self.assertEqual(self.notes("C:9", "rootless-b"), ["Bb3", "D4", "E4", "G4"])
        self.assertEqual(self.notes("C:13", "rootless-a"), ["E3", "A3", "Bb3", "D4"])
        self.assertIsNone(self.notes("C:7", "rootless-a"))
        self.assertIsNone(self.notes("C:maj", "rootless-b"))

    def test_upper_structures_and_fourths(self):
        upper = self.notes("C:13(#11)", "upper")
        self.assertEqual(upper[:2], ["E3", "Bb3"])
        self.assertEqual({n[:-1] for n in upper[2:5]}, {"D", "Gb", "A"})  # D major over the shell, in any inversion
        self.assertEqual(self.notes("D:min7(11)", "so-what"), ["D3", "G3", "C4", "F4", "A4"])
        self.assertIsNone(self.notes("C:7", "quartal"))

    def test_shells_drop_the_fifth_and_keep_the_tensions_on_top(self):
        for key in ("a", "b", "c", "d"):
            shell = self.notes("C:9", "shell", (key,))
            self.assertEqual(shell[0], "C3")
            self.assertEqual({n[:-1] for n in shell[1:3]}, {"E", "Bb"})  # 1-3-7 or 1-7-3
            self.assertEqual(shell[3:], ["D4"] if shell[2] == "Bb3" else ["D5"])

    def test_every_recipe_plays_every_note_it_must(self):
        for q in voice.SWEEP:
            chord = voice.as_played(voice.read("C:" + q))
            r = voice.roles(chord)
            need = voice.label_notes(chord) - {voice.pc(chord, voice.FIFTH)}
            for name, recipe in voice.RECIPES.items():
                got = recipe(chord, r, ("t",))
                if got is None:
                    continue
                heard = {m % 12 for m, _ in got[0]}
                must = need - ({chord.root} if name in voice.ROOTLESS else set())
                self.assertTrue(must <= heard, f"{q} {name}: {must - heard} missing")
                self.assertTrue(heard <= voice.label_notes(chord), f"{q} {name}: {heard - voice.label_notes(chord)}")
                self.assertTrue(all(m >= voice.KEYS_FLOOR for m, _ in got[0]), f"{q} {name} goes under the bass")


class Playing(unittest.TestCase):
    def test_textures_set_the_bass_the_label_names(self):
        chart = piece([(1, "C:maj/3"), (1, "F:maj"), (1, "G:7")])
        held = voice.play(chart, fixed("held", "close"), pushes=False)
        self.assertEqual([c[2] for c in held["chords"]], ["C:maj", "F:maj", "G:7"])
        kept = voice.play(chart, fixed("inversions", "close"), pushes=False)
        self.assertEqual([c[2] for c in kept["chords"]], ["C:maj/3", "F:maj", "G:7"])
        pedal = voice.play(chart, fixed("pedal", "close"), pushes=False)
        self.assertEqual([c[2] for c in pedal["chords"]], ["C:maj", "F:maj/5", "G:7/4"])
        alone = voice.play(chart, fixed("none", "inverted"), pushes=False)
        self.assertEqual([c[2] for c in alone["chords"]], ["C:maj/3", "F:maj", "G:7"])
        rootless = voice.play(piece([(1, "C:9/3")]), fixed("none", "rootless-a"), pushes=False)
        self.assertEqual(rootless["chords"][0][2], "C:9")
        for item in (held, kept, pedal, alone, rootless):
            self.assertEqual(voice.check(item), [])

    def test_a_walking_bass_steps_and_approaches_the_next_chord(self):
        chart = piece([(1, "C:7"), (1, "F:7")])
        out = voice.play(chart, fixed("walking", "shell"), pushes=False)
        bass = [n for p in out["parts"] if p["role"] == "bass" for n in zip(p["notes"], p["tags"])]
        self.assertEqual([n[0][0] for n in bass], [0, 480, 960, 1440, 1920, 2400, 2880, 3360])
        self.assertEqual(bass[3][1], "approach")
        self.assertIn(abs(bass[3][0][2] - bass[4][0][2]), (1, 2))
        self.assertEqual(voice.check(out), [])

    def test_a_push_strikes_the_keys_an_eighth_early_and_the_bass_on_the_beat(self):
        chart = piece([(1, "C:maj")] + [(1, "F:maj"), (1, "G:maj"), (1, "C:maj")] * 8)
        out = voice.play(chart, fixed("held", "close"), pushes=True)
        pushed = [k for k, (_, _, t) in enumerate(out["render"]) if t["push"] == "yes"]
        self.assertTrue(pushed)
        start = out["chords"][pushed[0]][0]
        keys = next(p for p in out["parts"] if p["role"] == "chords")
        early = [n for n, g in zip(keys["notes"], keys["tags"]) if g == "push" and n[0] == start - 240]
        self.assertTrue(early)
        bass = next(p for p in out["parts"] if p["role"] == "bass")
        self.assertTrue(any(n[0] == start for n in bass["notes"]))
        self.assertFalse(any(n[0] < start < n[0] + n[1] and n[0] + n[1] > start - 240 for n in keys["notes"]
                             if n not in early and n[0] < start - 240))
        self.assertEqual(voice.check(out), [])

    def test_the_tune_holds_only_the_labels_notes(self):
        chart = piece([(2, "C:maj7"), (2, "A:min7"), (2, "D:min9"), (2, "G:13")])
        out = voice.play(chart, fixed("held", "close", tune=True), pushes=False)
        tune = next(p for p in out["parts"] if p["role"] == "melody")
        self.assertTrue(tune["notes"])
        for (t, length, m, _), g in zip(tune["notes"], tune["tags"]):
            if g in ("passing", "neighbour"):
                self.assertLessEqual(length, 240)
        self.assertEqual(voice.check(out), [])

    def test_check_finds_a_wrong_label(self):
        out = voice.play(piece([(1, "C:maj"), (1, "A:min7")]), fixed("held", "close"), pushes=False)
        self.assertEqual(voice.check(out), [])
        wrong = copy.deepcopy(out)
        wrong["chords"][0][2] = "C:min"
        self.assertTrue(any("not in the label" in p for p in voice.check(wrong)))
        wrong = copy.deepcopy(out)
        wrong["chords"][1][2] = "A:min7/b3"
        self.assertTrue(voice.check(wrong))
        wrong = copy.deepcopy(out)
        keys = next(p for p in wrong["parts"] if p["role"] == "chords")
        drop = [k for k, n in enumerate(keys["notes"]) if n[2] % 12 == 4 and n[0] == 0]
        for k in reversed(drop):
            del keys["notes"][k], keys["tags"][k]
        self.assertTrue(any("never sounds" in p for p in voice.check(wrong)))

    def test_charts_play_the_same_every_time(self):
        chart = piece([(1, "C:maj"), (0.5, "A:min7"), (0.5, "D:min7"), (1, "G:7(b9)")] * 6,
                      keys=[[0, 10 ** 6, "C:maj"]])
        for which in ("A", "B"):
            first = voice.play(chart, voice.chart_plan(chart, which), pushes=True)
            again = voice.play(copy.deepcopy(chart), voice.chart_plan(chart, which), pushes=True)
            self.assertEqual(json.dumps(first, sort_keys=True), json.dumps(again, sort_keys=True))
            self.assertEqual(voice.check(first), [])

    def test_the_sweep_skips_what_doesnt_fit_and_plays_the_rest(self):
        self.assertIsNone(voice.sweep_item("maj", "rootless-a", "held", "alone"))
        self.assertIsNone(voice.sweep_item("7", "close", "pedal", "alone"))
        item = voice.sweep_item("13(#11)", "upper", "inversions", "ii-V-I")
        self.assertEqual(len(item["chords"]), 36)
        self.assertEqual(voice.check(item), [])
        basses = [c[2].split("/")[1] if "/" in c[2] else "1" for c in item["chords"][2::3]]
        self.assertEqual(basses[:4], ["1", "3", "5", "b7"])


if __name__ == "__main__":
    unittest.main()
