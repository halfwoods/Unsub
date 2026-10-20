//! Endpoints clients call while syncing or opening a screen, for things this server does not have.
//! Answering them with an empty result, as other lightweight servers do, keeps clients from
//! showing errors or retrying.

use std::time::Instant;

use serde_json::json;

use super::{Ctx, Reply, doc};
use crate::{
    Error, Result,
    wire::{AlbumDir, Directory, NowPlayingEntry, SongEntry},
};

/// An empty `name` element. `needs` is a parameter the real endpoint requires.
pub fn empty(ctx: &Ctx, name: &'static str, needs: Option<&'static str>) -> Result<Reply> {
    if let Some(parameter) = needs {
        ctx.params.required(parameter)?;
    }
    Ok(Reply::Doc(name, json!({})))
}

/// Rescans belong to the app, not to a phone, so a scan is never running.
pub fn scan_status() -> Result<Reply> {
    Ok(Reply::Doc("scanStatus", json!({ "scanning": false })))
}

/// OpenSubsonic: who owns the API key that was used. There is one account.
pub fn token_info(ctx: &Ctx) -> Result<Reply> {
    Ok(Reply::Doc(
        "tokenInfo",
        json!({ "username": ctx.state.config.username }),
    ))
}

/// Operations that need a user role this server never grants (sharing, jukebox, podcast and user
/// management). Subsonic answers a user without the role with 50.
pub fn not_permitted() -> Result<Reply> {
    Err(Error::NotAuthorized)
}

/// Folder browsing, for clients that use it instead of the tag-based `getArtist` and `getAlbum`:
/// an artist is a folder of albums and an album a folder of songs. An id is tried as an artist
/// first, so the ids of artists and albums must not overlap.
pub async fn music_directory(ctx: &Ctx) -> Result<Reply> {
    enum Found {
        Artist(crate::Artist, Vec<crate::Album>),
        Album(crate::Album, Vec<crate::Song>),
    }
    let id = ctx.params.required("id")?.to_owned();
    let found = ctx
        .source(move |source| {
            if let Some((artist, albums)) = source.artist(&id)? {
                return Ok(Some(Found::Artist(artist, albums)));
            }
            Ok(source
                .album(&id)?
                .map(|(album, songs)| Found::Album(album, songs)))
        })
        .await?
        .ok_or(Error::NotFound)?;
    match found {
        Found::Artist(artist, albums) => doc(
            "directory",
            &Directory {
                id: &artist.id,
                name: &artist.name,
                parent: None,
                child: AlbumDir::all(&albums),
            },
        ),
        Found::Album(album, songs) => doc(
            "directory",
            &Directory {
                id: &album.id,
                name: &album.name,
                parent: album.artist_id.as_deref(),
                child: SongEntry::all(&songs),
            },
        ),
    }
}

/// What clients that reported a song through `scrobble?submission=false` are playing.
pub async fn now_playing(ctx: &Ctx) -> Result<Reply> {
    let now = Instant::now();
    let playing = ctx.state.session.playing(now);
    let ids: Vec<String> = playing.iter().map(|entry| entry.song_id.clone()).collect();
    let songs = ctx
        .source(move |source| {
            ids.iter()
                .map(|id| source.song(id))
                .collect::<Result<Vec<_>>>()
        })
        .await?;
    let entry: Vec<_> = playing
        .iter()
        .zip(&songs)
        // A song deleted since it was reported is not playing.
        .filter_map(|(entry, song)| {
            Some(NowPlayingEntry {
                song: SongEntry::new(song.as_ref()?),
                username: &ctx.state.config.username,
                minutes_ago: now.saturating_duration_since(entry.since).as_secs() / 60,
                player_name: &entry.client,
            })
        })
        .collect();
    #[derive(serde::Serialize)]
    struct Playing<'a> {
        entry: Vec<NowPlayingEntry<'a>>,
    }
    doc("nowPlaying", &Playing { entry })
}
