use serde_json::json;

use super::{Ctx, Reply};
use crate::{Error, Result, Song};

/// OpenSubsonic `songLyrics`. The lyrics are plain text, so they are returned as one unsynced
/// block with a line per line of text; a song without lyrics gets an empty list.
pub async fn by_song_id(ctx: &Ctx) -> Result<Reply> {
    let id = ctx.params.required("id")?.to_owned();
    let (song, lyrics) = ctx
        .source(move |source| {
            let Some(song) = source.song(&id)? else {
                return Ok(None);
            };
            let lyrics = source.lyrics(&id)?;
            Ok(Some((song, lyrics)))
        })
        .await?
        .ok_or(Error::NotFound)?;
    let structured: Vec<_> = lyrics
        .map(|lyrics| {
            json!({
                "displayArtist": song.artist.unwrap_or_default(),
                "displayTitle": song.title,
                // "xxx" is the convention for lyrics whose language is not known.
                "lang": lyrics.language.unwrap_or_else(|| "xxx".to_owned()),
                "synced": false,
                "line": lines(&lyrics.text).into_iter().map(|value| json!({ "value": value })).collect::<Vec<_>>(),
            })
        })
        .into_iter()
        .collect();
    Ok(Reply::Doc(
        "lyricsList",
        json!({ "structuredLyrics": structured }),
    ))
}

/// The older `getLyrics`: lyrics by artist and title, as a single block of text. The song is found
/// by searching for the title and matching the artist, without regard to case.
pub async fn by_artist_and_title(ctx: &Ctx) -> Result<Reply> {
    let title = ctx
        .params
        .get("title")
        .ok_or(Error::MissingParameter("title"))?
        .to_owned();
    let artist = ctx.params.get("artist").map(str::to_owned);
    let wanted = (title.clone(), artist.clone());
    let found = ctx
        .source(move |source| {
            let candidates = source.search(&wanted.0, (0, 0), (0, 0), (50, 0))?.songs;
            let matching = candidates
                .into_iter()
                .find(|song| matches(song, &wanted.0, wanted.1.as_deref()));
            match matching {
                Some(song) => Ok(source.lyrics(&song.id)?.map(|lyrics| (song, lyrics))),
                None => Ok(None),
            }
        })
        .await?;
    let body = match found {
        Some((song, lyrics)) => json!({
            "artist": song.artist.unwrap_or_default(),
            "title": song.title,
            "value": lyrics.text,
        }),
        // No lyrics is an empty element, not an error.
        None => json!({}),
    };
    Ok(Reply::Doc("lyrics", body))
}

fn matches(song: &Song, title: &str, artist: Option<&str>) -> bool {
    song.title.eq_ignore_ascii_case(title)
        && artist.is_none_or(|artist| {
            song.artist
                .as_deref()
                .is_some_and(|own| own.eq_ignore_ascii_case(artist))
        })
}

/// One entry per line of text. A blank line stays, as the break between verses; blank lines at the
/// very end go.
fn lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_keep_verse_breaks_and_lose_trailing_blanks() {
        assert_eq!(
            lines("one\r\ntwo\n\nthree  \n\n\n"),
            ["one", "two", "", "three"]
        );
        assert!(lines("").is_empty());
        assert!(lines("\n\n").is_empty());
    }
}
