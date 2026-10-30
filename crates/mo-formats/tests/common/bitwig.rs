//! Made-up Bitwig files, written value by value in Bitwig's format, as the C# tests' `Fixtures.BwWriter` writes them.

use std::path::{Path, PathBuf};

/// Writes Bitwig's object stream: big-endian, typed fields.
#[derive(Default)]
pub struct BwWriter {
    pub s: Vec<u8>,
    objects: u32,
    ids: u32,
}

impl BwWriter {
    pub fn u32(&mut self, v: u32) {
        self.s.extend_from_slice(&v.to_be_bytes());
    }

    fn ty(&mut self, fid: u32, ty: u8) {
        self.u32(fid);
        self.s.push(ty);
    }

    pub fn str_val(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.s.extend_from_slice(s.as_bytes());
    }

    pub fn str_val_utf16(&mut self, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        self.u32(0x8000_0000 | units.len() as u32);
        self.s.extend(units.iter().flat_map(|u| u.to_be_bytes()));
    }

    /// Starts an object; returns its number (from 1), for references.
    pub fn begin(&mut self, class: u32) -> u32 {
        self.u32(class);
        self.objects += 1;
        self.objects
    }

    pub fn end(&mut self) {
        self.u32(0);
    }

    pub fn str(&mut self, fid: u32, s: &str) {
        self.ty(fid, 0x08);
        self.str_val(s);
    }

    pub fn str_utf16(&mut self, fid: u32, s: &str) {
        self.ty(fid, 0x08);
        self.str_val_utf16(s);
    }

    pub fn u8(&mut self, fid: u32, v: u8) {
        self.ty(fid, 0x01);
        self.s.push(v);
    }

    pub fn bool(&mut self, fid: u32, v: bool) {
        self.ty(fid, 0x05);
        self.s.push(v as u8);
    }

    pub fn f64(&mut self, fid: u32, v: f64) {
        self.ty(fid, 0x07);
        self.s.extend_from_slice(&v.to_be_bytes());
    }

    /// A made-up id, different each time (and the same each run).
    pub fn id(&mut self, fid: u32) {
        self.ty(fid, 0x15);
        self.ids += 1;
        let mut g = [0x11u8; 16];
        g[12..].copy_from_slice(&self.ids.to_be_bytes());
        self.s.extend_from_slice(&g);
    }

    pub fn uuid(&mut self, fid: u32, id: &str) {
        self.ty(fid, 0x15);
        let hex: String = id.chars().filter(|c| *c != '-').collect();
        self.s.extend((0..16).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex")));
    }

    pub fn null(&mut self, fid: u32) {
        self.ty(fid, 0x0a);
    }

    pub fn ints(&mut self, fid: u32, v: &[i32]) {
        self.ty(fid, 0x0f);
        self.u32(v.len() as u32);
        for i in v {
            self.u32(*i as u32);
        }
    }

    pub fn obj(&mut self, fid: u32) {
        self.ty(fid, 0x09);
    }

    pub fn reference(&mut self, fid: u32, index: u32) {
        self.ty(fid, 0x09);
        self.u32(1);
        self.u32(index);
    }

    pub fn list(&mut self, fid: u32) {
        self.ty(fid, 0x12);
    }

    pub fn list_ref(&mut self, index: u32) {
        self.u32(1);
        self.u32(index);
    }

    pub fn end_list(&mut self) {
        self.u32(3);
    }

    pub fn map(&mut self, fid: u32) {
        self.ty(fid, 0x14);
    }

    pub fn map_key(&mut self, key: &str) {
        self.s.push(1);
        self.str_val(key);
    }

    pub fn end_map(&mut self) {
        self.s.push(0);
    }

    pub fn named(&mut self, fid: u32) {
        self.ty(fid, 0x1a);
    }

    /// A reference written as a bare object number (type 0x0b), as Bitwig does for objects it already wrote.
    pub fn bare_ref(&mut self, fid: u32, index: u32) {
        self.ty(fid, 0x0b);
        self.u32(index);
    }

    pub fn i16(&mut self, fid: u32, v: i16) {
        self.ty(fid, 0x02);
        self.s.extend_from_slice(&v.to_be_bytes());
    }

    pub fn i32(&mut self, fid: u32, v: i32) {
        self.ty(fid, 0x03);
        self.u32(v as u32);
    }

    pub fn blob(&mut self, fid: u32, v: &[u8]) {
        self.ty(fid, 0x0d);
        self.u32(v.len() as u32);
        self.s.extend_from_slice(v);
    }

    /// A list of floats (type 0x17), as FM-4 saves its modulation matrix.
    pub fn floats(&mut self, fid: u32, v: &[f32]) {
        self.ty(fid, 0x17);
        self.u32(v.len() as u32);
        self.s.extend(v.iter().flat_map(|f| f.to_be_bytes()));
    }

    /// A list of doubles (type 0x18), as Bitwig 6.1 saves a Sampler sample's analysis.
    pub fn doubles(&mut self, fid: u32, v: &[f64]) {
        self.ty(fid, 0x18);
        self.u32(v.len() as u32);
        self.s.extend(v.iter().flat_map(|f| f.to_be_bytes()));
    }

    pub fn meta_entry(&mut self, key: &str, ty: u8) {
        self.u32(1);
        self.str_val(key);
        self.s.push(ty);
    }

    /// A whole file: the header, metadata, 8 bytes of padding as Bitwig writes, the body, then the zip.
    pub fn file(meta: impl FnOnce(&mut BwWriter), body: impl FnOnce(&mut BwWriter), zip: Option<&[u8]>) -> Vec<u8> {
        let mut m = BwWriter::default();
        m.u32(4);
        m.str_val("meta");
        meta(&mut m);
        m.u32(0);
        let mut b = BwWriter::default();
        body(&mut b);
        let body_offset = 42 + m.s.len() + 8;
        let zip_offset = if zip.is_some() { body_offset + b.s.len() } else { 0 };
        let mut out = format!("BtWg000300020000{body_offset:08x}{zip_offset:016x}00").into_bytes();
        out.extend_from_slice(&m.s);
        out.extend_from_slice(b"       \n");
        out.extend_from_slice(&b.s);
        if let Some(z) = zip {
            out.extend_from_slice(z);
        }
        out
    }
}

/// A zip holding the files uncompressed, as small as the format allows.
pub fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in files {
        let (offset, crc) = (out.len() as u32, crc32(data));
        let common = |v: &mut Vec<u8>| {
            v.extend_from_slice(&20u16.to_le_bytes()); // version needed
            v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // flags, stored, time, date
            v.extend_from_slice(&crc.to_le_bytes());
            v.extend_from_slice(&(data.len() as u32).to_le_bytes());
            v.extend_from_slice(&(data.len() as u32).to_le_bytes());
            v.extend_from_slice(&(name.len() as u16).to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes()); // extra
        };
        out.extend_from_slice(b"PK\x03\x04");
        common(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        common(&mut central);
        central.extend_from_slice(&[0; 6]); // comment length, disk, internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

pub const CLS_VST3: u32 = 0x728;

/// Bitwig's "structure" summary of a project's tracks: itself a small BtWg document, with no metadata.
fn structure(class: u32, tracks: &[(u32, &str)]) -> Vec<u8> {
    let mut st = BwWriter::default();
    st.begin(class);
    st.str(0xad3, "");
    st.obj(0xad2);
    st.begin(0x283);
    st.str(0xad1, "Project");
    st.list(0xace);
    for &(cls, name) in tracks {
        st.begin(cls);
        st.str(0xad1, name);
        st.end();
    }
    st.end_list();
    st.list(0xacf);
    st.begin(0x28a);
    st.str(0xad1, "FX 1");
    st.end();
    st.end_list();
    st.obj(0xad0);
    st.begin(0x28b);
    st.str(0xad1, "Master");
    st.end();
    st.end();
    st.end();
    let mut out = b"BtWg0003000200000000002b000000000000000001\n".to_vec();
    out.extend_from_slice(&st.s);
    out
}

/// The bytes of [`bitwig_project`].
pub fn bitwig_project_bytes() -> Vec<u8> {
    let zip = stored_zip(&[
        ("plugin-states/aaaa-keys.vstpreset", b"VST3 keys state"),
        ("plugin-states/bbbb-limiter.vstpreset", b"VST3 limiter state"),
    ]);
    let structure = structure(0x2a0, &[(0x288, "Keys Display")]);
    BwWriter::file(
        |m| {
            m.meta_entry("application_version_name", 0x08);
            m.str_val("5.2.4");
            m.meta_entry("structure", 0x0d);
            m.u32(structure.len() as u32);
            m.s.extend_from_slice(&structure);
            m.meta_entry("title", 0x08);
            m.str_val("");
            m.meta_entry("used_files", 0x19);
            m.u32(2);
            m.str_val("samples/Kick 01.wav");
            m.str_val("multi-samples/Pad.multisample");
        },
        |b| {
            b.begin(0x2e);
            b.id(0xf);
            b.str(0x44, "Loop Idea");
            b.obj(0x29a6);
            b.begin(0x4c1);
            b.f64(0x2c8, 128.5);
            b.end();
            b.obj(0x4dd);
            b.begin(0x1dd);
            b.id(0xf);
            b.str(0x15b, "Project");
            b.list(0x4de);
            // The instrument track.
            b.begin(0x90);
            b.id(0xf);
            b.str(0x15b, "");
            b.map(0x168);
            b.map_key("user");
            b.begin(0x88);
            b.u8(0x2603, 64);
            b.end();
            b.end_map();
            b.list(0x4ba);
            b.begin(CLS_VST3);
            b.str(0x12de, "");
            b.str(0x9a, "Keyscape");
            b.str(0x9b, "Spectrasonics");
            b.str(0x9c, "Synth");
            b.str(0xbe5, "aaaa-keys.vstpreset");
            b.end();
            b.begin(0x40);
            b.str(0x9a, "Delay+");
            b.str(0x9b, "Bitwig");
            b.str(0x9c, "Delay");
            b.str_utf16(0x133b, "Delay » Feedback");
            b.ints(0x14e1, &[0, 1]);
            b.end();
            b.end_list();
            b.obj(0x2bad);
            b.begin(0x2a);
            b.str(0x3b, r"C:\Program Files\Common Files\VST3\Keyscape.vst3");
            b.end();
            b.obj(0x2bae);
            b.begin(0x2a);
            b.str(0x3b, r"D:\Splice\Snare 808.wav");
            b.end();
            b.obj(0x2baf);
            b.begin(0x461);
            b.str(0x11a7, "Elektron Analog Rytm MKII");
            b.end();
            // The master track, defined here and referenced from the project group below.
            b.obj(0x2bb0);
            let master = b.begin(0x90);
            b.id(0xf);
            b.str(0x15b, "");
            b.list(0x4ba);
            b.begin(CLS_VST3);
            b.str(0x9a, "FabFilter Pro-L 2");
            b.str(0x9b, "FabFilter");
            b.str(0x9c, "Dynamics");
            b.str(0xbe5, "bbbb-limiter.vstpreset");
            b.end();
            b.end_list();
            b.end();
            b.obj(0x15e);
            b.begin(0x1e);
            b.list(0x231);
            b.begin(0xe3);
            b.obj(0x240);
            b.begin(0x47);
            b.f64(0x2af, 0.0);
            b.f64(0x26, 4.0);
            b.obj(0x288);
            b.begin(0xbf);
            b.obj(0x49c);
            b.begin(0x6cc);
            b.list(0x18cb);
            let mut lane = |key: u8, notes: &[(f64, f64, f64)]| {
                b.begin(0x42);
                b.u8(0xee, key);
                b.list(0x21f);
                for &(s, l, v) in notes {
                    b.begin(0x66);
                    b.f64(0x2af, s);
                    b.f64(0x26, l);
                    b.f64(0xef, v);
                    b.end();
                }
                b.end_list();
                b.end();
            };
            lane(60, &[(0.0, 1.0, 0.5), (1.5, 0.5, 1.0)]);
            lane(64, &[(2.0, 1.0, 0.8)]);
            b.end_list();
            b.end(); // 0x6cc
            b.end(); // 0xbf
            b.end(); // clip
            b.named(0x89b);
            b.list_ref(master);
            b.str_val("named ref");
            b.end(); // 0xe3
            b.end_list();
            b.end(); // 0x1e
            b.end(); // track
            b.end_list();
            b.list(0x4df);
            b.begin(0x90);
            b.id(0xf);
            b.str(0x15b, "");
            b.end();
            b.end_list();
            b.reference(0x4e0, master);
            b.end(); // project group
            b.end(); // root
        },
        Some(&zip),
    )
}

/// A Bitwig 5-style project in its own folder (with a `samples` folder): one unnamed instrument track with Keyscape
/// (VST3) and Delay+, a note clip, a master track holding Pro-L 2 that's defined inside the first track and referenced
/// from the project (as Bitwig does), an effect track, plugin states in the embedded zip and a "structure" summary.
pub fn bitwig_project(dir: &Path, name: &str) -> PathBuf {
    let project = dir.join(name);
    std::fs::create_dir_all(project.join("samples")).expect("make the project folder");
    std::fs::write(project.join("samples").join("Recorded Bass.wav"), [0]).expect("write a sample");
    let path = project.join(format!("{name}.bwproject"));
    std::fs::write(&path, bitwig_project_bytes()).expect("write the project");
    path
}

/// The bytes of [`bitwig_undecodable`]: a project whose header has no offsets and whose body isn't Bitwig's objects,
/// only readable text (some of it in a zlib stream), for the reader's text scan.
pub fn bitwig_undecodable_bytes(name: &str) -> Vec<u8> {
    use std::io::Write;
    let mut ms = b"BtWg00010001".to_vec();
    ms.extend_from_slice(&[0, 1, 2, 0xFF, 0x10]);
    ms.extend_from_slice(b"application_version_name\0\0\x055.2.4");
    ms.extend_from_slice(&[0, 0, 0x07]);
    ms.extend_from_slice(b"Polymer");
    ms.extend_from_slice(&[0, 0, 0x03]);
    ms.extend_from_slice(b"Delay+");
    ms.extend_from_slice(&[0, 0x10]);
    ms.extend("Analog Rytm MKII".encode_utf16().flat_map(|u| u.to_le_bytes()));
    ms.extend_from_slice(&[0, 0, 0x22]);
    ms.extend_from_slice(br"C:\Program Files\Common Files\VST3\Valhalla Room.vst3");
    ms.extend_from_slice(&[0, 0, 0x05]);
    ms.extend_from_slice(b"Serum");
    ms.extend_from_slice(&[0, 0]);
    // Bitwig writes a string's length just before it, and a length that is a readable letter joins the text: ">" (62)
    // before this drive path, "-" (45) before the path in the project folder. A bare name is a sound inside a
    // multisample or a Grid's sample list.
    let recording = format!(r"F:\Bitwig Studio\Projects\{name}\recordings\Syntakt 2-1.wav");
    for text in [
        recording.as_str(),
        "recordings/Untitled Project [0m27s-1m22s].wav",
        "BASSDRUM_0002 from the Kick Selector.wav",
    ] {
        ms.extend_from_slice(&[0, 0, 0, text.len() as u8]);
        ms.extend_from_slice(text.as_bytes());
    }
    ms.extend_from_slice(&[0, 0]);
    // A compressed section, as some Bitwig versions store parts of the project that way.
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(b"\0\0\x15D:\\Splice\\Snare 808.wav\0\0").expect("compress");
    ms.extend_from_slice(&z.finish().expect("compress"));
    ms.extend_from_slice(&[0, 0, 0, 0]);
    ms
}

/// [`bitwig_undecodable_bytes`] in its own project folder, with a `samples` folder beside it.
pub fn bitwig_undecodable(dir: &Path, name: &str) -> PathBuf {
    let project = dir.join(name);
    std::fs::create_dir_all(project.join("samples")).expect("make the project folder");
    std::fs::write(project.join("samples").join("Recorded Bass.wav"), [0]).expect("write a sample");
    let path = project.join(format!("{name}.bwproject"));
    std::fs::write(&path, bitwig_undecodable_bytes(name)).expect("write the project");
    path
}

/// Paths [`bitwig_scan_paths_bytes`] holds: one with an accent, a network one, one in Cyrillic and one in Japanese.
pub const SCANNED_PATHS: [&str; 4] = [
    r"F:\Samples\Beyoncé Acapellas\Vocal Chop 03.wav",
    r"\\NAS\Samples\Breaks\Amen Break 136bpm.wav",
    r"D:\Музыка\Сэмплы\Ударные\Бочка\Kick 01.wav",
    r"D:\音楽\サンプル\ドラム\キック.wav",
];

/// A project ("Song") whose metadata reads but whose body doesn't (a value of a kind this app doesn't know, as 6.1's
/// lists of doubles were), holding [`SCANNED_PATHS`] and a recording that was collected into the project.
pub fn bitwig_scan_paths_bytes() -> Vec<u8> {
    let [accented, network, cyrillic, japanese] = SCANNED_PATHS;
    BwWriter::file(
        |m| {
            m.meta_entry("used_files", 0x19);
            m.u32(1);
            m.str_val("recordings/BD 1-2.wav");
        },
        |b| {
            b.begin(0x2e);
            // Bitwig writes a string's length before it: 42 is "*", which joins the network path's text.
            for s in [r"F:\Bitwig Studio\Projects\Song\recordings\BD 1-2.wav", network, r"F:\Samples\Drums\Kick 01.wav"]
            {
                b.str(0x2b9, s);
            }
            // As Bitwig writes any text with a letter outside ASCII, even when most of it is.
            for s in [accented, cyrillic, japanese] {
                b.str_utf16(0x2b9, s);
            }
            b.u32(0x123);
            b.s.push(0x7e);
        },
        None,
    )
}

/// A project that collected Kick-01 and Snare (as "Snare-2", the name being taken) into its folder, and still uses
/// Kick-02 from the same pack where it is. When not `decodable`, its body ends in a value of a kind this app doesn't
/// know.
pub fn bitwig_collected_bytes(decodable: bool) -> Vec<u8> {
    BwWriter::file(
        |m| {
            m.meta_entry("used_files", 0x19);
            m.u32(2);
            m.str_val("samples/Kick-01.wav");
            m.str_val("samples/Snare-2.wav");
        },
        |b| {
            b.begin(0x2e);
            let files = [r"D:\Packs\Drums\Kick-01.wav", r"D:\Packs\Drums\Kick-02.wav", r"D:\Packs\Drums\Snare.wav"];
            for (id, file) in (0x4100..).zip(files) {
                b.obj(id);
                b.begin(0x2a);
                b.str(0x3b, file);
                b.end();
            }
            if !decodable {
                b.u32(0x123);
                b.s.push(0x7e);
            }
            b.end();
        },
        None,
    )
}

/// What [`bitwig_template`] holds, as the C# test's options.
#[derive(Clone, Copy)]
pub struct Template {
    pub automation_lane: bool,
    pub audio_track: bool,
    pub device_power: bool,
    pub master_plugin: bool,
}

impl Default for Template {
    fn default() -> Template {
        Template { automation_lane: true, audio_track: true, device_power: true, master_plugin: false }
    }
}

/// A Bitwig 6-style template for converting FL projects, laid out like one saved in Bitwig 6.1: a Sampler track
/// (Sampler and Amp, root key A2, sends to an effect track that's defined inside it, and the master defined in its
/// output routing), an audio track with a clip playing a 32 second sample (if `audio_track`), a VST3 track with a
/// Note Transpose before Serum 2 and a clip of notes, and a VST2 track with Drumazon, whose output chain sends to the
/// effect track. The Sampler track's volume is automated. As in Bitwig's files, the mixer volume is first written in
/// the track's per-user view state and referenced from the mixer, and the project's view state and clip selection
/// point into the tracks.
pub fn bitwig_template(t: Template) -> Vec<u8> {
    const SAMPLER: &str = "468bc14b-b2e7-45a1-9666-e83117fe404e";
    const NOTE_TRANSPOSE: &str = "0815cd9e-3a31-4429-a268-dabd952a3b68";
    const AMP: &str = "41be8f3a-6d24-4442-9508-8548dbe62d47";
    let mut files: Vec<(&str, &[u8])> = vec![
        ("plugin-states/serum.vstpreset", b"template serum"),
        ("plugin-states/drumazon.fxb", b"template drumazon"),
    ];
    if t.master_plugin {
        files.push(("plugin-states/limiter.vstpreset", b"template limiter"));
    }
    let zip = stored_zip(&files);
    let names: &[(u32, &str)] = if t.audio_track {
        &[(0x288, "Kick"), (0x287, "Drops"), (0x288, "Serum 2"), (0x288, "Drumazon")]
    } else {
        &[(0x288, "Kick"), (0x288, "Serum 2"), (0x288, "Drumazon")]
    };
    let structure = structure(0x285, names);
    BwWriter::file(
        |m| {
            m.meta_entry("application_version_name", 0x08);
            m.str_val("6.1.3");
            m.meta_entry("structure", 0x0d);
            m.u32(structure.len() as u32);
            m.s.extend_from_slice(&structure);
            m.meta_entry("used_files", 0x19);
            m.u32(0);
        },
        |b| template_body(b, t, SAMPLER, NOTE_TRANSPOSE, AMP),
        Some(&zip),
    )
}

fn param(b: &mut BwWriter, class: u32, name: &str, value: f64) {
    b.begin(class);
    b.str(0x2bd, name);
    b.f64(0x2c8, value);
    b.end();
}

fn file_ref(b: &mut BwWriter, path: &str) {
    b.begin(0x2a);
    b.str(0x38, "PC");
    b.str(0x3a, "user");
    b.str(0x3b, path);
    b.end();
}

/// A track's start: ids, per-user view state that first writes its volume automation lane and mixer volume, its
/// automation lanes. An automated lane holds an automation clip and a point after it.
fn track_start(b: &mut BwWriter, name: &str, automated: bool, lanes: impl FnOnce(&mut BwWriter)) -> u32 {
    let t = b.begin(0x90);
    b.id(0xf);
    b.str(0x15b, name);
    b.map(0x168);
    b.map_key("user");
    b.begin(0x88);
    b.obj(0x4e6);
    b.begin(0x23d);
    b.obj(0x2a3);
    b.begin(0xf1);
    b.obj(0x41);
    param(b, 0x4c4, "VOLUME", 1.0);
    b.str(0x133b, "Mixer");
    b.str(0x133a, "Volume");
    b.end();
    b.obj(0x2d8c);
    b.begin(0xd91);
    b.list(0x21f);
    if automated {
        b.begin(0xd90);
        b.f64(0x2af, 0.0);
        b.f64(0x26, 4.0);
        b.end();
    }
    b.end_list();
    b.obj(0x35ac);
    b.begin(0x6e);
    b.list(0x21f);
    if automated {
        b.begin(0x108);
        b.f64(0x2af, 4.0);
        b.f64(0x28f, 1.25992105);
        b.u8(0x359e, 0);
        b.f64(0xba6, 0.5);
        b.f64(0x2ebe, 0.0);
        b.end();
    }
    b.end_list();
    b.end();
    b.end();
    b.obj(0x2aec);
    b.begin(0xe6c);
    b.list(0x10e6);
    b.end_list();
    b.end();
    b.end();
    b.end();
    b.end_map();
    b.obj(0x2dc9);
    b.begin(0xfa2);
    b.list(0x4ba);
    if automated {
        b.list_ref(t + 2);
    }
    lanes(b);
    b.end_list();
    b.list(0x3f83);
    b.end_list();
    b.end();
    b.id(0xa51);
    t
}

/// What [`track_end`] needs to know about the objects written so far.
#[derive(Default)]
struct Numbers {
    master: u32,
    fx: u32,
    content: u32,
    clip: u32,
}

type Part<'a> = Box<dyn FnOnce(&mut BwWriter, &mut Numbers) + 'a>;

/// The rest of a track: arrangement clips, clip contents, devices, mixer (volume by reference), output.
fn track_end(
    b: &mut BwWriter,
    n: &mut Numbers,
    volume: u32,
    clips: Option<Part>,
    devices: Part,
    sends: Part,
    output: Part,
) {
    let has_clips = clips.is_some();
    b.obj(0x2bf8);
    b.begin(0xef7);
    b.obj(0x2d8e);
    b.begin(0x52);
    b.list(0x21f);
    if let Some(c) = clips {
        c(b, n);
    }
    b.end_list();
    b.end();
    b.end();
    b.obj(0x2aed);
    b.begin(0xe6d);
    b.list(0x10e6);
    if has_clips {
        b.list_ref(n.content);
    }
    b.end_list();
    b.end();
    b.obj(0x164);
    b.begin(0x8a);
    b.list(0x144);
    devices(b, n);
    b.end_list();
    b.end();
    b.obj(0x165);
    b.begin(0xab);
    b.bare_ref(0x1a4, volume);
    b.obj(0x1a5);
    param(b, 0x4c3, "PANORAMA", 0.0);
    b.obj(0x1a6);
    b.begin(0x4ce);
    b.str(0x2bd, "MUTE");
    b.bool(0xd2, false);
    b.end();
    b.list(0xa5);
    sends(b, n);
    b.end_list();
    b.end();
    b.obj(0x897);
    b.begin(0x241);
    b.named(0x89b);
    output(b, n);
    b.str_val("361");
    b.end();
    b.end();
}

fn send_to_fx(b: &mut BwWriter, n: &mut Numbers) {
    b.begin(0x27c);
    b.obj(0xab3);
    param(b, 0x4c2, "AMOUNT", 0.0);
    b.reference(0xbd0, n.fx);
    b.end();
}

fn to_master(b: &mut BwWriter, n: &mut Numbers) {
    b.list_ref(n.master);
}

fn contents(b: &mut BwWriter, items: &[&dyn Fn(&mut BwWriter)]) {
    b.obj(0xa4);
    b.begin(0xd3);
    b.str(0x2b9, "CONTENTS");
    b.list(0x20c);
    for item in items {
        item(b);
    }
    b.end_list();
    b.end();
}

fn named_obj(b: &mut BwWriter, class: u32, name: &str, fields: impl FnOnce(&mut BwWriter)) {
    b.begin(class);
    b.str(0x2b9, name);
    fields(b);
    b.end();
}

fn template_body(b: &mut BwWriter, t: Template, sampler: &str, note_transpose: &str, amp: &str) {
    let mut n = Numbers::default();
    b.begin(0x2e);
    b.id(0xf);
    b.obj(0x29a6);
    param(b, 0x4c1, "TEMPO", 120.0);
    b.obj(0x4dd);
    b.begin(0x1dd);
    b.id(0xf);
    b.str(0x15b, "Project");
    b.list(0x4de);

    // The Sampler track.
    let first_track = track_start(b, "", t.automation_lane, |_| {});
    // The track, its view state, lane, target, then the volume.
    let volume = first_track + 4;
    track_end(
        b,
        &mut n,
        volume,
        None,
        Box::new(|b, _| {
            b.begin(0xa3);
            b.id(0xf);
            b.obj(0x197);
            b.begin(0x40);
            b.str(0x12de, "Kick");
            b.str(0x9a, "Sampler");
            b.str(0x9b, "Bitwig");
            b.str(0x9c, "Synth");
            b.u8(0x9d, 2);
            b.uuid(0x99, sampler);
            contents(
                b,
                &[&|b| named_obj(b, 0x85, "TRANSPOSE", |b| b.f64(0x136, 0.0)), &|b| {
                    named_obj(b, 0x212, "SAMPLE", |b| {
                        b.obj(0x74c);
                        b.begin(0x210);
                        b.str(0x2b9, "SAMPLE");
                        b.obj(0x4112);
                        b.begin(0x15eb);
                        b.obj(0x748);
                        b.begin(0x4a9);
                        b.str(0x129f, "Kick.wav");
                        b.obj(0x129e);
                        b.begin(0x4a6);
                        b.str(0x512, "Kick.wav");
                        b.obj(0xd3b);
                        file_ref(b, r"J:\samples\Kick.wav");
                        b.obj(0x513);
                        b.begin(0x1e2);
                        b.blob(0x16, &[0; 16]);
                        b.end();
                        b.i16(0x17, 3856);
                        b.end();
                        b.i32(0x94, 44100);
                        b.f64(0x95, 0.25);
                        b.u8(0x96, 1);
                        b.end();
                        b.end();
                        b.u8(0x75c, 57);
                        b.f64(0x75f, 0.25);
                        b.f64(0x761, 0.25);
                        b.end();
                    })
                }],
            );
            b.end();
            b.str(0x199, "Kick");
            b.end();
            b.begin(0xa3);
            b.id(0xf);
            b.obj(0x197);
            b.begin(0x40);
            b.str(0x9a, "Amp");
            b.u8(0x9d, 0);
            b.uuid(0x99, amp);
            b.end();
            b.end();
        }),
        // The effect track is first written inside this send.
        Box::new(|b, n| {
            b.begin(0x27c);
            b.obj(0xab3);
            param(b, 0x4c2, "AMOUNT", 0.0);
            b.obj(0xbd0);
            n.fx = b.begin(0x90);
            b.id(0xf);
            b.str(0x15b, "FX 1");
            b.end();
            b.end();
        }),
        Box::new(|b, n| {
            n.master = b.begin(0x90);
            b.id(0xf);
            b.str(0x15b, "Master");
            b.obj(0x164);
            b.begin(0x8a);
            b.list(0x144);
            if t.master_plugin {
                b.begin(0x729);
                b.id(0xf);
                b.obj(0x197);
                b.begin(CLS_VST3);
                b.str(0x9a, "Limiter");
                b.str(0x9b, "Some Vendor");
                b.str(0x9c, "Audio FX");
                b.u8(0x9d, 0);
                b.str(0xbe5, "limiter.vstpreset");
                b.end();
                b.end();
            }
            b.end_list();
            b.end();
            b.obj(0x165);
            b.begin(0xab);
            b.obj(0x1a4);
            param(b, 0x4c4, "VOLUME", 1.0);
            b.obj(0x1a5);
            param(b, 0x4c3, "PANORAMA", 0.0);
            b.end();
            b.end();
        }),
    );

    if t.audio_track {
        let audio = track_start(b, "Drops", false, |_| {});
        track_end(
            b,
            &mut n,
            audio + 4,
            Some(Box::new(|b, n| {
                b.begin(0xee);
                b.f64(0x2af, 0.0);
                b.f64(0x26, 64.0);
                b.bool(0x10f8, false);
                b.obj(0x2ae9);
                b.begin(0xec2);
                b.obj(0x10e9);
                n.content = b.begin(0x105);
                b.str(0x2d24, "Drops");
                b.obj(0x49c);
                b.begin(0x6ce);
                b.list(0x18cb);
                b.begin(0xce);
                b.list(0x21f);
                b.begin(0xd4);
                b.f64(0x2af, 0.0);
                b.f64(0x26, 64.0);
                b.bool(0x10f8, false);
                b.obj(0x288);
                b.begin(0x9f);
                b.null(0xc33);
                b.obj(0x20d);
                b.begin(0x1db);
                b.obj(0x4b4);
                let drops = b.begin(0x4a9);
                b.str(0x129f, "Drops.wav");
                b.obj(0x129e);
                b.begin(0x4a6);
                b.str(0x512, "Drops.wav");
                b.obj(0xd3b);
                file_ref(b, r"J:\samples\Drops.wav");
                b.obj(0x513);
                b.begin(0x1e2);
                b.blob(0x16, &[0; 16]);
                b.end();
                b.i32(0x17, 8467244);
                b.list(0x129c);
                b.list_ref(drops);
                b.end_list();
                b.end();
                b.i32(0x94, 44100);
                b.f64(0x95, 32.0);
                b.u8(0x96, 2);
                b.end();
                b.end();
                b.u8(0x4b0, 3);
                b.obj(0x192);
                b.begin(0x8b);
                b.list(0x21f);
                for (at, sample) in [(0.0, 0.0), (64.0, 32.0)] {
                    b.begin(0x43);
                    b.f64(0x2af, at);
                    b.f64(0xa6, sample);
                    b.end();
                }
                b.end_list();
                b.end();
                b.obj(0x10ea);
                b.begin(0x36b);
                b.obj(0x10e9);
                b.begin(0xb6);
                b.list(0x21f);
                for at in [0.0, 8.5] {
                    b.begin(0x203);
                    b.f64(0x2af, at);
                    b.f64(0x703, 1.99);
                    b.end();
                }
                b.end_list();
                b.u8(0x4406, 1);
                b.end();
                b.end();
                b.end(); // play
                b.f64(0xc04, 0.0);
                b.f64(0xc05, 32.0);
                b.end(); // audio
                b.end_list();
                b.end(); // lane
                b.end_list();
                b.end();
                b.end();
                b.end();
                b.obj(0x2adb);
                b.begin(0x267);
                b.u8(0x991, 0);
                b.obj(0x992);
                b.begin(0x58);
                b.f64(0x2af, 0.0);
                b.f64(0x26, 69.33);
                b.end();
                b.obj(0x98c);
                b.begin(0x269);
                b.f64(0x2af, 0.0);
                b.end();
                b.obj(0x98d);
                b.begin(0x268);
                b.f64(0x2af, 64.0);
                b.end();
                b.end();
                b.end();
            })),
            Box::new(|_, _| {}),
            Box::new(send_to_fx),
            Box::new(to_master),
        );
    }

    // The VST3 track: Note Transpose, Serum 2, a clip with two notes on key 63 and one on 60.
    let vst3 = track_start(b, "", false, |_| {});
    track_end(
        b,
        &mut n,
        vst3 + 4,
        Some(Box::new(|b, n| {
            n.clip = b.begin(0x47);
            b.f64(0x2af, 0.0);
            b.f64(0x26, 4.0);
            b.obj(0x2ae9);
            b.begin(0xec0);
            b.obj(0x10e9);
            n.content = b.begin(0xbf);
            b.obj(0x49c);
            b.begin(0x6cc);
            b.list(0x18cb);
            for (key, starts) in [(63u8, &[0.0, 2.0][..]), (60, &[1.0][..])] {
                b.begin(0x42);
                b.list(0x21f);
                for &at in starts {
                    b.begin(0x66);
                    b.f64(0x2af, at);
                    b.f64(0x26, 0.25);
                    b.obj(0x288);
                    b.begin(0x29);
                    b.null(0xc33);
                    b.end();
                    b.f64(0xef, 0.79);
                    b.f64(0xf0, 0.79);
                    b.end();
                }
                b.end_list();
                b.u8(0xee, key);
                b.end();
            }
            b.end_list();
            b.end();
            b.end();
            b.end();
            b.obj(0x2adb);
            b.begin(0x267);
            b.obj(0x992);
            b.begin(0x58);
            b.f64(0x2af, 0.0);
            b.f64(0x26, 4.0);
            b.end();
            b.obj(0x98c);
            b.begin(0x269);
            b.f64(0x2af, 0.0);
            b.end();
            b.obj(0x98d);
            b.begin(0x268);
            b.f64(0x2af, 4.0);
            b.end();
            b.end();
            b.end();
        })),
        Box::new(|b, _| {
            b.begin(0xa3);
            b.id(0xf);
            b.obj(0x197);
            b.begin(0x40);
            b.str(0x9a, "Note Transpose");
            b.str(0x9c, "Note FX");
            b.u8(0x9d, 1);
            b.uuid(0x99, note_transpose);
            contents(
                b,
                &[&|b| named_obj(b, 0x189, "COARSE", |b| b.u8(0x330, 3)), &|b| {
                    named_obj(b, 0xf7, "OCTAVES", |b| b.u8(0x273, 3))
                }],
            );
            b.end();
            b.end();
            b.begin(0x729);
            b.id(0xf);
            b.obj(0x197);
            b.begin(CLS_VST3);
            b.str(0x12de, "");
            b.str(0x9a, "Serum 2");
            b.str(0x9b, "Xfer Records");
            b.str(0x9c, "Synth");
            b.u8(0x9d, 2);
            if t.device_power {
                b.bool(0xa3, true);
            }
            b.obj(0xc5a);
            b.begin(0x2b4);
            b.list(0xc5b);
            file_ref(b, r"C:\Program Files\Common Files\VST3\Serum2.vst3");
            b.end_list();
            b.end();
            b.str(0xbe5, "serum.vstpreset");
            b.list(0x1318);
            b.begin(0x4e0);
            b.i32(0x1319, 2000003);
            b.f64(0x131a, 0.42);
            b.end();
            b.end_list();
            b.obj(0x137c);
            b.begin(0x4f3);
            b.list(0x8e0);
            b.end_list();
            b.end();
            b.u8(0x1809, 1);
            b.str(0xce4, "56534558667350736572756D20320000");
            b.str(0xc5e, "2.0.16");
            b.end();
            b.end();
        }),
        Box::new(send_to_fx),
        Box::new(to_master),
    );

    // The VST2 track: Drumazon, with an output chain that sends to the effect track, and its first setting
    // automated. The plugin is first written in the automation lane, which comes first.
    let mut drumazon = 0;
    let vst2 = track_start(b, "", false, |b| {
        b.begin(0x23d);
        b.obj(0x2d8c);
        b.begin(0xd91);
        b.list(0x21f);
        b.begin(0xd90);
        b.f64(0x2af, 0.0);
        b.f64(0x26, 4.0);
        b.end();
        b.end_list();
        b.obj(0x35ac);
        b.begin(0x6e);
        b.list(0x21f);
        b.begin(0x108);
        b.f64(0x2af, 4.0);
        b.f64(0x28f, 0.92);
        b.u8(0x359e, 1);
        b.f64(0xba6, 0.0);
        b.end();
        b.end_list();
        b.end();
        b.end();
        b.obj(0x2a3);
        b.begin(0x65);
        b.str(0x133b, "Drumazon");
        b.str(0x133a, "Bass Drum Level");
        b.obj(0xec);
        drumazon = drumazon_device(b, t.device_power);
        b.str(0xed, "CONTENTS/ROOT_GENERIC_MODULE/PARAM0");
        b.obj(0xe92);
        b.begin(0x7b);
        b.f64(0x124, 0.0);
        b.f64(0x125, 1.0);
        b.end();
        b.end();
        b.obj(0x2aec);
        b.begin(0xe6c);
        b.list(0x10e6);
        b.end_list();
        b.end();
        b.end();
    });
    track_end(
        b,
        &mut n,
        vst2 + 4,
        None,
        Box::new(move |b, _| b.list_ref(drumazon)),
        Box::new(send_to_fx),
        Box::new(to_master),
    );
    b.end_list();
    b.list(0x4df);
    b.list_ref(n.fx);
    b.end_list();
    b.bare_ref(0x4e0, n.master);
    // The arrangement's clip selection points at the template's clip.
    b.obj(0x2bfb);
    b.begin(0xef9);
    b.obj(0x2d8b);
    b.begin(0x60a);
    b.list(0x21f);
    b.begin(0x60b);
    b.f64(0x2af, 0.0);
    b.f64(0x26, 4.0);
    b.list(0x3f8b);
    b.list_ref(n.clip);
    b.end_list();
    b.end();
    b.end_list();
    b.end();
    b.end();
    b.end(); // project group
    // The project's per-user view state: the selected track.
    b.map(0x4d);
    b.map_key("user");
    b.begin(0xe4);
    b.obj(0xee6);
    b.begin(0x325);
    b.obj(0xb5e);
    b.begin(0x29e);
    b.bare_ref(0xb5d, first_track);
    b.end();
    b.end();
    b.end();
    b.end_map();
    b.end(); // root
}

fn drumazon_device(b: &mut BwWriter, device_power: bool) -> u32 {
    let d = b.begin(0x1a6);
    b.id(0xf);
    b.obj(0x197);
    b.begin(0x1a5);
    b.str(0x12de, "");
    b.str(0x9a, "Drumazon");
    b.str(0x9b, "D16 Group Audio Software");
    b.str(0x9c, "Synth");
    b.u8(0x9d, 2);
    if device_power {
        b.bool(0xa3, true);
    }
    b.obj(0x1b75);
    b.begin(0x7b1);
    b.str(0x1b72, "Bass Drum");
    b.end();
    b.obj(0xc5a);
    b.begin(0x2b4);
    b.list(0xc5b);
    file_ref(b, r"C:\Program Files\Vstplugins\Drumazon.dll");
    b.end_list();
    b.end();
    b.str(0xbe5, "drumazon.fxb");
    b.list(0x1318);
    b.end_list();
    b.obj(0x137c);
    b.begin(0x4f3);
    b.list(0x8e0);
    b.begin(0x5a0);
    b.str(0x2b9, "CHAIN0");
    b.id(0x2ab8);
    b.obj(0x825);
    b.begin(0x23a);
    b.str(0x2b9, "MIXER");
    b.list(0xbd2);
    b.begin(0x732);
    b.str(0x2b9, "TRACK_SEND(96d49fae-f171-4587-b7dd-1202ce2a9915)");
    b.end();
    b.end_list();
    b.end();
    b.end();
    b.end_list();
    b.end();
    b.u8(0x1809, 0);
    b.i32(0xce4, 1144278322);
    b.i32(0xc5e, 67074);
    b.end();
    b.end();
    d
}
