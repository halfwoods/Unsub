//! Request handling: parse, authenticate, dispatch to an endpoint, render the reply.

mod browse;
mod delivery;
mod lists;
mod media;
mod playlists;
mod system;

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    Error, MusicSource, Result, auth, params::Params, render, render::Format, server::AppState,
};

/// What an endpoint produces.
enum Reply {
    /// A bare `ok` envelope.
    Empty,
    /// An `ok` envelope with a named payload.
    Doc(&'static str, Value),
    /// Bytes or a ready-made response, for the media endpoints.
    Raw(Response),
}

/// Everything an endpoint can see about the request.
struct Ctx {
    state: Arc<AppState>,
    params: Params,
    headers: HeaderMap,
}

impl Ctx {
    /// Runs a [`MusicSource`] call on the blocking pool; sources are free to block on a database,
    /// and a panicking source must not take a worker down with it.
    async fn source<T, F>(&self, call: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&dyn MusicSource) -> Result<T> + Send + 'static,
    {
        let source = Arc::clone(&self.state.source);
        tokio::task::spawn_blocking(move || call(source.as_ref()))
            .await
            .map_err(|err| Error::Source(format!("music source failed: {err}")))?
    }
}

/// Builds a payload from a serializable value. Absent optional fields serialize as null and are
/// dropped here, so neither format ever shows one.
fn doc<T: Serialize>(name: &'static str, body: &T) -> Result<Reply> {
    let mut value = serde_json::to_value(body)?;
    strip_nulls(&mut value);
    Ok(Reply::Doc(name, value))
}

fn strip_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, member| !member.is_null());
            map.values_mut().for_each(strip_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// Handles `/rest/<name>` and `/rest/<name>.view`, over GET or POST.
pub async fn handle(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !matches!(method, Method::GET | Method::HEAD | Method::POST) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let form: &[u8] = if method == Method::POST { &body } else { &[] };
    let params = Params::parse(uri.query(), form);
    let format = Format::of(&params);
    let endpoint = name.strip_suffix(".view").unwrap_or(&name);

    let ctx = Ctx {
        state,
        params,
        headers,
    };
    let outcome = match auth::authenticate(&ctx.state.config, &ctx.params) {
        Ok(()) => dispatch(&ctx, endpoint).await,
        Err(err) => Err(err),
    };
    match outcome {
        Ok(Reply::Empty) => render::ok(format, None),
        Ok(Reply::Doc(name, body)) => render::ok(format, Some((name, body))),
        Ok(Reply::Raw(response)) => response,
        Err(err) => {
            // Clients fetching media treat a missing resource as an HTTP failure; everything else
            // is a 200 carrying the Subsonic error, as the protocol specifies.
            let status = if matches!(err, Error::NotFound) && media::is_media(endpoint) {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::OK
            };
            render::error(format, &err, status)
        }
    }
}

async fn dispatch(ctx: &Ctx, endpoint: &str) -> Result<Reply> {
    match endpoint {
        "ping" => Ok(Reply::Empty),
        "getLicense" => system::license(),
        "getMusicFolders" => system::music_folders(),
        "getUser" => system::user(ctx),
        "getOpenSubsonicExtensions" => system::extensions(ctx),
        "scrobble" => system::scrobble(ctx).await,
        "getIndexes" => browse::artist_index(ctx, "indexes").await,
        "getArtists" => browse::artist_index(ctx, "artists").await,
        "getArtist" => browse::artist(ctx).await,
        "getAlbum" => browse::album(ctx).await,
        "getSong" => browse::song(ctx).await,
        "getGenres" => browse::genres(ctx).await,
        "getAlbumList" => lists::album_list(ctx, "albumList", false).await,
        "getAlbumList2" => lists::album_list(ctx, "albumList2", true).await,
        "getRandomSongs" => lists::random_songs(ctx).await,
        "search2" => lists::search(ctx, "searchResult2", false).await,
        "search3" => lists::search(ctx, "searchResult3", true).await,
        "getStarred" => lists::starred("starred"),
        "getStarred2" => lists::starred("starred2"),
        "getPlaylists" => playlists::list(ctx).await,
        "getPlaylist" => playlists::get(ctx).await,
        "getCoverArt" => media::cover_art(ctx).await,
        "stream" => media::stream(ctx).await,
        "download" => media::download(ctx).await,
        other => Err(Error::UnsupportedEndpoint(other.to_owned())),
    }
}
