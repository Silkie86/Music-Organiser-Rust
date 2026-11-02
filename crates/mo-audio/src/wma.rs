//! WMA (Windows Media Audio), decoded by Windows itself through Media Foundation, as the C# app's Windows decoders
//! did. There's no WMA decoder written in Rust, so on a Mac or Linux these files can't be read (see
//! [`crate::can_decode`]).
//!
//! Media Foundation is reached through COM, which Rust can only call through `unsafe` code: this file is the one place
//! in mo-audio that does. Each call is the documented Media Foundation one, and every buffer Windows lends is copied
//! out before it's given back.

/// The kinds Windows decodes here, by extension.
pub const WMA_EXTENSIONS: [&str; 1] = ["wma"];

/// Whether this file is a [`WMA_EXTENSIONS`] one (ignoring case).
pub fn is_wma(path: &std::path::Path) -> bool {
    crate::read::has_extension(path, &WMA_EXTENSIONS)
}

#[cfg(windows)]
pub use windows_mf::read_wma;

/// On a Mac or Linux: no WMA decoder.
#[cfg(not(windows))]
pub fn read_wma(_path: &std::path::Path, _max_seconds: f64) -> Result<crate::AudioData, crate::AudioError> {
    Err(crate::AudioError::NotSupported("WMA files can only be played on Windows".into()))
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_mf {
    use std::path::Path;

    use windows::Win32::Media::MediaFoundation::{
        IMFMediaType, IMFSample, IMFSourceReader, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
        MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_PD_DURATION, MF_SOURCE_READER_ALL_STREAMS,
        MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_MEDIASOURCE, MF_SOURCE_READERF_ENDOFSTREAM,
        MF_SOURCE_READERF_ERROR, MF_VERSION, MFAudioFormat_Float, MFCreateMediaType, MFCreateSourceReaderFromURL,
        MFMediaType_Audio, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup,
    };
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::core::HSTRING;

    use crate::{AudioData, AudioError};

    /// COM and Media Foundation started on this thread for one read, and stopped again when it ends (each start is
    /// counted, so reads on several threads at once are fine).
    struct Started {
        com: bool,
    }

    impl Started {
        fn new() -> Result<Started, AudioError> {
            // SAFETY: plain start-up calls with no pointers. A thread that already runs COM another way (the window's)
            // answers "changed mode": COM is still on there, so carry on without stopping it afterwards.
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            // SAFETY: as above; balanced by MFShutdown in drop.
            if let Err(e) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) } {
                if com {
                    // SAFETY: balances the CoInitializeEx that succeeded just above.
                    unsafe { CoUninitialize() };
                }
                return Err(AudioError::NotSupported(format!("Windows' Media Foundation isn't available: {e}")));
            }
            Ok(Started { com })
        }
    }

    impl Drop for Started {
        fn drop(&mut self) {
            // SAFETY: balances the MFStartup and CoInitializeEx in `new`; every COM object of the read has been dropped
            // by now (they're made after this and dropped before it).
            unsafe {
                let _ = MFShutdown();
                if self.com {
                    CoUninitialize();
                }
            }
        }
    }

    fn format(e: windows::core::Error) -> AudioError {
        AudioError::Format(format!("Windows couldn't decode this file: {}", e.message()))
    }

    /// Up to `max_seconds` from the start of a WMA file, as 32-bit float samples at the file's own rate.
    pub fn read_wma(path: &Path, max_seconds: f64) -> Result<AudioData, AudioError> {
        // A missing or locked file says so plainly, before Windows gets a say.
        std::fs::File::open(path)?;
        let _started = Started::new()?;
        decode(path, max_seconds)
    }

    fn decode(path: &Path, max_seconds: f64) -> Result<AudioData, AudioError> {
        let audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        // SAFETY: Media Foundation calls on objects it made, each with the arguments its documentation gives. The
        // reader and media types are reference-counted COM objects that the windows crate releases when dropped.
        let reader: IMFSourceReader = unsafe {
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None).map_err(format)?;
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false).map_err(format)?;
            reader.SetStreamSelection(audio, true).map_err(format)?;
            let wanted: IMFMediaType = MFCreateMediaType().map_err(format)?;
            wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(format)?;
            wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).map_err(format)?;
            reader.SetCurrentMediaType(audio, None, &wanted).map_err(format)?;
            reader
        };
        // SAFETY: as above.
        let (count, rate) = unsafe {
            let got = reader.GetCurrentMediaType(audio).map_err(format)?;
            (
                got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).map_err(format)?,
                got.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).map_err(format)?,
            )
        };
        if count == 0 || rate == 0 {
            return Err(AudioError::Format("The file has no audio in it".into()));
        }
        let count = count as usize;
        // The whole length, in 100-nanosecond steps, when the file says.
        // SAFETY: MF_PD_DURATION is a whole number (VT_UI8) when present; anything else is left out.
        let length = unsafe {
            reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION).ok().and_then(|v| {
                let inner = &v.Anonymous.Anonymous;
                (inner.vt.0 == 21).then(|| inner.Anonymous.uhVal)
            })
        };

        // `as` saturates: NaN and anything below zero give 0, infinity gives every frame. Never more than one read may
        // hold, as the WAV reader.
        let most = usize::try_from(crate::read::MOST_BYTES).unwrap_or(usize::MAX) / (4 * count);
        let wanted = ((max_seconds * f64::from(rate)) as usize).min(most);
        let mut channels: Vec<Vec<f32>> = (0..count).map(|_| Vec::new()).collect();
        let mut whole = true;
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            // SAFETY: the out-pointers point at the locals above, which outlive the call.
            let read = unsafe { reader.ReadSample(audio, 0, None, Some(&mut flags), None, Some(&mut sample)) };
            if let Err(e) = read {
                // A damaged stretch part way in: keep what came before it.
                if channels[0].is_empty() {
                    return Err(format(e));
                }
                break;
            }
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 && channels[0].is_empty() {
                return Err(AudioError::Format("Windows couldn't decode this file".into()));
            }
            if let Some(sample) = sample {
                // SAFETY: the buffer stays locked only while its bytes are copied out, and is unlocked straight after;
                // Windows says how many bytes it holds, and only whole float frames of them are read.
                unsafe {
                    let buffer = sample.ConvertToContiguousBuffer().map_err(format)?;
                    let mut data = std::ptr::null_mut();
                    let mut len = 0u32;
                    buffer.Lock(&mut data, None, Some(&mut len)).map_err(format)?;
                    if !data.is_null() {
                        let frames = len as usize / (4 * count);
                        let bytes = std::slice::from_raw_parts(data.cast_const(), frames * 4 * count);
                        let room = wanted.saturating_sub(channels[0].len());
                        for frame in bytes.chunks_exact(4 * count).take(room) {
                            for (ch, s) in channels.iter_mut().zip(frame.as_chunks::<4>().0) {
                                ch.push(f32::from_le_bytes(*s));
                            }
                        }
                    }
                    buffer.Unlock().map_err(format)?;
                }
            }
            if channels[0].len() >= wanted {
                whole = false;
                break;
            }
            if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 | MF_SOURCE_READERF_ERROR.0) as u32 != 0 {
                break;
            }
        }
        if channels[0].is_empty() {
            return Err(AudioError::Format("The file has no audio in it".into()));
        }
        let decoded = channels[0].len() as f64 / f64::from(rate);
        let told = length.map(|l| l as f64 / 1e7);
        let file_seconds = if whole { decoded } else { told.unwrap_or(decoded).max(decoded) };
        Ok(AudioData { sample_rate: rate, file_seconds, channels, ..AudioData::default() })
    }
}
