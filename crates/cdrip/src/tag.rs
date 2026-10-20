//! Writing the tags and cover art into a finished file.

use std::path::Path;

use lofty::{
    config::WriteOptions,
    picture::{Picture, PictureType},
    tag::{Accessor, ItemKey, Tag, TagExt, TagType},
};

use crate::{Result, encode::Format};

/// Everything written to one track.
#[derive(Debug, Clone, Default)]
pub struct TrackTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track: u32,
    pub track_total: u32,
    pub disc: u32,
    pub disc_total: u32,
    pub year: Option<i32>,
    pub release_mbid: Option<String>,
    pub release_group_mbid: Option<String>,
    pub recording_mbid: Option<String>,
    pub track_mbid: Option<String>,
    pub artist_mbid: Option<String>,
    pub album_artist_mbid: Option<String>,
}

impl TrackTags {
    fn into_tag(self, tag_type: TagType, cover: Option<&Picture>) -> Tag {
        let mut tag = Tag::new(tag_type);
        tag.set_title(self.title);
        tag.set_artist(self.artist);
        tag.set_album(self.album);
        tag.set_track(self.track);
        tag.set_track_total(self.track_total);
        tag.set_disk(self.disc);
        tag.set_disk_total(self.disc_total);
        tag.insert_text(ItemKey::AlbumArtist, self.album_artist);
        if let Some(year) = self.year {
            tag.insert_text(ItemKey::RecordingDate, year.to_string());
        }
        let ids = [
            (ItemKey::MusicBrainzReleaseId, self.release_mbid),
            (ItemKey::MusicBrainzReleaseGroupId, self.release_group_mbid),
            (ItemKey::MusicBrainzRecordingId, self.recording_mbid),
            (ItemKey::MusicBrainzTrackId, self.track_mbid),
            (ItemKey::MusicBrainzArtistId, self.artist_mbid),
            (ItemKey::MusicBrainzReleaseArtistId, self.album_artist_mbid),
        ];
        for (key, id) in ids {
            if let Some(id) = id {
                tag.insert_text(key, id);
            }
        }
        if let Some(cover) = cover {
            tag.push_picture(cover.clone());
        }
        tag
    }
}

/// A front cover from the raw bytes of a JPEG, PNG or similar image. `None` when the bytes are not
/// a recognisable image, so a bad download costs the cover rather than the rip.
pub(crate) fn cover(bytes: &[u8]) -> Option<Picture> {
    let mut picture = Picture::from_reader(&mut &*bytes).ok()?;
    picture.set_pic_type(PictureType::CoverFront);
    Some(picture)
}

pub(crate) fn write(
    path: &Path,
    format: Format,
    tags: TrackTags,
    cover: Option<&Picture>,
) -> Result<()> {
    let tag_type = match format {
        Format::Flac { .. } => TagType::VorbisComments,
        Format::Alac | Format::Aac { .. } => TagType::Mp4Ilst,
    };
    tags.into_tag(tag_type, cover)
        .save_to_path(path, WriteOptions::default())?;
    Ok(())
}
