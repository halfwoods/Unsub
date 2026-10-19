//! The fallback for downloads whose names do not say what they are: read the first track's tags.

use std::io::{BufReader, Read, Seek};

use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::ItemKey;

use crate::guess::store_keyword;
use crate::model::Store;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TagInfo {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub store: Store,
}

/// `None` when the data is not audio lofty can read or carries no tag. The album artist wins over
/// the track artist because it is the folder the album belongs under.
pub(crate) fn read<R: Read + Seek>(reader: R) -> Option<TagInfo> {
    let file = Probe::new(BufReader::new(reader))
        .guess_file_type()
        .ok()?
        .read()
        .ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let text = |key| {
        tag.get_string(key)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    // Stores sometimes stamp themselves into a comment or copyright line, so every text field is
    // searched rather than a fixed few.
    let all_text = tag
        .items()
        .filter_map(|item| item.value().text())
        .collect::<Vec<_>>()
        .join("\n");
    Some(TagInfo {
        artist: text(ItemKey::AlbumArtist).or_else(|| text(ItemKey::TrackArtist)),
        album: text(ItemKey::AlbumTitle),
        store: store_keyword(&all_text),
    })
}
