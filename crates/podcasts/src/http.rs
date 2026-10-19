use std::time::Duration;

use ureq::ResponseExt;
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{Connector, DefaultConnector};

use crate::error::{Error, Result};
use crate::stall::StallConnector;

pub(crate) const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A feed is XML text; anything bigger than this is not a feed.
const FEED_SIZE_LIMIT: u64 = 64 * 1024 * 1024;
const FEED_TIMEOUT: Duration = Duration::from_secs(60);

/// Cache validators the server handed out with a feed, replayed to get `304 Not Modified`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl Validators {
    fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }
}

pub(crate) struct FeedResponse {
    /// The URL after redirects, which is what gets stored for later refreshes.
    pub final_url: String,
    pub body: Vec<u8>,
    pub validators: Validators,
}

/// `stall_timeout` bounds each wait for data, not the whole transfer: a download may take hours
/// as long as bytes keep arriving, so no global or body timeout is configured.
pub(crate) fn agent(stall_timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .user_agent(concat!("AntiStreaming/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(stall_timeout))
        .max_redirects(10)
        // Callers branch on 206/304/416 themselves.
        .http_status_as_error(false)
        .build();
    ureq::Agent::with_parts(
        config,
        DefaultConnector::new().chain(StallConnector::new(stall_timeout)),
        DefaultResolver::default(),
    )
}

/// Maps the subscription schemes Apple and podcast apps use onto plain HTTP(S).
pub(crate) fn normalize_feed_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    // `feed:https://example.com/rss` wraps a complete URL.
    let unwrapped = trimmed
        .strip_prefix("feed:")
        .filter(|rest| rest.starts_with("http://") || rest.starts_with("https://"))
        .unwrap_or(trimmed);
    let invalid = || Error::InvalidUrl(raw.to_owned());
    let (scheme, rest) = unwrapped.split_once("://").ok_or_else(invalid)?;
    if rest.is_empty() {
        return Err(invalid());
    }
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        "http" | "https" => Ok(format!("{scheme}://{rest}")),
        "itpc" | "pcast" | "podcast" | "feed" => Ok(format!("http://{rest}")),
        _ => Err(invalid()),
    }
}

/// Returns `None` when the server answers `304 Not Modified` to the supplied validators.
pub(crate) fn fetch_feed(
    agent: &ureq::Agent,
    url: &str,
    validators: &Validators,
) -> Result<Option<FeedResponse>> {
    let mut request = agent
        .get(url)
        .header(
            "Accept",
            "application/rss+xml, application/atom+xml, application/xml;q=0.9, text/xml;q=0.8, */*;q=0.5",
        )
        .config()
        .timeout_global(Some(FEED_TIMEOUT))
        .build();
    if let Some(etag) = &validators.etag {
        request = request.header("If-None-Match", etag);
    }
    if let Some(last_modified) = &validators.last_modified {
        request = request.header("If-Modified-Since", last_modified);
    }

    let mut response = request.call()?;
    let status = response.status().as_u16();
    if status == 304 && !validators.is_empty() {
        return Ok(None);
    }
    if !(200..300).contains(&status) {
        return Err(Error::Status {
            status,
            url: url.to_owned(),
        });
    }

    let validators = Validators {
        etag: header(&response, "etag"),
        last_modified: header(&response, "last-modified"),
    };
    let final_url = response.get_uri().to_string();
    let body = response
        .body_mut()
        .with_config()
        .limit(FEED_SIZE_LIMIT)
        .read_to_vec()?;
    Ok(Some(FeedResponse {
        final_url,
        body,
        validators,
    }))
}

pub(crate) fn header<B>(response: &ureq::http::Response<B>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn itpc_and_podcast_schemes_become_http() {
        assert_eq!(
            normalize_feed_url("itpc://example.com/feed.xml").unwrap(),
            "http://example.com/feed.xml"
        );
        assert_eq!(
            normalize_feed_url("podcast://example.com/feed.xml").unwrap(),
            "http://example.com/feed.xml"
        );
        assert_eq!(
            normalize_feed_url("PCAST://example.com/a?b=C").unwrap(),
            "http://example.com/a?b=C"
        );
    }

    #[test]
    fn feed_prefix_wrapping_a_full_url_is_stripped() {
        assert_eq!(
            normalize_feed_url("feed:https://example.com/rss").unwrap(),
            "https://example.com/rss"
        );
        assert_eq!(
            normalize_feed_url("feed://example.com/rss").unwrap(),
            "http://example.com/rss"
        );
    }

    #[test]
    fn http_urls_are_kept_and_trimmed() {
        assert_eq!(
            normalize_feed_url("  HTTPS://Example.com/Feed  ").unwrap(),
            "https://Example.com/Feed"
        );
    }

    #[test]
    fn other_schemes_and_garbage_are_rejected() {
        for raw in ["ftp://example.com/feed", "example.com/feed", "", "https://"] {
            assert!(
                matches!(normalize_feed_url(raw), Err(Error::InvalidUrl(_))),
                "{raw}"
            );
        }
    }
}
