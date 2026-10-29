//! Read-only video rescue. The source device and the audio library are never written.
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Ipod, IpodTrack};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VideoKind {
    Movie,
    MusicVideo,
    Tv,
    VideoPodcast,
    Other,
}
impl VideoKind {
    pub(crate) fn from_flags(flags: u32, movie: bool) -> Option<Self> {
        if flags & (2 | 32 | 64) == 0 && !movie {
            return None;
        }
        Some(if flags & 4 != 0 {
            Self::VideoPodcast
        } else if flags & 64 != 0 {
            Self::Tv
        } else if flags & 32 != 0 {
            Self::MusicVideo
        } else if flags & 2 != 0 || movie {
            Self::Movie
        } else {
            Self::Other
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Protection {
    Protected,
    Unprotected,
    Unknown,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoItem {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub kind: VideoKind,
    pub bytes: Option<u64>,
    pub duration_ms: u64,
    pub protection: Protection,
    pub missing: bool,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub metadata: serde_json::Value,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub items: Vec<VideoItem>,
    pub from_files: bool,
    pub warning: Option<String>,
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Inspect only MP4 sample-description atoms, never decode or decrypt media. Atom lengths and
/// depth are bounded, and mdat payloads are skipped rather than loaded into memory.
/// Inspect MP4 sample descriptions without decoding media. The boolean reports whether an
/// actual video sample entry was found; callers must reject unknown protection or no video.
pub fn sample_protection(path: &Path) -> io::Result<(Protection, bool)> {
    fn atoms(
        file: &mut File,
        mut pos: u64,
        end: u64,
        depth: u8,
        seen: &mut (bool, bool),
        budget: &mut usize,
    ) -> io::Result<()> {
        if depth > 12 {
            return Err(invalid("Video atom nesting is too deep"));
        }
        while pos < end {
            if *budget == 0 || end - pos < 8 {
                return Err(invalid("Invalid video atom table"));
            }
            *budget -= 1;
            file.seek(SeekFrom::Start(pos))?;
            let mut header = [0; 8];
            file.read_exact(&mut header)?;
            let raw = u32::from_be_bytes(header[..4].try_into().unwrap());
            let (len, header_len) = if raw == 1 {
                let mut wide = [0; 8];
                file.read_exact(&mut wide)?;
                (u64::from_be_bytes(wide), 16)
            } else if raw == 0 {
                (end - pos, 8)
            } else {
                (u64::from(raw), 8)
            };
            if len < header_len || len > end - pos {
                return Err(invalid("Invalid video atom length"));
            }
            let body = pos + header_len;
            match &header[4..] {
                b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => {
                    atoms(file, body, pos + len, depth + 1, seen, budget)?
                }
                b"stsd" => {
                    if len - header_len < 8 {
                        return Err(invalid("Invalid sample descriptions"));
                    }
                    file.seek(SeekFrom::Start(body + 4))?;
                    let mut n = [0; 4];
                    file.read_exact(&mut n)?;
                    let count = u32::from_be_bytes(n);
                    if count > 4096 {
                        return Err(invalid("Too many sample descriptions"));
                    }
                    let mut at = body + 8;
                    for _ in 0..count {
                        if pos + len - at < 8 {
                            return Err(invalid("Truncated sample description"));
                        }
                        file.seek(SeekFrom::Start(at))?;
                        let mut entry = [0; 8];
                        file.read_exact(&mut entry)?;
                        let size = u64::from(u32::from_be_bytes(entry[..4].try_into().unwrap()));
                        if size < 8 || size > pos + len - at {
                            return Err(invalid("Invalid sample description size"));
                        }
                        match &entry[4..] {
                            b"encv" | b"drmi" => {
                                seen.0 = true;
                                seen.1 = true;
                            }
                            b"enca" | b"drms" => seen.0 = true,
                            b"avc1" | b"avc3" | b"mp4v" | b"hvc1" | b"hev1" | b"jpeg" => {
                                seen.1 = true
                            }
                            _ => {}
                        }
                        at += size;
                    }
                }
                _ => {}
            }
            pos += len;
        }
        Ok(())
    }
    let mut file = File::open(path)?;
    let end = file.metadata()?.len();
    let mut seen = (false, false);
    atoms(&mut file, 0, end, 0, &mut seen, &mut 100_000)?;
    Ok((
        if seen.0 {
            Protection::Protected
        } else if seen.1 {
            Protection::Unprotected
        } else {
            Protection::Unknown
        },
        seen.1,
    ))
}
fn safe_source(mount: &Path, path: &Path) -> io::Result<PathBuf> {
    let actual = fs::canonicalize(path)?;
    if !actual.starts_with(mount) || !actual.is_file() {
        return Err(invalid("Video path leaves the iPod or is not a file"));
    }
    Ok(actual)
}
fn item(mount: &Path, track: IpodTrack, kind: VideoKind, fallback: bool) -> VideoItem {
    let relative = track
        .path
        .strip_prefix(mount)
        .unwrap_or(&track.path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let id = if fallback {
        format!("file-{}", hash_bytes(relative.as_bytes()))
    } else {
        format!("db-{:016x}-{}", track.dbid, track.id)
    };
    let safe = safe_source(mount, &track.path).ok();
    let bytes = safe
        .as_ref()
        .and_then(|p| fs::metadata(p).ok())
        .map(|m| m.len());
    let protection = if crate::is_copy_protected(&track) {
        Protection::Protected
    } else {
        safe.as_ref()
            .and_then(|p| sample_protection(p).ok())
            .map_or(Protection::Unknown, |v| v.0)
    };
    let mut metadata = serde_json::to_value(&track).unwrap_or_default();
    metadata["path"] = serde_json::Value::String(relative);
    VideoItem {
        id,
        title: if track.title.is_empty() {
            track
                .path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            track.title
        },
        artist: track.artist,
        album: track.album,
        kind,
        bytes,
        duration_ms: track.duration_ms,
        protection,
        missing: bytes.is_none(),
        path: track.path,
        metadata,
    }
}
/// A broken database never prevents a file-only rescue. The fallback is explicit to the caller.
pub fn list(ipod: &Ipod) -> io::Result<Catalog> {
    let mount = fs::canonicalize(&ipod.mount_path)?;
    let mut canonical_ipod = ipod.clone();
    canonical_ipod.mount_path = mount.clone();
    if crate::device::database_exists(&canonical_ipod)
        && let Ok(database) = crate::read_database(&canonical_ipod)
    {
        return Ok(Catalog {
            items: database
                .tracks
                .into_iter()
                .filter_map(|t| t.video_kind.map(|kind| item(&mount, t, kind, false)))
                .collect(),
            from_files: false,
            warning: None,
        });
    }
    let mut paths = Vec::new();
    let music = mount.join("iPod_Control/Music");
    if let Ok(folders) = fs::read_dir(music) {
        for folder in folders.flatten() {
            let name = folder.file_name().to_string_lossy().into_owned();
            if name.len() != 3
                || !name.starts_with('F')
                || !name[1..].bytes().all(|b| b.is_ascii_digit())
                || folder.file_type()?.is_symlink()
            {
                continue;
            }
            if let Ok(files) = fs::read_dir(folder.path()) {
                for entry in files.flatten() {
                    let path = entry.path();
                    let ext = extension(&path);
                    if !matches!(ext.as_str(), "mp4" | "m4v" | "mov")
                        || safe_source(&mount, &path).is_err()
                    {
                        continue;
                    }
                    // An .mp4 with recognizable audio only is not a video; unidentified .m4v/.mov remain rescuable.
                    if ext == "mp4" && !sample_protection(&path).is_ok_and(|v| v.1) {
                        continue;
                    }
                    paths.push(path);
                }
            }
        }
    }
    paths.sort();
    Ok(Catalog{items:paths.into_iter().map(|path|item(&mount,IpodTrack{path,..Default::default()},VideoKind::Other,true)).collect(),from_files:true,
        warning:Some("The iPod database is unavailable. Videos are identified by their files; some titles and metadata may be missing.".into())})
}
fn selected<'a>(catalog: &'a Catalog, ids: &[String]) -> io::Result<Vec<&'a VideoItem>> {
    let wanted: HashSet<_> = ids.iter().collect();
    let found: Vec<_> = catalog
        .items
        .iter()
        .filter(|v| wanted.contains(&v.id))
        .collect();
    if found.len() != wanted.len() {
        return Err(invalid(
            "The video selection changed. Reopen Copy Videos and select again.",
        ));
    }
    Ok(found)
}
fn safe_name(value: &str) -> String {
    let name: String = value
        .chars()
        .take(80)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut name = name;
    while name.len() > 72 {
        name.pop();
    }
    let name = name.trim_matches([' ', '.']);
    if name.is_empty() {
        "Untitled".into()
    } else {
        name.into()
    }
}
fn destination(ipod: &Ipod, root: &Path) -> io::Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    let mount = fs::canonicalize(&ipod.mount_path)?;
    if !root.is_dir() || root.starts_with(&mount) {
        return Err(invalid("Choose a destination folder outside this iPod"));
    }
    let folder = root.join(format!(
        "{} Videos - {}",
        safe_name(&ipod.name),
        &hash_bytes(ipod.id.as_bytes())[..12]
    ));
    if folder.exists()
        && (fs::symlink_metadata(&folder)?.file_type().is_symlink() || !folder.is_dir())
    {
        return Err(invalid("The export folder is not a regular directory"));
    }
    Ok(folder)
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub videos: usize,
    pub bytes: u64,
    pub free_bytes: u64,
    pub fits: bool,
    pub missing: usize,
    pub protected: usize,
    pub unknown_protection: usize,
    pub folder: String,
}
pub fn preview(ipod: &Ipod, catalog: &Catalog, ids: &[String], root: &Path) -> io::Result<Preview> {
    preview_with_cancel(ipod, catalog, ids, root, &AtomicBool::new(false))
        .map(|(preview, _)| preview)
}
fn preview_with_cancel(
    ipod: &Ipod,
    catalog: &Catalog,
    ids: &[String],
    root: &Path,
    cancel: &AtomicBool,
) -> io::Result<(Preview, ExistingCopies)> {
    let rows = selected(catalog, ids)?;
    let folder = destination(ipod, root)?;
    let existing = ExistingCopies::read(&folder)?;
    let bytes = rows.iter().try_fold(0u64, |sum, v| {
        sum.checked_add(v.bytes.unwrap_or(0))
            .ok_or_else(|| invalid("Video sizes exceed supported capacity"))
    })?;
    let free_bytes = fs4::available_space(root)?;
    let mount = fs::canonicalize(&ipod.mount_path)?;
    let mut required_bytes = bytes;
    if bytes > free_bytes && folder.is_dir() {
        for row in &rows {
            match verified_copy(&mount, &folder, row, &existing, cancel) {
                Ok(Some(_)) => {
                    required_bytes = required_bytes.saturating_sub(row.bytes.unwrap_or(0))
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
                _ => {}
            }
        }
    }
    Ok((
        Preview {
            videos: rows.len(),
            bytes,
            free_bytes,
            fits: required_bytes <= free_bytes,
            missing: rows.iter().filter(|v| v.missing).count(),
            protected: rows
                .iter()
                .filter(|v| v.protection == Protection::Protected)
                .count(),
            unknown_protection: rows
                .iter()
                .filter(|v| v.protection == Protection::Unknown)
                .count(),
            folder: folder.to_string_lossy().into_owned(),
        },
        existing,
    ))
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub id: String,
    pub title: String,
    pub file: Option<String>,
    pub sha256: Option<String>,
    pub status: String,
    pub detail: Option<String>,
    pub protection: Protection,
    pub metadata: serde_json::Value,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub version: u32,
    pub device_id: String,
    pub from_files: bool,
    pub stopped: bool,
    pub disconnected: bool,
    pub items: Vec<Outcome>,
}
fn hash_file(path: &Path, cancel: &AtomicBool) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Export cancelled",
            ));
        }
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
/// Index generated names once per preview/export. Gaps left by deleted collision files must
/// not hide later valid copies. The stable ID also survives edits to a video's display title.
#[derive(Default)]
struct ExistingCopies {
    by_id_and_extension: HashMap<(String, String), Vec<String>>,
}
impl ExistingCopies {
    fn read(folder: &Path) -> io::Result<Self> {
        let entries = match fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        let mut index = Self::default();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let filename = entry.file_name();
            let Some(filename) = filename.to_str() else {
                continue;
            };
            let Some(stem) = Path::new(filename).file_stem().and_then(|v| v.to_str()) else {
                continue;
            };
            let stem = if let Some((base, suffix)) = stem.rsplit_once(" (") {
                match suffix
                    .strip_suffix(')')
                    .and_then(|v| v.parse::<usize>().ok())
                {
                    Some(number) if (1..10_000).contains(&number) => base,
                    _ => stem,
                }
            } else {
                stem
            };
            let Some((_, id)) = stem.rsplit_once(" [") else {
                continue;
            };
            let Some(id) = id.strip_suffix(']') else {
                continue;
            };
            index
                .by_id_and_extension
                .entry((id.into(), extension(Path::new(filename))))
                .or_default()
                .push(filename.into());
        }
        for candidates in index.by_id_and_extension.values_mut() {
            candidates.sort();
        }
        Ok(index)
    }

    fn candidates(&self, item: &VideoItem, ext: String) -> &[String] {
        self.by_id_and_extension
            .get(&(safe_name(&item.id), ext))
            .map_or(&[], Vec::as_slice)
    }
}
/// Existing files are never trusted from names or lengths alone. Hash both originals and
/// destinations, and only read the source when a plausible prior output actually exists.
fn verified_copy(
    mount: &Path,
    folder: &Path,
    item: &VideoItem,
    existing: &ExistingCopies,
    cancel: &AtomicBool,
) -> io::Result<Option<(String, String)>> {
    let source = safe_source(mount, &item.path)?;
    let before = fs::metadata(&source)?;
    let mut source_digest = None;
    for name in existing.candidates(item, safe_name(&extension(&source)).to_ascii_lowercase()) {
        let target = folder.join(name);
        match fs::symlink_metadata(&target) {
            Ok(meta)
                if meta.is_file()
                    && !meta.file_type().is_symlink()
                    && meta.len() == before.len() =>
            {
                if source_digest.is_none() {
                    source_digest = Some(hash_file(&source, cancel)?);
                }
                let digest = source_digest.as_ref().unwrap();
                if hash_file(&target, cancel)? == *digest {
                    let after = fs::metadata(&source)?;
                    if before.len() != after.len()
                        || before.modified().ok() != after.modified().ok()
                    {
                        return Err(invalid("The source video changed during verification"));
                    }
                    return Ok(Some((name.clone(), digest.clone())));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
            _ => {}
        }
    }
    Ok(None)
}
fn copy_one(
    mount: &Path,
    folder: &Path,
    item: &VideoItem,
    existing: &ExistingCopies,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> io::Result<(String, String, bool)> {
    let source = safe_source(mount, &item.path)?;
    let before = fs::metadata(&source)?;
    let expected = before.len();
    let mut input = File::open(&source)?;
    let ext = safe_name(&extension(&source));
    let base = format!("{} [{}]", safe_name(&item.title), safe_name(&item.id));
    if let Some((name, digest)) = verified_copy(mount, folder, item, existing, cancel)? {
        progress(expected, expected);
        return Ok((name, digest, true));
    }
    let mut output = tempfile::NamedTempFile::new_in(folder)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 128 * 1024];
    let mut done = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Export cancelled",
            ));
        }
        if !mount.join("iPod_Control").is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "The iPod was disconnected",
            ));
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n])?;
        hash.update(&buf[..n]);
        done += n as u64;
        progress(done, expected);
    }
    let after = fs::metadata(&source)?;
    if done != expected
        || before.modified().ok() != after.modified().ok()
        || before.len() != after.len()
    {
        return Err(invalid("The source video changed during export"));
    }
    output.flush()?;
    output.as_file().sync_all()?;
    let digest = format!("{:x}", hash.finalize());
    if hash_file(output.path(), cancel)? != digest {
        return Err(invalid("Export verification failed"));
    }
    for suffix in 0..10_000 {
        let name = if suffix == 0 {
            format!("{base}.{ext}")
        } else {
            format!("{base} ({suffix}).{ext}")
        };
        let target = folder.join(&name);
        if let Ok(meta) = fs::symlink_metadata(&target) {
            if meta.is_file()
                && !meta.file_type().is_symlink()
                && meta.len() == done
                && hash_file(&target, cancel)? == digest
            {
                return Ok((name, digest, true));
            }
            continue;
        }
        match output.persist_noclobber(&target) {
            Ok(_) => return Ok((name, digest, false)),
            Err(error) => {
                if error.error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error.error);
                }
                output = error.file;
            }
        }
    }
    Err(invalid("Too many files with this video name"))
}
/// The returned report is also saved beside the videos. Each run has its own report; old reports
/// are never overwritten. Repeat copies are skipped only after comparing complete file hashes.
pub fn export(
    ipod: &Ipod,
    catalog: &Catalog,
    ids: &[String],
    root: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize, &str, u64, u64),
) -> io::Result<Report> {
    let (checked, existing) = preview_with_cancel(ipod, catalog, ids, root, cancel)?;
    if !checked.fits {
        return Err(invalid("Not enough free space for the selected videos"));
    }
    let rows = selected(catalog, ids)?;
    let mount = fs::canonicalize(&ipod.mount_path)?;
    let folder = destination(ipod, root)?;
    if !folder.exists() {
        fs::create_dir(&folder)?;
    }
    if fs::symlink_metadata(&folder)?.file_type().is_symlink()
        || !fs::canonicalize(&folder)?.starts_with(fs::canonicalize(root)?)
    {
        return Err(invalid("Export folder changed"));
    }
    let mut report = Report {
        version: 1,
        device_id: ipod.id.clone(),
        from_files: catalog.from_files,
        stopped: false,
        disconnected: false,
        items: Vec::new(),
    };
    for (index, item) in rows.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.stopped = true;
            break;
        }
        if !mount.join("iPod_Control").is_dir() {
            report.disconnected = true;
            break;
        }
        progress(index, rows.len(), &item.title, 0, item.bytes.unwrap_or(0));
        let result = copy_one(
            &mount,
            &folder,
            item,
            &existing,
            cancel,
            &mut |done, total| progress(index, rows.len(), &item.title, done, total),
        );
        let (file, sha256, status, detail) = match result {
            Ok((name, hash, skipped)) => (
                Some(name),
                Some(hash),
                if skipped { "skipped" } else { "copied" }.into(),
                None,
            ),
            Err(e) => {
                if e.kind() == io::ErrorKind::Interrupted {
                    report.stopped = true;
                }
                if !mount.join("iPod_Control").is_dir() {
                    report.disconnected = true;
                }
                (None, None, "failed".into(), Some(e.to_string()))
            }
        };
        report.items.push(Outcome {
            id: item.id.clone(),
            title: item.title.clone(),
            file,
            sha256,
            status,
            detail,
            protection: item.protection,
            metadata: item.metadata.clone(),
        });
        if report.stopped || report.disconnected {
            break;
        }
    }
    for item in rows.iter().skip(report.items.len()) {
        report.items.push(Outcome {
            id: item.id.clone(),
            title: item.title.clone(),
            file: None,
            sha256: None,
            status: "notCopied".into(),
            detail: Some(
                if report.disconnected {
                    "The iPod disconnected"
                } else {
                    "Export stopped"
                }
                .into(),
            ),
            protection: item.protection,
            metadata: item.metadata.clone(),
        });
    }
    let mut file = tempfile::Builder::new()
        .prefix("Export report ")
        .suffix(".json")
        .tempfile_in(&folder)?;
    serde_json::to_writer_pretty(&mut file, &report)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.keep().map_err(|e| e.error)?;
    progress(report.items.len(), rows.len(), "Finished", 0, 0);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatabaseFormat, SyncSupport};
    fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend(kind);
        bytes.extend(body);
        bytes
    }
    fn movie(codec: &[u8; 4]) -> Vec<u8> {
        let mut entries = vec![0, 0, 0, 0, 0, 0, 0, 1];
        entries.extend(atom(codec, &[0; 16]));
        let mut bytes = atom(b"stsd", &entries);
        for kind in [b"stbl", b"minf", b"mdia", b"trak", b"moov"] {
            bytes = atom(kind, &bytes);
        }
        bytes.extend(atom(b"mdat", &vec![17; 300_000]));
        bytes
    }
    fn ipod(mount: &Path) -> Ipod {
        Ipod {
            id: "test-device".into(),
            name: "Test iPod".into(),
            mount_path: mount.into(),
            model: None,
            capacity_bytes: 0,
            free_bytes: 0,
            firewire_guid: None,
            guid_from_usb: false,
            serial_number: None,
            database: DatabaseFormat::ITunesDb,
            sync: SyncSupport::Supported,
        }
    }
    fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Ipod, PathBuf) {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let path = source.path().join("iPod_Control/Music/F00/ABCD.m4v");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, movie(b"encv")).unwrap();
        let device = ipod(source.path());
        (source, target, device, path)
    }
    fn ids(catalog: &Catalog) -> Vec<String> {
        catalog.items.iter().map(|v| v.id.clone()).collect()
    }
    #[test]
    fn subtype_and_protection_do_not_confuse_audio_or_video_podcasts() {
        assert_eq!(VideoKind::from_flags(1, false), None);
        assert_eq!(VideoKind::from_flags(4, false), None);
        assert_eq!(
            VideoKind::from_flags(2 | 4, false),
            Some(VideoKind::VideoPodcast)
        );
        assert_eq!(
            VideoKind::from_flags(32, false),
            Some(VideoKind::MusicVideo)
        );
        assert_eq!(VideoKind::from_flags(64, false), Some(VideoKind::Tv));
        assert_eq!(VideoKind::from_flags(0, true), Some(VideoKind::Movie));
        let (_source, _target, device, path) = fixture();
        assert_eq!(
            sample_protection(&path).unwrap(),
            (Protection::Protected, true)
        );
        fs::write(&path, movie(b"avc1")).unwrap();
        assert_eq!(
            sample_protection(&path).unwrap(),
            (Protection::Unprotected, true)
        );
        fs::write(&path, b"unrecognized").unwrap();
        assert_eq!(
            list(&device).unwrap().items[0].protection,
            Protection::Unknown
        );
    }
    #[test]
    fn fallback_is_stable_excludes_audio_and_handles_corrupt_database() {
        let (_source, _target, device, path) = fixture();
        fs::write(path.with_file_name("audio.mp4"), movie(b"mp4a")).unwrap();
        let db = device.mount_path.join("iPod_Control/iTunes/iTunesDB");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        fs::write(&db, "broken db").unwrap();
        let first = list(&device).unwrap();
        let second = list(&device).unwrap();
        assert!(first.from_files);
        assert_eq!(first.items.len(), 1);
        assert_eq!(ids(&first), ids(&second));
    }
    #[test]
    fn protected_originals_are_unchanged_and_repeat_exports_verify_hashes() {
        let (_source, target, device, path) = fixture();
        let original = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let catalog = list(&device).unwrap();
        let chosen = ids(&catalog);
        let cancel = AtomicBool::new(false);
        let first = export(
            &device,
            &catalog,
            &chosen,
            target.path(),
            &cancel,
            &mut |_, _, _, _, _| {},
        )
        .unwrap();
        assert_eq!(first.items[0].status, "copied");
        assert_eq!(first.items[0].protection, Protection::Protected);
        let folder = destination(&device, target.path()).unwrap();
        assert_eq!(
            fs::read(folder.join(first.items[0].file.as_ref().unwrap())).unwrap(),
            original
        );
        let second = export(
            &device,
            &catalog,
            &chosen,
            target.path(),
            &cancel,
            &mut |_, _, _, _, _| {},
        )
        .unwrap();
        assert_eq!(second.items[0].status, "skipped");
        // A same-size corrupt destination must never be trusted or overwritten.
        let copied = folder.join(first.items[0].file.as_ref().unwrap());
        fs::write(&copied, vec![0; original.len()]).unwrap();
        let third = export(
            &device,
            &catalog,
            &chosen,
            target.path(),
            &cancel,
            &mut |_, _, _, _, _| {},
        )
        .unwrap();
        assert_eq!(third.items[0].status, "copied");
        assert_ne!(first.items[0].file, third.items[0].file);
        assert_eq!(fs::read(&copied).unwrap(), vec![0; original.len()]);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert!(
            first.items[0].metadata["path"]
                .as_str()
                .unwrap()
                .starts_with("iPod_Control/")
        );
    }
    #[test]
    fn resume_finds_verified_copies_after_a_collision_gap_and_title_change() {
        let (_source, target, device, path) = fixture();
        let original = fs::read(&path).unwrap();
        let mut catalog = list(&device).unwrap();
        let chosen = ids(&catalog);
        let folder = destination(&device, target.path()).unwrap();
        fs::create_dir(&folder).unwrap();
        let base = folder.join(format!(
            "{} [{}].m4v",
            safe_name(&catalog.items[0].title),
            catalog.items[0].id
        ));
        fs::write(&base, b"unrelated collision file").unwrap();
        let first = export(
            &device,
            &catalog,
            &chosen,
            target.path(),
            &AtomicBool::new(false),
            &mut |_, _, _, _, _| {},
        )
        .unwrap();
        let filename = first.items[0].file.as_ref().unwrap();
        assert!(filename.ends_with(" (1).m4v"));
        let copied = folder.join(filename);
        let modified = fs::metadata(&copied).unwrap().modified().unwrap();
        fs::remove_file(&base).unwrap();

        // Force the low-space branch without filling the host disk. Verified copies
        // still require no new video space, even when an earlier filename is absent.
        catalog.items[0].bytes = Some(u64::MAX);
        assert!(
            preview(&device, &catalog, &chosen, target.path())
                .unwrap()
                .fits
        );
        catalog.items[0].bytes = Some(original.len() as u64);
        catalog.items[0].title = "Edited title".into();
        let resumed = export(
            &device,
            &catalog,
            &chosen,
            target.path(),
            &AtomicBool::new(false),
            &mut |_, _, _, _, _| {},
        )
        .unwrap();
        assert_eq!(resumed.items[0].status, "skipped");
        assert_eq!(resumed.items[0].file, first.items[0].file);
        assert!(!base.exists());
        assert_eq!(fs::read(&copied).unwrap(), original);
        assert_eq!(fs::metadata(&copied).unwrap().modified().unwrap(), modified);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    #[test]
    fn cancellation_and_missing_files_have_reports_without_partial_videos() {
        let (_source, target, device, path) = fixture();
        let catalog = list(&device).unwrap();
        let chosen = ids(&catalog);
        let cancel = AtomicBool::new(false);
        let report = export(
            &device,
            &catalog,
            &chosen,
            target.path(),
            &cancel,
            &mut |_, _, _, bytes, _| {
                if bytes > 0 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap();
        assert!(report.stopped);
        assert_eq!(report.items[0].status, "failed");
        let folder = destination(&device, target.path()).unwrap();
        assert!(
            fs::read_dir(&folder)
                .unwrap()
                .all(|e| extension(&e.unwrap().path()) == "json")
        );
        cancel.store(false, Ordering::Relaxed);
        fs::remove_file(path).unwrap();
        let report = export(
            &device,
            &catalog,
            &chosen,
            target.path(),
            &cancel,
            &mut |_, _, _, _, _| {},
        )
        .unwrap();
        assert_eq!(report.items[0].status, "failed");
    }
    #[test]
    fn destination_and_selection_are_checked() {
        let (_source, target, device, _) = fixture();
        let catalog = list(&device).unwrap();
        assert!(preview(&device, &catalog, &ids(&catalog), &device.mount_path).is_err());
        assert!(preview(&device, &catalog, &["invented".into()], target.path()).is_err());
        assert!(safe_name(&"長".repeat(100)).len() <= 72);
    }
    #[test]
    fn disconnect_and_capacity_failures_leave_originals_intact() {
        let (_source, target, device, path) = fixture();
        let mut catalog = list(&device).unwrap();
        let chosen = ids(&catalog);
        let original = fs::read(&path).unwrap();
        catalog.items[0].bytes = Some(u64::MAX);
        assert!(
            !preview(&device, &catalog, &chosen, target.path())
                .unwrap()
                .fits
        );
        assert!(
            export(
                &device,
                &catalog,
                &chosen,
                target.path(),
                &AtomicBool::new(false),
                &mut |_, _, _, _, _| {}
            )
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        catalog.items[0].bytes = Some(original.len() as u64);
        // Keep the in-flight handle outside the marker directory: Windows refuses
        // to rename a directory containing an open file. Removing the marker still
        // simulates device loss between chunks while that handle remains readable.
        let in_flight = device.mount_path.join("in-flight.m4v");
        fs::copy(&path, &in_flight).unwrap();
        catalog.items[0].path = in_flight.clone();
        let marker = device.mount_path.join("iPod_Control");
        let detached = device.mount_path.join("detached");
        let report = export(
            &device,
            &catalog,
            &chosen,
            target.path(),
            &AtomicBool::new(false),
            &mut |_, _, _, bytes, _| {
                if bytes > 0 && marker.exists() {
                    fs::rename(&marker, &detached).unwrap();
                }
            },
        )
        .unwrap();
        fs::rename(&detached, &marker).unwrap();
        assert!(report.disconnected);
        assert_eq!(report.items[0].status, "failed");
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&in_flight).unwrap(), original);
        assert!(
            fs::read_dir(destination(&device, target.path()).unwrap())
                .unwrap()
                .all(|e| extension(&e.unwrap().path()) == "json")
        );
    }
    #[cfg(unix)]
    #[test]
    fn source_and_destination_symlink_escapes_are_rejected() {
        use std::os::unix::fs::symlink;
        let (_source, target, device, path) = fixture();
        let outside = target.path().join("outside.m4v");
        fs::write(&outside, movie(b"avc1")).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();
        assert!(list(&device).unwrap().items.is_empty());
        let folder = destination(&device, target.path()).unwrap();
        symlink(&device.mount_path, &folder).unwrap();
        assert!(destination(&device, target.path()).is_err());
    }
}
