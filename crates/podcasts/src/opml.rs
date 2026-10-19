use quick_xml::escape::escape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Decoder, Reader, XmlVersion};

use crate::error::{Error, Result};
use crate::model::Feed;

/// Feed URLs in document order. Outlines without `xmlUrl` are folders and are skipped.
pub(crate) fn feed_urls(opml: &str) -> Result<Vec<String>> {
    let mut reader = Reader::from_str(opml);
    let mut urls = Vec::new();
    let mut has_root = false;
    let mut open_elements = 0usize;
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Start(element) => {
                open_elements += 1;
                has_root |= visit(&element, reader.decoder(), &mut urls)?;
            }
            Event::Empty(element) => has_root |= visit(&element, reader.decoder(), &mut urls)?,
            Event::End(_) => open_elements = open_elements.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
    }
    // The reader accepts a document cut off mid-way; importing half a list silently is worse.
    if open_elements > 0 {
        return Err(Error::Opml("document ends inside an element".into()));
    }
    if !has_root {
        return Err(Error::Opml("missing <opml> element".into()));
    }
    Ok(urls)
}

/// Collects an outline's `xmlUrl`; returns whether the element was the `<opml>` root.
fn visit(element: &BytesStart, decoder: Decoder, urls: &mut Vec<String>) -> Result<bool> {
    let name = element.local_name();
    if name.as_ref().eq_ignore_ascii_case(b"opml") {
        return Ok(true);
    }
    if name.as_ref().eq_ignore_ascii_case(b"outline") {
        for attribute in element.attributes() {
            let attribute = attribute.map_err(invalid)?;
            if attribute.key.as_ref().eq_ignore_ascii_case(b"xmlurl") {
                let url = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                    .map_err(invalid)?;
                if !url.trim().is_empty() {
                    urls.push(url.trim().to_owned());
                }
            }
        }
    }
    Ok(false)
}

fn invalid(err: impl std::fmt::Display) -> Error {
    Error::Opml(err.to_string())
}

pub(crate) fn write(feeds: &[Feed]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n  <head>\n    \
         <title>AntiStreaming Subscriptions</title>\n  </head>\n  <body>\n",
    );
    for feed in feeds {
        let title = escape(feed.title.as_str());
        out.push_str(&format!(
            "    <outline type=\"rss\" text=\"{title}\" title=\"{title}\" xmlUrl=\"{}\"",
            escape(feed.url.as_str())
        ));
        if let Some(link) = &feed.link {
            out.push_str(&format!(" htmlUrl=\"{}\"", escape(link.as_str())));
        }
        out.push_str("/>\n");
    }
    out.push_str("  </body>\n</opml>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_outlines_and_entities() {
        let opml = r#"<?xml version="1.0"?>
            <OPML version="1.0"><body>
              <outline text="Folder"><outline xmlurl="http://a.example/x?p=1&amp;q=2"/></outline>
              <outline XMLURL=" http://b.example/y "/>
              <outline text="No feed here"/>
              <outline xmlUrl=""/>
            </body></OPML>"#;
        assert_eq!(
            feed_urls(opml).unwrap(),
            ["http://a.example/x?p=1&q=2", "http://b.example/y"]
        );
    }

    #[test]
    fn written_documents_parse_back() {
        let feed = |url: &str| Feed {
            id: 1,
            url: url.to_owned(),
            title: "A <b> & \"c\"".to_owned(),
            author: String::new(),
            description: String::new(),
            artwork_url: None,
            link: Some("https://example.com/?a=1&b=2".to_owned()),
            last_refreshed: None,
            unplayed_count: 0,
            settings: Default::default(),
        };
        let document = write(&[
            feed("http://one.example/rss?x=1&y=2"),
            feed("http://two.example/"),
        ]);
        assert_eq!(
            feed_urls(&document).unwrap(),
            ["http://one.example/rss?x=1&y=2", "http://two.example/"]
        );
    }
}
