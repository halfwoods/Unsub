use serde::Serialize;

use super::{Ctx, Reply, doc};
use crate::{
    Error, PlaylistEdit, Result,
    model::Playlist,
    wire::{PlaylistDetail, SongEntry},
};

pub async fn list(ctx: &Ctx) -> Result<Reply> {
    #[derive(Serialize)]
    struct Playlists {
        playlist: Vec<Playlist>,
    }
    let playlist = ctx.source(|source| source.playlists()).await?;
    doc("playlists", &Playlists { playlist })
}

pub async fn get(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    detail(ctx, id).await
}

async fn detail(ctx: &Ctx, id: String) -> Result<Reply> {
    let (playlist, songs) = ctx
        .source(move |source| source.playlist(&id))
        .await?
        .ok_or(Error::NotFound)?;
    doc(
        "playlist",
        &PlaylistDetail {
            playlist: &playlist,
            entry: SongEntry::all(&songs),
        },
    )
}

/// Subsonic ties playlist editing to a user role, so a library that cannot edit them answers as a
/// user without the role would: not authorised.
fn editing(error: Error) -> Error {
    match error {
        Error::Unsupported(_) => Error::NotAuthorized,
        other => other,
    }
}

/// Creates a playlist from `name` and the `songId`s, or with `playlistId` replaces the songs of an
/// existing one. Either way the answer is the playlist as it now stands.
pub async fn create(ctx: &Ctx) -> Result<Reply> {
    let songs = ids(ctx, "songId");
    let id = match ctx.params.get("playlistId") {
        Some(existing) => {
            let id = existing.to_owned();
            let target = id.clone();
            ctx.source(move |source| {
                let (_, current) = source.playlist(&target)?.ok_or(Error::NotFound)?;
                let replace = PlaylistEdit {
                    remove: (0..current.len()).collect(),
                    add: songs,
                    ..PlaylistEdit::default()
                };
                source.update_playlist(&target, &replace)
            })
            .await
            .map_err(editing)?;
            id
        }
        None => {
            let name = ctx
                .params
                .get("name")
                .ok_or(Error::MissingParameter("name"))?
                .to_owned();
            ctx.source(move |source| source.create_playlist(&name, &songs))
                .await
                .map_err(editing)?
        }
    };
    detail(ctx, id).await
}

/// `songIdToAdd` and `songIndexToRemove` may each repeat; the indexes are positions before the edit.
pub async fn update(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("playlistId")?.to_owned();
    let remove = ctx
        .params
        .all("songIndexToRemove")
        .map(|index| {
            index
                .trim()
                .parse::<usize>()
                .map_err(|_| Error::InvalidParameter("songIndexToRemove"))
        })
        .collect::<Result<Vec<_>>>()?;
    let edit = PlaylistEdit {
        name: ctx.params.get("name").map(str::to_owned),
        // Unlike a name, a comment may be emptied.
        comment: ctx.params.raw("comment").map(str::to_owned),
        public: ctx
            .params
            .get("public")
            .map(|value| value.eq_ignore_ascii_case("true")),
        remove,
        add: ids(ctx, "songIdToAdd"),
    };
    ctx.source(move |source| source.update_playlist(&id, &edit))
        .await
        .map_err(editing)?;
    Ok(Reply::Empty)
}

pub async fn delete(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    ctx.source(move |source| source.delete_playlist(&id))
        .await
        .map_err(editing)?;
    Ok(Reply::Empty)
}

fn ids(ctx: &Ctx, name: &str) -> Vec<String> {
    ctx.params
        .all(name)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}
