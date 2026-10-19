//! Ripping a disc: read each track, encode it, check it against AccurateRip, tag it and move it
//! into place.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use lofty::picture::Picture;

use crate::{
    AccurateRip, Disc, DiscTrack, Error, Format, MetadataClient, Release, Result, TrackAudio,
    accuraterip,
    disc::{DiscReader, system_reader},
    encode, naming,
    tag::{self, TrackTags},
};

#[derive(Debug, Clone)]
pub struct RipSettings {
    pub format: Format,
    /// The library folder; files land in `<dest_dir>/<Album Artist>/<Album>/`.
    pub dest_dir: PathBuf,
    pub verify_accuraterip: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RippedTrack {
    /// The track's number on the disc.
    pub number: u8,
    pub path: PathBuf,
    pub accuraterip: AccurateRip,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RipProgress {
    TrackStarted {
        number: u8,
        index: usize,
        count: usize,
    },
    /// How much of the track has been read, from 0 to 1; reported about once per percent.
    TrackProgress {
        number: u8,
        fraction: f32,
    },
    TrackFinished(RippedTrack),
}

/// Rips `disc` with this platform's drive reader. `release` supplies the tags and cover; without it
/// tracks are named "Track NN". `client` fetches the cover art and the AccurateRip entry.
///
/// `cancel` is polled while reading. A cancelled rip returns `Error::Cancelled`, removes the track
/// it was in the middle of, and leaves earlier tracks (already announced through `progress`) in
/// place.
pub fn rip(
    disc: &Disc,
    release: Option<&Release>,
    settings: &RipSettings,
    client: &dyn MetadataClient,
    progress: &mut dyn FnMut(RipProgress),
    cancel: &AtomicBool,
) -> Result<Vec<RippedTrack>> {
    rip_with(
        &*system_reader(),
        disc,
        release,
        settings,
        client,
        progress,
        cancel,
    )
}

/// [`rip`] with an explicit reader, for other platforms' drives and for tests.
pub fn rip_with(
    reader: &dyn DiscReader,
    disc: &Disc,
    release: Option<&Release>,
    settings: &RipSettings,
    client: &dyn MetadataClient,
    progress: &mut dyn FnMut(RipProgress),
    cancel: &AtomicBool,
) -> Result<Vec<RippedTrack>> {
    if disc.tracks.is_empty() {
        return Err(Error::Disc("the disc has no audio tracks".into()));
    }
    let session = Session {
        reader,
        disc,
        release,
        settings,
        cancel,
        cover: release.and_then(|release| fetch_cover(release, client)),
        database: settings
            .verify_accuraterip
            .then(|| fetch_database(disc, client))
            .flatten(),
    };

    let mut ripped = Vec::new();
    for (index, track) in disc.tracks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        progress(RipProgress::TrackStarted {
            number: track.number,
            index,
            count: disc.tracks.len(),
        });
        let finished = session.rip_track(index, track, progress)?;
        progress(RipProgress::TrackFinished(finished.clone()));
        ripped.push(finished);
    }
    Ok(ripped)
}

/// What stays the same from one track of a rip to the next.
struct Session<'a> {
    reader: &'a dyn DiscReader,
    disc: &'a Disc,
    release: Option<&'a Release>,
    settings: &'a RipSettings,
    cancel: &'a AtomicBool,
    cover: Option<Picture>,
    /// `None` when the database could not be consulted, which leaves every track `Skipped`.
    database: Option<Vec<accuraterip::Pressing>>,
}

impl Session<'_> {
    fn rip_track(
        &self,
        index: usize,
        track: &DiscTrack,
        progress: &mut dyn FnMut(RipProgress),
    ) -> Result<RippedTrack> {
        let (tags, path) = describe(self.disc, self.release, track, index, self.settings);
        let directory = path.parent().unwrap_or(&self.settings.dest_dir);
        fs::create_dir_all(directory).map_err(|err| Error::io(directory, err))?;
        let sums = match self.encode_track(index, track, &path, tags, progress) {
            Ok(sums) => sums,
            Err(err) => {
                remove_empty_directories(directory, &self.settings.dest_dir);
                return Err(err);
            }
        };
        let accuraterip = self
            .database
            .as_deref()
            .map_or(AccurateRip::Skipped, |pressings| {
                accuraterip::verdict(pressings, index, sums)
            });
        Ok(RippedTrack {
            number: track.number,
            path,
            accuraterip,
        })
    }

    /// Reads, encodes and tags a track into a temporary file beside its destination, then renames it
    /// into place, so the destination only ever holds a complete, tagged file.
    fn encode_track(
        &self,
        index: usize,
        track: &DiscTrack,
        path: &Path,
        tags: TrackTags,
        progress: &mut dyn FnMut(RipProgress),
    ) -> Result<accuraterip::Sums> {
        let partial = Partial::beside(path);
        let audio = self.reader.open_track(self.disc, track)?;
        let frames = audio.frames();
        let mut monitor = Monitor {
            inner: audio,
            cancel: self.cancel,
            progress,
            number: track.number,
            checksum: accuraterip::Checksum::new(
                frames,
                index == 0,
                index + 1 == self.disc.tracks.len(),
            ),
            done: 0,
            frames,
            reported: 0.0,
        };
        encode::encode(self.settings.format, &mut monitor, &partial.path)?;
        tag::write(
            &partial.path,
            self.settings.format,
            tags,
            self.cover.as_ref(),
        )?;
        fs::rename(&partial.path, path).map_err(|err| Error::io(path, err))?;
        partial.keep();
        Ok(monitor.checksum.finish())
    }
}

/// A temporary file that disappears unless it was kept, however the rip ends.
struct Partial {
    path: PathBuf,
    kept: bool,
}

impl Partial {
    fn beside(target: &Path) -> Self {
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            path: target.with_file_name(format!(".{name}.partial")),
            kept: false,
        }
    }

    fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.kept {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Wraps a track's audio to honour cancellation, report progress and checksum what passes through.
struct Monitor<'a> {
    inner: Box<dyn TrackAudio>,
    cancel: &'a AtomicBool,
    progress: &'a mut dyn FnMut(RipProgress),
    number: u8,
    checksum: accuraterip::Checksum,
    done: u64,
    frames: u64,
    reported: f32,
}

impl TrackAudio for Monitor<'_> {
    fn frames(&self) -> u64 {
        self.frames
    }

    fn read(&mut self, out: &mut [i16]) -> Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let frames = self.inner.read(out)?;
        self.checksum.update(&out[..frames * 2]);
        self.done += frames as u64;
        let fraction = self.done as f32 / self.frames.max(1) as f32;
        if fraction - self.reported >= 0.01 {
            self.reported = fraction;
            (self.progress)(RipProgress::TrackProgress {
                number: self.number,
                fraction: fraction.min(1.0),
            });
        }
        Ok(frames)
    }
}

fn remove_empty_directories(from: &Path, stop_at: &Path) {
    for directory in from
        .ancestors()
        .take_while(|directory| *directory != stop_at)
    {
        // Fails, harmlessly, for any directory that still holds files.
        let _ = fs::remove_dir(directory);
    }
}

/// A missing or broken cover only costs the artwork.
fn fetch_cover(release: &Release, client: &dyn MetadataClient) -> Option<Picture> {
    let bytes = client.get(release.cover_art_url.as_deref()?).ok()??;
    tag::cover(&bytes)
}

/// `None` (so every track is `Skipped`) when the database cannot be consulted: offline, or a disc
/// whose data tracks mean the audio does not line up with the TOC the database keys on.
fn fetch_database(disc: &Disc, client: &dyn MetadataClient) -> Option<Vec<accuraterip::Pressing>> {
    if disc.tracks.len() != disc.toc.track_count() {
        return None;
    }
    accuraterip::fetch(&disc.toc, client).ok()
}

/// The tags and destination path for a track, from the release when there is one.
fn describe(
    disc: &Disc,
    release: Option<&Release>,
    track: &DiscTrack,
    index: usize,
    settings: &RipSettings,
) -> (TrackTags, PathBuf) {
    let disc_total = release.map_or(1, |release| release.disc_count);
    let disc_number = release.map_or(1, |release| release.disc_number);
    let track_total = u32::try_from(disc.tracks.len()).unwrap_or(u32::MAX);
    let album = release.map_or_else(|| fallback_album(disc), |release| release.title.clone());
    let album_artist = release.map_or_else(
        || "Unknown Artist".to_owned(),
        |release| release.artist.clone(),
    );
    let named = release.and_then(|release| release.tracks.get(index));

    let tags = TrackTags {
        title: named.map_or_else(
            || format!("Track {:02}", track.number),
            |named| named.title.clone(),
        ),
        artist: named.map_or_else(|| album_artist.clone(), |named| named.artist.clone()),
        track: named.map_or_else(|| u32::from(track.number), |named| named.position),
        track_total,
        disc: disc_number,
        disc_total,
        year: release.and_then(|release| release.year),
        release_mbid: release.map(|release| release.mbid.clone()),
        release_group_mbid: release.and_then(|release| release.release_group_mbid.clone()),
        recording_mbid: named.and_then(|named| named.recording_mbid.clone()),
        track_mbid: named.and_then(|named| named.track_mbid.clone()),
        artist_mbid: named.and_then(|named| named.artist_mbid.clone()),
        album_artist_mbid: release.and_then(|release| release.artist_mbid.clone()),
        album,
        album_artist,
    };
    let path = naming::track_path(
        &settings.dest_dir,
        &tags.album_artist,
        &tags.album,
        tags.disc,
        tags.track,
        &tags.title,
        settings.format.extension(),
    );
    (tags, path)
}

/// The volume name is the album when the drive found CD-TEXT; plain "Audio CD" says nothing.
fn fallback_album(disc: &Disc) -> String {
    if disc.name.is_empty() || disc.name.starts_with("Audio CD") {
        "Unknown Album".to_owned()
    } else {
        disc.name.clone()
    }
}

#[cfg(test)]
mod tests;
