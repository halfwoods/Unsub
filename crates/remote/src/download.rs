//! Songs from a remote library, fetched into a cache folder so the local player can decode them.
//!
//! The playback crate decodes files, not URLs. A download therefore runs on its own thread and
//! writes into `<key>.<ext>.part`; a `GrowingReader` follows that file as it grows, so playback can
//! start after the first few hundred kilobytes instead of after the last. When the last byte lands
//! the file is renamed to `<key>.<ext>`, which is also how an interrupted download is told apart
//! from a finished one: a `.part` file is resumed with an HTTP `Range` request, never trusted.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use serde::Serialize;
use ureq::Body;
use ureq::Timeout;
use ureq::http::Response;

use crate::cache;
use crate::client::{Client, check_media};
use crate::error::{Error, Result};

const CHUNK: usize = 64 * 1024;

/// How much of a download has arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub downloaded: u64,
    /// `None` while the server has not said how big the file is.
    pub total: Option<u64>,
    pub finished: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    /// The `.part` file while running, the finished file after.
    path: PathBuf,
    downloaded: u64,
    total: Option<u64>,
    phase: Phase,
    cancelled: bool,
    /// Set by `detach`: nobody is holding on, and the download should finish regardless.
    detached: bool,
}

enum Phase {
    Running,
    Done,
    Failed(String),
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed
            .wait(state)
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Held by the `Download` and by every reader taken from it. When the last one is dropped, a
/// download that is still running and was not detached is cancelled.
struct Lifeline(Arc<Shared>);

impl Drop for Lifeline {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        if !state.detached && matches!(state.phase, Phase::Running) {
            state.cancelled = true;
        }
    }
}

/// A song being fetched into the cache.
///
/// Dropping the handle cancels the download, so a skipped song stops costing bandwidth; the
/// partial file stays and a later request resumes it. It is not cancelled while a reader taken
/// from it is still alive, since that is playback in progress. Call [`Download::wait`] to
/// see it through, or [`Download::detach`] to let it finish in the background with nobody
/// waiting.
pub struct Download {
    shared: Arc<Shared>,
    lifeline: Arc<Lifeline>,
    thread: Option<JoinHandle<Result<PathBuf>>>,
}

impl Download {
    fn new(shared: Arc<Shared>, thread: Option<JoinHandle<Result<PathBuf>>>) -> Self {
        Self {
            lifeline: Arc::new(Lifeline(Arc::clone(&shared))),
            shared,
            thread,
        }
    }

    fn finished(path: PathBuf) -> Self {
        let size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                path,
                downloaded: size,
                total: Some(size),
                phase: Phase::Done,
                cancelled: false,
                detached: false,
            }),
            changed: Condvar::new(),
        });
        Self::new(shared, None)
    }

    /// Where the bytes are: the partial file while downloading, the finished file afterwards.
    pub fn path(&self) -> PathBuf {
        self.shared.lock().path.clone()
    }

    pub fn progress(&self) -> Progress {
        let state = self.shared.lock();
        Progress {
            downloaded: state.downloaded,
            total: state.total,
            finished: matches!(state.phase, Phase::Done),
        }
    }

    /// Blocks until at least `bytes` have arrived, or the whole song has if it is smaller. This
    /// is the signal that playback can begin.
    pub fn wait_for(&self, bytes: u64) -> Result<()> {
        let mut state = self.shared.lock();
        loop {
            if state.downloaded >= bytes {
                return Ok(());
            }
            match &state.phase {
                Phase::Done => return Ok(()),
                Phase::Failed(message) => return Err(Error::DownloadFailed(message.clone())),
                Phase::Running => state = self.shared.wait(state),
            }
        }
    }

    /// A reader over the song that waits for bytes that have not arrived yet instead of ending
    /// early. It can seek forwards past what has arrived (the read then waits), and from the end
    /// once the length is known. The download keeps going for as long as the reader exists.
    pub fn reader(&self) -> Result<GrowingReader> {
        // Opened while holding the lock, so the rename to the finished name cannot slip in
        // between choosing the path and opening it.
        let state = self.shared.lock();
        let file = File::open(&state.path)?;
        Ok(GrowingReader {
            file,
            shared: Arc::clone(&self.shared),
            _lifeline: Arc::clone(&self.lifeline),
            position: 0,
        })
    }

    /// Stops the download after the chunk in flight. The partial file is kept for resuming.
    pub fn cancel(&self) {
        self.shared.lock().cancelled = true;
    }

    /// Lets the download finish in the background with nobody waiting; the cache keeps the
    /// result for next time.
    pub fn detach(mut self) {
        self.shared.lock().detached = true;
        // The thread carries on without a handle to join.
        drop(self.thread.take());
    }

    /// Waits for the song to finish and returns the cached file.
    pub fn wait(mut self) -> Result<PathBuf> {
        match self.thread.take() {
            Some(thread) => thread.join().unwrap_or_else(|_| {
                Err(Error::DownloadFailed(
                    "the download thread panicked".to_owned(),
                ))
            }),
            None => Ok(self.path()),
        }
    }
}

/// Reads a song that is still arriving. At the end of what has been downloaded it waits for more
/// rather than reporting the end of the file; it reports the end only once the download is done,
/// and an error if the download failed.
pub struct GrowingReader {
    file: File,
    shared: Arc<Shared>,
    _lifeline: Arc<Lifeline>,
    position: u64,
}

impl GrowingReader {
    /// The length of the song, once the server has said or the download has finished.
    pub fn len(&self) -> Option<u64> {
        let state = self.shared.lock();
        match state.phase {
            Phase::Done => Some(state.downloaded),
            _ => state.total,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }
}

impl Read for GrowingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let available = {
            let mut state = self.shared.lock();
            loop {
                if self.position < state.downloaded {
                    break state.downloaded;
                }
                match &state.phase {
                    Phase::Done => return Ok(0),
                    Phase::Failed(message) => return Err(io::Error::other(message.clone())),
                    Phase::Running => state = self.shared.wait(state),
                }
            }
        };
        let ready = usize::try_from(available - self.position).unwrap_or(usize::MAX);
        let count = ready.min(buffer.len());
        self.file.seek(SeekFrom::Start(self.position))?;
        let read = self.file.read(&mut buffer[..count])?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for GrowingReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => {
                let length = self.len().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Unsupported, "the length is not known yet")
                })?;
                length.checked_add_signed(offset)
            }
        };
        self.position = target.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before the start of the song",
            )
        })?;
        Ok(self.position)
    }
}

impl Client {
    /// Fetches a song into `dir` and returns the finished file. A song already in the cache is
    /// returned at once; an interrupted download is resumed. See [`Client::start_download`] for
    /// playing while it arrives.
    pub fn download_to_cache(&self, song_id: &str, dir: &Path) -> Result<PathBuf> {
        self.start_download(song_id, dir)?.wait()
    }

    /// Starts fetching a song and returns as soon as the server has agreed to send it, so that a
    /// missing song or a wrong password fails here and not later. The song is the original file
    /// (`format=raw`), whose bytes are the same on every request; that is what makes resuming
    /// valid, which a transcode would not be.
    ///
    /// The cache folder is kept under the size set by [`Client::with_cache_limit`]: before a
    /// download starts, the least recently used other songs are deleted to make room. A song
    /// larger than the whole cache is refused. A server that goes quiet for longer than
    /// [`Client::with_stall_timeout`] fails the download with [`Error::Stalled`], keeping what
    /// arrived.
    pub fn start_download(&self, song_id: &str, dir: &Path) -> Result<Download> {
        let key = cache::key(&self.base_url, &self.user, song_id);
        if let Some(cached) = cache::find_finished(dir, &key) {
            cache::touch(&cached);
            return Ok(Download::finished(cached));
        }
        fs::create_dir_all(dir)?;
        let mut partial = cache::find_partial(dir, &key);
        let have = partial.as_ref().map_or(0, |(path, _)| file_size(path));
        let url = self.url("stream", &[("id", song_id), ("format", "raw")]);

        let mut reply = self.request(&url, have)?;
        if matches!(reply, Reply::RangeRejected) {
            // The partial file cannot be continued from this server; begin again.
            discard(partial.take());
            reply = self.request(&url, 0)?;
        }
        let (response, offset, total, content_type) = match reply {
            Reply::Body {
                response,
                offset,
                total,
                content_type,
            } => (response, offset, total, content_type),
            Reply::Complete => {
                let Some((part, extension)) = partial else {
                    return Err(Error::Unexpected(
                        "the server sent an empty song".to_owned(),
                    ));
                };
                let finished = cache::final_path(dir, &key, &extension);
                replace(&part, &finished)?;
                return Ok(Download::finished(finished));
            }
            Reply::RangeRejected => {
                return Err(Error::Unexpected(
                    "the server refused to send the song from the start".to_owned(),
                ));
            }
        };

        if let Some(size) = total.filter(|size| *size > self.cache_limit) {
            return Err(Error::TooLargeForCache {
                size,
                limit: self.cache_limit,
            });
        }
        cache::make_room(dir, &key, total.unwrap_or(0), self.cache_limit)?;

        // A resumed download keeps the extension it began with; a fresh one takes it from the
        // type the server just named.
        let (part, extension, file) = match partial {
            Some((part, extension)) if offset > 0 => {
                let file = OpenOptions::new().append(true).open(&part)?;
                (part, extension, file)
            }
            old => {
                discard(old);
                let extension = cache::extension(content_type.as_deref());
                let part = cache::part_path(dir, &key, extension);
                (part.clone(), extension.to_owned(), File::create(&part)?)
            }
        };

        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                path: part.clone(),
                downloaded: offset,
                total,
                phase: Phase::Running,
                cancelled: false,
                detached: false,
            }),
            changed: Condvar::new(),
        });
        let job = Job {
            shared: Arc::clone(&shared),
            part,
            finished: cache::final_path(dir, &key, &extension),
            limit: self.cache_limit,
        };
        let body = response.into_body();
        let thread = thread::spawn(move || job.run(body.into_reader(), file));
        Ok(Download::new(shared, Some(thread)))
    }

    /// Asks for the song from byte `offset`, and works out what the server's answer means.
    fn request(&self, url: &str, offset: u64) -> Result<Reply> {
        let mut request = self.download_agent.get(url);
        if offset > 0 {
            request = request.header("Range", &format!("bytes={offset}-"));
        }
        let mut response = request.call().map_err(|error| match error {
            ureq::Error::Timeout(Timeout::RecvResponse | Timeout::RecvBody) => Error::Stalled,
            other => other.into(),
        })?;
        let status = response.status().as_u16();
        let content_type = header(&response, "content-type");
        let content_range =
            header(&response, "content-range").and_then(|value| parse_range(&value));

        if status == 416 {
            // "bytes */N": the partial file is already all N bytes, or does not belong to this song.
            return Ok(match content_range {
                Some((_, Some(total))) if total == offset => Reply::Complete,
                _ => Reply::RangeRejected,
            });
        }
        check_media(&mut response)?;

        let length =
            header(&response, "content-length").and_then(|value| value.parse::<u64>().ok());
        if status == 206 {
            let Some((Some(start), total)) = content_range else {
                return Ok(Reply::RangeRejected);
            };
            if start != offset {
                return Ok(Reply::RangeRejected);
            }
            return Ok(Reply::Body {
                response,
                offset: start,
                total: total.or(length.map(|length| start + length)),
                content_type,
            });
        }
        // A plain 200 to a ranged request means the server ignored the range and is sending
        // everything, so the bytes already held are discarded by starting the file afresh.
        Ok(Reply::Body {
            response,
            offset: 0,
            total: length,
            content_type,
        })
    }
}

/// What the server's answer to a request for a song amounts to.
enum Reply {
    /// The bytes from `offset` on are in the body.
    Body {
        response: Response<Body>,
        offset: u64,
        total: Option<u64>,
        content_type: Option<String>,
    },
    /// The partial file already holds the whole song.
    Complete,
    /// The partial file does not fit this song or this server.
    RangeRejected,
}

/// The work of the download thread.
struct Job {
    shared: Arc<Shared>,
    part: PathBuf,
    finished: PathBuf,
    limit: u64,
}

impl Job {
    fn run(self, mut body: impl Read, mut file: File) -> Result<PathBuf> {
        let outcome = self
            .copy(&mut body, &mut file)
            .and_then(|()| self.finish(file));
        if let Err(error) = &outcome {
            if matches!(error, Error::TooLargeForCache { .. }) {
                // Unlike an interrupted download, this one can never be resumed into a fit.
                let _ = fs::remove_file(&self.part);
            }
            let mut state = self.shared.lock();
            state.phase = Phase::Failed(error.to_string());
            self.shared.changed.notify_all();
        }
        outcome
    }

    fn copy(&self, body: &mut impl Read, file: &mut File) -> Result<()> {
        let mut buffer = vec![0; CHUNK];
        loop {
            if self.shared.lock().cancelled {
                return Err(Error::DownloadFailed("cancelled".to_owned()));
            }
            let read = body.read(&mut buffer).map_err(read_error)?;
            if read == 0 {
                return Ok(());
            }
            file.write_all(&buffer[..read])?;
            let mut state = self.shared.lock();
            state.downloaded += read as u64;
            // A server that never says how big the file is still cannot overfill the cache.
            if state.total.is_none() && state.downloaded > self.limit {
                return Err(Error::TooLargeForCache {
                    size: state.downloaded,
                    limit: self.limit,
                });
            }
            self.shared.changed.notify_all();
        }
    }

    fn finish(&self, file: File) -> Result<PathBuf> {
        drop(file);
        // Renamed while holding the lock, so a reader opening the file sees one name or the other.
        let mut state = self.shared.lock();
        if let Some(total) = state.total
            && state.downloaded != total
        {
            return Err(Error::DownloadFailed(format!(
                "the connection closed after {} of {total} bytes",
                state.downloaded
            )));
        }
        replace(&self.part, &self.finished)?;
        state.path = self.finished.clone();
        state.total = Some(state.downloaded);
        state.phase = Phase::Done;
        self.shared.changed.notify_all();
        Ok(self.finished.clone())
    }
}

/// A read that times out means the server went quiet.
fn read_error(error: io::Error) -> Error {
    match ureq::Error::from(error) {
        ureq::Error::Timeout(_) => Error::Stalled,
        ureq::Error::Io(error) => Error::Io(error),
        other => Error::Http(other),
    }
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// Deletes a partial file that cannot be continued.
fn discard(partial: Option<(PathBuf, String)>) {
    if let Some((path, _)) = partial {
        let _ = fs::remove_file(path);
    }
}

/// Renames over an existing file, which Windows refuses to do on its own.
fn replace(from: &Path, to: &Path) -> io::Result<()> {
    let _ = fs::remove_file(to);
    fs::rename(from, to)
}

fn header<B>(response: &Response<B>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// `bytes 100-199/200` is `(Some(100), Some(200))`; `bytes */200` is `(None, Some(200))`.
fn parse_range(value: &str) -> Option<(Option<u64>, Option<u64>)> {
    let (range, total) = value.trim().strip_prefix("bytes ")?.split_once('/')?;
    let start = range.split('-').next().and_then(|start| start.parse().ok());
    Some((start, total.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decoders take their input as a `Read + Seek` they may move to another thread.
    #[test]
    fn a_growing_reader_can_serve_as_a_media_source() {
        fn media_source<T: Read + Seek + Send + Sync + 'static>() {}
        media_source::<GrowingReader>();
    }

    #[test]
    fn parses_content_range_headers() {
        assert_eq!(
            parse_range("bytes 100-199/200"),
            Some((Some(100), Some(200)))
        );
        assert_eq!(parse_range("bytes 0-0/*"), Some((Some(0), None)));
        assert_eq!(parse_range("bytes */200"), Some((None, Some(200))));
        assert_eq!(parse_range("items 1-2/3"), None);
        assert_eq!(parse_range("bytes 5"), None);
    }
}
