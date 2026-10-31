//! Made-up files the tests share (never Solomon's: rule 4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, dead_code)]

use std::io::Write;

/// Bitwig's object stream, enough for a preset: big-endian, typed fields (as mo-formats' test writer).
struct Bw(Vec<u8>);

impl Bw {
    fn u32(&mut self, v: u32) {
        self.0.extend(v.to_be_bytes());
    }
    fn text(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend(s.as_bytes());
    }
    fn str_field(&mut self, field: u32, s: &str) {
        self.u32(field);
        self.0.push(0x08);
        self.text(s);
    }
}

/// A .bwpreset: metadata, one device object, and a zip holding `state` as `plugin-states/<entry>`.
pub fn bwpreset(
    meta: &[(&str, &str)],
    tags: &[&str],
    device: &str,
    preset: &str,
    entry: &str,
    state: &[u8],
) -> Vec<u8> {
    bwpreset_field(meta, tags, device, preset, entry, entry, state)
}

/// [`bwpreset`], with the device's state field written as `field` rather than the zip file's name.
pub fn bwpreset_field(
    meta: &[(&str, &str)],
    tags: &[&str],
    device: &str,
    preset: &str,
    field: &str,
    entry: &str,
    state: &[u8],
) -> Vec<u8> {
    bwpreset_full(meta, tags, device, preset, field, entry, state, &[])
}

/// [`bwpreset_field`], with more files in the zip after the plugin's settings (a preview picture...).
#[allow(clippy::too_many_arguments)]
pub fn bwpreset_full(
    meta: &[(&str, &str)],
    tags: &[&str],
    device: &str,
    preset: &str,
    field: &str,
    entry: &str,
    state: &[u8],
    extra: &[(&str, &[u8])],
) -> Vec<u8> {
    let mut m = Bw(Vec::new());
    m.u32(4);
    m.text("meta");
    for (k, v) in meta {
        m.u32(1);
        m.text(k);
        m.0.push(0x08);
        m.text(v);
    }
    if !tags.is_empty() {
        m.u32(1);
        m.text("tags");
        m.0.push(0x19);
        m.u32(tags.len() as u32);
        for t in tags {
            m.text(t);
        }
    }
    m.u32(0);
    let mut b = Bw(Vec::new());
    b.u32(0x728); // a VST3 plugin device
    b.str_field(0x9a, device);
    b.str_field(0x12de, preset);
    if !field.is_empty() {
        b.str_field(0xbe5, field);
    }
    b.u32(0);

    let mut zip_bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_bytes));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file(format!("plugin-states/{entry}"), options).unwrap();
        zip.write_all(state).unwrap();
        for (name, data) in extra {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }
    let body_offset = 42 + m.0.len() + 8;
    let zip_offset = body_offset + b.0.len();
    let mut out = format!("BtWg000300020000{body_offset:08x}{zip_offset:016x}00").into_bytes();
    out.extend(&m.0);
    out.extend(b"       \n");
    out.extend(&b.0);
    out.extend(&zip_bytes);
    out
}
