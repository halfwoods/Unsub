use serde::Serialize;

use super::{Ctx, Reply, doc};
use crate::{
    Error, Result,
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
