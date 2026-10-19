use serde::Serialize;

use super::{Ctx, Reply, doc};
use crate::{
    AlbumListKind, Error, Result,
    wire::{AlbumDir, SongEntry},
};

/// Subsonic's documented ceiling for list sizes.
const MAX_SIZE: usize = 500;
/// Clients that sync the whole library page through `search3` in much larger slices.
const MAX_SEARCH_COUNT: usize = 10_000;

/// `getAlbumList` (older, directory-style entries) and `getAlbumList2` (ID3 entries).
pub async fn album_list(ctx: &Ctx, root: &'static str, id3: bool) -> Result<Reply> {
    let params = &ctx.params;
    let kind = match params.required("type")? {
        // Starring and ratings are not tracked, so these are valid but always empty.
        "starred" | "highest" => return list::<()>(root, "album", &[]),
        "random" => AlbumListKind::Random,
        "newest" => AlbumListKind::Newest,
        "frequent" => AlbumListKind::Frequent,
        "recent" => AlbumListKind::Recent,
        "alphabeticalByName" => AlbumListKind::AlphabeticalByName,
        "alphabeticalByArtist" => AlbumListKind::AlphabeticalByArtist,
        "byYear" => AlbumListKind::ByYear,
        "byGenre" => AlbumListKind::ByGenre,
        _ => return Err(Error::InvalidParameter("type")),
    };
    let size = params.number("size")?.unwrap_or(10).min(MAX_SIZE);
    let offset = params.number("offset")?.unwrap_or(0);
    let genre = match kind {
        AlbumListKind::ByGenre => Some(
            params
                .get("genre")
                .ok_or(Error::MissingParameter("genre"))?
                .to_owned(),
        ),
        _ => None,
    };
    let years = match kind {
        AlbumListKind::ByYear => Some((
            params
                .number("fromYear")?
                .ok_or(Error::MissingParameter("fromYear"))?,
            params
                .number("toYear")?
                .ok_or(Error::MissingParameter("toYear"))?,
        )),
        _ => None,
    };

    let albums = ctx
        .source(move |source| source.album_list(kind, size, offset, genre.as_deref(), years))
        .await?;
    if id3 {
        list(root, "album", &albums)
    } else {
        list(root, "album", &AlbumDir::all(&albums))
    }
}

pub async fn random_songs(ctx: &Ctx) -> Result<Reply> {
    let params = &ctx.params;
    let size = params.number("size")?.unwrap_or(10).min(MAX_SIZE);
    let genre = params.get("genre").map(str::to_owned);
    let years = match (
        params.number::<i32>("fromYear")?,
        params.number::<i32>("toYear")?,
    ) {
        (None, None) => None,
        (from, to) => {
            let (from, to) = (from.unwrap_or(i32::MIN), to.unwrap_or(i32::MAX));
            Some((from.min(to), from.max(to)))
        }
    };
    let songs = ctx
        .source(move |source| source.random_songs(size, genre.as_deref(), years))
        .await?;
    list("randomSongs", "song", &SongEntry::all(&songs))
}

/// `search2` (directory-style entries) and `search3` (ID3 entries).
pub async fn search(ctx: &Ctx, root: &'static str, id3: bool) -> Result<Reply> {
    #[derive(Serialize)]
    struct Found<A: Serialize, B: Serialize, S: Serialize> {
        artist: A,
        album: B,
        song: S,
    }

    let params = &ctx.params;
    // Clients quote the query for Lucene-style servers; an empty `""` is how they ask for everything.
    let query = params.required("query")?.trim_matches('"').to_owned();
    let range = |count: &'static str, offset: &'static str| -> Result<(usize, usize)> {
        Ok((
            params.number(count)?.unwrap_or(20).min(MAX_SEARCH_COUNT),
            params.number(offset)?.unwrap_or(0),
        ))
    };
    let (artists, albums, songs) = (
        range("artistCount", "artistOffset")?,
        range("albumCount", "albumOffset")?,
        range("songCount", "songOffset")?,
    );

    let found = ctx
        .source(move |source| source.search(&query, artists, albums, songs))
        .await?;
    let song = SongEntry::all(&found.songs);
    if id3 {
        doc(
            root,
            &Found {
                artist: &found.artists,
                album: &found.albums,
                song,
            },
        )
    } else {
        doc(
            root,
            &Found {
                artist: &found.artists,
                album: AlbumDir::all(&found.albums),
                song,
            },
        )
    }
}

/// Favourites are not tracked, so the lists are always empty.
pub fn starred(root: &'static str) -> Result<Reply> {
    doc(root, &serde_json::Map::new())
}

fn list<T: Serialize>(root: &'static str, key: &str, items: &[T]) -> Result<Reply> {
    doc(root, &serde_json::json!({ key: items }))
}
