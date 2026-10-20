use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, PictureType};

use crate::error::Result;
use crate::library::Library;
use crate::model::{Artwork, MediaKind};
use crate::tags;

const FOLDER_ART_STEMS: [&str; 3] = ["cover", "folder", "front"];
const FOLDER_ART_EXTENSIONS: [(&str, &str); 2] = [("jpg", "image/jpeg"), ("png", "image/png")];

impl Library {
    /// A missing or unreadable file only means there is no art to show, so
    /// only database errors surface. A file that is only in the cloud has
    /// none either: opening it would download the whole song.
    pub fn artwork(&self, track_id: i64) -> Result<Option<Artwork>> {
        let Some(track) = self.track(track_id)? else {
            return Ok(None);
        };
        // A station's path is a URL, not somewhere to look for a picture.
        if track.kind == MediaKind::Radio {
            return Ok(None);
        }
        let path = Path::new(&track.path);
        if track.in_cloud || (self.is_dataless)(path) {
            return Ok(None);
        }
        Ok(own_art(path, &track.album, &*self.is_dataless))
    }
}

/// The track's embedded picture, else an image beside the file.
pub(crate) fn own_art(
    path: &Path,
    album: &str,
    dataless: &dyn Fn(&Path) -> bool,
) -> Option<Artwork> {
    embedded(path).or_else(|| {
        let (image, mime) = sibling_path(path, album, dataless)?;
        Some(Artwork {
            mime: mime.to_owned(),
            bytes: fs::read(image).ok()?,
        })
    })
}

fn embedded(path: &Path) -> Option<Artwork> {
    let options = ParseOptions::new().read_properties(false);
    let file = tags::probe(path, options).ok()?.read().ok()?;
    let pictures = || file.tags().iter().flat_map(|tag| tag.pictures());
    let picture = pictures()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures().next())?;
    Some(Artwork {
        mime: picture
            .mime_type()
            .map_or("application/octet-stream", MimeType::as_str)
            .to_owned(),
        bytes: picture.data().to_vec(),
    })
}

/// Candidates are matched against the directory listing rather than joined
/// onto the path, so case differences and odd album names (slashes) are safe.
/// An image that is only in the cloud is passed over, since reading it downloads it.
pub(crate) fn sibling_path(
    track_path: &Path,
    album: &str,
    dataless: &dyn Fn(&Path) -> bool,
) -> Option<(PathBuf, &'static str)> {
    let names: Vec<(String, String)> = fs::read_dir(track_path.parent()?)
        .ok()?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some((name.to_lowercase(), name))
        })
        .collect();

    let album = album.to_lowercase();
    let stems = FOLDER_ART_STEMS
        .iter()
        .copied()
        .chain((!album.is_empty()).then_some(album.as_str()));
    for stem in stems {
        for (extension, mime) in FOLDER_ART_EXTENSIONS {
            let wanted = format!("{stem}.{extension}");
            if let Some((_, actual)) = names.iter().find(|(lowered, _)| *lowered == wanted) {
                let image = track_path.with_file_name(OsStr::new(actual));
                if !dataless(&image) {
                    return Some((image, mime));
                }
            }
        }
    }
    None
}
