//! Made-up Ableton Live sets, laid out as Live 10, 11 and 12 save them.

use std::io::Write;

/// A made-up Live 12 set: a group holding a MIDI track (Serum, then EQ Eight) with one looping arrangement clip and
/// one session clip, an audio track with a warped clip, a return track, two locators, 6/8 at 128 BPM.
pub fn set_xml() -> String {
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

pub fn gzipped(text: &str) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(text.as_bytes()).expect("gzip in memory");
    gz.finish().expect("gzip in memory")
}

/// A set with one MIDI track (an Instrument Rack holding a VST3, a Drum Rack whose return holds a VST2 reverb) and
/// one clip, with its scenes as Live 10 saves them, at the tempo given.
pub fn small_set(clip: &str, tempo: f64, arranged: bool) -> String {
    let (arrangement, session) = if arranged { (clip, "") } else { ("", clip) };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 10.1.30"><LiveSet>
  <Tracks><MidiTrack Id="1"><Name><EffectiveName Value="Keys" /></Name><TrackGroupId Value="-1" />
    <DeviceChain>
      <MainSequencer>
        <ClipSlotList><ClipSlot Id="0"><ClipSlot><Value>{session}</Value></ClipSlot></ClipSlot></ClipSlotList>
        <ClipTimeable><ArrangerAutomation><Events>{arrangement}</Events></ArrangerAutomation></ClipTimeable>
      </MainSequencer>
      <DeviceChain><Devices>
        <InstrumentGroupDevice Id="0"><Branches><InstrumentBranch Id="0"><DeviceChain><MidiToAudioDeviceChain><Devices>
          <PluginDevice Id="0"><PluginDesc><Vst3PluginInfo Id="0"><Name Value="Diva" /></Vst3PluginInfo></PluginDesc></PluginDevice>
        </Devices></MidiToAudioDeviceChain></DeviceChain></InstrumentBranch></Branches></InstrumentGroupDevice>
        <DrumGroupDevice Id="1"><ReturnBranches><ReturnBranch Id="0"><DeviceChain><AudioToAudioDeviceChain><Devices>
          <PluginDevice Id="0"><PluginDesc><VstPluginInfo Id="0"><PlugName Value="ValhallaRoom" /></VstPluginInfo></PluginDesc></PluginDevice>
        </Devices></AudioToAudioDeviceChain></DeviceChain></ReturnBranch></ReturnBranches></DrumGroupDevice>
      </Devices></DeviceChain>
    </DeviceChain></MidiTrack></Tracks>
  <MasterTrack><DeviceChain><Mixer><Tempo><Manual Value="{tempo}" /></Tempo></Mixer></DeviceChain></MasterTrack>
  <SceneNames><Scene Id="0" Value="Verse" /></SceneNames>
</LiveSet></Ableton>"#
    )
}
