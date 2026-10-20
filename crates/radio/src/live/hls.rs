//! HLS live streams: poll the media playlist, fetch segments as they appear, and hand their bytes
//! on back to back as if they were one file.
//!
//! Segments are fetched by a thread that stays a few segments ahead of the reader, so the time
//! spent downloading one does not become a gap in the audio.
//!
//! Supported: master playlists (an audio-only variant at or under 320 kbps is preferred), MPEG-TS
//! and packed-audio (ADTS/MP3) segments, fragmented MP4 with `EXT-X-MAP`, AES-128 (CBC) encryption,
//! sequence gaps and restarts, playlists that end. Not supported, and reported as
//! [`Error::Unsupported`]: SAMPLE-AES and other encryption methods, and byte-range segments.

use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use aes::Aes128;
use cbc::{
    Decryptor,
    cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7},
};
use url::Url;

use super::{
    Event, Options,
    format::{Format, id3_len, sniff},
};
use crate::{
    error::{Error, Result},
    http::Http,
    text::decode,
};

/// Preferred ceiling for the variant chosen from a master playlist.
const MAX_VARIANT_BPS: u64 = 320_000;
/// A new listener starts this many segments behind the newest, as the HLS spec asks, so that the
/// first fetches finish before the audio they carry is needed.
const LIVE_EDGE_SEGMENTS: usize = 3;
/// A playlist that lags the position by up to this many segments is a stale cache, not a restart.
const STALE_TOLERANCE: u64 = 3;
/// Fetched chunks waiting for the reader.
const CHUNKS_AHEAD: usize = 2;
const MAX_PLAYLIST_BYTES: u64 = 1 << 20;
const MAX_SEGMENT_BYTES: u64 = 64 << 20;
const MAX_KEY_BYTES: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Variant {
    uri: Url,
    bandwidth: u64,
    audio_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    uri: Url,
    iv: Option<[u8; 16]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    seq: u64,
    uri: Url,
    key: Option<Key>,
    map: Option<Url>,
}

#[derive(Debug, PartialEq, Eq)]
struct Media {
    target: Duration,
    first_seq: u64,
    segments: Vec<Segment>,
    ended: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Master {
        variants: Vec<Variant>,
        audio_renditions: Vec<(Url, bool)>,
    },
    Media(Media),
}

/// `KEY=value,KEY2="quoted, value"` as pairs, keys upper-cased.
fn attributes(list: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut rest = list;
    while let Some((key, after)) = rest.split_once('=') {
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => quoted.split_once('"').unwrap_or((quoted, "")),
            None => after.split_once(',').unwrap_or((after, "")),
        };
        pairs.push((key.trim().to_ascii_uppercase(), value.to_owned()));
        rest = next.trim_start_matches(',');
    }
    pairs
}

fn attribute<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn is_video_codec(codec: &str) -> bool {
    [
        "avc", "hvc", "hev", "dvh", "dvhe", "vp0", "vp9", "av01", "mp4v",
    ]
    .iter()
    .any(|prefix| codec.starts_with(prefix))
}

fn parse_iv(text: &str) -> Option<[u8; 16]> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))?;
    u128::from_str_radix(digits, 16).ok().map(u128::to_be_bytes)
}

fn parse(text: &str, base: &Url) -> Result<Parsed> {
    let join = |reference: &str| {
        base.join(reference.trim())
            .map_err(|_| Error::Hls(format!("bad address {reference:?}")))
    };
    let lines: Vec<&str> = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines
        .first()
        .is_none_or(|line| !line.starts_with("#EXTM3U"))
    {
        return Err(Error::Hls("not a playlist".to_owned()));
    }

    if lines
        .iter()
        .any(|line| line.starts_with("#EXT-X-STREAM-INF:"))
    {
        let mut variants = Vec::new();
        let mut audio_renditions = Vec::new();
        let mut pending: Option<Vec<(String, String)>> = None;
        for line in &lines {
            if let Some(list) = line.strip_prefix("#EXT-X-STREAM-INF:") {
                pending = Some(attributes(list));
            } else if let Some(list) = line.strip_prefix("#EXT-X-MEDIA:") {
                let pairs = attributes(list);
                if attribute(&pairs, "TYPE") == Some("AUDIO")
                    && let Some(uri) = attribute(&pairs, "URI")
                {
                    audio_renditions
                        .push((join(uri)?, attribute(&pairs, "DEFAULT") == Some("YES")));
                }
            } else if !line.starts_with('#')
                && let Some(pairs) = pending.take()
            {
                let bandwidth = attribute(&pairs, "AVERAGE-BANDWIDTH")
                    .or_else(|| attribute(&pairs, "BANDWIDTH"))
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                let video_codec = attribute(&pairs, "CODECS")
                    .is_some_and(|codecs| codecs.split(',').any(|c| is_video_codec(c.trim())));
                let audio_only = attribute(&pairs, "RESOLUTION").is_none() && !video_codec;
                variants.push(Variant {
                    uri: join(line)?,
                    bandwidth,
                    audio_only,
                });
            }
        }
        return Ok(Parsed::Master {
            variants,
            audio_renditions,
        });
    }

    let mut target = Duration::from_secs(10);
    let mut first_seq = 0;
    let mut ended = false;
    let mut key: Option<Key> = None;
    let mut map: Option<Url> = None;
    let mut segments = Vec::new();
    for line in &lines {
        if let Some(value) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            target = Duration::from_secs(value.trim().parse().unwrap_or(10));
        } else if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            first_seq = value.trim().parse().unwrap_or(0);
        } else if line.starts_with("#EXT-X-ENDLIST") {
            ended = true;
        } else if let Some(list) = line.strip_prefix("#EXT-X-KEY:") {
            let pairs = attributes(list);
            key = match attribute(&pairs, "METHOD") {
                Some("NONE") => None,
                Some("AES-128") => {
                    let uri = attribute(&pairs, "URI").ok_or_else(|| {
                        Error::Hls("an encrypted playlist without a key address".to_owned())
                    })?;
                    Some(Key {
                        uri: join(uri)?,
                        iv: attribute(&pairs, "IV").and_then(parse_iv),
                    })
                }
                _ => {
                    return Err(Error::Unsupported(
                        "HLS encryption other than AES-128 (SAMPLE-AES)",
                    ));
                }
            };
        } else if let Some(list) = line.strip_prefix("#EXT-X-MAP:") {
            let pairs = attributes(list);
            if attribute(&pairs, "BYTERANGE").is_some() {
                return Err(Error::Unsupported("HLS byte-range segments"));
            }
            let uri = attribute(&pairs, "URI")
                .ok_or_else(|| Error::Hls("EXT-X-MAP without an address".to_owned()))?;
            map = Some(join(uri)?);
        } else if line.starts_with("#EXT-X-BYTERANGE:") {
            return Err(Error::Unsupported("HLS byte-range segments"));
        } else if !line.starts_with('#') {
            segments.push(Segment {
                seq: first_seq + segments.len() as u64,
                uri: join(line)?,
                key: key.clone(),
                map: map.clone(),
            });
        }
    }
    Ok(Parsed::Media(Media {
        target,
        first_seq,
        segments,
        ended,
    }))
}

/// The media playlist to follow from a master: audio-only at or under 320 kbps if there is one,
/// the lowest audio-only if everything is above that, an audio rendition if the variants carry
/// video, and the smallest variant if nothing else is on offer.
fn select(variants: &[Variant], audio_renditions: &[(Url, bool)]) -> Option<Url> {
    let audio_only: Vec<&Variant> = variants
        .iter()
        .filter(|variant| variant.audio_only)
        .collect();
    if !audio_only.is_empty() {
        let best = audio_only
            .iter()
            .filter(|variant| variant.bandwidth <= MAX_VARIANT_BPS)
            .max_by_key(|variant| variant.bandwidth)
            .or_else(|| audio_only.iter().min_by_key(|variant| variant.bandwidth));
        return best.map(|variant| variant.uri.clone());
    }
    let rendition = audio_renditions
        .iter()
        .find(|(_, default)| *default)
        .or_else(|| audio_renditions.first());
    if let Some((uri, _)) = rendition {
        return Some(uri.clone());
    }
    variants
        .iter()
        .min_by_key(|variant| variant.bandwidth)
        .map(|variant| variant.uri.clone())
}

/// What the reader receives: audio bytes, or why there will be no more.
type Chunk = Result<Vec<u8>>;

pub(super) struct Reader {
    chunks: Receiver<Chunk>,
    current: Vec<u8>,
    position: usize,
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.position >= self.current.len() {
            match self.chunks.recv() {
                Ok(Ok(chunk)) => {
                    self.current = chunk;
                    self.position = 0;
                }
                Ok(Err(error)) => return Err(error.into_io()),
                // The worker ended the stream by finishing.
                Err(_) => return Ok(0),
            }
        }
        let count = buf.len().min(self.current.len() - self.position);
        buf[..count].copy_from_slice(&self.current[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

/// Starts following the HLS stream whose playlist (already fetched, as `text`) is at `base`.
pub(super) fn start(
    http: Http,
    base: &Url,
    text: &str,
    options: &Options,
    events: Sender<Event>,
) -> Result<(Reader, Format, Url)> {
    let (media_url, media) = match parse(text, base)? {
        Parsed::Media(media) => (base.clone(), media),
        Parsed::Master {
            variants,
            audio_renditions,
        } => {
            let chosen = select(&variants, &audio_renditions)
                .ok_or_else(|| Error::Hls("the master playlist lists no streams".to_owned()))?;
            let (response, body) = http.get_document(&chosen, MAX_PLAYLIST_BYTES)?;
            match parse(&decode(&body), &response.url)? {
                Parsed::Media(media) => (response.url, media),
                Parsed::Master { .. } => {
                    return Err(Error::Hls(
                        "a master playlist inside a master playlist".to_owned(),
                    ));
                }
            }
        }
    };

    let mut worker = Worker::new(http, media_url.clone(), options.clone(), events);
    worker.merge(media);
    let first = worker
        .next()?
        .ok_or_else(|| Error::Hls("the playlist has no audio".to_owned()))?;
    let format = sniff(&first).ok_or_else(|| Error::UnknownFormat(worker.media_url.to_string()))?;

    let (sender, chunks) = mpsc::sync_channel(CHUNKS_AHEAD);
    // The receiver is held right here, so this cannot fail.
    let _ = sender.send(Ok(first));
    thread::spawn(move || {
        loop {
            let chunk = match worker.next() {
                Ok(Some(chunk)) => Ok(chunk),
                // Finishing drops the sender, which the reader takes as the end.
                Ok(None) => return,
                Err(error) => {
                    if !matches!(error, Error::Cancelled) {
                        let _ = worker.events.send(Event::Failed(error.to_string()));
                    }
                    Err(error)
                }
            };
            let failed = chunk.is_err();
            // A closed receiver means the listener is gone.
            if sender.send(chunk).is_err() || failed {
                return;
            }
        }
    });
    Ok((
        Reader {
            chunks,
            current: Vec::new(),
            position: 0,
        },
        format,
        media_url,
    ))
}

struct Worker {
    http: Http,
    options: Options,
    events: Sender<Event>,
    media_url: Url,
    /// Segments known from the playlist and not yet fetched, oldest first.
    queue: VecDeque<Segment>,
    /// The sequence number after the newest segment seen. `None` before the first playlist.
    next_seq: Option<u64>,
    target: Duration,
    ended: bool,
    last_refresh: Instant,
    last_refresh_added: bool,
    /// Downloaded and waiting to be handed on: an init segment, then the segment.
    ready: VecDeque<Vec<u8>>,
    sent_map: Option<Url>,
    keys: HashMap<Url, [u8; 16]>,
    failures: u32,
}

impl Worker {
    fn new(http: Http, media_url: Url, options: Options, events: Sender<Event>) -> Self {
        Self {
            http,
            options,
            events,
            media_url,
            queue: VecDeque::new(),
            next_seq: None,
            target: Duration::from_secs(10),
            ended: false,
            last_refresh: Instant::now(),
            last_refresh_added: true,
            ready: VecDeque::new(),
            sent_map: None,
            keys: HashMap::new(),
            failures: 0,
        }
    }

    /// The next piece of audio, or `None` once a finished playlist is used up.
    fn next(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(chunk) = self.ready.pop_front() {
                return Ok(Some(chunk));
            }
            if self.http.cancel_handle().is_cancelled() {
                return Err(Error::Cancelled);
            }
            let step = if self.queue.is_empty() {
                if self.ended {
                    return Ok(None);
                }
                self.refresh()
            } else {
                self.fetch_front()
            };
            if let Err(error) = step {
                self.fail(error)?;
            }
        }
    }

    /// Notes a failed request and waits before the next try, or gives up if it keeps happening or
    /// can never succeed.
    fn fail(&mut self, error: Error) -> Result<()> {
        if matches!(error, Error::Cancelled | Error::Unsupported(_)) {
            return Err(error);
        }
        self.failures += 1;
        if self.failures > self.options.max_failures {
            return Err(error);
        }
        let _ = self.events.send(Event::Reconnecting {
            attempt: self.failures,
        });
        self.http
            .cancel_handle()
            .sleep(self.options.delay(self.failures))
    }

    /// Waits until the playlist is due, then reads it again. The spec's rhythm is one reload per
    /// target duration, and half that after a reload that brought nothing new.
    fn refresh(&mut self) -> Result<()> {
        self.http.cancel_handle().sleep(
            self.poll_interval()
                .saturating_sub(self.last_refresh.elapsed()),
        )?;
        let (response, body) = self
            .http
            .get_document(&self.media_url, MAX_PLAYLIST_BYTES)?;
        match parse(&decode(&body), &response.url)? {
            Parsed::Media(media) => {
                self.merge(media);
                Ok(())
            }
            Parsed::Master { .. } => Err(Error::Hls(
                "the media playlist turned into a master playlist".to_owned(),
            )),
        }
    }

    fn poll_interval(&self) -> Duration {
        self.options.hls_poll.unwrap_or(if self.last_refresh_added {
            self.target
        } else {
            self.target / 2
        })
    }

    /// Folds a freshly read playlist into what is queued, noticing segments that fell off the
    /// server before they were fetched.
    fn merge(&mut self, media: Media) {
        let end_seq = media.first_seq + media.segments.len() as u64;
        let mut added = false;
        match self.next_seq {
            Some(next) if end_seq < next && next - end_seq <= STALE_TOLERANCE => {}
            Some(next) if end_seq >= next => {
                let queued = self.queue.len();
                self.queue.retain(|segment| segment.seq >= media.first_seq);
                let mut skipped = (queued - self.queue.len()) as u64;
                if self.queue.is_empty() && next < media.first_seq {
                    skipped += media.first_seq - next;
                }
                if skipped > 0 {
                    let _ = self.events.send(Event::Skipped {
                        segments: Some(skipped),
                    });
                }
                for segment in media.segments.iter().filter(|segment| segment.seq >= next) {
                    self.queue.push_back(segment.clone());
                    added = true;
                }
                self.next_seq = Some(end_seq);
            }
            previous => {
                // First playlist, or the server restarted its numbering: begin at the live edge.
                if previous.is_some() {
                    let _ = self.events.send(Event::Skipped { segments: None });
                    self.queue.clear();
                }
                let start = if media.ended {
                    0
                } else {
                    media.segments.len().saturating_sub(LIVE_EDGE_SEGMENTS)
                };
                self.queue.extend(media.segments[start..].iter().cloned());
                added = !self.queue.is_empty();
                self.next_seq = Some(end_seq);
            }
        }
        self.target = media.target;
        self.ended = media.ended;
        self.last_refresh = Instant::now();
        self.last_refresh_added = added;
    }

    fn fetch_front(&mut self) -> Result<()> {
        let Some(segment) = self.queue.front().cloned() else {
            return Ok(());
        };
        let mut downloaded = Vec::new();
        if let Some(map) = segment
            .map
            .as_ref()
            .filter(|map| self.sent_map.as_ref() != Some(*map))
        {
            downloaded.push(self.download(map, &segment)?);
        }
        let data = self.download(&segment.uri, &segment)?;
        // Packed-audio segments start with a timestamp tag that would only be noise mid-stream.
        downloaded.push(strip_id3(data));

        self.sent_map.clone_from(&segment.map);
        self.queue.pop_front();
        self.ready.extend(downloaded);
        if self.failures > 0 {
            self.failures = 0;
            let _ = self.events.send(Event::Reconnected);
        }
        Ok(())
    }

    fn download(&mut self, uri: &Url, segment: &Segment) -> Result<Vec<u8>> {
        let (_, body) = self.http.get_document(uri, MAX_SEGMENT_BYTES)?;
        match &segment.key {
            Some(key) => self.decrypt(body, key, segment.seq),
            None => Ok(body),
        }
    }

    /// AES-128 in CBC mode with PKCS#7 padding. Without an explicit IV the segment's sequence number
    /// is the IV, as a 128-bit big-endian integer.
    fn decrypt(&mut self, data: Vec<u8>, key: &Key, seq: u64) -> Result<Vec<u8>> {
        let key_bytes = match self.keys.get(&key.uri) {
            Some(bytes) => *bytes,
            None => {
                let (_, body) = self.http.get_document(&key.uri, MAX_KEY_BYTES)?;
                let bytes: [u8; 16] = body
                    .try_into()
                    .map_err(|_| Error::Hls("the decryption key is not 16 bytes".to_owned()))?;
                self.keys.insert(key.uri.clone(), bytes);
                bytes
            }
        };
        let iv = key.iv.unwrap_or_else(|| u128::from(seq).to_be_bytes());
        Decryptor::<Aes128>::new(&key_bytes.into(), &iv.into())
            .decrypt_padded_vec::<Pkcs7>(&data)
            .map_err(|_| Error::Hls("a segment did not decrypt".to_owned()))
    }
}

fn strip_id3(data: Vec<u8>) -> Vec<u8> {
    match id3_len(&data) {
        Some(length) if data.starts_with(b"ID3") && length <= data.len() => data[length..].to_vec(),
        _ => data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("http://radio.example.com/hls/live.m3u8").unwrap()
    }

    fn media(text: &str) -> Media {
        match parse(text, &base()).unwrap() {
            Parsed::Media(media) => media,
            Parsed::Master { .. } => panic!("parsed as a master playlist"),
        }
    }

    fn url(path: &str) -> Url {
        Url::parse(&format!("http://radio.example.com{path}")).unwrap()
    }

    #[test]
    fn attribute_lists_handle_quotes_with_commas() {
        let pairs = attributes(
            "BANDWIDTH=128000,CODECS=\"mp4a.40.2, avc1.4d401f\",URI=\"a/b.m3u8\",DEFAULT=YES",
        );
        assert_eq!(attribute(&pairs, "BANDWIDTH"), Some("128000"));
        assert_eq!(attribute(&pairs, "CODECS"), Some("mp4a.40.2, avc1.4d401f"));
        assert_eq!(attribute(&pairs, "URI"), Some("a/b.m3u8"));
        assert_eq!(attribute(&pairs, "DEFAULT"), Some("YES"));
        assert_eq!(attribute(&pairs, "MISSING"), None);
        assert!(attributes("").is_empty());
    }

    #[test]
    fn a_live_media_playlist_is_read_with_sequence_numbers_and_relative_addresses() {
        let parsed = media(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:2680\n#EXTINF:6.006,\nseg/2680.ts\n#EXTINF:6.006,\n/abs/2681.ts\n#EXTINF:6.006,title\nhttp://cdn.example.com/2682.ts\n",
        );
        assert_eq!(parsed.target, Duration::from_secs(6));
        assert_eq!(parsed.first_seq, 2680);
        assert!(!parsed.ended);
        let seen: Vec<(u64, String)> = parsed
            .segments
            .iter()
            .map(|s| (s.seq, s.uri.to_string()))
            .collect();
        assert_eq!(
            seen,
            [
                (2680, "http://radio.example.com/hls/seg/2680.ts".to_owned()),
                (2681, "http://radio.example.com/abs/2681.ts".to_owned()),
                (2682, "http://cdn.example.com/2682.ts".to_owned()),
            ]
        );
    }

    #[test]
    fn keys_and_maps_apply_to_the_segments_after_them() {
        let parsed = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\na.m4s\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.bin\",IV=0x000102030405060708090a0b0c0d0e0f\n#EXTINF:4,\nb.m4s\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:4,\nc.m4s\n#EXT-X-ENDLIST\n",
        );
        assert!(parsed.ended);
        assert_eq!(parsed.first_seq, 0);
        assert!(
            parsed
                .segments
                .iter()
                .all(|s| s.map == Some(url("/hls/init.mp4")))
        );
        assert_eq!(parsed.segments[0].key, None);
        let key = parsed.segments[1].key.clone().unwrap();
        assert_eq!(key.uri, url("/hls/k.bin"));
        assert_eq!(
            key.iv,
            Some(*b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f")
        );
        assert_eq!(parsed.segments[2].key, None);
    }

    #[test]
    fn ivs_are_128_bit_hex() {
        assert_eq!(parse_iv("0x1"), Some(1u128.to_be_bytes()));
        assert_eq!(parse_iv("0XFF"), Some(255u128.to_be_bytes()));
        assert_eq!(parse_iv("12"), None);
        assert_eq!(parse_iv("0xzz"), None);
    }

    #[test]
    fn what_is_not_supported_is_refused_by_name() {
        let sample_aes = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"k\"\n#EXTINF:6,\na.ts\n";
        assert!(
            matches!(parse(sample_aes, &base()), Err(Error::Unsupported(what)) if what.contains("SAMPLE-AES"))
        );
        let ranges =
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\n#EXT-X-BYTERANGE:1000@0\nfile.aac\n";
        assert!(
            matches!(parse(ranges, &base()), Err(Error::Unsupported(what)) if what.contains("byte-range"))
        );
        assert!(matches!(parse("<html>", &base()), Err(Error::Hls(_))));
    }

    fn master(text: &str) -> Url {
        let Parsed::Master {
            variants,
            audio_renditions,
        } = parse(text, &base()).unwrap()
        else {
            panic!("not a master playlist")
        };
        select(&variants, &audio_renditions).expect("a choice")
    }

    #[test]
    fn the_best_audio_only_variant_under_320_kbps_wins() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=70000,CODECS=\"mp4a.40.5\"\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=140000,CODECS=\"mp4a.40.2\"\nmid.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=330000,AVERAGE-BANDWIDTH=310000,CODECS=\"mp4a.40.2\"\nhigh.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=900000,CODECS=\"avc1.4d401f,mp4a.40.2\",RESOLUTION=640x360\nvideo.m3u8\n";
        // The high variant's average rate is under the line, its peak over: the average counts.
        assert_eq!(master(text), url("/hls/high.m3u8"));
    }

    #[test]
    fn when_every_audio_variant_is_over_the_line_the_smallest_is_taken() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=640000,CODECS=\"mp4a.40.2\"\na.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=512000,CODECS=\"mp4a.40.2\"\nb.m3u8\n";
        assert_eq!(master(text), url("/hls/b.m3u8"));
    }

    #[test]
    fn variants_without_codecs_or_resolution_count_as_audio() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\naudio.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=64000,RESOLUTION=320x180\nvideo.m3u8\n";
        assert_eq!(master(text), url("/hls/audio.m3u8"));
    }

    #[test]
    fn video_variants_fall_back_to_an_audio_rendition_and_then_to_the_smallest() {
        let with_rendition = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"en\",URI=\"en.m3u8\"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"fr\",DEFAULT=YES,URI=\"fr.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,AUDIO=\"a\"\nv.m3u8\n";
        assert_eq!(master(with_rendition), url("/hls/fr.m3u8"));
        let video_only = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720\nbig.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nsmall.m3u8\n";
        assert_eq!(master(video_only), url("/hls/small.m3u8"));
    }

    #[test]
    fn the_playlist_is_polled_once_per_target_duration_and_twice_as_often_when_nothing_changed() {
        let worker = |poll: Option<Duration>| {
            let (events, _) = mpsc::channel();
            let http = Http::new(
                crate::http::HttpConfig::default(),
                crate::http::CancelHandle::new(),
            );
            let mut worker = Worker::new(
                http,
                base(),
                Options {
                    hls_poll: poll,
                    ..Options::default()
                },
                events,
            );
            worker.target = Duration::from_secs(6);
            worker
        };
        let mut default = worker(None);
        assert_eq!(default.poll_interval(), Duration::from_secs(6));
        default.last_refresh_added = false;
        assert_eq!(default.poll_interval(), Duration::from_secs(3));
        assert_eq!(
            worker(Some(Duration::from_millis(40))).poll_interval(),
            Duration::from_millis(40)
        );
    }

    #[test]
    fn id3_tags_are_stripped_from_packed_audio_only() {
        use crate::live::format::samples::{adts_frame, id3v2};
        let mut tagged = id3v2(45);
        tagged.extend(adts_frame(200));
        assert_eq!(strip_id3(tagged), adts_frame(200));
        assert_eq!(strip_id3(adts_frame(200)), adts_frame(200));
        assert_eq!(strip_id3(b"ID3".to_vec()), b"ID3");
    }
}
