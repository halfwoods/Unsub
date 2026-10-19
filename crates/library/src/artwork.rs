use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, PictureType};

use crate::error::Result;
use crate::library::Library;
use crate::model::Artwork;
use crate::tags;

const FOLDER_ART_STEMS: [&str; 3] = ["cover", "folder", "front"];
const FOLDER_ART_EXTENSIONS: [(&str, &str); 2] = [("jpg", "image/jpeg"), ("png", "image/png")];

impl Library {
    /// A missing or unreadable file only means there is no art to show, so
    /// only database errors surface.
    pub fn artwork(&self, track_id: i64) -> Result<Option<Artwork>> {
        let Some(track) = self.track(track_id)? else {
            return Ok(None);
        };
        let path = Path::new(&track.path);
        Ok(embedded(path).or_else(|| sibling(path, &track.album)))
    }
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
fn sibling(track_path: &Path, album: &str) -> Option<Artwork> {
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
            let Some((_, actual)) = names.iter().find(|(lowered, _)| *lowered == wanted) else {
                continue;
            };
            let bytes = fs::read(track_path.with_file_name(OsStr::new(actual))).ok()?;
            return Some(Artwork {
                mime: mime.to_owned(),
                bytes,
            });
        }
    }
    None
}
