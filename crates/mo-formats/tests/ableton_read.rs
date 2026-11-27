//! Reading Ableton Live sets, on made-up sets laid out as Live 11 and 12 save them (none of Solomon's are in the repo).

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod common;

use std::io::Write;

use common::test_dir;
use mo_formats::ableton::{self, LiveSet};
use mo_formats::arrangement::{LaneKind, Stretch, TimeSignature};
use mo_formats::{PluginKind, velocity};

/// A made-up Live 12 set: a group holding a MIDI track (Serum, then EQ Eight) with one looping arrangement clip and
/// one session clip, an audio track with a warped clip, a return track, two locators, 6/8 at 128 BPM.
fn set_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="5" MinorVersion="12.0_12049" SchemaChangeCount="7" Creator="Ableton Live 12.1.5" Revision="x">
  <LiveSet>
    <Tracks>
      <GroupTrack Id="10">
        <Name><EffectiveName Value="2-Group" /><UserName Value="Music" /></Name>
        <TrackGroupId Value="-1" />
        <DeviceChain><Mixer><Speaker><Manual Value="true" /></Speaker></Mixer></DeviceChain>
      </GroupTrack>
      <MidiTrack Id="11">
        <Name><EffectiveName Value="1-Serum" /><UserName Value="" /></Name>
        <TrackGroupId Value="10" />
        <DeviceChain>
          <Mixer><Speaker><Manual Value="true" /></Speaker></Mixer>
          <MainSequencer>
            <ClipSlotList>
              <ClipSlot Id="0"><ClipSlot><Value /></ClipSlot></ClipSlot>
              <ClipSlot Id="1"><ClipSlot><Value>
                <MidiClip Id="5" Time="0">
                  <CurrentStart Value="0" /><CurrentEnd Value="8" />
                  <Loop><LoopStart Value="0" /><LoopEnd Value="8" /><StartRelative Value="0" /><LoopOn Value="true" /></Loop>
                  <Name Value="Session Hook" /><Disabled Value="false" />
                  <Notes><KeyTracks>
                    <KeyTrack Id="0"><Notes><MidiNoteEvent Time="0" Duration="1" Velocity="127" IsEnabled="true" /></Notes><MidiKey Value="72" /></KeyTrack>
                  </KeyTracks></Notes>
                </MidiClip>
              </Value></ClipSlot></ClipSlot>
            </ClipSlotList>
            <ClipTimeable><ArrangerAutomation><Events>
              <MidiClip Id="1" Time="16">
                <CurrentStart Value="16" /><CurrentEnd Value="26" />
                <Loop><LoopStart Value="0" /><LoopEnd Value="4" /><StartRelative Value="1" /><LoopOn Value="true" /></Loop>
                <Name Value="Hook" /><Disabled Value="false" />
                <Notes><KeyTracks>
                  <KeyTrack Id="0"><Notes>
                    <MidiNoteEvent Time="0" Duration="0.5" Velocity="100" IsEnabled="true" NoteId="1" />
                    <MidiNoteEvent Time="2" Duration="1" Velocity="63.5" IsEnabled="false" NoteId="2" />
                  </Notes><MidiKey Value="60" /></KeyTrack>
                  <KeyTrack Id="1"><Notes>
                    <MidiNoteEvent Time="1.5" Duration="0.25" Velocity="80" IsEnabled="true" NoteId="3" />
                  </Notes><MidiKey Value="64" /></KeyTrack>
                </KeyTracks></Notes>
              </MidiClip>
            </Events></ArrangerAutomation></ClipTimeable>
          </MainSequencer>
          <FreezeSequencer><AudioClip Id="9" Time="0"><SampleRef><FileRef><Path Value="C:/Project/Samples/Freeze/Freeze Serum.wav" /></FileRef></SampleRef></AudioClip></FreezeSequencer>
          <DeviceChain><Devices>
            <PluginDevice Id="0"><PluginDesc><VstPluginInfo Id="0">
              <Path Value="C:/VstPlugins/Serum_x64.dll" /><PlugName Value="Serum_x64" /><UniqueId Value="1483109208" /><Category Value="2" />
            </VstPluginInfo></PluginDesc></PluginDevice>
            <Eq8 Id="1" />
          </Devices></DeviceChain>
        </DeviceChain>
      </MidiTrack>
      <AudioTrack Id="12">
        <Name><EffectiveName Value="3-Drums" /><UserName Value="Drums" /></Name>
        <TrackGroupId Value="-1" />
        <DeviceChain>
          <Mixer><Speaker><Manual Value="false" /></Speaker></Mixer>
          <MainSequencer><Sample><ArrangerAutomation><Events>
            <AudioClip Id="2" Time="4">
              <CurrentStart Value="4" /><CurrentEnd Value="12" />
              <Loop><LoopStart Value="0" /><LoopEnd Value="8" /><StartRelative Value="0" /><LoopOn Value="false" /></Loop>
              <Name Value="Break" /><Disabled Value="true" />
              <SampleRef><FileRef><RelativePathType Value="3" /><RelativePath Value="Samples/Imported/Break.wav" /><Path Value="C:/Samples/Break.wav" /></FileRef></SampleRef>
              <Fades><FadeInLength Value="0.5" /><FadeOutLength Value="1" /></Fades>
              <PitchCoarse Value="2" /><PitchFine Value="50" /><SampleVolume Value="0.5" />
              <WarpMode Value="6" /><IsWarped Value="true" />
              <WarpMarkers><WarpMarker Id="0" SecTime="0" BeatTime="0" /><WarpMarker Id="1" SecTime="4" BeatTime="8" /></WarpMarkers>
            </AudioClip>
          </Events></ArrangerAutomation></Sample></MainSequencer>
          <DeviceChain><Devices>
            <PluginDevice Id="0"><PluginDesc><Vst3PluginInfo Id="0">
              <Name Value="Pro-Q 3" />
              <Uid><Fields.0 Value="1920098929" /><Fields.1 Value="-1" /><Fields.2 Value="0" /><Fields.3 Value="1" /></Uid>
            </Vst3PluginInfo></PluginDesc></PluginDevice>
            <OriginalSimpler Id="1"><Player><MultiSampleMap><SampleParts><MultiSamplePart><SampleRef><FileRef>
              <RelativePath><RelativePathElement Dir="Samples" /></RelativePath><Name Value="Kick.aif" />
              <SearchHint><PathHint><RelativePathElement Dir="C:" /><RelativePathElement Dir="Drums" /></PathHint></SearchHint>
            </FileRef></SampleRef></MultiSamplePart></SampleParts></MultiSampleMap></Player></OriginalSimpler>
          </Devices></DeviceChain>
        </DeviceChain>
      </AudioTrack>
      <ReturnTrack Id="13">
        <Name><EffectiveName Value="A-Reverb" /><UserName Value="" /></Name>
        <TrackGroupId Value="-1" />
        <DeviceChain><DeviceChain><Devices><Reverb Id="0" /></Devices></DeviceChain></DeviceChain>
      </ReturnTrack>
    </Tracks>
    <MainTrack>
      <Name><EffectiveName Value="Main" /><UserName Value="" /></Name>
      <DeviceChain>
        <Mixer>
          <Tempo><Manual Value="128" /></Tempo>
          <TimeSignature><Manual Value="302" /></TimeSignature>
        </Mixer>
        <DeviceChain><Devices><GlueCompressor Id="0" /></Devices></DeviceChain>
      </DeviceChain>
    </MainTrack>
    <Locators><Locators>
      <Locator Id="1"><Time Value="32" /><Name Value="Drop" /></Locator>
      <Locator Id="0"><Time Value="0" /><Name Value="Intro" /></Locator>
    </Locators></Locators>
    <Scenes><Scene Id="0"><Name Value="" /></Scene><Scene Id="1"><Name Value="Hook" /></Scene></Scenes>
  </LiveSet>
</Ableton>
"#
    .to_owned()
}

fn gzipped(text: &str) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(text.as_bytes()).unwrap();
    gz.finish().unwrap()
}

#[test]
fn reads_tracks_plugins_samples_and_tempo() {
    let dir = test_dir("als-read");
    let path = dir.join("Night Drive.als");
    std::fs::write(&path, gzipped(&set_xml())).unwrap();
    let info = ableton::read(&path).expect("read");
    assert_eq!(info.daw, "Ableton Live");
    assert_eq!(info.version, "Ableton Live 12.1.5");
    assert_eq!(info.tempo, Some(128.0));
    assert_eq!(info.tracks, ["Music", "1-Serum", "Drums", "A-Reverb"]);
    let plugins: Vec<(&str, PluginKind, &str, &str)> =
        info.plugins.iter().map(|p| (p.name.as_str(), p.kind, p.format.as_str(), p.id.as_str())).collect();
    assert_eq!(
        plugins,
        [
            ("Serum_x64", PluginKind::Instrument, "VST2", "XfsX"),
            ("EQ Eight", PluginKind::Effect, "Native", ""),
            ("Pro-Q 3", PluginKind::Effect, "VST3", "72726271FFFFFFFF0000000000000001"),
            ("Simpler", PluginKind::Instrument, "Native", ""),
            ("Reverb", PluginKind::Effect, "Native", ""),
            ("Glue Compressor", PluginKind::Effect, "Native", ""),
        ]
    );
    assert_eq!(info.plugins[0].path, r"C:\VstPlugins\Serum_x64.dll");
    // The frozen track's audio is Live's own, so it isn't listed.
    assert_eq!(info.samples, [r"C:\Samples\Break.wav", r"C:\Drums\Kick.aif"]);
    assert_eq!(info.details.get("Time signature").map(String::as_str), Some("6/8"));
    assert_eq!(info.details.get("Clips").map(String::as_str), Some("3"));
    assert_eq!(info.details.get("Notes").map(String::as_str), Some("4"));
    assert_eq!(info.details.get("Scenes").map(String::as_str), Some("2"));
}

#[test]
fn a_plain_xml_set_reads_too_and_other_files_dont() {
    let set = LiveSet::open(set_xml().as_bytes()).expect("plain XML");
    assert_eq!(set.info().tempo, Some(128.0));
    assert!(LiveSet::open(b"PK\x03\x04 not a set").is_err());
    assert!(LiveSet::open(&gzipped("<?xml version=\"1.0\"?><Bitwig/>")).is_err());
    // XML that won't parse is listed with a warning, not dropped.
    let broken = LiveSet::open(b"<?xml version=\"1.0\"?><Ableton><LiveSet>").unwrap().info();
    assert_eq!(broken.warnings.len(), 1);
}

#[test]
fn midi_clips_come_out_by_track_from_where_they_start_playing() {
    let set = LiveSet::open(&gzipped(&set_xml())).unwrap();
    let clips = set.midi();
    assert_eq!(clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Hook", "Session Hook"]);
    let hook = &clips[0];
    assert_eq!(hook.ppq, 960);
    assert_eq!(hook.tracks[0].0, "1-Serum");
    // Played from beat 1 of the clip's own (the start marker): the note at 0 is before it.
    let notes: Vec<(i64, i64, u8, bool)> =
        hook.tracks[0].1.iter().map(|n| (n.start, n.length, n.key, n.muted)).collect();
    assert_eq!(notes, [(480, 240, 64, false), (960, 960, 60, true)]);
    assert_eq!(hook.tracks[0].1[0].velocity, velocity::from_unit(80.0 / 127.0));
    assert_eq!(hook.end, Some(2880));
}

#[test]
fn the_arrangement_unrolls_loops_and_times_audio_by_its_warp_markers() {
    let set = LiveSet::open(&gzipped(&set_xml())).unwrap();
    let views = set.arrangements();
    assert_eq!(views.len(), 1, "the arrangement has clips, so the session isn't shown");
    let view = &views[0];
    assert_eq!(view.view, "Ableton arrangement");
    assert_eq!((view.tempo, view.signature), (128.0, TimeSignature::new(6, 8).unwrap()));
    assert_eq!(
        view.markers.iter().map(|m| (m.beat, m.name.as_str())).collect::<Vec<_>>(),
        [(0.0, "Intro"), (32.0, "Drop")]
    );
    let lanes: Vec<(&str, LaneKind, u32, bool)> =
        view.lanes.iter().map(|l| (l.name.as_str(), l.kind, l.depth, l.muted)).collect();
    assert_eq!(
        lanes,
        [
            ("Music", LaneKind::Group, 0, false),
            ("1-Serum", LaneKind::Instrument, 1, false),
            ("Drums", LaneKind::Audio, 0, true),
        ]
    );
    assert_eq!(view.lanes[1].detail, "Serum_x64");
    let hook = &view.lanes[1].clips[0];
    assert_eq!((hook.start, hook.length, hook.name.as_str()), (16.0, 10.0, "Hook"));
    // First pass from beat 1 of the loop (3 beats), then the 4-beat loop again from 0, twice (the second cut short).
    let notes: Vec<(f64, u8)> = hook.notes.iter().map(|n| (n.start, n.key)).collect();
    assert_eq!(notes, [(0.5, 64), (1.0, 60), (3.0, 60), (4.5, 64), (5.0, 60), (7.0, 60), (8.5, 64), (9.0, 60)]);

    let brk = &view.lanes[2].clips[0];
    assert!(brk.muted);
    assert_eq!((brk.start, brk.length, brk.fade_in, brk.fade_out), (4.0, 8.0, 0.5, 1.0));
    let part = &brk.audio[0];
    assert_eq!(part.path, r"C:\Samples\Break.wav");
    assert_eq!((part.start, part.length, part.from, part.seconds_per_beat), (0.0, 8.0, 0.0, Some(0.5)));
    assert_eq!((part.stretch, part.semitones), (Some(Stretch::ElastiquePro), 2.5));
    assert!((part.gain_db + 6.0206).abs() < 1e-3);
}

#[test]
fn an_empty_arrangement_shows_the_session_scenes_one_after_another() {
    // The arrangement's clips taken out: only the session clip in scene 2 is left.
    let xml = set_xml();
    let cut = |s: &str, from: &str, to: &str| {
        let a = s.find(from).unwrap();
        let b = s[a..].find(to).unwrap() + a + to.len();
        format!("{}{}", &s[..a], &s[b..])
    };
    let xml = cut(&xml, "<ClipTimeable>", "</ClipTimeable>");
    let xml = cut(&xml, "<MainSequencer><Sample>", "</Sample></MainSequencer>");
    let views = LiveSet::open(xml.as_bytes()).unwrap().arrangements();
    assert_eq!(views.iter().map(|v| v.view).collect::<Vec<_>>(), ["Ableton session", "Ableton arrangement"]);
    let session = &views[0];
    // Scene 1 is empty (a 6/8 bar, 3 beats); scene 2 starts after it.
    assert_eq!(
        session.markers.iter().map(|m| (m.beat, m.name.as_str())).collect::<Vec<_>>(),
        [(0.0, "Scene 1"), (3.0, "Hook")]
    );
    let clip = &session.lanes[1].clips[0];
    assert_eq!((clip.start, clip.length, clip.notes.len()), (3.0, 8.0, 1));
}
