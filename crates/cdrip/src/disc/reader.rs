use super::{Disc, DiscTrack};
use crate::Result;

/// Where a platform's audio CDs are found and how their audio is read. macOS reads the files
/// cddafs exposes (`VolumeReader`); a Windows reader would issue `IOCTL_CDROM_RAW_READ` instead.
pub trait DiscReader: Send + Sync {
    /// The audio CDs currently inserted.
    fn discs(&self) -> Result<Vec<Disc>>;

    /// Opens one track's audio for sequential reading.
    fn open_track(&self, disc: &Disc, track: &DiscTrack) -> Result<Box<dyn TrackAudio>>;
}

/// A track's audio as 16-bit stereo at 44.1 kHz, read from start to end.
pub trait TrackAudio {
    /// Stereo frames in the track.
    fn frames(&self) -> u64;

    /// Fills `out` with up to `out.len() / 2` interleaved left/right frames and returns how many it
    /// wrote; 0 means the end of the track.
    fn read(&mut self, out: &mut [i16]) -> Result<usize>;
}
