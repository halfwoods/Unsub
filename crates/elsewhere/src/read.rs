use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::model::{Export, Service};
use crate::{listenbrainz, spotify, youtube};

/// Reads what `service` sent from a file, or from a folder holding the export (a ZIP is unpacked
/// first). Files are told apart by their names and headers, so the folder may hold the whole
/// export and anything else, and only what is recognised is read.
pub fn read(service: Service, path: &Path) -> Result<Export> {
    let files = files(path)?;
    let export = match service {
        Service::Spotify => spotify::read(&files)?,
        Service::YoutubeMusic => youtube::read(&files)?,
        Service::ListenBrainz => listenbrainz::read(&files)?,
    };
    if export.playlists.is_empty() && export.plays.is_empty() {
        return Err(Error::NothingFound(service));
    }
    Ok(export)
}

fn files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_owned()]);
    }
    let mut files = Vec::new();
    for entry in WalkDir::new(path).sort_by_file_name() {
        let entry = entry.map_err(std::io::Error::from)?;
        if entry.file_type().is_file() {
            files.push(entry.into_path());
        }
    }
    Ok(files)
}

/// Lower case, because the services' own capitalisation has changed between exports.
pub(crate) fn file_name(file: &Path) -> String {
    file.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// What a playlist file is called: Exportify names it after the playlist in lower case with
/// underscores, and Google adds " videos".
pub(crate) fn playlist_name(file: &Path) -> String {
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().replace('_', " "))
        .unwrap_or_default();
    let stem = stem.trim();
    let name = [" videos", "-videos"]
        .iter()
        .find_map(|suffix| {
            let split = stem.len().checked_sub(suffix.len())?;
            stem.get(split..)?
                .eq_ignore_ascii_case(suffix)
                .then(|| stem[..split].trim())
        })
        .unwrap_or(stem);
    if name.is_empty() {
        "Playlist".to_owned()
    } else {
        name.to_owned()
    }
}

/// Streamed rather than read whole: a Takeout watch history is mostly videos, and can be huge.
pub(crate) fn json<T: DeserializeOwned>(file: &Path) -> Result<T> {
    serde_json::from_reader(BufReader::new(File::open(file)?)).map_err(|source| Error::Json {
        file: file.to_owned(),
        source,
    })
}

/// Lossy so that one stray byte in a title doesn't fail a whole playlist.
pub(crate) fn text(file: &Path) -> Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(file)?).into_owned())
}
