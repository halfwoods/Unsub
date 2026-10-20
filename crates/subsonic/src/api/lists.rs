use serde::Serialize;

use super::{Ctx, Reply, doc};
use crate::{
    AlbumListKind, Error, Result,
    wire::{AlbumDir, ByKind, SongEntry},
};

/// Subsonic's documented ceiling for list sizes.
const MAX_SIZE: usize = 500;
/// Clients that sync the whole library page through `search3` in much larger slices, and some ask
/// for it all at once. A cap below the library's size would hand them a short page, which they
/// take for the last one, so this only guards the source against absurd requests.
const MAX_SEARCH_COUNT: usize = 100_000;

/// `getAlbumList` (older, directory-style entries) and `getAlbumList2` (ID3 entries).
pub async fn album_list(ctx: &Ctx, root: &'static str, id3: bool) -> Result<Reply> {
    let params = &ctx.params;
    let kind = match params.required("type")? {
        // Ratings across the library are not served, so this one is valid but always empty.
        "highest" => return list::<()>(root, "album", &[]),
        "starred" => AlbumListKind::Starred,
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
            &ByKind {
                artist: &found.artists,
                album: &found.albums,
                song,
            },
        )
    } else {
        doc(
            root,
            &ByKind {
                artist: &found.artists,
                album: AlbumDir::all(&found.albums),
                song,
            },
        )
    }
}

/// `getStarred` (directory-style albums) and `getStarred2` (ID3).
pub async fn starred(ctx: &Ctx, root: &'static str, id3: bool) -> Result<Reply> {
    let starred = ctx.source(|source| source.starred()).await?;
    let song = SongEntry::all(&starred.songs);
    if id3 {
        doc(
            root,
            &ByKind {
                artist: &starred.artists,
                album: &starred.albums,
                song,
            },
        )
    } else {
        doc(
            root,
            &ByKind {
                artist: &starred.artists,
                album: AlbumDir::all(&starred.albums),
                song,
            },
        )
    }
}

/// `getSongsByGenre`: `count` songs from `offset`, up to the usual 500.
pub async fn songs_by_genre(ctx: &Ctx) -> Result<Reply> {
    #[derive(Serialize)]
    struct Songs<'a> {
        song: Vec<SongEntry<'a>>,
    }
    let genre = ctx.params.required("genre")?.to_owned();
    let count = ctx.params.number("count")?.unwrap_or(10).min(MAX_SIZE);
    let offset = ctx.params.number("offset")?.unwrap_or(0);
    let songs = ctx
        .source(move |source| source.songs_by_genre(&genre, count, offset))
        .await?;
    doc(
        "songsByGenre",
        &Songs {
            song: SongEntry::all(&songs),
        },
    )
}

fn list<T: Serialize>(root: &'static str, key: &str, items: &[T]) -> Result<Reply> {
    doc(root, &serde_json::json!({ key: items }))
}
