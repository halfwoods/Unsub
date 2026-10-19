//! The strings and numbers of a track that the derived structures (album and artist lists, sort
//! indexes) are computed from.

use super::mhit::at;
use super::mhod;
use super::raw::RawTrack;
use crate::bytes::u32_at;
use crate::error::Result;

pub(super) struct TrackView {
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) album: String,
    pub(super) album_artist: String,
    pub(super) genre: String,
    pub(super) composer: String,
    pub(super) tv_show: String,
    pub(super) sort_title: String,
    pub(super) sort_artist: String,
    pub(super) sort_album: String,
    pub(super) sort_album_artist: String,
    pub(super) sort_composer: String,
    pub(super) track_number: u32,
    pub(super) disc_number: u32,
}

impl TrackView {
    pub(super) fn of(track: &RawTrack) -> Result<Self> {
        let text = |kind| track.text(kind).map(Option::unwrap_or_default);
        Ok(Self {
            title: text(mhod::TITLE)?,
            artist: text(mhod::ARTIST)?,
            album: text(mhod::ALBUM)?,
            album_artist: text(mhod::ALBUM_ARTIST)?,
            genre: text(mhod::GENRE)?,
            composer: text(mhod::COMPOSER)?,
            tv_show: text(mhod::TV_SHOW)?,
            sort_title: text(mhod::SORT_TITLE)?,
            sort_artist: text(mhod::SORT_ARTIST)?,
            sort_album: text(mhod::SORT_ALBUM)?,
            sort_album_artist: text(mhod::SORT_ALBUM_ARTIST)?,
            sort_composer: text(mhod::SORT_COMPOSER)?,
            track_number: u32_at(&track.header, at::TRACK_NUMBER).unwrap_or(0),
            disc_number: u32_at(&track.header, at::DISC_NUMBER).unwrap_or(0),
        })
    }
}
