//! Bitwig's own devices by the IDs Bitwig keeps them under: the same in every project and on every computer
//! (`tools/formatlab/registry/bitwig.json`, field 0x99), and what the link's `insert` takes to add one.

use std::fmt;

/// One of Bitwig's own devices, by its ID: a UUID, written `8ea97e45-0255-40fd-bc7e-94419741e9d1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId([u8; 16]);

impl DeviceId {
    /// Bitwig's Drum Machine, `8ea97e45-0255-40fd-bc7e-94419741e9d1`: the ID DrivenByMoss (the Bitwig extension for
    /// most hardware controllers) finds Drum Machines by. The C# app didn't know it, so it read it from Bitwig files
    /// that have one; where it kept what it found (the setting [`DRUM_MACHINE_KEY`]), [`drum_machine`] takes that.
    pub const DRUM_MACHINE: DeviceId =
        DeviceId([0x8e, 0xa9, 0x7e, 0x45, 0x02, 0x55, 0x40, 0xfd, 0xbc, 0x7e, 0x94, 0x41, 0x97, 0x41, 0xe9, 0xd1]);
    /// Bitwig's Sampler, `468bc14b-b2e7-45a1-9666-e83117fe404e` (proved on Solomon's saves; the registry has it).
    pub const SAMPLER: DeviceId =
        DeviceId([0x46, 0x8b, 0xc1, 0x4b, 0xb2, 0xe7, 0x45, 0xa1, 0x96, 0x66, 0xe8, 0x31, 0x17, 0xfe, 0x40, 0x4e]);
    /// Bitwig's Note Transpose, `0815cd9e-3a31-4429-a268-dabd952a3b68` (likewise).
    pub const NOTE_TRANSPOSE: DeviceId =
        DeviceId([0x08, 0x15, 0xcd, 0x9e, 0x3a, 0x31, 0x44, 0x29, 0xa2, 0x68, 0xda, 0xbd, 0x95, 0x2a, 0x3b, 0x68]);

    /// The ID as the 16 bytes a Bitwig file keeps it as (in the order it's written).
    pub const fn from_bytes(bytes: [u8; 16]) -> DeviceId {
        DeviceId(bytes)
    }

    pub const fn bytes(&self) -> [u8; 16] {
        self.0
    }

    /// Reads an ID written the ways .NET writes a Guid: `8ea97e45-0255-40fd-bc7e-94419741e9d1`, the same in braces,
    /// or the 32 digits alone, in capitals or not. None when it isn't one (an empty setting, say).
    pub fn parse(text: &str) -> Option<DeviceId> {
        let t = text.trim();
        let t = t.strip_prefix('{').and_then(|t| t.strip_suffix('}')).unwrap_or(t);
        let digits: Vec<u8> = match t.len() {
            32 => t.bytes().collect(),
            36 => {
                let b = t.as_bytes();
                if [8, 13, 18, 23].iter().any(|&i| b.get(i) != Some(&b'-')) {
                    return None;
                }
                t.bytes().filter(|&c| c != b'-').collect()
            }
            _ => return None,
        };
        // from_str_radix would also take a "+" before a digit.
        if digits.len() != 32 || !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks(2)) {
            let hex = std::str::from_utf8(pair).ok()?;
            *byte = u8::from_str_radix(hex, 16).ok()?;
        }
        Some(DeviceId(bytes))
    }
}

impl fmt::Display for DeviceId {
    /// `8ea97e45-0255-40fd-bc7e-94419741e9d1`, as the link and .NET write it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// The setting in `library.db` where the C# app keeps the Drum Machine's ID it read from Bitwig's files (its
/// `SoundsViewModel.DrumMachineKey`); "" once it found that ID didn't give a Drum Machine.
pub const DRUM_MACHINE_KEY: &str = "bitwig_drum_machine";

/// Bitwig's Drum Machine for a kit: the ID the C# app read from Bitwig's own files on this computer, when it kept one
/// (`stored`, the setting [`DRUM_MACHINE_KEY`]), else [`DeviceId::DRUM_MACHINE`].
pub fn drum_machine(stored: Option<&str>) -> DeviceId {
    stored.and_then(DeviceId::parse).unwrap_or(DeviceId::DRUM_MACHINE)
}

/// The name Bitwig gives its Drum Machine, which the app checks the new device has: an ID read wrongly adds some
/// other device.
pub const DRUM_MACHINE_NAME: &str = "Drum Machine";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ids_are_the_format_registrys() {
        // What's known about Bitwig's files is kept in the format registry (CLAUDE.md): field 0x99 lists these.
        let registry: serde_json::Value =
            serde_json::from_str(include_str!("../../../../tools/formatlab/registry/bitwig.json")).unwrap();
        let field = registry["fields"].as_array().unwrap().iter().find(|f| f["id"] == "0x99").unwrap();
        let about = field["about"].as_str().unwrap();
        for id in [DeviceId::DRUM_MACHINE, DeviceId::SAMPLER, DeviceId::NOTE_TRANSPOSE] {
            assert!(about.contains(&id.to_string()), "{id} isn't in the registry's field 0x99");
        }
    }
}
