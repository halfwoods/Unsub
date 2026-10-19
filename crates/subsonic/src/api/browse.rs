use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use super::{Ctx, Reply, doc};
use crate::{
    Error, Result,
    model::Artist,
    wire::{AlbumDetail, ArtistDetail, SongEntry},
};

/// Subsonic's default list; names starting with one of these (and a space) sort under the next word.
const IGNORED_ARTICLES: &str = "The El La Los Las Le Les";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistIndex {
    ignored_articles: &'static str,
    /// `getIndexes` only; milliseconds since the epoch.
    last_modified: Option<u128>,
    index: Vec<IndexGroup>,
}

#[derive(Serialize)]
struct IndexGroup {
    name: String,
    artist: Vec<Artist>,
}

/// Serves both `getArtists` (root `artists`) and `getIndexes` (root `indexes`): the same A-Z
/// listing under a different name.
pub async fn artist_index(ctx: &Ctx, root: &'static str) -> Result<Reply> {
    let artists = ctx.source(|source| source.artists()).await?;
    let last_modified = (root == "indexes").then(|| {
        // Always "now" so clients that cache by `lastModified` never serve a stale index.
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis())
    });
    doc(
        root,
        &ArtistIndex {
            ignored_articles: IGNORED_ARTICLES,
            last_modified,
            index: group(artists),
        },
    )
}

pub async fn artist(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    let (artist, albums) = ctx
        .source(move |source| source.artist(&id))
        .await?
        .ok_or(Error::NotFound)?;
    doc(
        "artist",
        &ArtistDetail {
            artist: &artist,
            album: &albums,
        },
    )
}

pub async fn album(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    let (album, songs) = ctx
        .source(move |source| source.album(&id))
        .await?
        .ok_or(Error::NotFound)?;
    doc(
        "album",
        &AlbumDetail {
            album: &album,
            song: SongEntry::all(&songs),
        },
    )
}

pub async fn song(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    let song = ctx
        .source(move |source| source.song(&id))
        .await?
        .ok_or(Error::NotFound)?;
    doc("song", &SongEntry::new(&song))
}

pub async fn genres(ctx: &Ctx) -> Result<Reply> {
    #[derive(Serialize)]
    struct Genres {
        genre: Vec<crate::Genre>,
    }
    let genre = ctx.source(|source| source.genres()).await?;
    doc("genres", &Genres { genre })
}

/// The name with a leading ignored article removed ("The Beatles" is filed under B).
fn sort_name(name: &str) -> &str {
    IGNORED_ARTICLES
        .split(' ')
        .find_map(|article| {
            let (head, tail) = (name.get(..article.len())?, name.get(article.len()..)?);
            let rest = tail.strip_prefix(' ')?.trim_start();
            (head.eq_ignore_ascii_case(article) && !rest.is_empty()).then_some(rest)
        })
        .unwrap_or(name)
}

/// Groups artists under the uppercased first letter of their sort name, with `#` for anything that
/// is not a letter.
fn group(artists: Vec<Artist>) -> Vec<IndexGroup> {
    let mut groups: BTreeMap<char, Vec<Artist>> = BTreeMap::new();
    for artist in artists {
        let letter = match sort_name(&artist.name).chars().next() {
            Some(first) if first.is_alphabetic() => first.to_uppercase().next().unwrap_or(first),
            _ => '#',
        };
        groups.entry(letter).or_default().push(artist);
    }
    groups
        .into_iter()
        .map(|(letter, mut artist)| {
            artist.sort_by_cached_key(|artist| sort_name(&artist.name).to_lowercase());
            IndexGroup {
                name: letter.to_string(),
                artist,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artist(name: &str) -> Artist {
        Artist {
            id: name.to_owned(),
            name: name.to_owned(),
            ..Artist::default()
        }
    }

    #[test]
    fn strips_leading_articles_only_when_followed_by_a_word() {
        assert_eq!(sort_name("The Beatles"), "Beatles");
        assert_eq!(sort_name("the  beatles"), "beatles");
        assert_eq!(sort_name("La Roux"), "Roux");
        assert_eq!(sort_name("The"), "The");
        assert_eq!(sort_name("The "), "The ");
        assert_eq!(sort_name("Theory of a Deadman"), "Theory of a Deadman");
        assert_eq!(sort_name("Les Claypool"), "Claypool");
        assert_eq!(sort_name("Élodie"), "Élodie");
    }

    #[test]
    fn groups_and_orders_by_sort_name() {
        let groups = group(
            [
                "The Beatles",
                "Bjork",
                "Aphex Twin",
                "10cc",
                "(hed) p.e.",
                "Ólafur Arnalds",
            ]
            .map(artist)
            .to_vec(),
        );
        let shape: Vec<_> = groups
            .iter()
            .map(|group| {
                (
                    group.name.as_str(),
                    group
                        .artist
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("#", vec!["(hed) p.e.", "10cc"]),
                ("A", vec!["Aphex Twin"]),
                ("B", vec!["The Beatles", "Bjork"]),
                ("Ó", vec!["Ólafur Arnalds"]),
            ]
        );
    }
}
