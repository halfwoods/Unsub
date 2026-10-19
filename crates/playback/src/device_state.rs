//! Leaves the output device as playback found it.
//!
//! cpal sets the hardware's stream format (rate and bit depth) to match the stream it opens and
//! never sets it back, so every other app on the Mac would stay on whatever the last track used.

pub use imp::Saved;

#[cfg(target_os = "macos")]
mod imp {
    use std::mem::{self, MaybeUninit};
    use std::ptr::{NonNull, null};

    use coreaudio::audio_unit::macos_helpers::{
        get_default_device_id, set_device_physical_stream_format, set_device_sample_rate,
    };
    use objc2_core_audio::{
        AudioDeviceID, AudioObjectGetPropertyData, AudioObjectPropertyAddress,
        kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyElementMain,
        kAudioObjectPropertyScopeGlobal, kAudioStreamPropertyPhysicalFormat,
    };
    use objc2_core_audio_types::AudioStreamBasicDescription;

    /// The default output device's hardware format at some earlier moment.
    pub struct Saved {
        device: AudioDeviceID,
        format: AudioStreamBasicDescription,
        nominal_rate: f64,
    }

    impl Saved {
        /// Records the default output device as it is now; `None` if it cannot be read.
        pub fn capture() -> Option<Saved> {
            let device = get_default_device_id(false)?;
            Some(Saved {
                device,
                format: read(device, kAudioStreamPropertyPhysicalFormat)?,
                nominal_rate: read(device, kAudioDevicePropertyNominalSampleRate)?,
            })
        }

        /// Puts the device back. Best effort: it may have been unplugged since, or may not allow
        /// its format to be set, and either way there is nothing more to do.
        pub fn restore(self) {
            let _ = set_device_physical_stream_format(self.device, self.format);
            let _ = set_device_sample_rate(self.device, self.nominal_rate);
        }

        #[cfg(test)]
        fn is_plausible(&self) -> bool {
            self.nominal_rate >= 8_000.0
                && self.format.mSampleRate >= 8_000.0
                && self.format.mChannelsPerFrame > 0
                && self.format.mBitsPerChannel > 0
        }
    }

    /// Reads a plain-data property of `device`.
    fn read<T: Copy>(device: AudioDeviceID, selector: u32) -> Option<T> {
        let address = AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        let mut size = mem::size_of::<T>() as u32;
        let mut value = MaybeUninit::<T>::zeroed();
        // SAFETY: `value` has room for `size` bytes, and `T` is only ever `f64` or
        // `AudioStreamBasicDescription`, plain-data types for which all-zero bytes are valid.
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                NonNull::from(&address),
                0,
                null(),
                NonNull::from(&mut size),
                NonNull::from(&mut value).cast(),
            )
        };
        // SAFETY: on success CoreAudio has filled the value; on failure we do not read it.
        (status == 0).then(|| unsafe { value.assume_init() })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Only reads device properties; it never opens a stream or changes the device.
        #[test]
        fn the_default_output_device_reads_back_a_plausible_format() {
            if let Some(saved) = Saved::capture() {
                assert!(saved.is_plausible());
            }
        }
    }
}

/// Other platforms leave the device's format alone, so there is never anything to restore.
#[cfg(not(target_os = "macos"))]
mod imp {
    pub enum Saved {}

    impl Saved {
        pub fn capture() -> Option<Saved> {
            None
        }

        pub fn restore(self) {
            match self {}
        }
    }
}
