// The C# half of the twin test. Reads each file named on its input (one path a line) with the C# app's FL Studio,
// Bitwig, Studio One or MIDI file reader (or with --sounds, its sound analyser; with --classify, its type classifier;
// with --similar, its likeness index), unchanged, and writes what it finds as one JSON a line, in the same shape as
// tools/twin/src/rust_side.rs (sounds.rs for sounds, classify.rs for the classifier, similar.rs for likeness).
// Files are read on every core; the lines come out in whatever order they finish, each naming its file.
// With --roots it lists the library's folders instead (below).

using System.Security.Cryptography;
using System.Text.Json.Nodes;
using MusicOrganiser.Core.Parsers;

// `twin-cs --roots <library.db>`: the library's folders and the MIDI page's, one a line, from the app's database
// opened read-only.
if (args is ["--roots", var db])
{
    using var connection = new Microsoft.Data.Sqlite.SqliteConnection($"Data Source={db};Mode=ReadOnly");
    connection.Open();
    using var command = connection.CreateCommand();
    command.CommandText = "SELECT path FROM roots ORDER BY path";
    using var rows = command.ExecuteReader();
    // UTF-8 whatever the console's code page, so a folder named in any language comes through whole.
    using var roots = new StreamWriter(Console.OpenStandardOutput(), new System.Text.UTF8Encoding(false));
    while (rows.Read()) roots.WriteLine(rows.GetString(0));
    rows.Close();
    // And the MIDI page's folders (a JSON list in the settings), so the MIDI files are read too.
    command.CommandText = "SELECT value FROM settings WHERE key = 'midi_folders'";
    if (command.ExecuteScalar() is string json)
        foreach (var folder in System.Text.Json.JsonSerializer.Deserialize<List<string>>(json) ?? []) roots.WriteLine(folder);
    return;
}

// `twin-cs --songs <copy>`: how the C# app's Projects page groups the library into songs, one save a line
// ({"path", "key", "latest"}). The C# library opens its database to write (it adds tables and columns it's missing),
// so it's given a copy of the library: the Rust side makes it (and reads the same one) and deletes it afterwards.
if (args is ["--songs", var copy])
{
    try
    {
        using var lib = new MusicOrganiser.Core.Library.LibraryDb(copy);
        // As ProjectsViewModel.Load: Fix's copies waiting for Keep are left out.
        var pending = lib.PendingFixes();
        var projects = lib.Projects().Where(p => !pending.Contains(p.Path)).ToList();
        using var songs = new StreamWriter(Console.OpenStandardOutput(), new System.Text.UTF8Encoding(false));
        foreach (var g in MusicOrganiser.Core.Library.SongVersions.Group(projects, lib.SongChoices()))
            foreach (var p in g.Older.Prepend(g.Latest))
                songs.WriteLine(new JsonObject { ["path"] = p.Path, ["key"] = g.Key, ["latest"] = ReferenceEquals(p, g.Latest) }.ToJsonString());
    }
    finally
    {
        // Lets go of the copy so the Rust side can delete it.
        Microsoft.Data.Sqlite.SqliteConnection.ClearAllPools();
    }
    return;
}

// The Rust side writes the paths as UTF-8; read them so, not in the console's code page.
var files = new List<string>();
using (var input = new StreamReader(Console.OpenStandardInput(), new System.Text.UTF8Encoding(false)))
    for (string? line; (line = input.ReadLine()) != null;)
        if (line.Length > 0) files.Add(line);

// `twin-cs --sounds`: each file is a sound, analysed by the C# Sound Lab's analyser instead of read as a project.
var sounds = args is ["--sounds"];
// `twin-cs --classify`: each file is a classifier case (examples, options, probes) made by tools/twin/src/classify.rs.
var classify = args is ["--classify"];
// `twin-cs --similar`: each file is a likeness case (vectors and probes) made by tools/twin/src/similar.rs.
var similar = args is ["--similar"];
var output = Console.OpenStandardOutput();
var gate = new object();
Parallel.ForEach(files, file =>
{
    string json;
    // One odd value (JSON has no NaN) loses that file's reading, not the whole run.
    try { json = (sounds ? DumpSound(file) : classify ? DumpClassify(file) : similar ? DumpSimilar(file) : Dump(file)).ToJsonString(); }
    catch (Exception e) { json = new JsonObject { ["file"] = file, ["error_more"] = $"couldn't write it: {e.Message}" }.ToJsonString(); }
    var bytes = System.Text.Encoding.UTF8.GetBytes(json + "\n");
    lock (gate) output.Write(bytes);
});
output.Flush();

// A sound's analysis, as the C# app stores it in sounds.db (the analysis JSON, which holds the features too).
static JsonObject DumpSound(string path)
{
    var o = new JsonObject { ["file"] = path };
    try { o["analysis"] = System.Text.Json.JsonSerializer.SerializeToNode(MusicOrganiser.Core.Sounds.SoundAnalyzer.Analyse(path)); }
    catch (Exception e) { o["error"] = $"{e.GetType().Name}: {e.Message}"; }
    return o;
}

// The C# app's type classifier trained on one case: the model as sounds.db would hold it (text), and what it says of
// each probe, then what the model Rust trained says of them once C# has read it. A case it won't train on gives the
// reason.
static JsonObject DumpClassify(string path)
{
    var o = new JsonObject { ["file"] = path };
    try
    {
        var c = JsonNode.Parse(File.ReadAllText(path))!;
        var opt = c["Options"]!;
        var options = new MusicOrganiser.Core.Sounds.SoundClassifier.Options(
            opt["Epochs"]!.GetValue<int>(), opt["LearningRate"]!.GetValue<double>(), opt["L2"]!.GetValue<double>(),
            opt["MaxPerClass"]!.GetValue<int>(), opt["Seed"]!.GetValue<int>());
        static float[] Vector(JsonNode? v) => v!.AsArray().Select(n => n!.GetValue<float>()).ToArray();
        var examples = c["Examples"]!.AsArray()
            .Select(e => new MusicOrganiser.Core.Sounds.TrainingExample(Vector(e!["X"]), e["Label"]!.GetValue<string>(), e["Weight"]!.GetValue<float>()))
            .ToList();
        MusicOrganiser.Core.Sounds.SoundClassifier model;
        try { model = MusicOrganiser.Core.Sounds.SoundClassifier.Train(examples, c["Space"]!.GetValue<string>(), options); }
        catch (InvalidOperationException e) { o["refused"] = e.Message; return o; }
        o["model"] = model.ToJson();
        var predictions = new JsonArray();
        foreach (var p in c["Probes"]!.AsArray())
            predictions.Add(new JsonArray(model.Predict(Vector(p))
                .Select(t => (JsonNode)new JsonObject { ["Label"] = t.Label, ["P"] = t.P }).ToArray()));
        o["predictions"] = predictions;
        // The model the Rust app trained, read back as the C# app reads it from sounds.db.
        if (MusicOrganiser.Core.Sounds.SoundClassifier.FromJson(c["RustModel"]?.GetValue<string>()) is { } rust)
        {
            var fromRust = new JsonArray();
            foreach (var p in c["Probes"]!.AsArray())
                fromRust.Add(new JsonArray(rust.Predict(Vector(p))
                    .Select(t => (JsonNode)new JsonObject { ["Label"] = t.Label, ["P"] = t.P }).ToArray()));
            o["rust_model_predictions"] = fromRust;
        }
    }
    catch (Exception e) { o["error"] = $"{e.GetType().Name}: {e.Message}"; }
    return o;
}

// The C# app's likeness index built from one case's vectors (standardised with chroma counting less, as
// SoundOrganiser builds the features index, or as they are, as for CLAP), and each probe's nearest sounds.
static JsonObject DumpSimilar(string path)
{
    var o = new JsonObject { ["file"] = path };
    try
    {
        var c = JsonNode.Parse(File.ReadAllText(path))!;
        var vectors = new Dictionary<long, float[]>();
        foreach (var e in c["Vectors"]!.AsArray())
            vectors[e![0]!.GetValue<long>()] = e[1]!.AsArray().Select(n => n!.GetValue<float>()).ToArray();
        var features = c["Features"]!.GetValue<bool>();
        // The app's own chroma weights (private to SoundOrganiser), so a change there shows up here.
        var chroma = typeof(MusicOrganiser.Core.Sounds.SoundOrganiser).GetMethod("ChromaDownweight",
            System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Static)
            ?? throw new InvalidOperationException("SoundOrganiser.ChromaDownweight is gone");
        var weights = chroma.CreateDelegate<Func<int, float[]>>();
        var index = MusicOrganiser.Core.Sounds.VectorIndex.Build(vectors, standardise: features, features ? weights : null);
        var max = c["Max"]!.GetValue<int>();
        var nearest = new JsonArray();
        foreach (var q in c["Queries"]!.AsArray())
        {
            var v = index.Get(q!.GetValue<long>());
            IReadOnlyList<(long Id, double Score)> found = v is null ? [] : index.Nearest(v, max);
            nearest.Add(new JsonArray(found
                .Select(t => (JsonNode)new JsonObject { ["Id"] = t.Id, ["Score"] = t.Score }).ToArray()));
        }
        o["nearest"] = nearest;
    }
    catch (Exception e) { o["error"] = $"{e.GetType().Name}: {e.Message}"; }
    return o;
}

static JsonObject Dump(string path) => Path.GetExtension(path).ToLowerInvariant() switch
{
    ".bwproject" => DumpBitwig(path),
    ".song" => DumpStudioOne(path),
    _ when MusicOrganiser.Core.Midi.MidiFileReader.IsMidiPath(path) => DumpMidi(path),
    _ => DumpFl(path),
};

// A MIDI file's parts. A file that isn't one says just "unreadable" in both apps.
static JsonObject DumpMidi(string path)
{
    var o = new JsonObject { ["file"] = path };
    try
    {
        o["parts"] = new JsonArray(MusicOrganiser.Core.Midi.MidiFileReader.Read(path).Select(p => (JsonNode?)new JsonObject
        {
            ["name"] = p.Name, ["ppq"] = p.Ppq, ["notes"] = Notes(p.Notes), ["drums"] = p.Drums, ["tempo"] = p.Tempo,
            ["program"] = p.Program, ["track_name"] = p.TrackName,
        }).ToArray());
    }
    catch (InvalidDataException) { o["error"] = "unreadable"; }
    catch (Exception e) { o["error"] = $"{e.GetType().Name}: {e.Message}"; }
    return o;
}

// What the project list shows, the note clips and the plugin states. A file that isn't a zip can't be read; the two
// apps word that differently, so both say just "unreadable".
static JsonObject DumpStudioOne(string path)
{
    var reader = new StudioOneReader();
    var o = new JsonObject { ["file"] = path };
    try { o["info"] = Info(reader.Read(path)); }
    catch (InvalidDataException) { o["info"] = new JsonObject { ["error"] = "unreadable" }; }
    catch (Exception e) { o["info"] = new JsonObject { ["error"] = e.Message }; }
    try
    {
        o["midi"] = new JsonArray(reader.ExtractMidi(path).Select(Clip).ToArray<JsonNode?>());
        o["presets"] = new JsonArray(reader.ExtractPresets(path).Select(Preset).ToArray<JsonNode?>());
    }
    catch (InvalidDataException) { o["error_more"] = "unreadable"; }
    catch (Exception e) { o["error_more"] = $"{e.GetType().Name}: {e.Message}"; }
    return o;
}

// What the project list shows, the note clips and the plugin states.
static JsonObject DumpBitwig(string path)
{
    var reader = new BitwigReader();
    var o = new JsonObject { ["file"] = path };
    try { o["info"] = Info(reader.Read(path)); }
    catch (Exception e) { o["info"] = new JsonObject { ["error"] = e.Message }; }
    try
    {
        o["midi"] = new JsonArray(reader.ExtractMidi(path).Select(Clip).ToArray<JsonNode?>());
        o["presets"] = new JsonArray(reader.ExtractPresets(path).Select(Preset).ToArray<JsonNode?>());
    }
    catch (Exception e) { o["error_more"] = $"{e.GetType().Name}: {e.Message}"; }
    return o;
}

static JsonObject DumpFl(string path)
{
    var reader = new FlStudioReader();
    var o = new JsonObject { ["file"] = path };
    try { o["info"] = Info(reader.Read(path)); }
    catch (Exception e) { o["info"] = new JsonObject { ["error"] = e.Message }; }
    try
    {
        // In the order the Rust side fills them, so the JSON reads the same.
        var channels = reader.ExtractChannels(path);
        var song = reader.ReadSong(path);
        o["midi"] = new JsonArray(reader.ExtractMidi(path).Select(Clip).ToArray<JsonNode?>());
        o["channels"] = new JsonArray(channels.Select(Channel).ToArray<JsonNode?>());
        o["presets"] = new JsonArray(reader.ExtractPresets(path).Select(Preset).ToArray<JsonNode?>());
        o["song"] = Song(song);
    }
    catch (InvalidDataException) { o["error_more"] = "damaged"; }
    catch (Exception e) { o["error_more"] = $"{e.GetType().Name}: {e.Message}"; }
    return o;
}

static JsonArray Strings(IEnumerable<string> s) => new(s.Select(x => (JsonNode?)x).ToArray());
static JsonNode? Int(int v) => v < 0 ? null : JsonValue.Create(v);

static JsonObject Info(ProjectInfo i)
{
    var details = new JsonObject();
    foreach (var (k, v) in i.Details.OrderBy(d => d.Key, StringComparer.Ordinal)) details[k] = v;
    var unplayed = new JsonObject();
    foreach (var (k, v) in i.Unplayed.OrderBy(d => d.Key, StringComparer.Ordinal)) unplayed[k] = v.ToString();
    return new JsonObject
    {
        ["daw"] = i.Daw,
        ["confidence"] = i.Confidence.ToString(),
        ["tempo"] = i.Tempo,
        ["version"] = i.Version,
        ["title"] = i.Title,
        ["plugins"] = new JsonArray(i.Plugins.Select(p => (JsonNode?)new JsonObject
        {
            ["name"] = p.Name, ["vendor"] = p.Vendor, ["kind"] = p.Kind.ToString(), ["format"] = p.Format,
            ["path"] = p.Path, ["id"] = p.Id,
        }).ToArray()),
        ["samples"] = Strings(i.Samples),
        ["tracks"] = Strings(i.Tracks),
        ["hardware"] = Strings(i.Hardware),
        ["details"] = details,
        ["created"] = i.Created?.ToString("yyyy-MM-ddTHH:mm:ss.fff"),
        ["warnings"] = Strings(i.Warnings),
        ["can_extract_midi"] = i.CanExtractMidi,
        ["can_extract_presets"] = i.CanExtractPresets,
        ["unplayed"] = unplayed,
    };
}

static JsonNode Note(NoteEvent n) => new JsonArray(n.Start, n.Length, n.Key, n.Velocity, n.Muted, n.Step);
static JsonArray Notes(IEnumerable<NoteEvent> notes) => new(notes.Select(Note).ToArray<JsonNode?>());

static JsonNode Clip(MidiClip c) => new JsonObject
{
    ["name"] = c.Name,
    ["ppq"] = c.Ppq,
    ["end"] = c.End,
    ["tracks"] = new JsonArray(c.Tracks.Select(t => (JsonNode?)new JsonObject { ["name"] = t.Key, ["notes"] = Notes(t.Value) }).ToArray()),
};

static JsonNode Channel(MidiChannel c) => new JsonObject
{
    ["name"] = c.Name, ["source"] = c.Source, ["ppq"] = c.Ppq, ["plugin"] = c.Plugin, ["sample_path"] = c.SamplePath,
    ["root_key"] = c.RootKey, ["pitch_cents"] = c.PitchCents, ["channel"] = c.Channel, ["notes"] = Notes(c.Notes),
};

static string Sha256(byte[] data) => Convert.ToHexStringLower(SHA256.HashData(data));

static JsonNode Preset(PresetBlob p) => new JsonObject
{
    ["name"] = p.Name, ["plugin"] = p.Plugin, ["extension"] = p.Extension, ["kind"] = p.Kind.ToString(),
    ["native"] = p.Native, ["place"] = p.Where, ["channel"] = p.Channel, ["program"] = p.Program,
    ["size"] = p.Data.Length, ["sha256"] = Sha256(p.Data),
};

static JsonNode Plugin(FlPlugin p) => new JsonObject
{
    ["name"] = p.Name, ["vendor"] = p.Vendor, ["format"] = p.Format, ["device_id"] = p.DeviceId, ["path"] = p.Path,
    ["nothing_saved"] = p.NothingSaved, ["slot"] = Int(p.Slot), ["enabled"] = p.Enabled,
    ["state"] = p.State is { } s ? new JsonObject { ["extension"] = s.Extension, ["size"] = s.Data.Length, ["sha256"] = Sha256(s.Data) } : null,
    ["parameter_ids"] = p.ParameterIds is { } ids ? new JsonArray(ids.Select(i => (JsonNode?)i).ToArray()) : null,
};

static JsonArray Points(IEnumerable<FlPoint> points) => new(points.Select(p => (JsonNode?)new JsonArray(p.Beat, p.Value)).ToArray());

static JsonNode SongChannel(FlChannel c) => new JsonObject
{
    ["index"] = c.Index, ["name"] = c.Name, ["instrument"] = c.Instrument is { } i ? Plugin(i) : null, ["sample"] = c.Sample,
    ["insert"] = c.Insert, ["notes"] = Notes(c.Notes), ["root_key"] = c.RootKey, ["pitch_cents"] = c.PitchCents,
    ["volume"] = c.Volume, ["pan"] = c.Pan,
    ["clips"] = new JsonArray(c.Clips.Select(k => (JsonNode?)new JsonObject
    {
        ["name"] = k.Name, ["start"] = k.Start, ["length"] = k.Length, ["notes"] = Notes(k.Notes), ["muted"] = k.Muted,
        ["track"] = k.Track, ["on_muted_track"] = k.OnMutedTrack,
    }).ToArray()),
    ["volume_envelope"] = c.VolumeEnvelope is { } e ? new JsonObject
    {
        ["delay"] = e.Delay, ["attack"] = e.Attack, ["hold"] = e.Hold, ["decay"] = e.Decay, ["sustain"] = e.Sustain,
        ["release"] = e.Release,
    } : null,
    ["audio_clips"] = new JsonArray(c.AudioClips.Select(a => (JsonNode?)new JsonObject
    {
        ["start"] = a.Start, ["length"] = a.Length, ["from"] = a.From, ["muted"] = a.Muted, ["track"] = a.Track,
        ["on_muted_track"] = a.OnMutedTrack, ["partly_off"] = a.PartlyOff,
    }).ToArray()),
    ["stretch"] = new JsonObject
    {
        ["multiplier"] = c.Stretch.Multiplier, ["pitch_cents"] = c.Stretch.PitchCents, ["mode"] = c.Stretch.Mode,
        ["time"] = c.Stretch.Time, ["reversed"] = c.Stretch.Reversed,
    },
    ["enabled"] = c.Enabled,
    ["switched_by"] = Strings(c.SwitchedBy),
};

static JsonNode Insert(FlInsert i) => new JsonObject
{
    ["index"] = i.Index, ["name"] = i.Name, ["effects"] = new JsonArray(i.Effects.Select(Plugin).ToArray<JsonNode?>()),
    ["fader"] = i.Fader, ["pan"] = i.Pan,
};

static JsonNode Automation(FlAutomation a) => new JsonObject
{
    ["name"] = a.Name, ["channel"] = Int(a.Channel), ["control"] = a.Control.ToString(), ["parameter"] = a.Parameter,
    ["points"] = Points(a.Points), ["insert"] = Int(a.Insert), ["slot"] = Int(a.Slot),
    ["starts"] = new JsonArray(a.Starts.Select(s => (JsonNode?)s).ToArray()),
};

static JsonNode Song(FlSong s)
{
    var curves = new JsonObject();
    foreach (var (c, p) in s.Curves.OrderBy(c => c.Key)) curves[c.ToString()] = Points(p);
    return new JsonObject
    {
        ["title"] = s.Title, ["ppq"] = s.Ppq, ["tempo"] = s.Tempo,
        ["channels"] = new JsonArray(s.Channels.Select(SongChannel).ToArray<JsonNode?>()),
        ["inserts"] = new JsonArray(s.Inserts.Select(Insert).ToArray<JsonNode?>()),
        ["automation"] = new JsonArray(s.Automation.Select(Automation).ToArray<JsonNode?>()),
        ["muted_tracks"] = new JsonArray(s.MutedTracks.Select(t => (JsonNode?)new JsonObject { ["number"] = t.Number, ["name"] = t.Name }).ToArray()),
        ["curves"] = curves,
    };
}
