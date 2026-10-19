//! RSS 2.0 (with the iTunes namespace) and Atom parsing into plain values the store can save.
//!
//! `rss` and `atom_syndication` are used instead of a unified parser because podcast feeds need the
//! raw `itunes:duration` text, per-item `itunes:explicit` and the real (possibly absent) guid.

use std::collections::HashSet;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedFeed {
    pub title: String,
    pub author: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub link: Option<String>,
    pub episodes: Vec<ParsedEpisode>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedEpisode {
    pub guid: String,
    pub title: String,
    pub description: String,
    pub published: Option<i64>,
    pub duration_ms: Option<u64>,
    pub enclosure_url: String,
    pub enclosure_type: Option<String>,
    pub enclosure_length: Option<u64>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub explicit: bool,
    pub artwork_url: Option<String>,
}

pub(crate) fn parse(bytes: &[u8]) -> Result<ParsedFeed> {
    let mut feed = match rss::Channel::read_from(bytes) {
        Ok(channel) => from_rss(channel),
        Err(rss::Error::InvalidStartTag) => match atom_syndication::Feed::read_from(bytes) {
            Ok(feed) => from_atom(feed),
            Err(atom_syndication::Error::InvalidStartTag) => {
                return Err(Error::Parse("the document is neither RSS nor Atom".into()));
            }
            Err(err) => return Err(Error::Parse(err.to_string())),
        },
        Err(err) => return Err(Error::Parse(err.to_string())),
    };
    dedupe(&mut feed.episodes);
    Ok(feed)
}

/// Keeps the first of any items sharing a guid or an enclosure URL. Otherwise the store would
/// see two items claiming one row and rewrite it back and forth on every refresh.
fn dedupe(episodes: &mut Vec<ParsedEpisode>) {
    let mut guids = HashSet::new();
    let mut enclosures = HashSet::new();
    episodes.retain(|episode| {
        let fresh_guid = guids.insert(episode.guid.clone());
        let fresh_enclosure = enclosures.insert(episode.enclosure_url.clone());
        fresh_guid && fresh_enclosure
    });
}

fn from_rss(channel: rss::Channel) -> ParsedFeed {
    let itunes = channel.itunes_ext.as_ref();
    let artwork_url = itunes
        .and_then(|ext| clean(ext.image.as_deref()))
        .or_else(|| {
            channel
                .image
                .as_ref()
                .and_then(|image| clean(Some(&image.url)))
        });
    let author = itunes
        .and_then(|ext| clean(ext.author.as_deref()))
        .or_else(|| {
            itunes
                .and_then(|ext| ext.owner.as_ref())
                .and_then(|owner| clean(owner.name.as_deref()))
        })
        .unwrap_or_default();
    let description = clean(Some(&channel.description))
        .or_else(|| itunes.and_then(|ext| clean(ext.summary.as_deref())))
        .unwrap_or_default();

    ParsedFeed {
        title: clean(Some(&channel.title)).unwrap_or_default(),
        author,
        description,
        artwork_url,
        link: clean(Some(&channel.link)),
        episodes: channel.items.iter().filter_map(rss_episode).collect(),
    }
}

fn rss_episode(item: &rss::Item) -> Option<ParsedEpisode> {
    let enclosure = item.enclosure.as_ref()?;
    let enclosure_url = clean(Some(&enclosure.url))?;
    let itunes = item.itunes_ext.as_ref();
    let guid = item
        .guid
        .as_ref()
        .and_then(|guid| clean(Some(&guid.value)))
        .unwrap_or_else(|| enclosure_url.clone());
    let description = clean(item.description.as_deref())
        .or_else(|| clean(item.content.as_deref()))
        .or_else(|| itunes.and_then(|ext| clean(ext.summary.as_deref())))
        .unwrap_or_default();

    Some(ParsedEpisode {
        guid,
        title: clean(item.title.as_deref()).unwrap_or_default(),
        description,
        published: item.pub_date.as_deref().and_then(parse_timestamp),
        duration_ms: itunes
            .and_then(|ext| ext.duration.as_deref())
            .and_then(parse_duration_ms),
        enclosure_url,
        enclosure_type: clean(Some(&enclosure.mime_type)),
        enclosure_length: enclosure.length.trim().parse().ok(),
        season: itunes.and_then(|ext| parse_number(ext.season.as_deref())),
        episode: itunes.and_then(|ext| parse_number(ext.episode.as_deref())),
        explicit: itunes
            .and_then(|ext| ext.explicit.as_deref())
            .is_some_and(is_explicit),
        artwork_url: itunes.and_then(|ext| clean(ext.image.as_deref())),
    })
}

fn from_atom(feed: atom_syndication::Feed) -> ParsedFeed {
    ParsedFeed {
        title: clean(Some(&feed.title.value)).unwrap_or_default(),
        author: feed
            .authors
            .first()
            .and_then(|person| clean(Some(&person.name)))
            .unwrap_or_default(),
        description: feed
            .subtitle
            .as_ref()
            .and_then(|text| clean(Some(&text.value)))
            .unwrap_or_default(),
        artwork_url: feed
            .logo
            .as_deref()
            .or(feed.icon.as_deref())
            .and_then(|url| clean(Some(url))),
        link: atom_alternate_link(&feed.links),
        episodes: feed.entries.iter().filter_map(atom_episode).collect(),
    }
}

fn atom_episode(entry: &atom_syndication::Entry) -> Option<ParsedEpisode> {
    let enclosure = entry.links.iter().find(|link| link.rel == "enclosure")?;
    let enclosure_url = clean(Some(&enclosure.href))?;
    let guid = clean(Some(&entry.id)).unwrap_or_else(|| enclosure_url.clone());
    let description = entry
        .summary
        .as_ref()
        .and_then(|text| clean(Some(&text.value)))
        .or_else(|| {
            entry
                .content
                .as_ref()
                .and_then(|content| clean(content.value.as_deref()))
        })
        .unwrap_or_default();
    let published = entry.published.unwrap_or(entry.updated).timestamp();

    Some(ParsedEpisode {
        guid,
        title: clean(Some(&entry.title.value)).unwrap_or_default(),
        description,
        published: Some(published),
        duration_ms: None,
        enclosure_url,
        enclosure_type: clean(enclosure.mime_type.as_deref()),
        enclosure_length: enclosure
            .length
            .as_deref()
            .and_then(|length| length.trim().parse().ok()),
        season: None,
        episode: None,
        explicit: false,
        artwork_url: None,
    })
}

fn atom_alternate_link(links: &[atom_syndication::Link]) -> Option<String> {
    links
        .iter()
        .find(|link| link.rel == "alternate")
        .and_then(|link| clean(Some(&link.href)))
}

fn clean(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn parse_number(text: Option<&str>) -> Option<u32> {
    text?.trim().parse().ok()
}

fn is_explicit(text: &str) -> bool {
    ["yes", "true", "explicit"]
        .iter()
        .any(|flag| text.trim().eq_ignore_ascii_case(flag))
}

fn parse_timestamp(text: &str) -> Option<i64> {
    diligent_date_parser::parse_date(text.trim()).map(|date| date.timestamp())
}

/// Parses `itunes:duration`, which feeds write as `S`, `MM:SS`, `H:MM:SS` or any of those with a
/// fractional last field. Minutes and seconds are not range-checked because `62:03` is common.
pub(crate) fn parse_duration_ms(text: &str) -> Option<u64> {
    let mut fields: Vec<&str> = text.trim().split(':').collect();
    if fields.len() > 3 {
        return None;
    }
    let last = fields.pop()?;
    let (whole, fraction) = last.split_once('.').unwrap_or((last, ""));

    let mut seconds = digits(whole)?;
    let mut unit = 60u64;
    for field in fields.iter().rev() {
        seconds = seconds.checked_add(digits(field)?.checked_mul(unit)?)?;
        unit *= 60;
    }
    let millis = if fraction.is_empty() {
        0
    } else {
        let fraction = format!("{:0<3}", digits_str(fraction)?);
        fraction[..3].parse::<u64>().ok()?
    };
    seconds.checked_mul(1000)?.checked_add(millis)
}

fn digits(field: &str) -> Option<u64> {
    digits_str(field)?.parse().ok()
}

fn digits_str(field: &str) -> Option<&str> {
    (!field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit())).then_some(field)
}

#[cfg(test)]
mod tests;
