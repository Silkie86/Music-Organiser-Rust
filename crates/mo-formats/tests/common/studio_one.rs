//! Made-up Studio One songs, the C# tests' `Fixtures.StudioOneSong` and `Fixtures.StudioOneInserts`, file for file.

use std::path::{Path, PathBuf};

use super::bitwig::stored_zip;

/// "Sunday Song": tempo, two samples, an instrument and an effect, one note part and one stored preset.
pub fn studio_one_song_bytes() -> Vec<u8> {
    let meta = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetaInformation>
  <Attribute id="Document:Generator" value="Studio One/6.5.2"/>
  <Attribute id="Media:Title" value="Sunday Song"/>
  <Attribute id="Media:Tempo" value="92"/>
</MetaInformation>"#;
    let pool = r#"<?xml version="1.0" encoding="UTF-8"?>
<MediaPool>
  <Attributes x:id="rootFolder">
    <AudioClip mediaType="Audio"><Url x:id="path" url="file:///C:/Users/Sol/Samples/Vox%20Chop.wav"/></AudioClip>
    <AudioClip mediaType="Audio"><Url x:id="path" url="file:///C:/Users/Sol/Samples/Rim.aif"/></AudioClip>
  </Attributes>
</MediaPool>"#;
    let synths = r#"<?xml version="1.0" encoding="UTF-8"?>
<SynthFolder>
  <Attributes x:id="ghostData" name="Inst 1">
    <Attributes x:id="deviceData" name="Diva" classID="{ABC}" vendor="u-he" deviceClass="AudioSynth VST3"/>
  </Attributes>
  <Attributes x:id="ghostData" name="Inst 2">
    <Attributes x:id="deviceData" name="Mai Tai" classID="{DEF}" vendor="PreSonus"/>
  </Attributes>
</SynthFolder>"#;
    let mixer = r#"<?xml version="1.0" encoding="UTF-8"?>
<AudioMixer>
  <Attributes x:id="deviceData" name="Pro-C 2" classID="{123}" vendor="FabFilter" deviceClass="AudioEffect VST3"/>
</AudioMixer>"#;
    let song = r#"<?xml version="1.0" encoding="UTF-8"?>
<Song>
  <Tracks>
    <MusicTrack name="Keys">
      <Events>
        <MusicPart name="Verse Chords">
          <NoteEvent start="0" length="1" pitch="60" velocity="0.8"/>
          <NoteEvent start="1" length="0.5" pitch="64" velocity="1"/>
          <NoteEvent start="1.5" length="0.5" pitch="67" velocity="100"/>
        </MusicPart>
      </Events>
    </MusicTrack>
    <AudioTrack name="Vocals"/>
  </Tracks>
</Song>"#;
    stored_zip(&[
        ("metainfo.xml", meta.as_bytes()),
        ("Song/mediapool.xml", pool.as_bytes()),
        ("Devices/audiosynthfolder.xml", synths.as_bytes()),
        ("Devices/audiomixer.xml", mixer.as_bytes()),
        ("Song/song.xml", song.as_bytes()),
        ("Presets/Synths/Diva/Inst 1.vstpreset", b"VST3 fake preset"),
    ])
}

pub fn studio_one_song(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(format!("{name}.song"));
    std::fs::write(&path, studio_one_song_bytes()).expect("write");
    path
}

/// One insert as Studio One 7 saves it: the name it shows ("deviceData") and a note of the plugin itself
/// ("classInfo": its own name and id, and whether it's a VST2, a VST3 or one of Studio One's own).
fn insert(slot: &str, shown: &str, class_id: &str, plugin: &str, category: &str, sub: &str) -> String {
    let sub = if sub.is_empty() { String::new() } else { format!(r#" subCategory="{sub}""#) };
    format!(
        r#"<Attributes name="{slot}">
  <UID x:id="deviceClassID" uid="{class_id}"/>
  <Attributes x:id="deviceData" name="{shown}"/>
  <Attributes x:id="ghostData">
    <Attributes x:id="classInfo" classID="{class_id}" name="{plugin}" category="{category}"{sub}/>
  </Attributes>
</Attributes>"#
    )
}

/// "Click Clack" (Solomon's, Studio One 7): inserts that show numbered or old names, and Studio One's own effects.
pub fn studio_one_inserts_bytes() -> Vec<u8> {
    let meta = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetaInformation>
  <Attribute id="Document:Generator" value="Studio One/7.2.1.106594"/>
  <Attribute id="Media:Tempo" value="140"/>
</MetaInformation>"#;
    let mixer = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<AudioMixer>
  <AudioTrackChannel name="Channel01" label="Kt. st. 1">
    <Attributes x:id="Inserts">
{}
{}
{}
{}
{}
    </Attributes>
  </AudioTrackChannel>
</AudioMixer>"#,
        insert(
            "FX01",
            "FabFilter Pro-Q 2 2",
            "{56535446-5132-7066-6162-66696C746572}",
            "FabFilter Pro-Q 2",
            "AudioEffect",
            "VST2"
        ),
        insert(
            "FX02",
            "TG12345 Stereo",
            "{56535454-4735-5374-6731-323334352073}",
            "Abbey Road EMI TG12345 Ch Stereo",
            "AudioEffect",
            "VST3/Mixing"
        ),
        insert(
            "FX03",
            "Kramer Tape Stereo",
            "{56535454-4150-536B-7261-6D6572207461}",
            "Kramer Tape Stereo",
            "AudioEffect",
            "VST3/Distortion"
        ),
        insert("FX04", "Pro EQ", "{D9AE9ACD-69B4-4B43-B8D5-983E39C559A5}", "Pro EQ", "AudioEffect", "(Native)/EQ"),
        insert("FX05", "Gate", "{132C0E1F-06FD-4C96-BD65-678C04FE1C92}", "Gate", "AudioEffect", "(Native)/Dynamics"),
    );
    let synths = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<SynthFolder>
{}
{}
</SynthFolder>"#,
        insert(
            "Inst03",
            "Kontakt 5 16out 2",
            "{5653544E-694F-376B-6F6E-74616B742035}",
            "Kontakt 5 16out",
            "AudioSynth",
            "VST2"
        ),
        insert(
            "Inst01",
            "Impact",
            "{3713E26C-2FCA-4024-9F25-17E9D2BE2B9B}",
            "Impact",
            "AudioSynth",
            "(Native)/Sampler/Drum"
        ),
    );
    let note_fx = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<MusicTrackDevice>
{}
</MusicTrackDevice>"#,
        insert("Slot02", "Arpeggiator 2", "{BDD2D6E5-3819-41B4-BF91-7B832DAAFA0B}", "Arpeggiator", "MusicEffect", ""),
    );
    let song = r#"<?xml version="1.0" encoding="UTF-8"?>
<Song><Tracks><AudioTrack name="Kick"/></Tracks></Song>"#;
    stored_zip(&[
        ("metainfo.xml", meta.as_bytes()),
        ("Devices/audiomixer.xml", mixer.as_bytes()),
        ("Devices/audiosynthfolder.xml", synths.as_bytes()),
        ("Devices/musictrackdevice.xml", note_fx.as_bytes()),
        ("Song/song.xml", song.as_bytes()),
    ])
}

pub fn studio_one_inserts(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(format!("{name}.song"));
    std::fs::write(&path, studio_one_inserts_bytes()).expect("write");
    path
}
