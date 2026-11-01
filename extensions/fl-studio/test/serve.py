"""
Runs the FL Studio script in the stand-in FL Studio (fake_fl.py) for the app's own tests, in place of the
Music Organiser MIDI port: a line "sysex <hex>" on its input is a SysEx message coming in on the port, and it
prints each message the script sends out the same way. Changes are allowed. Prints "ready" once it's
running, and stops when its input closes. Each other line of input changes the stand-in, as if done in FL
Studio: {"title": "Other Song - FL Studio 2025", "modified": 1} opens another project or edits this one,
{"channels": [["Lead", "Serum"], ["Kick", null]]} puts these channels in the channel rack (plugins it has
keep their knobs), and {"knob": [channel, param, value]} turns a knob. It prints "ok" once the change is made.

    python serve.py
"""
import contextlib
import io
import json
import os
import shutil
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from fake_fl import Channel, Plugin, SysEx, load_script, night_drive  # noqa: E402


def main():
    installed = tempfile.mkdtemp()   # stands in for FL Studio's Settings/Hardware/Music Organiser
    studio = night_drive()
    printing = threading.Lock()

    def out(line):
        with printing:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()

    with contextlib.redirect_stdout(io.StringIO()):   # what the script prints in FL Studio's script window
        script = load_script(studio, installed)
    script.LET_THE_APP_MAKE_CHANGES = True
    studio.on_send = lambda message: out("sysex " + message.hex())
    done = threading.Event()
    lock = threading.Lock()

    def follow_input():
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            if line.startswith("sysex "):
                with lock:
                    script.OnSysEx(SysEx(bytes.fromhex(line[6:])))
                continue
            change = json.loads(line)
            with lock:
                if "title" in change:
                    studio.title = change["title"]
                if "modified" in change:
                    studio.modified = change["modified"]
                if "channels" in change:
                    known = {c.plugin.name: c.plugin for c in studio.channels if c.plugin}
                    studio.channels = [
                        Channel(name, kind=2 if plugin else 0, mixer=i + 1,
                                plugin=(known.get(plugin) or Plugin(plugin)) if plugin else None)
                        for i, (name, plugin) in enumerate(change["channels"])]
                    studio.group = list(range(len(studio.channels)))
                if "knob" in change:
                    channel, param, value = change["knob"]
                    studio.channels[channel].plugin.params[param][1] = value
            out("ok")
        done.set()

    threading.Thread(target=follow_input, daemon=True).start()
    out("ready")
    while not done.is_set():
        with lock:
            script.OnIdle()   # FL Studio calls it about every 20 ms
        time.sleep(0.02)
    shutil.rmtree(installed, ignore_errors=True)


if __name__ == "__main__":
    main()
