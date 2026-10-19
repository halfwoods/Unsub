//! AAC and Apple Lossless through AudioToolbox's `ExtAudioFile`, which also writes the M4A
//! container. Apple's encoders are the reference ones for both formats.

use std::{ffi::c_void, mem::size_of, path::Path, ptr, ptr::NonNull};

use objc2_audio_toolbox::{
    AudioConverterRef, AudioConverterSetProperty, AudioFileFlags, AudioFileID,
    AudioFileSetProperty, ExtAudioFileCreateWithURL, ExtAudioFileDispose, ExtAudioFileGetProperty,
    ExtAudioFileRef, ExtAudioFileSetProperty, ExtAudioFileWrite, kAudioConverterEncodeBitRate,
    kAudioFileM4AType, kAudioFilePropertyReserveDuration, kExtAudioFileProperty_AudioConverter,
    kExtAudioFileProperty_AudioFile, kExtAudioFileProperty_ClientDataFormat,
    kExtAudioFileProperty_ConverterConfig,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription,
    kAppleLosslessFormatFlag_16BitSourceData, kAudioFormatAppleLossless, kAudioFormatFlagIsPacked,
    kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC,
};
use objc2_core_foundation::CFURL;

use super::Format;
use crate::{Error, Result, disc::TrackAudio};

const SAMPLE_RATE: f64 = 44_100.0;
const CHANNELS: u32 = 2;
const CHUNK_FRAMES: usize = 4096;
/// AAC-LC at 44.1 kHz stereo. Bitrates outside this are refused up front instead of leaving it to
/// the encoder to pick something else.
const AAC_KBPS: std::ops::RangeInclusive<u32> = 32..=320;

pub(super) fn encode(format: Format, audio: &mut dyn TrackAudio, out: &Path) -> Result<()> {
    let (file_format, kbps) = match format {
        Format::Alac => (
            AudioStreamBasicDescription {
                mSampleRate: SAMPLE_RATE,
                mFormatID: kAudioFormatAppleLossless,
                mFormatFlags: kAppleLosslessFormatFlag_16BitSourceData,
                mBytesPerPacket: 0,
                mFramesPerPacket: 0,
                mBytesPerFrame: 0,
                mChannelsPerFrame: CHANNELS,
                mBitsPerChannel: 0,
                mReserved: 0,
            },
            None,
        ),
        Format::Aac { kbps } => {
            if !AAC_KBPS.contains(&kbps) {
                return Err(Error::Encode(format!(
                    "AAC bitrate {kbps} kbps is outside {AAC_KBPS:?}"
                )));
            }
            (
                AudioStreamBasicDescription {
                    mSampleRate: SAMPLE_RATE,
                    mFormatID: kAudioFormatMPEG4AAC,
                    mFormatFlags: 0,
                    mBytesPerPacket: 0,
                    mFramesPerPacket: 0,
                    mBytesPerFrame: 0,
                    mChannelsPerFrame: CHANNELS,
                    mBitsPerChannel: 0,
                    mReserved: 0,
                },
                Some(kbps),
            )
        }
        Format::Flac { .. } => {
            return Err(Error::Encode("FLAC is not an AudioToolbox format".into()));
        }
    };

    let mut file = OutputFile::create(out, file_format)?;
    file.set_client_format()?;
    file.reserve_header_for(audio.frames() as f64 / SAMPLE_RATE)?;
    if let Some(kbps) = kbps {
        file.set_bit_rate(kbps * 1000)?;
    }

    let mut pcm = vec![0i16; CHUNK_FRAMES * 2];
    loop {
        let frames = audio.read(&mut pcm)?;
        if frames == 0 {
            break;
        }
        file.write(&mut pcm[..frames * 2])?;
    }
    file.finish()
}

/// An open `ExtAudioFile`, closed when dropped. Closing is also what finishes the M4A (the encoder
/// flushes and the container's index is written), so `finish` does it explicitly to see the error.
struct OutputFile(ExtAudioFileRef);

impl OutputFile {
    fn create(path: &Path, mut format: AudioStreamBasicDescription) -> Result<Self> {
        let url = CFURL::from_file_path(path)
            .ok_or_else(|| Error::Encode(format!("{} is not a valid path", path.display())))?;
        let mut file: ExtAudioFileRef = ptr::null_mut();
        // SAFETY: `url` is a live CFURL, `format` and `file` outlive the call, a null channel
        // layout is allowed, and the flags are a valid `AudioFileFlags` value.
        let status = unsafe {
            ExtAudioFileCreateWithURL(
                &url,
                kAudioFileM4AType,
                NonNull::from(&mut format),
                ptr::null(),
                AudioFileFlags::EraseFile.bits(),
                NonNull::from(&mut file),
            )
        };
        check(status, "creating the M4A file")?;
        Ok(Self(file))
    }

    /// What `write` is handed: interleaved native-endian signed 16-bit stereo.
    fn set_client_format(&mut self) -> Result<()> {
        let mut client = AudioStreamBasicDescription {
            mSampleRate: SAMPLE_RATE,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: CHANNELS,
            mBitsPerChannel: 16,
            mReserved: 0,
        };
        self.set_property(
            kExtAudioFileProperty_ClientDataFormat,
            &mut client,
            "setting the PCM format",
        )
    }

    /// Tells the M4A writer how much audio is coming, so it reserves only as much header space as
    /// that needs. Without it the file carries tens of kilobytes of padding after the header.
    fn reserve_header_for(&mut self, seconds: f64) -> Result<()> {
        let mut audio_file: AudioFileID = ptr::null_mut();
        let mut size = size_of::<AudioFileID>() as u32;
        // SAFETY: `self.0` is open; `size` and `audio_file` are valid for the sizes given.
        let status = unsafe {
            ExtAudioFileGetProperty(
                self.0,
                kExtAudioFileProperty_AudioFile,
                NonNull::from(&mut size),
                NonNull::from(&mut audio_file).cast::<c_void>(),
            )
        };
        check(status, "getting the file")?;
        let mut seconds = seconds;
        // SAFETY: `audio_file` belongs to the open file and `seconds` is a valid f64.
        let status = unsafe {
            AudioFileSetProperty(
                audio_file,
                kAudioFilePropertyReserveDuration,
                size_of::<f64>() as u32,
                NonNull::from(&mut seconds).cast::<c_void>(),
            )
        };
        check(status, "reserving header space")
    }

    fn set_bit_rate(&mut self, bits_per_second: u32) -> Result<()> {
        let mut converter: AudioConverterRef = ptr::null_mut();
        let mut size = size_of::<AudioConverterRef>() as u32;
        // SAFETY: `self.0` is open; `size` and `converter` are valid for the sizes given.
        let status = unsafe {
            ExtAudioFileGetProperty(
                self.0,
                kExtAudioFileProperty_AudioConverter,
                NonNull::from(&mut size),
                NonNull::from(&mut converter).cast::<c_void>(),
            )
        };
        check(status, "getting the encoder")?;
        let mut rate = bits_per_second;
        // SAFETY: `converter` was just returned by the open file and `rate` is a valid u32.
        let status = unsafe {
            AudioConverterSetProperty(
                converter,
                kAudioConverterEncodeBitRate,
                size_of::<u32>() as u32,
                NonNull::from(&mut rate).cast::<c_void>(),
            )
        };
        check(status, "setting the AAC bitrate")?;
        // Changing the converter behind ExtAudioFile's back needs this nudge (a null config) so it
        // picks the change up before the first write.
        let mut no_config: *const c_void = ptr::null();
        self.set_property(
            kExtAudioFileProperty_ConverterConfig,
            &mut no_config,
            "applying the encoder settings",
        )
    }

    fn set_property<T>(&mut self, property: u32, value: &mut T, what: &str) -> Result<()> {
        // SAFETY: `self.0` is open and `value` is valid for `size_of::<T>()` bytes; every property
        // set through here takes a plain value of that type.
        let status = unsafe {
            ExtAudioFileSetProperty(
                self.0,
                property,
                size_of::<T>() as u32,
                NonNull::from(value).cast::<c_void>(),
            )
        };
        check(status, what)
    }

    fn write(&mut self, interleaved: &mut [i16]) -> Result<()> {
        let frames = (interleaved.len() / 2) as u32;
        let mut list = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [AudioBuffer {
                mNumberChannels: CHANNELS,
                mDataByteSize: frames * 4,
                mData: interleaved.as_mut_ptr().cast::<c_void>(),
            }],
        };
        // SAFETY: `self.0` is open, and `list` describes `interleaved`, which is `frames * 4`
        // bytes long and outlives the call.
        let status = unsafe { ExtAudioFileWrite(self.0, frames, NonNull::from(&mut list)) };
        check(status, "encoding audio")
    }

    fn finish(mut self) -> Result<()> {
        let file = std::mem::replace(&mut self.0, ptr::null_mut());
        // SAFETY: `file` is the open handle; `self.0` is null now so Drop will not close it again.
        let status = unsafe { ExtAudioFileDispose(file) };
        check(status, "finishing the M4A file")
    }
}

impl Drop for OutputFile {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle is still open (`finish` nulls it when it closes the file).
            let _ = unsafe { ExtAudioFileDispose(self.0) };
        }
    }
}

fn check(status: i32, what: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::Encode(format!(
            "AudioToolbox failed {what} (OSStatus {status})"
        )))
    }
}
