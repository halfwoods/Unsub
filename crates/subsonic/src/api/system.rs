use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::{Ctx, Reply};
use crate::{Error, Result};

/// The one folder every library is presented as.
const MUSIC_FOLDER_ID: u32 = 1;

pub fn license() -> Result<Reply> {
    Ok(Reply::Doc("license", json!({ "valid": true })))
}

pub fn music_folders() -> Result<Reply> {
    let folders = json!({ "musicFolder": [{ "id": MUSIC_FOLDER_ID, "name": "Music" }] });
    Ok(Reply::Doc("musicFolders", folders))
}

pub fn user(ctx: &Ctx) -> Result<Reply> {
    let config = &ctx.state.config;
    let username = ctx.params.required("username")?;
    // There is a single account, and non-admins may only look at themselves.
    if username != config.username {
        return Err(Error::NotAuthorized);
    }
    Ok(Reply::Doc(
        "user",
        json!({
            "username": config.username,
            "scrobblingEnabled": true,
            "adminRole": false,
            "settingsRole": false,
            "downloadRole": true,
            "uploadRole": false,
            // Clients show playlist editing only to a user who has this role.
            "playlistRole": ctx.state.source.capabilities().playlist_editing,
            "coverArtRole": true,
            "commentRole": false,
            "podcastRole": false,
            "streamRole": true,
            "jukeboxRole": false,
            "shareRole": false,
            "videoConversionRole": false,
            "folder": [MUSIC_FOLDER_ID],
        }),
    ))
}

pub fn extensions(ctx: &Ctx) -> Result<Reply> {
    let mut extensions = vec![json!({ "name": "formPost", "versions": [1] })];
    if ctx.state.config.api_key.is_some() {
        extensions.push(json!({ "name": "apiKeyAuthentication", "versions": [1] }));
    }
    if ctx.state.source.capabilities().lyrics {
        extensions.push(json!({ "name": "songLyrics", "versions": [1] }));
    }
    Ok(Reply::Doc("openSubsonicExtensions", extensions.into()))
}

/// A play is a `scrobble` with `submission=true` (the default) and a "now playing" ping is one with
/// `submission=false`. Repeated `id`s pair up with repeated `time`s by position; `time` is in
/// milliseconds, and the current time stands in for a missing one.
pub async fn scrobble(ctx: &Ctx) -> Result<Reply> {
    let ids: Vec<String> = ctx.params.all("id").map(str::to_owned).collect();
    if ids.is_empty() {
        return Err(Error::MissingParameter("id"));
    }
    let times = ctx
        .params
        .all("time")
        .map(|time| {
            time.parse::<i64>()
                .map_err(|_| Error::InvalidParameter("time"))
        })
        .collect::<Result<Vec<_>>>()?;
    let submission = !ctx
        .params
        .get("submission")
        .is_some_and(|value| value.eq_ignore_ascii_case("false"));
    let client = ctx.params.get("c").unwrap_or("unknown").to_owned();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });

    let (reported, who) = (ids.clone(), client.clone());
    ctx.source(move |source| {
        ids.iter().enumerate().try_for_each(|(position, id)| {
            if submission {
                source.scrobble(id, times.get(position).copied().unwrap_or(now_ms))
            } else {
                source.now_playing(id, &client)
            }
        })
    })
    .await?;
    for id in &reported {
        if submission {
            ctx.state.session.finished(id, &who);
        } else {
            ctx.state.session.started(id, &who, Instant::now());
        }
    }
    Ok(Reply::Empty)
}
