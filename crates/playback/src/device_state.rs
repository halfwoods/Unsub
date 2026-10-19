//! Leaves the macOS output device as playback found it, and takes exclusive access to it.
//!
//! cpal sets the hardware's stream format (rate and bit depth) to match the stream it opens and
//! never sets it back, so every other app on the Mac would stay on whatever the last track used.
//! Bit-perfect playback additionally takes hog mode, which shuts every other app out of the
//! device until it is released.

use std::ffi::c_void;
use std::mem::{self, MaybeUninit};
use std::ptr::{NonNull, null};
use std::sync::Once;

use coreaudio::audio_unit::macos_helpers::{
    get_default_device_id, get_hogging_pid, set_device_physical_stream_format,
    set_device_sample_rate, toggle_hog_mode,
};
use objc2_core_audio::{
    AudioDeviceID, AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectSetPropertyData, kAudioDevicePropertyNominalSampleRate,
    kAudioHardwarePropertyRunLoop, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioStreamPropertyPhysicalFormat,
};
use objc2_core_audio_types::AudioStreamBasicDescription;

use crate::host::DeviceState;

/// `kAudioFormatFlagIsFloat`.
const IS_FLOAT: u32 = 1 << 0;

/// The default output device's hardware format at some earlier moment.
pub struct Saved {
    device: AudioDeviceID,
    format: AudioStreamBasicDescription,
    nominal_rate: f64,
    hogged: bool,
}

/// The hardware stream format the device is running right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicalFormat {
    pub sample_rate: u32,
    pub bits: u32,
    pub float: bool,
}

impl Saved {
    /// Records the default output device as it is now; `None` if it cannot be read.
    pub fn capture() -> Option<Saved> {
        own_notification_thread();
        let device = get_default_device_id(false).filter(|&device| device != 0)?;
        Some(Saved {
            device,
            format: read(device, kAudioStreamPropertyPhysicalFormat)?,
            nominal_rate: read(device, kAudioDevicePropertyNominalSampleRate)?,
            hogged: false,
        })
    }

    pub fn device(&self) -> AudioDeviceID {
        self.device
    }

    #[cfg(test)]
    fn is_plausible(&self) -> bool {
        self.nominal_rate >= 8_000.0
            && self.format.mSampleRate >= 8_000.0
            && self.format.mChannelsPerFrame > 0
            && self.format.mBitsPerChannel > 0
    }
}

impl DeviceState for Saved {
    fn take_exclusive(&mut self) -> bool {
        if self.hogged {
            return true;
        }
        // Another process holding the device is never overridden: `toggle_hog_mode` would
        // simply leave it theirs, and playing on regardless would not be exclusive.
        if !matches!(get_hogging_pid(self.device), Ok(-1)) {
            return false;
        }
        self.hogged = toggle_hog_mode(self.device).is_ok_and(|owner| owner == own_pid());
        self.hogged
    }

    fn release_exclusive(&mut self) {
        if self.hogged && toggle_hog_mode(self.device).is_ok_and(|owner| owner == -1) {
            self.hogged = false;
        }
    }

    /// Puts the device back. Best effort: it may have been unplugged since, or may not allow
    /// its format to be set, and either way there is nothing more to do.
    fn restore(mut self: Box<Self>) {
        let _ = set_device_physical_stream_format(self.device, self.format);
        let _ = set_device_sample_rate(self.device, self.nominal_rate);
        // Last, so no other app can grab the device while its format is still ours.
        self.release_exclusive();
    }
}

/// The hardware format `device` is running, or `None` if it cannot be read.
pub fn physical_format(device: AudioDeviceID) -> Option<PhysicalFormat> {
    let format: AudioStreamBasicDescription = read(device, kAudioStreamPropertyPhysicalFormat)?;
    Some(PhysicalFormat {
        sample_rate: format.mSampleRate.round() as u32,
        bits: format.mBitsPerChannel,
        float: format.mFormatFlags & IS_FLOAT != 0,
    })
}

/// Has CoreAudio deliver its notifications on a thread of its own instead of a run loop.
///
/// Without this, a process that has taken hog mode keeps seeing "no default output device" long
/// after releasing it, because the cached answer is only refreshed by notifications that nothing
/// is running a loop to receive. It is process-wide, but is Apple's documented setting for code
/// that cannot promise a run loop, and cpal's own listeners only forward to channels.
fn own_notification_thread() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let address = AudioObjectPropertyAddress {
            mSelector: kAudioHardwarePropertyRunLoop,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        let run_loop: *const c_void = null();
        // SAFETY: the property takes one run loop pointer, and null means "no run loop".
        // Failure only leaves the default behaviour in place.
        let _ = unsafe {
            AudioObjectSetPropertyData(
                kAudioObjectSystemObject as AudioObjectID,
                NonNull::from(&address),
                0,
                null(),
                mem::size_of::<*const c_void>() as u32,
                NonNull::from(&run_loop).cast(),
            )
        };
    });
}

fn own_pid() -> i32 {
    // Process ids fit i32 on macOS, which is what hog mode compares against.
    std::process::id() as i32
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
        if let Some(saved) = Saved::capture()
            && let Some(format) = physical_format(saved.device())
        {
            assert!(format.sample_rate >= 8_000 && format.bits > 0);
        }
    }
}
