//! Finding audio CDs and reading their table of contents and audio.

pub(crate) mod aiff;
pub(crate) mod plist;
mod reader;
mod volume;

use std::path::PathBuf;

pub use reader::{DiscReader, TrackAudio};
pub use volume::VolumeReader;

use crate::{
    Error, Result,
    toc::{FRAMES_PER_SECTOR, SECTORS_PER_SECOND, Toc},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disc {
    /// The MusicBrainz DiscID.
    pub id: String,
    /// The volume's name, e.g. "Audio CD", or the disc title when the drive found CD-TEXT.
    pub name: String,
    pub toc: Toc,
    /// The audio tracks, in order.
    pub tracks: Vec<DiscTrack>,
    pub mount: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscTrack {
    pub number: u8,
    /// First sector, counting the 150 sector lead-in like `Toc::offsets`.
    pub start_sector: u32,
    pub sectors: u32,
    /// The track's audio file on the mounted volume.
    pub path: PathBuf,
}

impl DiscTrack {
    pub fn frames(&self) -> u64 {
        u64::from(self.sectors) * u64::from(FRAMES_PER_SECTOR)
    }

    pub fn duration_seconds(&self) -> f64 {
        f64::from(self.sectors) / f64::from(SECTORS_PER_SECOND)
    }
}

/// The audio CDs inserted right now. Empty where the platform has no reader.
pub fn discs() -> Vec<Disc> {
    system_reader().discs().unwrap_or_default()
}

/// The reader for this platform's drives.
pub fn system_reader() -> Box<dyn DiscReader> {
    #[cfg(target_os = "macos")]
    {
        Box::new(VolumeReader::volumes())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(Unsupported)
    }
}

/// Asks the system to eject the disc.
pub fn eject(disc: &Disc) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/sbin/diskutil")
            .arg("eject")
            .arg(&disc.mount)
            .status()
            .map_err(|err| Error::io(&disc.mount, err))?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Disc(format!(
                "diskutil could not eject {}: {status}",
                disc.mount.display()
            )))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = disc;
        Err(Error::Unsupported("ejecting discs"))
    }
}

/// The reader for platforms that cannot read audio CDs yet.
#[cfg(not(target_os = "macos"))]
struct Unsupported;

#[cfg(not(target_os = "macos"))]
impl DiscReader for Unsupported {
    fn discs(&self) -> Result<Vec<Disc>> {
        Err(Error::Unsupported("reading audio CDs"))
    }

    fn open_track(&self, _: &Disc, _: &DiscTrack) -> Result<Box<dyn TrackAudio>> {
        Err(Error::Unsupported("reading audio CDs"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(sectors: u32) -> DiscTrack {
        DiscTrack {
            number: 1,
            start_sector: 150,
            sectors,
            path: PathBuf::new(),
        }
    }

    #[test]
    fn a_tracks_length_follows_from_its_sectors() {
        let track = track(75 * 60);
        assert_eq!(track.frames(), 75 * 60 * 588);
        assert_eq!(track.frames(), 44_100 * 60);
        assert_eq!(track.duration_seconds(), 60.0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ejecting_something_that_is_not_a_disc_fails_cleanly() {
        let disc = Disc {
            id: String::new(),
            name: String::new(),
            toc: Toc {
                first_track: 1,
                last_track: 1,
                lead_out: 1_000,
                offsets: vec![150],
            },
            tracks: vec![track(850)],
            mount: PathBuf::from("/Volumes/definitely-not-a-mounted-disc"),
        };
        assert!(matches!(eject(&disc), Err(Error::Disc(_))));
    }
}
