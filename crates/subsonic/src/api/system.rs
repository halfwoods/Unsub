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
            "playlistRole": false,
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
    Ok(Reply::Doc("openSubsonicExtensions", extensions.into()))
}

/// Subsonic sends `time` in milliseconds; the source takes seconds. Repeated `id`s pair up with
/// repeated `time`s by position.
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

    ctx.source(move |source| {
        ids.iter().enumerate().try_for_each(|(position, id)| {
            let at_unix = times.get(position).map(|millis| millis.div_euclid(1_000));
            source.scrobble(id, at_unix, submission)
        })
    })
    .await?;
    Ok(Reply::Empty)
}
