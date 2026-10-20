use std::{io, io::Read, path::Path};

use axum::{
    body::{Body, Bytes},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
        },
    },
    response::IntoResponse,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
    sync::mpsc,
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::ReaderStream;

use super::{
    Ctx, Reply,
    delivery::{self, Delivery},
};
use crate::{
    Error, Result, Stream,
    range::{self, ByteRange},
};

const CHUNK: usize = 64 * 1024;
/// Chunks buffered between a blocking reader and the socket; small, so a slow client holds back
/// the transcoder instead of memory growing.
const CHUNKS_IN_FLIGHT: usize = 4;

pub fn is_media(endpoint: &str) -> bool {
    matches!(endpoint, "stream" | "download" | "getCoverArt")
}

pub async fn cover_art(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    let size = ctx.params.number::<u32>("size")?;
    let (mime, bytes) = ctx
        .source(move |source| source.cover_art(&id, size))
        .await?
        .ok_or(Error::NotFound)?;
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, content_type(&mime));
    Ok(Reply::Raw((headers, Body::from(bytes)).into_response()))
}

/// Sends the song as stored, or converted when the client's `format` and `maxBitRate`, or the
/// server's default cap, call for it and the source can. A conversion is chunked: no length and no
/// `Range`. Where the source cannot convert, the original goes out.
pub async fn stream(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    let request = delivery::Request {
        format: ctx.params.get("format"),
        max_bitrate: ctx
            .params
            .number::<u32>("maxBitRate")?
            .filter(|&kbps| kbps > 0),
        default_max: ctx.state.config.default_max_kbps,
    };
    if request.may_convert() {
        let lookup = id.clone();
        let song = ctx
            .source(move |source| source.song(&lookup))
            .await?
            .ok_or(Error::NotFound)?;
        if let Delivery::Convert { format, max_kbps } = delivery::choose(&request, &song) {
            let wanted = id.clone();
            let converted = ctx
                .source(move |source| Ok(source.transcoded(&wanted, format, max_kbps)))
                .await?;
            if let Some(converted) = converted {
                let (reader, content_type) = converted?;
                let stream = Stream::Reader {
                    reader,
                    content_type: content_type.to_owned(),
                    length: None,
                };
                return deliver(stream, &ctx.headers, false).await;
            }
        }
    }
    original(ctx, id, false).await
}

/// Always the original file, never a conversion.
pub async fn download(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    original(ctx, id, true).await
}

async fn original(ctx: &Ctx, id: String, attachment: bool) -> Result<Reply> {
    let stream = ctx
        .source(move |source| source.open_stream(&id))
        .await?
        .ok_or(Error::NotFound)?;
    deliver(stream, &ctx.headers, attachment).await
}

async fn deliver(stream: Stream, request: &HeaderMap, attachment: bool) -> Result<Reply> {
    let response = match stream {
        Stream::File {
            path,
            content_type: mime,
        } => {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, content_type(&mime));
            if attachment {
                set_attachment(&mut headers, &path);
            }
            serve_file(&path, request, headers).await?
        }
        Stream::Reader {
            reader,
            content_type: mime,
            length,
        } => {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, content_type(&mime));
            if let Some(length) = length {
                headers.insert(CONTENT_LENGTH, length.into());
            }
            (headers, reader_body(reader)).into_response()
        }
    };
    Ok(Reply::Raw(response))
}

/// Serves a file with single-range support, which players need to seek without downloading the lot.
async fn serve_file(
    path: &Path,
    request: &HeaderMap,
    mut headers: HeaderMap,
) -> Result<axum::response::Response> {
    let mut file = File::open(path).await?;
    let len = file.metadata().await?.len();
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    let requested = request.get(RANGE).and_then(|value| value.to_str().ok());
    let (status, start, count) = match range::parse(requested, len) {
        ByteRange::Full => (StatusCode::OK, 0, len),
        ByteRange::Partial { start, end } => {
            if let Ok(value) = HeaderValue::try_from(format!("bytes {start}-{end}/{len}")) {
                headers.insert(CONTENT_RANGE, value);
            }
            (StatusCode::PARTIAL_CONTENT, start, end - start + 1)
        }
        ByteRange::Unsatisfiable => {
            if let Ok(value) = HeaderValue::try_from(format!("bytes */{len}")) {
                headers.insert(CONTENT_RANGE, value);
            }
            return Ok((StatusCode::RANGE_NOT_SATISFIABLE, headers).into_response());
        }
    };

    file.seek(io::SeekFrom::Start(start)).await?;
    headers.insert(CONTENT_LENGTH, count.into());
    let body = Body::from_stream(ReaderStream::with_capacity(file.take(count), CHUNK));
    Ok((status, headers, body).into_response())
}

/// Bridges a blocking `Read` (typically a transcoder) into a response body. The reader runs on the
/// blocking pool; it stops as soon as the client goes away and the receiver is dropped.
fn reader_body(mut reader: Box<dyn Read + Send>) -> Body {
    let (tx, rx) = mpsc::channel::<io::Result<Bytes>>(CHUNKS_IN_FLIGHT);
    tokio::task::spawn_blocking(move || {
        loop {
            let mut chunk = vec![0; CHUNK];
            let sent = match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    chunk.truncate(read);
                    tx.blocking_send(Ok(Bytes::from(chunk)))
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    let _ = tx.blocking_send(Err(err));
                    break;
                }
            };
            if sent.is_err() {
                break;
            }
        }
    });
    Body::from_stream(ReceiverStream::new(rx))
}

/// A host-supplied mime type that is not a valid header value falls back to opaque bytes.
fn content_type(mime: &str) -> HeaderValue {
    HeaderValue::try_from(mime)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
}

/// Offers both an ASCII fallback name and the RFC 5987 UTF-8 name so non-Latin titles survive.
fn set_attachment(headers: &mut HeaderMap, path: &Path) {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " ._-()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded = utf8_percent_encode(name, NON_ALPHANUMERIC);
    let disposition = format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}");
    if let Ok(value) = HeaderValue::try_from(disposition) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
}
