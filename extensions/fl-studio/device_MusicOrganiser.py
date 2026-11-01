# name=Music Organiser
# supportedDevices=Music Organiser
"""
The Music Organiser link for FL Studio.

Lets the Music Organiser app ask FL Studio what it shows for the open project: the tempo, the channels,
the mixer and the plugins, and each plugin's parameters. The app uses this to check how it reads .flp
files and to learn what plugin settings mean.

FL Studio runs the script once it's the controller type of a MIDI input (Options > MIDI settings). A MIDI
input called "Music Organiser" (for example a loopMIDI port) gets it by itself. FL Studio doesn't let scripts
open files or connect to anything, so the app and the script talk in SysEx messages on that MIDI port: the
app sends a request into the port, and the script answers through the Music Organiser MIDI output, which
needs the same Port number as the input in FL Studio's MIDI settings.

Nothing in the project changes unless LET_THE_APP_MAKE_CHANGES below is True.
"""

# True lets the app change plugin parameters. The app keeps this setting when it updates the script.
LET_THE_APP_MAKE_CHANGES = False

# True measures FL Studio's fader and tempo curves once, in the project "MO Live Test automation" only (see measure()):
# it moves that project's faders and tempo, renames 122 of its inserts and saves it.
MEASURE_IN_LIVE_TEST = False

import channels
import device
import general
import midi
import mixer
import plugins
import transport
import ui

try:
    import json
    import os
    import sys
    import time
except ImportError:  # a Python without its standard library can't read the app's requests
    json = None

LINK_VERSION = 2
SCRIPT = "device_MusicOrganiser.py"
STALE_SECONDS = 15    # a request older than this is one the app gave up on
PAGE_SECONDS = 0.04   # how long one list of parameters may take, so FL Studio never stalls
SLOTS = 10            # effect slots on each mixer track
VST_PARAMS = 4096     # VST plugins list 4240 parameters: their own 4096, then MIDI CC and aftertouch
CHANGES_OFF = ("Changes are off. To let the app change FL Studio, set LET_THE_APP_MAKE_CHANGES = True in "
               "device_MusicOrganiser.py (in FL Studio's Settings, Hardware, Music Organiser folder), then "
               "reload the script.")
NO_OUTPUT = ("Music Organiser: the app asked something, but the script has no MIDI output to answer through. In "
             "Options > MIDI settings, give the Music Organiser output the same Port number as the Music Organiser "
             "input.")

# The messages: F0, 7D (the maker ID MIDI keeps for non-commercial use), "MO", then whether it's a request (1)
# or an answer (2), the request's id in three 7-bit bytes, this part's number and how many parts there are in
# two 7-bit bytes each (lowest first), then up to PART_BYTES of the JSON text, which is plain ASCII, and F7.
HEAD = bytes([0xF0, 0x7D, 0x4D, 0x4F])
REQUEST, ANSWER = 1, 2
PART_BYTES = 512
PARTS_PER_IDLE = 8    # answer messages sent each time FL Studio calls OnIdle, so a long answer doesn't flood the port


class LinkError(Exception):
    """A problem with a request, told to the app in these words."""


_on = False
_script = None   # the folder FL Studio loaded the script from
_incoming = None  # the request coming in: [id, parts, the parts so far]
_outgoing = []    # answer messages still to send
_told = False     # said there's no MIDI output to answer through


def OnInit():
    global _on, _script, _told
    if json is None:
        print("Music Organiser: this FL Studio's Python can't read the app's requests, which the link needs.")
        return
    _on, _told = True, False
    _script = script_folder()
    print("Music Organiser link " + str(LINK_VERSION) + " is on: the app's requests come in on this MIDI port.")
    if not device.isAssigned():
        print(NO_OUTPUT.replace("the app asked something, but ", ""))


def OnDeInit():
    global _on, _incoming
    _on, _incoming = False, None
    del _outgoing[:]


def OnSysEx(event):
    """A request from the app, in one message or several."""
    message = read_message(bytes(event.sysex))
    if message is None:
        return  # someone else's
    event.handled = True
    kind, request_id, part, parts, text = message
    if not _on or kind != REQUEST:
        return  # answers come back round to this input on a loopback port
    data = take_part(request_id, part, parts, text)
    if data is None:
        return
    reply = answer(data, time.time())
    if reply is not None:
        send(reply)


def OnIdle():
    if _outgoing:
        flush()
    measure()


def script_folder():
    """
    The folder FL Studio loaded this script from, or None if it can't tell. FL Studio puts the folder on the
    module path, and scripts there don't get __file__.
    """
    try:
        here = os.path.dirname(os.path.abspath(__file__))
        if os.path.isfile(os.path.join(here, SCRIPT)):
            return here
    except NameError:
        pass
    for folder in sys.path:
        if isinstance(folder, str) and folder and os.path.isfile(os.path.join(folder, SCRIPT)):
            return os.path.abspath(folder)
    return None


# Measuring (Format Lab): how FL Studio turns an automation value into decibels and BPM

MEASURE_PROJECT = "MO Live Test automation"
FADER_STEPS = 100     # the fader at 0.00, 0.01 ... 1.00, each on its own insert (1 to 101)
TEMPO_STEPS = 20      # the tempo at 0.00, 0.05 ... 1.00, written on inserts 102 to 122
TEMPO_FIRST = FADER_STEPS + 2

_measuring = None     # the measurement under way: the step, what was found, and what to put back


def in_live_test():
    """Whether the project open is exactly MEASURE_PROJECT (the title is "<project> - FL Studio ...")."""
    name = ui.getProgTitle().split(" - ")[0].strip().rstrip("*").strip()
    return name == MEASURE_PROJECT


def measure():
    """
    Runs once, a step at each idle, when MEASURE_IN_LIVE_TEST is True and the project open is MEASURE_PROJECT: sets
    each insert's fader to a known value and reads, at the next idle, the decibels FL Studio shows; then the tempo to
    known automation values and reads the BPM; puts the faders and tempo back, writes what it found as the inserts'
    names, and saves the project, so Format Lab reads the table from the .flp (FL Studio doesn't let scripts write
    files).
    """
    global _measuring
    if not MEASURE_IN_LIVE_TEST or _measuring == "done":
        return
    try:
        if not in_live_test():
            return
        if _measuring is None:
            if mixer.trackCount() - 1 <= TEMPO_FIRST + TEMPO_STEPS:
                print("Music Organiser: this FL Studio has too few mixer inserts to measure on, so nothing changed.")
                _measuring = "done"
                return
            _measuring = {"step": 0, "names": {}, "tempo": tempo(), "faders": {}, "tempos": []}
            print("Music Organiser: measuring the fader and tempo in " + MEASURE_PROJECT)
        m = _measuring
        step = m["step"]
        fader_end = 2 * (FADER_STEPS + 1)
        tempo_end = fader_end + 2 * (TEMPO_STEPS + 1)
        if step < fader_end:
            # Set at one idle, read at the next, once FL Studio has moved it.
            track = step // 2 + 1
            value = (step // 2) / float(FADER_STEPS)
            if step % 2 == 0:
                m["faders"][track] = mixer.getTrackVolume(track)
                mixer.setTrackVolume(track, value)
            else:
                m["names"][track] = "v%.2f %sdB" % (value, round(mixer.getTrackVolume(track, 1), 3))
        elif step < tempo_end:
            t = step - fader_end
            value = (t // 2) / float(TEMPO_STEPS)
            if t % 2 == 0:
                set_tempo(value)
            else:
                bpm = tempo()
                m["tempos"].append((value, bpm))
                m["names"][TEMPO_FIRST + t // 2] = "t%.2f %sbpm" % (value, round(bpm, 3))
        elif step == tempo_end:
            # Back as it was: each fader, and the tempo at the automation value that gave it.
            for track, value in m["faders"].items():
                mixer.setTrackVolume(track, value)
            back = value_for(m["tempos"], m["tempo"])
            if back is not None:
                set_tempo(back)
        else:
            for track, name in m["names"].items():
                mixer.setTrackName(track, name)
            transport.globalTransport(midi.FPT_Save, 1)
            print("Music Organiser: measured and saved " + MEASURE_PROJECT)
            _measuring = "done"
            return
        m["step"] = step + 1
    except Exception as e:  # never stop FL Studio over a measurement
        print("Music Organiser: measuring stopped: " + str(e))
        _measuring = "done"


def set_tempo(value):
    """The tempo at an automation value (0..1), by the route FL Studio's automation takes."""
    general.processRECEvent(midi.REC_Tempo, int(round(value * midi.FromMIDI_Max)),
                            midi.REC_FromMIDI | midi.REC_UpdateValue | midi.REC_UpdateControl)


def value_for(pairs, bpm):
    """The automation value that gives `bpm`, between the measured (value, bpm) pairs, or None."""
    if bpm is None:
        return None
    for (v0, b0), (v1, b1) in zip(pairs, pairs[1:]):
        if b0 is not None and b1 is not None and min(b0, b1) <= bpm <= max(b0, b1):
            return v0 if b1 == b0 else v0 + (v1 - v0) * (bpm - b0) / (b1 - b0)
    return None


# The messages

def read_message(data):
    """(kind, id, part, parts, text) from one of the link's messages, or None for any other SysEx message."""
    if len(data) < 13 or data[:4] != HEAD or data[-1] != 0xF7 or any(b > 0x7F for b in data[4:-1]):
        return None
    request_id = data[5] | data[6] << 7 | data[7] << 14
    part, parts = data[8] | data[9] << 7, data[10] | data[11] << 7
    if part >= parts:
        return None
    return data[4], request_id, part, parts, data[12:-1]


def messages(kind, request_id, text):
    """The messages that carry this text, PART_BYTES at a time."""
    parts = max(1, (len(text) + PART_BYTES - 1) // PART_BYTES)
    found = []
    for part in range(parts):
        found.append(HEAD + bytes([kind, request_id & 0x7F, request_id >> 7 & 0x7F, request_id >> 14 & 0x7F,
                                   part & 0x7F, part >> 7 & 0x7F, parts & 0x7F, parts >> 7 & 0x7F])
                     + text[part * PART_BYTES:(part + 1) * PART_BYTES] + b"\xF7")
    return found


def take_part(request_id, part, parts, text):
    """Keeps a part of a request, and returns the whole request once every part is in."""
    global _incoming
    if _incoming is None or _incoming[0] != request_id or _incoming[1] != parts:
        _incoming = [request_id, parts, {}]  # the app only asks one thing at a time
    _incoming[2][part] = text
    if len(_incoming[2]) < parts:
        return None
    got = _incoming[2]
    _incoming = None
    return b"".join(got[p] for p in range(parts))


def send(reply):
    """Sends a reply to the app: the first messages now, the rest as FL Studio calls OnIdle."""
    global _told
    if not device.isAssigned():
        if not _told:
            print(NO_OUTPUT)
            _told = True
        return
    _told = False
    try:
        text = json.dumps(reply, ensure_ascii=True, allow_nan=False)
    except ValueError as e:
        text = json.dumps({"id": reply["id"], "ok": False, "error": "FL Studio's answer couldn't be sent: " + str(e)})
    # The app asks one thing at a time, so whatever is left of an earlier answer is one it stopped waiting for.
    _outgoing[:] = messages(ANSWER, reply["id"], text.encode("ascii"))
    flush()


def flush():
    for message in _outgoing[:PARTS_PER_IDLE]:
        device.midiOutSysex(message)
    del _outgoing[:PARTS_PER_IDLE]


def answer(data, now):
    """The reply to one request, or None for something that isn't a request (or is too old to answer)."""
    try:
        request = json.loads(data.decode("utf-8-sig"))
    except ValueError:
        return None
    if not isinstance(request, dict) or not is_whole(request.get("id")) or not 0 <= request["id"] < 1 << 21:
        return None
    sent = request.get("sent")
    if is_number(sent) and now - sent > STALE_SECONDS:
        return None
    reply = {"id": request["id"]}
    try:
        command = COMMANDS.get(request.get("cmd"))
        if command is None:
            raise LinkError("Unknown command: " + str(request.get("cmd")))
        reply["result"] = command(request)
        reply["ok"] = True
    except LinkError as e:
        reply["ok"] = False
        reply["error"] = str(e)
    except Exception as e:  # FL Studio turned a call down; the app shows why
        reply["ok"] = False
        reply["error"] = "FL Studio couldn't answer (" + type(e).__name__ + ": " + str(e) + ")"
    return reply


# Commands

def hello(request):
    return {
        "link": LINK_VERSION,
        "product": "FL Studio",
        "version": ui.getVersion(4),
        "api": general.getVersion(),
        "title": ui.getProgTitle(),
        "modified": general.getChangedFlag() != 0,
        "changes": LET_THE_APP_MAKE_CHANGES,
        "platform": sys.platform,
        "script": _script,
    }


def project(request):
    api = needs_api(26)
    return {
        "title": ui.getProgTitle(),
        "tempo": tempo(),
        "modified": general.getChangedFlag() != 0,
        # Before scripting version 33, FL Studio only lists the channels in the group the channel rack shows.
        "grouped": api < 33,
        "channels": [channel(ref, index) for ref, index in channel_list(api)],
        # The last mixer track is FL Studio's "current" track, not one of the project's.
        "mixer": [mixer_track(t) for t in range(mixer.trackCount() - 1)],
    }


def params(request):
    """A plugin's named parameters, from "from" on: as many as fit in PAGE_SECONDS, with where to go on."""
    needs_api(26)
    target, _ = plugin_at(request)
    count = plugins.getParamCount(*target)
    end = VST_PARAMS if count == VST_PARAMS + 144 and request.get("midi") is not True else count
    start = request.get("from", 0)
    if not is_whole(start) or start < 0:
        raise LinkError('"from" must be a parameter number.')
    found = []
    stop = time.time() + PAGE_SECONDS
    p = start
    while p < end:
        name = plugins.getParamName(p, *target)
        if name:
            found.append(param(p, name, target))
        p += 1
        if time.time() > stop:
            break
    return {
        "plugin": plugins.getPluginName(target[0], target[1], False, target[2]),
        "count": count,
        "params": found,
        "next": p if p < end else None,
    }


def set_param(request):
    """Sets a parameter (0 to 1), and says what it became."""
    if not LET_THE_APP_MAKE_CHANGES:
        raise LinkError(CHANGES_OFF)
    needs_api(26)
    target, where = plugin_at(request)
    p = request.get("param")
    if not is_whole(p) or p < 0 or p >= plugins.getParamCount(*target) or not plugins.getParamName(p, *target):
        raise LinkError("There's no parameter " + str(p) + " on " + where + ".")
    value = request.get("value")
    if not is_number(value) or value < 0 or value > 1:
        raise LinkError('"value" must be a number from 0 to 1.')
    plugins.setParamValue(float(value), p, target[0], target[1], 0, target[2])
    return param(p, plugins.getParamName(p, *target), target)


COMMANDS = {"hello": hello, "project": project, "params": params, "set": set_param}


# What FL Studio shows

def channel_list(api):
    """Each channel as (its index for the channels module, its global index), in channel rack order."""
    if api >= 33:
        return [((i, True), i) for i in range(channels.channelCount(True))]
    return [((i,), channels.getChannelIndex(i)) for i in range(channels.channelCount())]


def channel(ref, index):
    info = {
        "index": index,
        "name": channels.getChannelName(*ref),
        "type": channels.getChannelType(*ref),
        "mixer": channels.getTargetFxTrack(*ref),
        "plugin": None,
        "params": 0,
    }
    if plugins.isValid(index, -1, True):
        info["plugin"] = plugins.getPluginName(index, -1, False, True)
        info["params"] = plugins.getParamCount(index, -1, True)
    return info


def mixer_track(t):
    slots = []
    for s in range(SLOTS):
        if mixer.isTrackPluginValid(t, s):
            slots.append({
                "slot": s,
                "name": plugins.getPluginName(t, s),
                "label": plugins.getPluginName(t, s, True),
                "params": plugins.getParamCount(t, s),
            })
    return {"index": t, "name": mixer.getTrackName(t), "plugins": slots}


def param(p, name, target):
    index, slot, by_global = target
    return {
        "index": p,
        "name": name,
        "value": num(plugins.getParamValue(p, index, slot, by_global)),
        "text": plugins.getParamValueString(p, index, slot, 0, by_global),
    }


def plugin_at(request):
    """The plugin a request names, as (index, slot, useGlobalIndex) for the plugins module, and in words."""
    if "channel" in request:
        c = request["channel"]
        if not is_whole(c) or c < 0 or c >= channels.channelCount(True):
            raise LinkError("There's no channel " + str(c) + ".")
        target, where = (c, -1, True), "channel " + str(c)
    elif "track" in request:
        t, s = request["track"], request.get("slot")
        if not is_whole(t) or t < 0 or t >= mixer.trackCount() - 1 or not is_whole(s) or s < 0 or s >= SLOTS:
            raise LinkError("There's no mixer track " + str(t) + " with a slot " + str(s) + ".")
        target, where = (t, s, False), "mixer track " + str(t) + ", slot " + str(s)
    else:
        raise LinkError('Say which plugin: a "channel", or a mixer "track" and "slot".')
    if not plugins.isValid(*target):
        raise LinkError("There's no plugin on " + where + ".")
    return target, where


def tempo():
    t = mixer.getCurrentTempo()
    if not is_number(t):
        return None
    return t / 1000.0 if t > 1000 else float(t)  # some versions give thousandths of a BPM


def needs_api(least):
    api = general.getVersion()
    if api < least:
        raise LinkError("This needs a newer FL Studio: its scripting version is " + str(api) + ", and the link needs "
                        + str(least) + " or later.")
    return api


def is_whole(v):
    return isinstance(v, int) and not isinstance(v, bool)


def is_number(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool) and v == v and abs(v) != float("inf")


def num(v):
    """A number JSON can carry, or None."""
    return float(v) if is_number(v) else None
