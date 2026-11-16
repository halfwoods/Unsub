use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{CommandError, CommandResult};
use crate::library::LibraryState;

const BACKUP_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const BACKUP_CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Remove permanent trim backups after their 30-day recovery window.
pub fn start_backup_cleanup(path: PathBuf) {
    let result = std::thread::Builder::new().name("trim-backup-cleanup".to_owned()).spawn(move || loop {
        if let Err(e) = cleanup_expired_backups(&path, SystemTime::now()) {
            eprintln!("Could not clean expired trim backups: {e}");
        }
        std::thread::sleep(BACKUP_CLEANUP_INTERVAL);
    });
    if let Err(e) = result {
        eprintln!("Could not start trim backup cleanup: {e}");
    }
}

fn cleanup_expired_backups(path: &Path, now: SystemTime) -> std::io::Result<usize> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let cutoff = now.checked_sub(BACKUP_RETENTION).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() || !entry.file_name().to_string_lossy().starts_with("track-") {
            continue;
        }
        if entry.metadata()?.modified().is_ok_and(|modified| modified <= cutoff) {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    if removed > 0 {
        eprintln!("Removed {removed} trim backup(s) older than 30 days.");
    }
    Ok(removed)
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongTrim {
    pub start_ms: u64,
    pub end_ms: u64,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrimEntry {
    path: String,
    #[serde(flatten)]
    trim: SongTrim,
}

pub struct TrimState {
    path: PathBuf,
    entries: Mutex<HashMap<i64, TrimEntry>>,
    applying: Mutex<()>,
}

impl TrimState {
    pub fn load(path: PathBuf) -> Self {
        let entries = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { path, entries: Mutex::new(entries), applying: Mutex::new(()) }
    }

    pub fn enabled_range(&self, id: i64, path: &str, duration: u64) -> Option<(u64, u64)> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(&id)
            .filter(|entry| entry.path == path).map(|entry| entry.trim)
            .filter(|trim| trim.enabled && trim.end_ms > trim.start_ms && trim.end_ms <= duration)
            .map(|trim| (trim.start_ms, trim.end_ms))
    }

    fn get(&self, id: i64, path: &str, duration: u64) -> SongTrim {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(&id)
            .filter(|entry| entry.path == path).map(|entry| entry.trim)
            .filter(|trim| trim.start_ms < trim.end_ms && trim.end_ms <= duration)
            .unwrap_or(SongTrim { start_ms: 0, end_ms: duration, enabled: false })
    }

    fn set(&self, id: i64, path: &str, value: SongTrim) -> CommandResult<()> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let parent = self.path.parent().ok_or_else(|| CommandError::from("No preferences folder".to_owned()))?;
        std::fs::create_dir_all(parent).map_err(|e| CommandError::from(e.to_string()))?;
        // Another instance may have saved a different song since this one started.
        let mut next: HashMap<i64, TrimEntry> = match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| error(format!("Cannot read saved trim settings: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => entries.clone(),
            Err(e) => return Err(error(e)),
        };
        next.insert(id, TrimEntry { path: path.to_owned(), trim: value });
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| CommandError::from(e.to_string()))?;
        serde_json::to_writer(&mut temp, &next).map_err(|e| CommandError::from(e.to_string()))?;
        temp.as_file().sync_all().map_err(|e| CommandError::from(e.to_string()))?;
        temp.persist(&self.path).map_err(|e| CommandError::from(e.to_string()))?;
        sync_directory(parent).map_err(error)?;
        *entries = next;
        Ok(())
    }
}

#[tauri::command]
pub fn trim_get(library: State<'_, LibraryState>, trims: State<'_, TrimState>, track_id: i64) -> CommandResult<SongTrim> {
    let track = library.queries().track(track_id)?.ok_or_else(|| CommandError::from("Song not found".to_owned()))?;
    Ok(trims.get(track_id, &track.path, track.duration_ms))
}

#[tauri::command]
pub fn trim_set(library: State<'_, LibraryState>, trims: State<'_, TrimState>, track_id: i64, trim: SongTrim) -> CommandResult<()> {
    let track = library.queries().track(track_id)?.ok_or_else(|| CommandError::from("Song not found".to_owned()))?;
    if track.kind != unsub_library::MediaKind::Music || track.duration_ms == 0 || trim.start_ms >= trim.end_ms || trim.end_ms > track.duration_ms {
        return Err(CommandError::from("Choose start and end times within the song, with the end after the start.".to_owned()));
    }
    let _editing = trims.applying.lock().unwrap_or_else(|e| e.into_inner());
    let _lock = mutation_lock(&trims)?;
    trims.set(track_id, &track.path, trim)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrimResult {
    backup_path: String,
    warning: Option<String>,
}

/// Encode and verify a new file, keep a backup, then atomically replace the original.
#[tauri::command]
pub async fn trim_apply(app: AppHandle, track_id: i64, trim: SongTrim) -> CommandResult<TrimResult> {
    tauri::async_runtime::spawn_blocking(move || apply(&app, track_id, trim)).await
        .map_err(|e| CommandError::from(e.to_string()))?
}

fn error(message: impl ToString) -> CommandError { CommandError::from(message.to_string()) }

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> { File::open(path)?.sync_all() }

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> { Ok(()) }

fn mutation_lock(trims: &TrimState) -> CommandResult<File> {
    let parent = trims.path.parent().ok_or_else(|| error("No preferences folder."))?;
    fs::create_dir_all(parent).map_err(error)?;
    let lock = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(parent.join("SongTrims.lock")).map_err(error)?;
    fs4::FileExt::try_lock(&lock).map_err(|_| error("Another trim operation is already running."))?;
    Ok(lock)
}

fn hash_file(path: &Path) -> CommandResult<[u8; 32]> {
    let mut file = File::open(path).map_err(error)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(error)?;
        if count == 0 { break; }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}

fn probe(path: &Path) -> CommandResult<serde_json::Value> {
    let output = Command::new("ffprobe").args(["-v", "error", "-show_streams", "-show_chapters", "-show_format", "-of", "json"])
        .arg(path).output().map_err(|e| error(format!("Could not run ffprobe: {e}. Install FFmpeg to trim files.")))?;
    if !output.status.success() { return Err(error(format!("Could not inspect audio: {}", String::from_utf8_lossy(&output.stderr)))); }
    serde_json::from_slice(&output.stdout).map_err(error)
}

struct Prepared {
    file: tempfile::NamedTempFile,
    original_hash: [u8; 32],
    permissions: fs::Permissions,
    suffix: String,
    actual_ms: u64,
}

fn prepare(path: &Path, trim: SongTrim, fallback_bitrate_kbps: Option<u32>) -> CommandResult<Prepared> {
    let file_metadata = fs::symlink_metadata(path).map_err(error)?;
    if !file_metadata.file_type().is_file() { return Err(error("The original must be a regular local file.")); }
    let original_hash = hash_file(path)?;
    let before = probe(path)?;
    let streams = before["streams"].as_array().ok_or_else(|| error("The audio file has no readable streams."))?;
    let audio: Vec<_> = streams.iter().filter(|s| s["codec_type"] == "audio").collect();
    if audio.len() != 1 || streams.iter().any(|s| s["codec_type"] != "audio" && !(s["codec_type"] == "video" && s["disposition"]["attached_pic"] == 1))
        || before["chapters"].as_array().is_some_and(|chapters| !chapters.is_empty()) {
        return Err(error("This file has multiple streams or chapters, which cannot yet be trimmed safely."));
    }
    let codec = audio[0]["codec_name"].as_str().unwrap_or("");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    // MP3 audio frames can be copied without decoding or another lossy encode.
    // Keep FFmpeg's fast-seek flag disabled so it does not use a coarse Xing TOC.
    let stream_copy = ext == "mp3" && codec == "mp3";
    let encoder = match (ext.as_str(), codec) {
        ("mp3", "mp3") => "libmp3lame",
        ("m4a", "aac") => "aac",
        ("m4a", "alac") => "alac",
        ("flac", "flac") => "flac",
        ("ogg" | "oga", "vorbis") => "libvorbis",
        ("opus" | "ogg" | "oga", "opus") => "libopus",
        ("wv", "wavpack") => "wavpack",
        ("wav", c) if c.starts_with("pcm_") => c,
        ("aif" | "aiff", c) if c.starts_with("pcm_") => c,
        _ => return Err(error("This audio format is not yet supported for permanent trimming; the original was not changed.")),
    };
    let lossy = matches!(codec, "mp3" | "aac" | "vorbis" | "opus");
    let lossy_reencode = lossy && !stream_copy;
    let bit_rate = audio[0]["bit_rate"].as_str().and_then(|v| v.parse::<u64>().ok())
        .or_else(|| fallback_bitrate_kbps.map(|kbps| u64::from(kbps) * 1000));
    if lossy_reencode && bit_rate.is_none() { return Err(error("The audio bitrate could not be determined; the original was not changed.")); }

    let parent = path.parent().ok_or_else(|| error("The song has no containing folder."))?;
    let suffix = format!(".{ext}");
    let temp = tempfile::Builder::new().prefix(".unsub-trim-").suffix(&suffix).tempfile_in(parent).map_err(error)?;
    let rate = audio[0]["sample_rate"].as_str().and_then(|v| v.parse::<u64>().ok()).filter(|rate| *rate > 0)
        .ok_or_else(|| error("The audio sample rate could not be determined."))?;
    let start_frame = trim.start_ms.checked_mul(rate).ok_or_else(|| error("Start time is too large."))? / 1000;
    let end_frame = trim.end_ms.checked_mul(rate).ok_or_else(|| error("End time is too large."))? / 1000;
    let filter = format!("atrim=start_sample={start_frame}:end_sample={end_frame},asetpts=PTS-STARTPTS");
    let start = format!("{}.{:03}", trim.start_ms / 1000, trim.start_ms % 1000);
    let length_ms = trim.end_ms - trim.start_ms;
    let length = format!("{}.{:03}", length_ms / 1000, length_ms % 1000);
    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
    if stream_copy {
        // Input-side seeking uses FFmpeg's normal MP3 seeking path; fastseek's
        // coarse Xing TOC path is explicitly disabled. Packet boundaries still
        // determine the final cut points, so the encoded frames remain intact.
        command.args(["-fflags", "-fastseek", "-ss", &start]);
    }
    command.arg("-i").arg(path)
        .args(["-map", "0:a:0", "-map", "0:v?", "-map_metadata", "0", "-map_chapters", "-1"]);
    if stream_copy {
        command.args(["-t", &length, "-c:a", "copy"]);
    } else {
        command.args(["-af", &filter, "-c:a", encoder]);
        if lossy_reencode { command.args(["-b:a", &bit_rate.unwrap().to_string()]); }
    }
    command.args(["-c:v", "copy"]);
    if ext == "mp3" { command.args(["-id3v2_version", "3"]); }
    let output = command.arg(temp.path()).output().map_err(|e| error(format!("Could not run FFmpeg: {e}")))?;
    if !output.status.success() { return Err(error(format!("Could not trim audio: {}", String::from_utf8_lossy(&output.stderr)))); }
    let after = probe(temp.path())?;
    let out_streams = after["streams"].as_array().ok_or_else(|| error("The new file has no readable streams."))?;
    let new_audio = out_streams.iter().find(|s| s["codec_type"] == "audio").ok_or_else(|| error("The new file has no audio."))?;
    if new_audio["codec_name"] != audio[0]["codec_name"] || new_audio["sample_rate"] != audio[0]["sample_rate"] || new_audio["channels"] != audio[0]["channels"]
        || audio[0]["channel_layout"].as_str().is_some_and(|layout| layout != "unknown" && new_audio["channel_layout"].as_str() != Some(layout))
        || (!lossy && audio[0]["bits_per_raw_sample"].as_str().is_some_and(|bits| bits != "0" && new_audio["bits_per_raw_sample"].as_str() != Some(bits)))
        || out_streams.iter().filter(|s| s["codec_type"] == "video").count() != streams.iter().filter(|s| s["codec_type"] == "video").count() {
        return Err(error("The trimmed file did not preserve the audio format or embedded artwork; the original was not changed."));
    }
    // Decode the whole result to check it is playable and has the selected duration.
    let mut decoder = unsub_playback::Decoder::open(temp.path()).map_err(error)?;
    let rate = u64::from(decoder.sample_rate());
    let channels = usize::from(decoder.channels());
    if rate == 0 || channels == 0 { return Err(error("The trimmed audio has an invalid format.")); }
    let mut original = if lossy { None } else { Some(unsub_playback::Decoder::open(path).map_err(error)?) };
    let mut to_skip = start_frame.saturating_mul(channels as u64);
    let mut source_buffer: Vec<f32> = Vec::new();
    let mut source_offset = 0usize;
    let mut frames = 0u64;
    while let Some(chunk) = decoder.next_chunk().map_err(error)? {
        frames += (chunk.len() / channels) as u64;
        if let Some(source) = original.as_mut() {
            let mut offset = 0;
            while offset < chunk.len() {
                while source_offset >= source_buffer.len() {
                    let next = source.next_chunk().map_err(error)?.ok_or_else(|| error("The original ended before the trimmed audio. It was not changed."))?;
                    let skip = next.len().min(usize::try_from(to_skip).unwrap_or(usize::MAX));
                    to_skip -= skip as u64;
                    source_buffer.clear();
                    source_buffer.extend_from_slice(&next[skip..]);
                    source_offset = 0;
                }
                let count = (chunk.len() - offset).min(source_buffer.len() - source_offset);
                if chunk[offset..offset + count].iter().zip(&source_buffer[source_offset..source_offset + count])
                    .any(|(a, b)| (a - b).abs() > 0.00001) {
                    return Err(error("The lossless trim did not match the selected audio; the original was not changed."));
                }
                offset += count;
                source_offset += count;
            }
        }
    }
    let actual_ms = frames.saturating_mul(1000) / rate;
    let selected_ms = trim.end_ms - trim.start_ms;
    let tolerance_ms = if stream_copy { 100 } else { (selected_ms / 20).clamp(2, 50) };
    if frames == 0 || actual_ms.abs_diff(selected_ms) > tolerance_ms {
        return Err(error("The trimmed audio duration did not match the selected times; the original was not changed."));
    }
    drop(decoder);
    Ok(Prepared { file: temp, original_hash, permissions: file_metadata.permissions(), suffix, actual_ms })
}

/// Keeps a byte-for-byte original before atomically switching the path to the verified result.
fn commit(
    path: &Path,
    prepared: Prepared,
    backups: &Path,
    track_id: i64,
    before_replace: impl FnOnce() -> CommandResult<()>,
) -> CommandResult<(PathBuf, u64, Option<String>)> {
    let Prepared { file: temp, original_hash, permissions, suffix, actual_ms } = prepared;
    if hash_file(path)? != original_hash { return Err(error("The original changed during the trim; it was not replaced.")); }
    fs::create_dir_all(backups).map_err(error)?;
    let mut backup = tempfile::Builder::new().prefix(&format!("track-{track_id}-")).suffix(&suffix).tempfile_in(backups).map_err(error)?;
    let mut input = File::open(path).map_err(error)?;
    std::io::copy(&mut input, &mut backup).map_err(error)?;
    backup.flush().map_err(error)?;
    backup.as_file().sync_all().map_err(error)?;
    if hash_file(backup.path())? != original_hash || hash_file(path)? != original_hash {
        return Err(error("The original changed during the trim; it was not replaced."));
    }
    fs::set_permissions(temp.path(), permissions).map_err(error)?;
    temp.as_file().sync_all().map_err(error)?;
    let (_, backup_path) = backup.keep().map_err(error)?;
    sync_directory(backups).map_err(|e| error(format!("Could not safely store the backup: {e}. The original was not replaced.")))?;
    before_replace()?;
    // Check once more after saving settings, before the atomic rename.
    if hash_file(path)? != original_hash { return Err(error(format!("The original changed; it was not replaced. Backup: {}", backup_path.display()))); }
    temp.persist(path).map_err(|e| error(format!("Could not replace the song: {e}. Backup: {}", backup_path.display())))?;
    let warning = path.parent().and_then(|parent| sync_directory(parent).err())
        .map(|e| format!("The file was replaced, but its folder could not be synced: {e}"));
    Ok((backup_path, actual_ms, warning))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/playback/tests/fixtures").join(name)
    }

    #[test]
    fn preparing_trim_keeps_original_and_verifies_multiple_formats() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["sine.flac", "sine.mp3", "sine-aac.m4a", "sine-alac.m4a", "sine-mono.wav", "sine.ogg", "sine.opus", "sine.wv", "sine-22k.aiff", "sine-24bit.flac", "tagged.flac", "surround-5.1.flac", "surround-7.1.flac"] {
            let original = dir.path().join(name);
            fs::copy(fixture(name), &original).unwrap();
            let before = hash_file(&original).unwrap();
            let range = match name {
                "tagged.flac" => (50, 200),
                "sine-24bit.flac" => (20, 150),
                "sine.opus" | "sine.wv" => (100, 400),
                "surround-5.1.flac" | "surround-7.1.flac" => (5, 15),
                _ => (200, 700),
            };
            let made = prepare(&original, SongTrim { start_ms: range.0, end_ms: range.1, enabled: false }, Some(64))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(hash_file(&original).unwrap(), before, "{name}");
            assert!(made.actual_ms.abs_diff(range.1 - range.0) <= 50, "{name}");
            if matches!(name, "sine.flac" | "sine-alac.m4a" | "sine-mono.wav" | "sine.wv" | "sine-22k.aiff" | "sine-24bit.flac" | "surround-5.1.flac" | "surround-7.1.flac") {
                let mut source = unsub_playback::Decoder::open(&original).unwrap();
                let channels = usize::from(source.channels());
                let rate = source.sample_rate() as usize;
                let mut input_samples = Vec::new();
                while let Some(chunk) = source.next_chunk().unwrap() { input_samples.extend_from_slice(chunk); }
                let mut output = unsub_playback::Decoder::open(made.file.path()).unwrap();
                let mut output_samples = Vec::new();
                while let Some(chunk) = output.next_chunk().unwrap() { output_samples.extend_from_slice(chunk); }
                let start = rate * range.0 as usize / 1000 * channels;
                let end = rate * range.1 as usize / 1000 * channels;
                assert_eq!(output_samples.len(), end - start, "{name}");
                assert!(output_samples.iter().zip(&input_samples[start..end]).all(|(a, b)| (a - b).abs() < 0.00001), "selected audio changed for {name}");
            }
            if matches!(name, "sine.mp3" | "sine-aac.m4a" | "sine.ogg" | "sine.opus") {
                let mut source = unsub_playback::Decoder::open(&original).unwrap();
                let channels = usize::from(source.channels());
                let rate = source.sample_rate() as usize;
                let mut input_samples = Vec::new();
                while let Some(chunk) = source.next_chunk().unwrap() { input_samples.extend_from_slice(chunk); }
                let mut output = unsub_playback::Decoder::open(made.file.path()).unwrap();
                let mut output_samples = Vec::new();
                while let Some(chunk) = output.next_chunk().unwrap() { output_samples.extend_from_slice(chunk); }
                let start = rate * range.0 as usize / 1000 * channels;
                // Ignore the first and last 20 ms where a lossy encoder settles after cutting.
                let margin = rate / 50 * channels;
                let compare = output_samples.len().min(input_samples.len() - start);
                let difference: f32 = output_samples[margin..compare-margin].iter()
                    .zip(&input_samples[start+margin..start+compare-margin])
                    .map(|(a, b)| (a - b).abs()).sum::<f32>() / (compare - 2 * margin) as f32;
                assert!(difference < 0.06, "selected timing differs for {name}: mean error {difference}");
            }
            if name == "tagged.flac" {
                let original_info = probe(&original).unwrap();
                let result_info = probe(made.file.path()).unwrap();
                assert_eq!(original_info["format"]["tags"]["title"], "Sine Title");
                assert_eq!(original_info["format"]["tags"]["artist"], "Test Artist");
                assert_eq!(original_info["format"]["tags"]["title"], result_info["format"]["tags"]["title"]);
                assert_eq!(original_info["format"]["tags"]["artist"], result_info["format"]["tags"]["artist"]);
                assert_eq!(result_info["streams"].as_array().unwrap().iter().filter(|s| s["disposition"]["attached_pic"] == 1).count(), 1);
            }
        }
    }

    #[test]
    fn trim_settings_round_trip_and_only_enabled_ranges_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trims.json");
        let state = TrimState::load(path.clone());
        state.set(5, "/music/a.flac", SongTrim { start_ms: 200, end_ms: 700, enabled: false }).unwrap();
        assert_eq!(state.enabled_range(5, "/music/a.flac", 1000), None);
        state.set(5, "/music/a.flac", SongTrim { start_ms: 200, end_ms: 700, enabled: true }).unwrap();
        let restored = TrimState::load(path);
        assert_eq!(restored.enabled_range(5, "/music/a.flac", 1000), Some((200, 700)));
        assert_eq!(restored.enabled_range(5, "/music/different.flac", 1000), None);
        assert_eq!(restored.enabled_range(5, "/music/a.flac", 500), None);
    }

    #[test]
    fn two_instances_cannot_mutate_trim_settings_simultaneously() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trims.json");
        let first = TrimState::load(path.clone());
        let second = TrimState::load(path);
        let held = mutation_lock(&first).unwrap();
        assert!(mutation_lock(&second).is_err());
        drop(held);
        assert!(mutation_lock(&second).is_ok());
    }

    #[test]
    fn unsupported_file_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.ape");
        fs::copy(fixture("sine.ape"), &path).unwrap();
        let before = hash_file(&path).unwrap();
        assert!(prepare(&path, SongTrim { start_ms: 10, end_ms: 100, enabled: false }, None).is_err());
        assert_eq!(hash_file(&path).unwrap(), before);
    }

    #[test]
    fn commit_replaces_only_after_a_verified_backup_and_refuses_a_changed_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.flac");
        fs::copy(fixture("sine.flac"), &path).unwrap();
        let original_hash = hash_file(&path).unwrap();
        let trim = SongTrim { start_ms: 200, end_ms: 700, enabled: true };
        let prepared = prepare(&path, trim, None).unwrap();
        let (backup, _, warning) = commit(&path, prepared, &dir.path().join("backups"), 1, || Ok(())).unwrap();
        assert!(warning.is_none());
        assert_eq!(hash_file(&backup).unwrap(), original_hash);
        assert_ne!(hash_file(&path).unwrap(), original_hash);
        assert!(unsub_playback::Decoder::open(&path).unwrap().duration_ms().unwrap().abs_diff(500) < 5);

        let prepared = prepare(&path, SongTrim { start_ms: 50, end_ms: 400, enabled: true }, None).unwrap();
        fs::write(&path, b"changed externally").unwrap();
        assert!(commit(&path, prepared, &dir.path().join("backups"), 1, || Ok(())).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"changed externally");
        assert_eq!(hash_file(&backup).unwrap(), original_hash);
    }

    #[test]
    fn commit_does_not_replace_if_settings_cannot_be_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.flac");
        fs::copy(fixture("sine.flac"), &path).unwrap();
        let before = hash_file(&path).unwrap();
        let prepared = prepare(&path, SongTrim { start_ms: 200, end_ms: 700, enabled: true }, None).unwrap();
        assert!(commit(&path, prepared, &dir.path().join("backups"), 1, || Err(error("cannot save settings"))).is_err());
        assert_eq!(hash_file(&path).unwrap(), before);
    }

    #[test]
    fn library_rescan_keeps_the_song_identity_and_updates_duration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.flac");
        fs::copy(fixture("tagged.flac"), &path).unwrap();
        let mut library = unsub_library::Library::open_in_memory().unwrap();
        library.add_paths(std::slice::from_ref(&path), |_| {}).unwrap();
        let original = library.tracks(&Default::default()).unwrap().pop().unwrap();
        let prepared = prepare(&path, SongTrim { start_ms: 50, end_ms: 200, enabled: false }, None).unwrap();
        commit(&path, prepared, &dir.path().join("backups"), original.id, || Ok(())).unwrap();
        let summary = library.add_paths(std::slice::from_ref(&path), |_| {}).unwrap();
        assert!(summary.failed.is_empty());
        assert_eq!(summary.updated, 1);
        let updated = library.track(original.id).unwrap().unwrap();
        assert_eq!(updated.title, original.title);
        assert_eq!(updated.artist, original.artist);
        assert!(updated.duration_ms.abs_diff(150) <= 5, "duration was {}", updated.duration_ms);
    }
}

fn apply(app: &AppHandle, track_id: i64, trim: SongTrim) -> CommandResult<TrimResult> {
    let trims = app.state::<TrimState>();
    let _editing = trims.applying.lock().unwrap_or_else(|e| e.into_inner());
    // Use one lock file for all instances sharing this library, including separate app processes.
    let _lock = mutation_lock(&trims)?;
    let library = app.state::<LibraryState>();
    let track = library.queries().track(track_id)?.ok_or_else(|| error("Song not found."))?;
    if track.kind != unsub_library::MediaKind::Music || track.duration_ms == 0 || trim.start_ms >= trim.end_ms || trim.end_ms > track.duration_ms {
        return Err(error("Choose start and end times within the song, with the end after the start."));
    }
    if trim.start_ms == 0 && trim.end_ms >= track.duration_ms {
        return Err(error("Choose a shorter section before making a permanent trim."));
    }
    let path = PathBuf::from(&track.path);
    if track.in_cloud || track.missing || unsub_library::is_dataless(&path) {
        return Err(error("Download or locate the original song file before trimming it."));
    }
    let prepared = prepare(&path, trim, track.bitrate_kbps)?;
    let playback = app.state::<crate::playback::PlaybackState>();
    if playback.stop_for_file_edit(track_id) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while playback.status().state != unsub_playback::PlaybackState::Stopped && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if playback.status().state != unsub_playback::PlaybackState::Stopped {
            return Err(error("Playback did not stop; the original was not changed."));
        }
    }
    let backups = app.path().app_data_dir().map_err(error)?.join("Trim Backups");
    let (backup_path, warnings) = library.with_writer_command(|catalog| {
        let current = catalog.track(track_id)?.ok_or_else(|| error("The song was removed from the library; its file was not replaced."))?;
        if current.path != track.path { return Err(error("The song moved during the trim; its file was not replaced.")); }
        // Holding this lock prevents the song from being started or queued while it is replaced.
        let playing = playback.playing();
        if playing.as_ref().is_some_and(|p| p.source == crate::playback::PlaySource::Library
            && (p.queue.current() == track_id || p.queue.peek_next() == Some(track_id))) {
            return Err(error("The song started playing again; its file was not replaced."));
        }
        let (backup_path, actual_ms, sync_warning) = commit(&path, prepared, &backups, track_id, || trims.set(track_id, &track.path, SongTrim { enabled: false, ..trim }))?;
        drop(playing);
        let mut warnings = Vec::new();
        if let Some(warning) = sync_warning { warnings.push(warning); }
        if let Err(e) = trims.set(track_id, &track.path, SongTrim { start_ms: 0, end_ms: actual_ms, enabled: false }) { warnings.push(format!("Could not reset trim settings: {e}")); }
        match catalog.add_paths(std::slice::from_ref(&path), |_| {}) {
            Ok(summary) if summary.failed.is_empty() && summary.scanned > 0 => {},
            Ok(_) => warnings.push("The library could not refresh this song; rescan it from its folder.".to_owned()),
            Err(e) => warnings.push(format!("The library could not refresh this song: {e}")),
        }
        Ok((backup_path, warnings))
    })?;
    let _ = app.emit(crate::library::CHANGED_EVENT, ());
    Ok(TrimResult { backup_path: backup_path.to_string_lossy().into_owned(), warning: (!warnings.is_empty()).then(|| warnings.join(" ")) })
}
