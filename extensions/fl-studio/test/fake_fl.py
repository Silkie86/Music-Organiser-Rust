"""
A stand-in for the parts of FL Studio's scripting modules the Music Organiser script uses, so the script can
be run and checked without FL Studio. Like FL Studio's own functions, these take their arguments by position,
and the useGlobalIndex flag only exists from the scripting version that added it.
"""
import os
import shutil
import sys
import types

SCRIPT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "device_MusicOrganiser.py")
VST_COUNT = 4240
REC_TEMPO = 0x10000 + 5
FROM_MIDI_MAX = 1 << 30
FPT_SAVE = 92


class Plugin:
    """A plugin: a name, and its parameters as (name, value, text) by index."""

    def __init__(self, name, params=(), label=None, vst=False):
        self.name = name
        self.label = label or name
        self.params = {i: list(p) for i, p in enumerate(params)}
        self.count = VST_COUNT if vst else len(params)
        if vst:
            for cc in range(128):
                self.params[4096 + cc] = ["MIDI CC #" + str(cc), 0.0, "0"]
        self.sets = []


class Channel:
    def __init__(self, name, kind=2, mixer=0, plugin=None):
        self.name, self.kind, self.mixer, self.plugin = name, kind, mixer, plugin


class Clock:
    """FL Studio's time: moves on by `step` each time it's read, as if every call took that long."""

    def __init__(self, now=1000.0, step=0.0):
        self.now, self.step = now, step

    def time(self):
        self.now += self.step
        return self.now


class Studio:
    """An FL Studio with a project open. `group` lists the global channels the channel rack shows."""

    def __init__(self, api=37, title="Night Drive - FL Studio 2025", tempo=124000, modified=0, channels=(),
                 mixer=(), group=None, version="Producer Edition v25.1.6 [build 4997]", info_title=""):
        self.api, self.title, self.tempo, self.modified, self.version = api, title, tempo, modified, version
        self.info_title = info_title  # the title in File > Project info
        self.channels = list(channels)
        self.mixer = list(mixer)   # (name, {slot: Plugin}) for each track, master first
        self.group = list(range(len(self.channels))) if group is None else list(group)
        self.broken = None         # a function name that fails, as FL Studio does when a call isn't allowed
        self.output = True         # whether the script's MIDI input has an output with the same port number
        self.sent = []             # the SysEx messages the script sent through that output
        self.on_send = None        # called with each one too, if set
        self.volumes = {}          # each mixer track's fader, 0..1, as the script set it
        self.names = {}            # mixer track names the script set
        self.saves = 0             # how many times the script saved the project

    def check(self, name):
        if self.broken == name:
            raise TypeError("Operation unsafe at current time")

    def flag(self, since, given):
        """The useGlobalIndex flag, refused before the scripting version that added it."""
        if given and self.api < since:
            raise TypeError("function takes at most fewer arguments")
        return given[0] if given else False

    def channel(self, index, by_global):
        return self.channels[index if by_global else self.group[index]]

    def plugin(self, index, slot, by_global):
        if slot == -1:
            p = self.channel(index, by_global).plugin
        else:
            p = self.mixer[index][1].get(slot)
        if p is None:
            raise TypeError("no plugin there")
        return p

    def modules(self):
        studio = self

        def channelCount(globalCount=False, /):
            return len(studio.channels) if globalCount else len(studio.group)

        def getChannelName(index, /, *flag):
            return studio.channel(index, studio.flag(33, flag)).name

        def getChannelType(index, /, *flag):
            return studio.channel(index, studio.flag(33, flag)).kind

        def getTargetFxTrack(index, /, *flag):
            return studio.channel(index, studio.flag(33, flag)).mixer

        def getChannelIndex(index, /):
            return studio.group[index]

        def isValid(index, slotIndex=-1, /, *flag):
            try:
                studio.plugin(index, slotIndex, studio.flag(26, flag))
                return True
            except (TypeError, IndexError):
                return False

        def getPluginName(index, slotIndex=-1, userName=False, /, *flag):
            studio.check("getPluginName")
            p = studio.plugin(index, slotIndex, studio.flag(26, flag))
            return p.label if userName else p.name

        def getParamCount(index, slotIndex=-1, /, *flag):
            return studio.plugin(index, slotIndex, studio.flag(26, flag)).count

        def getParamName(paramIndex, index, slotIndex=-1, /, *flag):
            p = studio.plugin(index, slotIndex, studio.flag(26, flag))
            return p.params.get(paramIndex, ["", 0.0, ""])[0]

        def getParamValue(paramIndex, index, slotIndex=-1, /, *flag):
            p = studio.plugin(index, slotIndex, studio.flag(26, flag))
            return p.params.get(paramIndex, ["", 0.0, ""])[1]

        def getParamValueString(paramIndex, index, slotIndex=-1, pickupMode=0, /, *flag):
            p = studio.plugin(index, slotIndex, studio.flag(26, flag))
            return p.params.get(paramIndex, ["", 0.0, ""])[2]

        def setParamValue(value, paramIndex, index, slotIndex=-1, pickupMode=0, /, *flag):
            p = studio.plugin(index, slotIndex, studio.flag(26, flag))
            p.sets.append((paramIndex, value))
            p.params[paramIndex][1] = value
            p.params[paramIndex][2] = str(round(value * 100)) + "%"

        def trackCount():
            return len(studio.mixer) + 1   # and the "current" track

        def getTrackName(index, /):
            return studio.mixer[index][0] if index < len(studio.mixer) else "Current"

        def isTrackPluginValid(index, plugIndex, /):
            return index < len(studio.mixer) and plugIndex in studio.mixer[index][1]

        def getCurrentTempo(asInt=False, /):
            return studio.tempo

        def setTrackVolume(index, volume, pickupMode=0, /):
            studio.volumes[index] = volume

        def getTrackVolume(index, mode=0, /):
            v = studio.volumes.get(index, 0.8)
            if mode != 1:
                return v
            import math
            return -float("inf") if v == 0 else 20 * math.log10(v / 0.8)   # a stand-in curve, not FL Studio's

        def setTrackName(index, name, /):
            studio.names[index] = name

        def processRECEvent(eventId, value, flags, /):
            if eventId == REC_TEMPO:
                studio.tempo = round((10 + 512 * value / FROM_MIDI_MAX) * 1000)   # a stand-in range

        def globalTransport(command, value, /, *rest):
            if command == FPT_SAVE:
                studio.saves += 1

        def getVersion():
            return studio.api

        def getChangedFlag():
            return studio.modified

        def getProjectTitle():
            return studio.info_title

        def ui_getVersion(mode=4, /):
            return studio.version

        def getProgTitle():
            return studio.title

        def isAssigned():
            return studio.output

        def midiOutSysex(message, /):
            if not studio.output:
                raise TypeError("no output port")
            message = bytes(message)
            assert message[0] == 0xF0 and message[-1] == 0xF7, "FL Studio sends whole SysEx messages"
            studio.sent.append(message)
            if studio.on_send:
                studio.on_send(message)

        found = {}
        for name, functions in {
            "channels": [channelCount, getChannelName, getChannelType, getTargetFxTrack, getChannelIndex],
            "plugins": [isValid, getPluginName, getParamCount, getParamName, getParamValue, getParamValueString,
                        setParamValue],
            "mixer": [trackCount, getTrackName, isTrackPluginValid, getCurrentTempo, setTrackVolume,
                      getTrackVolume, setTrackName],
            "general": [getVersion, getChangedFlag, processRECEvent, getProjectTitle],
            "transport": [globalTransport],
            "ui": [ui_getVersion, getProgTitle],
            "device": [isAssigned, midiOutSysex],
        }.items():
            module = types.ModuleType(name)
            for f in functions:
                setattr(module, f.__name__.replace("ui_", ""), f)
            found[name] = module
        midi = types.ModuleType("midi")
        midi.REC_Tempo, midi.FromMIDI_Max, midi.FPT_Save = REC_TEMPO, FROM_MIDI_MAX, FPT_SAVE
        midi.REC_FromMIDI, midi.REC_UpdateValue, midi.REC_UpdateControl = 1 << 20, 1 << 0, 1 << 1
        found["midi"] = midi
        return found


def compile_script(studio, path=SCRIPT):
    """A fresh copy of the script's module as FL Studio makes it (with no __file__), before OnInit."""
    with open(path, encoding="utf-8") as f:
        source = f.read()
    sys.modules.update(studio.modules())
    script = types.ModuleType("device_MusicOrganiser")
    exec(compile(source, path, "exec"), script.__dict__)
    return script


class SysEx:
    """The event FL Studio hands OnSysEx: the whole message, F0 to F7."""

    def __init__(self, data):
        self.sysex = bytes(data)
        self.handled = False


def load_script(studio, folder):
    """
    Loads the script as FL Studio does: installed in `folder`, which is on the module path while it starts.
    Returns the script's module, after OnInit.
    """
    shutil.copy(SCRIPT, folder)
    sys.path.insert(0, folder)
    try:
        script = compile_script(studio, os.path.join(folder, os.path.basename(SCRIPT)))
        script.OnInit()
    finally:
        sys.path.remove(folder)
    return script


def night_drive(**changes):
    """The song the checks use."""
    serum = Plugin("Serum", [("Master Volume", 0.7, "-3.1 dB"), ("A Octave", 0.5, "0"), ("", 0.0, ""),
                             ("Filter Cutoff", 0.25, "440 Hz")], label="Reese", vst=True)
    song = dict(
        channels=[
            Channel("Kick", kind=0, mixer=1),
            Channel("Bass", mixer=2, plugin=serum),
            Channel("Chops", mixer=3, plugin=Plugin("Slicex", [("Main volume", 0.8, "80%")])),
            Channel("Café pad", mixer=4, plugin=Plugin("FLEX", [("Cutoff", 1.0, "100%")])),
        ],
        mixer=[
            ("Master", {0: Plugin("Fruity Limiter", [("Gain", 0.5, "0.0 dB")])}),
            ("Kick", {}),
            ("Bass", {0: Plugin("Fruity Parametric EQ 2", [("Band 1 level", 0.5, "0.0 dB")]),
                      3: Plugin("Fruity Reeverb 2", [("Mix", 0.3, "30%")], label="Room")}),
        ],
    )
    song.update(changes)
    return Studio(**song)
