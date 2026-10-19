use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{Disc, DiscReader, DiscTrack, TrackAudio, aiff, plist};
use crate::{Error, Result, musicbrainz_disc_id};

/// Reads audio CDs mounted by macOS's cddafs: a volume holding a hidden `.TOC.plist` and one
/// `<n> <name>.aiff` file per audio track. Nothing in it is macOS specific, so a folder laid out the
/// same way works anywhere, which is how the tests stand in for a drive.
pub struct VolumeReader {
    /// Directory whose subdirectories are candidate volumes.
    volumes: PathBuf,
}

impl VolumeReader {
    /// Where macOS mounts removable media.
    pub fn volumes() -> Self {
        Self::new("/Volumes")
    }

    pub fn new(volumes: impl Into<PathBuf>) -> Self {
        Self {
            volumes: volumes.into(),
        }
    }

    /// Reads one mounted volume, such as `/Volumes/Audio CD`.
    pub fn open(mount: &Path) -> Result<Disc> {
        let parsed = plist::read(&mount.join(".TOC.plist"))?;
        let files = track_files(mount)?;
        let toc = parsed.toc;

        let mut tracks = Vec::new();
        // Each track runs to the next one's start; the last runs to the lead-out.
        let ends = toc.boundaries().skip(1);
        for (((number, &start), end), is_data) in (toc.first_track..)
            .zip(&toc.offsets)
            .zip(ends)
            .zip(&parsed.is_data)
        {
            if *is_data {
                continue;
            }
            let path = files
                .iter()
                .find(|(n, _)| *n == number)
                .map(|(_, path)| path.clone())
                .ok_or_else(|| {
                    Error::Disc(format!(
                        "{} has no audio file for track {number}",
                        mount.display()
                    ))
                })?;
            tracks.push(DiscTrack {
                number,
                start_sector: start,
                sectors: end - start,
                path,
            });
        }
        if tracks.is_empty() {
            return Err(Error::Disc(format!(
                "{} has no audio tracks",
                mount.display()
            )));
        }
        Ok(Disc {
            id: musicbrainz_disc_id(&toc),
            name: mount
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            toc,
            tracks,
            mount: mount.to_owned(),
        })
    }
}

impl DiscReader for VolumeReader {
    fn discs(&self) -> Result<Vec<Disc>> {
        let entries = match fs::read_dir(&self.volumes) {
            Ok(entries) => entries,
            // No volumes directory means no discs, not a failure.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(Error::io(&self.volumes, err)),
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.join(".TOC.plist").is_file())
            .collect();
        paths.sort();
        // A volume that looks like a CD but cannot be read (still mounting, data CD) is skipped.
        Ok(paths
            .iter()
            .filter_map(|path| Self::open(path).ok())
            .collect())
    }

    fn open_track(&self, _: &Disc, track: &DiscTrack) -> Result<Box<dyn TrackAudio>> {
        Ok(Box::new(aiff::open(&track.path)?))
    }
}

/// The volume's AIFF files by track number, which is the leading number of the file name.
fn track_files(mount: &Path) -> Result<Vec<(u8, PathBuf)>> {
    let entries = fs::read_dir(mount).map_err(|err| Error::io(mount, err))?;
    Ok(entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?;
            let is_aiff = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("aiff"));
            let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
            is_aiff.then_some((digits.parse().ok()?, path))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{
        testing::make_volume,
        toc::vectors::{ALL, GUANO_APES},
    };

    #[test]
    fn opens_a_volume_into_a_disc_with_the_musicbrainz_id() {
        let dir = tempfile::tempdir().unwrap();
        let mount = make_volume(dir.path(), "Audio CD", 1, 258_725, &GUANO_APES, &[]);
        let disc = VolumeReader::open(&mount).unwrap();
        assert_eq!(disc.id, ALL[0].musicbrainz);
        assert_eq!(disc.name, "Audio CD");
        assert_eq!(disc.toc, (ALL[0].toc)());
        assert_eq!(disc.tracks.len(), 15);
        assert_eq!(
            (
                disc.tracks[0].number,
                disc.tracks[0].start_sector,
                disc.tracks[0].sectors
            ),
            (1, 150, 17_360)
        );
        assert_eq!(disc.tracks[14].sectors, 258_725 - 235_590);
        assert!(disc.tracks[3].path.ends_with("4 Audio Track.aiff"));
        assert_eq!(disc.tracks[0].frames(), 17_360 * 588);
    }

    #[test]
    fn data_tracks_have_no_audio_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mount = make_volume(
            dir.path(),
            "Mixed",
            1,
            90_000,
            &[150, 20_000, 60_000],
            &[false, false, true],
        );
        let disc = VolumeReader::open(&mount).unwrap();
        assert_eq!(
            disc.tracks.iter().map(|t| t.number).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(disc.toc.track_count(), 3);
    }

    #[test]
    fn a_missing_audio_file_is_an_error_naming_the_track() {
        let dir = tempfile::tempdir().unwrap();
        let mount = make_volume(dir.path(), "Audio CD", 1, 100_000, &[150, 20_000], &[]);
        fs::remove_file(mount.join("2 Audio Track.aiff")).unwrap();
        let err = VolumeReader::open(&mount).unwrap_err();
        assert!(err.to_string().contains("track 2"), "{err}");
    }

    #[test]
    fn track_files_are_matched_by_leading_number_whatever_the_title() {
        let dir = tempfile::tempdir().unwrap();
        let mount = make_volume(
            dir.path(),
            "Audio CD",
            1,
            100_000,
            &[150, 20_000, 50_000],
            &[],
        );
        fs::rename(
            mount.join("2 Audio Track.aiff"),
            mount.join("2 Come Together.AIFF"),
        )
        .unwrap();
        fs::write(mount.join("notes.txt"), "ignored").unwrap();
        let disc = VolumeReader::open(&mount).unwrap();
        assert!(disc.tracks[1].path.ends_with("2 Come Together.AIFF"));
    }

    #[test]
    fn discs_lists_only_volumes_that_hold_a_toc() {
        let dir = tempfile::tempdir().unwrap();
        make_volume(dir.path(), "Audio CD", 1, 100_000, &[150, 20_000], &[]);
        make_volume(dir.path(), "Audio CD 1", 1, 120_000, &[150, 30_000], &[]);
        fs::create_dir(dir.path().join("Macintosh HD")).unwrap();
        fs::write(dir.path().join("loose file"), "x").unwrap();
        let discs = VolumeReader::new(dir.path()).discs().unwrap();
        assert_eq!(
            discs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["Audio CD", "Audio CD 1"]
        );
    }

    #[test]
    fn no_volumes_directory_means_no_discs() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            VolumeReader::new(dir.path().join("absent"))
                .discs()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_unreadable_volume_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("Broken");
        fs::create_dir(&broken).unwrap();
        fs::write(broken.join(".TOC.plist"), "not a plist").unwrap();
        make_volume(dir.path(), "Audio CD", 1, 100_000, &[150, 20_000], &[]);
        assert_eq!(VolumeReader::new(dir.path()).discs().unwrap().len(), 1);
    }
}
