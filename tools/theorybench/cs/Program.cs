// The C# half of the theory bench: the C# app's chord reader (ChordReader.Read, as the MIDI page, the Playground and
// the index use it), run unchanged on each piece of a bench file, so the Rust bench can score it beside the new engine
// on exactly the same notes.
//
//   theorybench-cs chords <labelled.jsonl> [--part <role>]
//
// Writes one JSON object a line: {"source", "id", "chords": [[start, end, name, root, bass, pitch classes], ...]}, the
// times in the piece's own ticks and the pitch classes as twelve bits (bit 0 for C). It hears the parts with the given
// role, or every part but drums.

using System.Text.Json;
using System.Text.Json.Nodes;
using MusicOrganiser.Core.Midi;
using MusicOrganiser.Core.Midi.Jazz;
using MusicOrganiser.Core.Parsers;

if (args is not ["chords", var path, ..])
{
    Console.Error.WriteLine("theorybench-cs chords <labelled.jsonl> [--part <role>]");
    return 2;
}
var part = args.Length >= 4 && args[2] == "--part" ? args[3] : null;
var lines = File.ReadAllLines(path).Where(l => l.Trim().Length > 0).ToList();
var results = new string[lines.Count];
Parallel.For(0, lines.Count, i => results[i] = Read(JsonNode.Parse(lines[i])!, part));
using var output = new StreamWriter(Console.OpenStandardOutput(), new System.Text.UTF8Encoding(false));
foreach (var r in results) output.WriteLine(r);
return 0;

static string Read(JsonNode item, string? part)
{
    var ppq = Math.Max(1, (int)item["ppq"]!);
    var firstDown = (long?)item["first_downbeat"] ?? 0;
    var length = (long)item["length"]!;
    // To the C# app's 96 ticks a beat, as PhraseAnalyser.Normalise does (keeping each note's place).
    var scale = (double)PhraseAnalyser.Ppq / ppq;
    var notes = new List<NoteEvent>();
    foreach (var p in item["parts"]!.AsArray())
    {
        var role = (string?)p!["role"] ?? "";
        if (part is null ? role == "drums" : role != part) continue;
        foreach (var n in p["notes"]!.AsArray())
        {
            var a = n!.AsArray();
            notes.Add(new NoteEvent((long)Math.Round((long)a[0]! * scale),
                Math.Clamp((long)Math.Round((long)a[1]! * scale), 1, PhraseAnalyser.MaxHold), (int)a[2]!, (int)a[3]!));
        }
    }
    notes = notes.OrderBy(n => n.Start).ThenBy(n => n.Key).ToList();
    // The bar lines as the labels give them: the C# reader counts a bar as four beats, so it's told where one starts.
    var downbeat = (long)Math.Round(firstDown * scale);
    var lane = ChordReader.Read(notes, (long)Math.Round(length * scale), null, downbeat);
    var chords = new JsonArray();
    foreach (var c in lane)
        chords.Add(new JsonArray((long)Math.Round(c.Start / scale), (long)Math.Round((c.Start + c.Length) / scale),
            c.Chord.Name, c.Chord.Root, c.Chord.Bass, c.Chord.Mask));
    return new JsonObject { ["source"] = (string?)item["source"], ["id"] = (string?)item["id"], ["chords"] = chords }
        .ToJsonString(new JsonSerializerOptions { WriteIndented = false });
}
