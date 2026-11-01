// The C# half of the theory bench: the C# app's chord reader (ChordReader.Read, as the MIDI page, the Playground and
// the index use it), run unchanged on each piece of a bench file, so the Rust bench can score it beside the new engine
// on exactly the same notes.
//
//   theorybench-cs chords <labelled.jsonl> [--part <role>] [--oracle-cuts]
//
// Writes one JSON object a line: {"source", "id", "chords": [[start, end, name, root, bass, pitch classes, others], ...]},
// the times in the piece's own ticks and the pitch classes as twelve bits (bit 0 for C). "others" are the other names
// the Playground offers for the same notes (ChordSpeller.Readings on the other roots, at most four, likeliest first),
// each [name, root, bass, pitch classes]: the C# app's pick list, for the bench's first-two and first-three scores. It
// hears the parts with the given role, or every part but drums.
//
// --oracle-cuts reads each labelled chord's stretch on its own (the notes sounding in it, cut to it) and keeps the
// chord read there for longest, on the label's own start and end: how the C# app names chords when the changes are
// given, apart from where it would change chord itself.

using System.Text.Json;
using System.Text.Json.Nodes;
using MusicOrganiser.Core.Midi;
using MusicOrganiser.Core.Midi.Jazz;
using MusicOrganiser.Core.Parsers;

const string Usage = "theorybench-cs chords <labelled.jsonl> [--part <role>] [--oracle-cuts]";
if (args is not ["chords", var path, ..])
{
    Console.Error.WriteLine(Usage);
    return 2;
}
string? part = null;
var oracle = false;
for (var a = 2; a < args.Length; a++)
{
    if (args[a] == "--part" && a + 1 < args.Length) part = args[++a];
    else if (args[a] == "--oracle-cuts") oracle = true;
    else
    {
        Console.Error.WriteLine(Usage);
        return 2;
    }
}
var lines = File.ReadAllLines(path).Where(l => l.Trim().Length > 0).ToList();
var results = new string[lines.Count];
Parallel.For(0, lines.Count, i => results[i] = Read(JsonNode.Parse(lines[i])!, part, oracle));
using var output = new StreamWriter(Console.OpenStandardOutput(), new System.Text.UTF8Encoding(false));
foreach (var r in results) output.WriteLine(r);
return 0;

static string Read(JsonNode item, string? part, bool oracle)
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
    var chords = new JsonArray();
    if (!oracle)
    {
        foreach (var c in ChordReader.Read(notes, (long)Math.Round(length * scale), null, downbeat))
            chords.Add(Row((long)Math.Round(c.Start / scale), (long)Math.Round((c.Start + c.Length) / scale), c.Chord,
                notes, c.Start, c.Start + c.Length));
    }
    else
    {
        foreach (var l in item["chords"]!.AsArray())
        {
            var label = l!.AsArray();
            if ((string?)label[2] is "N" or "X") continue;
            long start = (long)label[0]!, end = (long)label[1]!;
            long from = (long)Math.Round(start * scale), to = (long)Math.Round(end * scale);
            if (to <= from) continue;
            var inside = notes.Where(n => n.Start < to && n.Start + n.Length > from)
                .Select(n => n with { Start = Math.Max(n.Start, from) - from, Length = Math.Min(n.Start + n.Length, to) - Math.Max(n.Start, from) })
                .OrderBy(n => n.Start).ThenBy(n => n.Key).ToList();
            var longest = ChordReader.Read(inside, to - from, null, downbeat - from)
                .GroupBy(c => c.Chord).Select(g => (Chord: g.Key, Time: g.Sum(c => c.Length)))
                .OrderByDescending(g => g.Time).ThenBy(g => g.Chord.Name, StringComparer.Ordinal).FirstOrDefault();
            if (longest.Time > 0) chords.Add(Row(start, end, longest.Chord, inside, 0, to - from));
        }
    }
    return new JsonObject { ["source"] = (string?)item["source"], ["id"] = (string?)item["id"], ["chords"] = chords }
        .ToJsonString(new JsonSerializerOptions { WriteIndented = false });
}

// One chord read, with the other names the Playground offers for its notes: the lowest of its notes sounding from
// `from` to `to` leans their order when there's no bass under another root (PlaygroundViewModel.RefreshChordIdeas).
static JsonArray Row(long start, long end, ChordSymbol chord, IReadOnlyList<NoteEvent> notes, long from, long to)
{
    var lowest = notes.Where(n => n.Start < to && n.Start + n.Length > from && (chord.Mask >> n.Key % 12 & 1) != 0)
        .Select(n => n.Key).DefaultIfEmpty(-1).Min();
    var others = new JsonArray();
    foreach (var r in ChordSpeller.Readings(chord.Mask, chord.Bass != chord.Root ? chord.Bass : -1, lowest >= 0 ? lowest % 12 : chord.Bass)
                 .Where(r => r.Root != chord.Root).Take(4))
        others.Add(new JsonArray(r.Name, r.Root, r.Bass, r.Mask));
    return new JsonArray(start, end, chord.Name, chord.Root, chord.Bass, chord.Mask, others);
}
