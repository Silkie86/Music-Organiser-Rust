"""
Runs the FL Studio script in a stand-in FL Studio (fake_fl.py) and talks to it the way the app does: SysEx
messages in on its MIDI input, and out through its MIDI output.  python -m unittest discover -s extensions/fl-studio/test
"""
import io
import json
import os
import shutil
import tempfile
import unittest
from contextlib import redirect_stdout

import fake_fl
from fake_fl import SCRIPT, Clock, SysEx, load_script, night_drive

HEAD = bytes([0xF0, 0x7D, 0x4D, 0x4F])


def frame(kind, request_id, text, size=512):
    """The link's messages for this text, built from what extensions/README.md says, not from the script."""
    parts = max(1, -(-len(text) // size))
    return [HEAD + bytes([kind, request_id & 127, request_id >> 7 & 127, request_id >> 14 & 127,
                          part & 127, part >> 7, parts & 127, parts >> 7]) + text[part * size:(part + 1) * size] + b"\xF7"
            for part in range(parts)]


def unframe(messages):
    """(id, text) from the messages of one answer, checking each one."""
    ids, texts = set(), {}
    for m in messages:
        assert m[:4] == HEAD and m[-1] == 0xF7 and all(b < 128 for b in m[4:-1]), m
        assert m[4] == 2, "answers are kind 2"
        ids.add(m[5] | m[6] << 7 | m[7] << 14)
        part, parts = m[8] | m[9] << 7, m[10] | m[11] << 7
        assert parts == len(messages) and part not in texts and len(m) <= 512 + 13
        texts[part] = m[12:-1]
    assert len(ids) == 1
    return ids.pop(), b"".join(texts[p] for p in range(len(texts)))


class Link:
    """The script in a folder of its own, as installed, with a clock the checks move on."""

    def __init__(self, studio, changes=False):
        self.studio = studio
        self.installed = tempfile.mkdtemp()   # the script's folder in FL Studio's Settings/Hardware
        self.printed = io.StringIO()
        with redirect_stdout(self.printed):
            self.script = load_script(studio, self.installed)
        self.script.LET_THE_APP_MAKE_CHANGES = changes
        self.clock = Clock()
        self.script.time = self.clock
        self.last = 0

    def close(self):
        shutil.rmtree(self.installed, ignore_errors=True)

    def send(self, request, raw=None, kind=1, request_id=None):
        """Sends a request into the script's MIDI input, and returns whether it said the messages were its own."""
        text = raw if raw is not None else json.dumps(request).encode("ascii")
        handled = True
        for message in frame(kind, request["id"] if request_id is None else request_id, text):
            event = SysEx(message)
            with redirect_stdout(self.printed):
                self.script.OnSysEx(event)
            handled = handled and event.handled
        return handled

    def idle(self, times=1):
        for _ in range(times):
            self.script.OnIdle()

    def take(self):
        """Everything the script has sent, as it went out over FL Studio's idle calls."""
        self.idle(100)
        sent, self.studio.sent = self.studio.sent, []
        return sent

    def ask(self, request):
        """Sends a request and returns the one reply to it."""
        self.last += 1
        request = dict({"id": self.last, "sent": self.clock.now}, **request)
        self.send(request)
        reply_id, text = unframe(self.take())
        reply = json.loads(text)
        assert reply["id"] == reply_id == request["id"]
        return reply

    def result(self, request):
        reply = self.ask(request)
        assert reply["ok"], reply
        return reply["result"]

    def error(self, request):
        reply = self.ask(request)
        assert not reply["ok"], reply
        return reply["error"]


class ScriptFile(unittest.TestCase):
    def test_starts_with_the_name_fl_studio_lists(self):
        with open(SCRIPT, encoding="utf-8") as f:
            lines = f.read().splitlines()
        self.assertEqual(lines[0], "# name=Music Organiser")
        self.assertIn("# supportedDevices=Music Organiser", lines[:5])
        self.assertTrue(os.path.basename(SCRIPT).startswith("device_"))

    def test_changes_are_off_as_shipped(self):
        with open(SCRIPT, encoding="utf-8") as f:
            self.assertIn("\nLET_THE_APP_MAKE_CHANGES = False\n", f.read())

    def test_is_plain_ascii(self):
        # FL Studio reads the header lines itself, so the file stays readable in any encoding.
        with open(SCRIPT, "rb") as f:
            f.read().decode("ascii")

    def test_never_opens_files(self):
        # FL Studio stops scripts opening files ("SystemError: <class '_io.FileIO'> returned NULL").
        with open(SCRIPT, encoding="utf-8") as f:
            source = f.read()
        self.assertNotIn("open(", source)


class LinkTests(unittest.TestCase):
    def link(self, studio=None, changes=False):
        link = Link(studio or night_drive(), changes)
        self.addCleanup(link.close)
        return link

    def test_says_where_fl_studio_loaded_it_from_without_file(self):
        link = self.link()
        self.assertEqual(link.result({"cmd": "hello"})["script"], os.path.abspath(link.installed))
        self.assertIn("Music Organiser link 2 is on", link.printed.getvalue())
        self.assertNotIn("no MIDI output", link.printed.getvalue())

    def test_says_hello(self):
        link = self.link()
        hello = link.result({"cmd": "hello"})
        self.assertEqual(hello["product"], "FL Studio")
        self.assertEqual((hello["link"], hello["api"], hello["changes"]), (2, 37, False))
        self.assertEqual(hello["version"], "Producer Edition v25.1.6 [build 4997]")
        self.assertEqual(hello["title"], "Night Drive - FL Studio 2025")
        self.assertEqual(hello["modified"], False)

    def test_says_when_the_project_has_unsaved_changes(self):
        # The app watches this to re-read a project as soon as it's saved.
        studio = night_drive(modified=1)
        link = self.link(studio)
        self.assertEqual(link.result({"cmd": "hello"})["modified"], True)
        studio.modified = 0
        self.assertEqual(link.result({"cmd": "hello"})["modified"], False)

    def test_lists_the_channels_and_the_mixer(self):
        link = self.link()
        p = link.result({"cmd": "project"})
        self.assertEqual((p["tempo"], p["modified"], p["grouped"]), (124.0, False, False))
        self.assertEqual([c["name"] for c in p["channels"]], ["Kick", "Bass", "Chops", "Café pad"])
        kick, bass = p["channels"][0], p["channels"][1]
        self.assertEqual(kick, {"index": 0, "name": "Kick", "type": 0, "mixer": 1, "plugin": None, "params": 0})
        self.assertEqual((bass["plugin"], bass["params"], bass["type"], bass["mixer"]), ("Serum", 4240, 2, 2))
        self.assertEqual([t["name"] for t in p["mixer"]], ["Master", "Kick", "Bass"])
        self.assertEqual(p["mixer"][2]["plugins"], [
            {"slot": 0, "name": "Fruity Parametric EQ 2", "label": "Fruity Parametric EQ 2", "params": 1},
            {"slot": 3, "name": "Fruity Reeverb 2", "label": "Room", "params": 1},
        ])

    def test_reads_the_tempo_either_way_fl_studio_gives_it(self):
        self.assertEqual(self.link(night_drive(tempo=93.5)).result({"cmd": "project"})["tempo"], 93.5)
        self.assertEqual(self.link(night_drive(tempo=140000)).result({"cmd": "project"})["tempo"], 140.0)

    def test_older_fl_studio_lists_the_channels_the_rack_shows(self):
        link = self.link(night_drive(api=30, group=[1, 3]))
        p = link.result({"cmd": "project"})
        self.assertTrue(p["grouped"])
        self.assertEqual([(c["index"], c["name"], c["plugin"]) for c in p["channels"]],
                         [(1, "Bass", "Serum"), (3, "Café pad", "FLEX")])

    def test_needs_a_scripting_version_with_global_plugin_indexes(self):
        link = self.link(night_drive(api=25))
        self.assertIn("needs a newer FL Studio", link.error({"cmd": "project"}))
        self.assertEqual(link.result({"cmd": "hello"})["api"], 25)

    def test_lists_a_plugins_parameters_in_pages(self):
        link = self.link()
        link.clock.step = 0.001   # each call takes a millisecond, so a list runs over several requests
        found, pages, start = [], 0, 0
        while start is not None:
            page = link.result({"cmd": "params", "channel": 1, "from": start})
            self.assertEqual((page["plugin"], page["count"]), ("Serum", 4240))
            found += page["params"]
            pages += 1
            start = page["next"]
        self.assertGreater(pages, 1)
        self.assertEqual(found, [
            {"index": 0, "name": "Master Volume", "value": 0.7, "text": "-3.1 dB"},
            {"index": 1, "name": "A Octave", "value": 0.5, "text": "0"},
            {"index": 3, "name": "Filter Cutoff", "value": 0.25, "text": "440 Hz"},
        ])

    def test_lists_midi_cc_parameters_when_asked(self):
        link = self.link()
        page = link.result({"cmd": "params", "channel": 1, "from": 4000, "midi": True})
        while page["next"] is not None:
            more = link.result({"cmd": "params", "channel": 1, "from": page["next"], "midi": True})
            page["params"] += more["params"]
            page["next"] = more["next"]
        self.assertEqual(len(page["params"]), 128)
        self.assertEqual(page["params"][0]["name"], "MIDI CC #0")

    def test_lists_a_mixer_plugins_parameters(self):
        link = self.link()
        page = link.result({"cmd": "params", "track": 2, "slot": 3})
        self.assertEqual(page, {"plugin": "Fruity Reeverb 2", "count": 1, "next": None,
                                "params": [{"index": 0, "name": "Mix", "value": 0.3, "text": "30%"}]})

    def test_changes_are_off_unless_turned_on(self):
        link = self.link()
        self.assertIn("Changes are off", link.error({"cmd": "set", "channel": 2, "param": 0, "value": 0.5}))
        self.assertEqual(link.studio.channels[2].plugin.sets, [])

    def test_sets_a_parameter_and_says_what_it_became(self):
        link = self.link(changes=True)
        self.assertTrue(link.result({"cmd": "hello"})["changes"])
        self.assertEqual(link.result({"cmd": "set", "channel": 2, "param": 0, "value": 0.25}),
                         {"index": 0, "name": "Main volume", "value": 0.25, "text": "25%"})
        self.assertEqual(link.result({"cmd": "set", "track": 0, "slot": 0, "param": 0, "value": 1}),
                         {"index": 0, "name": "Gain", "value": 1.0, "text": "100%"})
        self.assertEqual(link.studio.channels[2].plugin.sets, [(0, 0.25)])

    def test_says_what_is_wrong_with_a_request(self):
        link = self.link(changes=True)
        self.assertEqual(link.error({"cmd": "dance"}), "Unknown command: dance")
        self.assertIn("Say which plugin", link.error({"cmd": "params"}))
        self.assertEqual(link.error({"cmd": "params", "channel": 9}), "There's no channel 9.")
        self.assertEqual(link.error({"cmd": "params", "channel": 0}), "There's no plugin on channel 0.")
        self.assertEqual(link.error({"cmd": "params", "track": 1, "slot": 0}), "There's no plugin on mixer track 1, slot 0.")
        self.assertIn("There's no mixer track 3", link.error({"cmd": "params", "track": 3, "slot": 0}))
        self.assertIn("There's no mixer track 2 with a slot 10", link.error({"cmd": "params", "track": 2, "slot": 10}))
        self.assertIn("must be a parameter number", link.error({"cmd": "params", "channel": 1, "from": -1}))
        self.assertEqual(link.error({"cmd": "set", "channel": 1, "param": 2, "value": 0.5}),
                         "There's no parameter 2 on channel 1.")
        self.assertIn("from 0 to 1", link.error({"cmd": "set", "channel": 1, "param": 0, "value": 2}))
        self.assertIn("from 0 to 1", link.error({"cmd": "set", "channel": 1, "param": 0, "value": True}))

    def test_passes_on_fl_studios_own_errors(self):
        link = self.link()
        link.studio.broken = "getPluginName"
        self.assertEqual(link.error({"cmd": "project"}),
                         "FL Studio couldn't answer (TypeError: Operation unsafe at current time)")

    def test_ignores_what_isnt_a_request(self):
        link = self.link()
        for raw in [b"not json", b"[1, 2]", b'{"cmd": "hello"}', b'{"id": "seven", "cmd": "hello"}']:
            self.assertTrue(link.send({"id": 1}, raw=raw))
            self.assertEqual(link.take(), [])

    def test_leaves_other_sysex_messages_alone(self):
        link = self.link()
        for message in [bytes([0xF0, 0x43, 0x10, 0x4C, 0x00, 0xF7]), HEAD + bytes([1, 1, 0, 0, 0, 0]) + b"\xF7",
                        frame(1, 1, b"{}")[0][:-1] + b"\x80\xF7"]:
            event = SysEx(message)
            link.script.OnSysEx(event)
            self.assertFalse(event.handled)
        self.assertEqual(link.take(), [])

    def test_ignores_its_own_answers_coming_back_round(self):
        # On a loopback port, what the script sends out comes back in on its input too.
        link = self.link()
        self.assertTrue(link.send({"id": 4, "cmd": "hello", "sent": link.clock.now}, kind=2))
        self.assertEqual(link.take(), [])

    def test_ignores_a_request_the_app_gave_up_on(self):
        link = self.link()
        link.send({"id": 5, "cmd": "hello", "sent": link.clock.now - 60})
        self.assertEqual(link.take(), [])
        self.assertEqual(link.ask({"cmd": "hello"})["ok"], True)

    def test_takes_a_request_in_several_messages(self):
        link = self.link()
        text = json.dumps({"id": 7, "sent": link.clock.now, "cmd": "hello", "padding": "x" * 1200}).encode("ascii")
        messages = frame(1, 7, text)
        self.assertEqual(len(messages), 3)
        for m in messages[:2]:
            link.script.OnSysEx(SysEx(m))
        self.assertEqual(link.take(), [])
        link.script.OnSysEx(SysEx(messages[2]))
        reply_id, answer = unframe(link.take())
        self.assertEqual((reply_id, json.loads(answer)["ok"]), (7, True))

    def test_sends_a_long_answer_a_few_messages_at_a_time(self):
        link = self.link()
        link.send({"id": 9, "sent": link.clock.now, "cmd": "params", "channel": 1, "from": 4000, "midi": True})
        each = link.script.PARTS_PER_IDLE
        self.assertEqual(len(link.studio.sent), each)
        link.idle()
        self.assertEqual(len(link.studio.sent), 2 * each)
        sent = link.take()
        self.assertGreater(len(sent), 2 * each)
        reply_id, text = unframe(sent)
        page = json.loads(text)["result"]
        self.assertEqual((reply_id, page["params"][0]["name"]), (9, "MIDI CC #0"))

    def test_a_new_request_drops_what_is_left_of_the_last_answer(self):
        link = self.link()
        link.send({"id": 1, "sent": link.clock.now, "cmd": "params", "channel": 1, "from": 4000, "midi": True})
        link.studio.sent = []
        reply = link.ask({"cmd": "hello"})
        self.assertTrue(reply["ok"])

    def test_says_when_it_has_no_midi_output_to_answer_through(self):
        studio = night_drive()
        studio.output = False
        link = self.link(studio)
        self.assertIn("no MIDI output to answer through", link.printed.getvalue())
        link.printed = io.StringIO()
        for i in range(2):
            link.send({"id": 20 + i, "cmd": "hello", "sent": link.clock.now})
        self.assertEqual(link.take(), [])
        self.assertEqual(link.printed.getvalue().count("give the Music Organiser output the same Port number"), 1)
        studio.output = True
        self.assertTrue(link.result({"cmd": "hello"})["link"])

    def test_keeps_names_intact_in_plain_ascii(self):
        link = self.link()
        p = link.result({"cmd": "project"})
        self.assertEqual(p["channels"][3]["name"], "Café pad")

    def test_stays_quiet_without_its_standard_library(self):
        link = self.link()
        link.script.json = None
        link.script.OnDeInit()
        printed = io.StringIO()
        with redirect_stdout(printed):
            link.script.OnInit()
        self.assertIn("can't read the app's requests", printed.getvalue())
        link.send({"id": 1, "cmd": "hello", "sent": link.clock.now})
        self.assertEqual(link.take(), [])

    def test_does_nothing_once_fl_studio_lets_it_go(self):
        link = self.link()
        link.script.OnDeInit()
        link.send({"id": 1, "cmd": "hello", "sent": link.clock.now})
        self.assertEqual(link.take(), [])


if __name__ == "__main__":
    unittest.main()


class Measuring(unittest.TestCase):
    def run_script(self, title, changes=True, idles=600, info_title=""):
        studio = fake_fl.night_drive(title=title, mixer=[("Master", {})] + [("Insert", {})] * 125,
                                     info_title=info_title)
        script = fake_fl.compile_script(studio)
        script.MEASURE_IN_LIVE_TEST = changes
        script.OnInit()
        for _ in range(idles):
            script.OnIdle()
        return studio

    def test_writes_the_fader_and_tempo_tables_as_insert_names_and_saves(self):
        # FL Studio 2025 shows the file name with ".flp", and a "*" while it has unsaved changes.
        studio = self.run_script("MO Live Test automation.flp* - FL Studio 2025")
        self.assertEqual(studio.saves, 1)
        self.assertEqual(studio.names[1], "v0.00 -infdB")
        self.assertEqual(studio.names[81], "v0.80 0.0dB")
        self.assertEqual(studio.names[101], "v1.00 1.938dB")
        self.assertEqual(studio.names[102], "t0.00 10.0bpm")
        self.assertEqual(studio.names[122], "t1.00 522.0bpm")
        self.assertEqual(len(studio.names), 122)
        # Everything put back as it was.
        self.assertEqual(studio.tempo, 124000)
        self.assertEqual(set(studio.volumes.values()), {0.8})

    def test_knows_the_live_test_by_its_project_info_title(self):
        # FL Studio 2025 gives scripts only "FL Studio 2025" as the title.
        studio = self.run_script("FL Studio 2025", info_title="MO Live Test automation ")
        self.assertEqual(studio.saves, 1)
        self.assertEqual(studio.names[81], "v0.80 0.0dB")
        for other in ("", "Night Drive", "MO Live Test automation 2"):
            studio = self.run_script("FL Studio 2025", info_title=other)
            self.assertEqual((studio.saves, studio.names, studio.volumes), (0, {}, {}), other)

    def test_reads_the_project_name_from_the_title_as_the_app_does(self):
        script = fake_fl.compile_script(fake_fl.night_drive())
        for title, name in (
            ("Night Drive - FL Studio 2025", "Night Drive"),
            ("Night Drive.flp - FL Studio 2025", "Night Drive"),
            ("Night Drive.FLP* - FL Studio 2025", "Night Drive"),
            ("FL Studio 2025 - Night Drive", "Night Drive"),
            ("Intro - Verse.flp - FL Studio 2025", "Intro - Verse"),
            ("FL Studio 2025", ""),
        ):
            self.assertEqual(script.project_name(title), name, title)

    def test_never_runs_in_another_project_or_with_changes_off(self):
        for title, changes in (("Night Drive - FL Studio 2025", True), ("MO Live Test automation", False),
                               ("MO Live Test automation (fixed) - FL Studio 2025", True),
                               ("MO Live Test automation 2.flp - FL Studio 2025", True)):
            studio = self.run_script(title, changes)
            self.assertEqual((studio.saves, studio.names, studio.volumes), (0, {}, {}))
