//! A source that implements only what is required: the optional parts (conversion, scrobbling, now
//! playing) must default to harmless.

mod common;

use std::{io::Write, sync::Arc};

use antistreaming_subsonic::{
    Album, AlbumListKind, Artist, Genre, MusicSource, Playlist, Result, SearchResult, Server,
    ServerConfig, Song, Stream,
};
use common::{Resp, error_of, parse_xml};

struct Bare {
    file: tempfile::NamedTempFile,
}

impl MusicSource for Bare {
    fn artists(&self) -> Result<Vec<Artist>> {
        Ok(Vec::new())
    }

    fn artist(&self, _: &str) -> Result<Option<(Artist, Vec<Album>)>> {
        Ok(None)
    }

    fn album(&self, _: &str) -> Result<Option<(Album, Vec<Song>)>> {
        Ok(None)
    }

    fn song(&self, id: &str) -> Result<Option<Song>> {
        Ok((id == "x").then(|| Song {
            id: "x".into(),
            title: "Lossless".into(),
            suffix: Some("flac".into()),
            bit_rate: Some(900),
            ..Song::default()
        }))
    }

    fn album_list(
        &self,
        _: AlbumListKind,
        _: usize,
        _: usize,
        _: Option<&str>,
        _: Option<(i32, i32)>,
    ) -> Result<Vec<Album>> {
        Ok(Vec::new())
    }

    fn random_songs(&self, _: usize, _: Option<&str>, _: Option<(i32, i32)>) -> Result<Vec<Song>> {
        Ok(Vec::new())
    }

    fn search(
        &self,
        _: &str,
        _: (usize, usize),
        _: (usize, usize),
        _: (usize, usize),
    ) -> Result<SearchResult> {
        Ok(SearchResult::default())
    }

    fn genres(&self) -> Result<Vec<Genre>> {
        Ok(Vec::new())
    }

    fn playlists(&self) -> Result<Vec<Playlist>> {
        Ok(Vec::new())
    }

    fn playlist(&self, _: &str) -> Result<Option<(Playlist, Vec<Song>)>> {
        Ok(None)
    }

    fn cover_art(&self, _: &str, _: Option<u32>) -> Result<Option<(String, Vec<u8>)>> {
        Ok(None)
    }

    fn open_stream(&self, id: &str) -> Result<Option<Stream>> {
        Ok((id == "x").then(|| Stream::File {
            path: self.file.path().to_owned(),
            content_type: "audio/flac".into(),
        }))
    }
}

fn get(server: &Server, endpoint: &str, query: &str) -> Resp {
    let agent = common::agent();
    let url = format!(
        "http://{}/rest/{endpoint}?u=alice&p=sesame&v=1.16.1&c=tests&{query}",
        server.local_addr()
    );
    let mut response = agent.get(&url).call().unwrap();
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
        .collect();
    Resp::new(status, headers, response.body_mut().read_to_vec().unwrap())
}

fn start() -> Server {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(b"flac bytes").unwrap();
    let config = ServerConfig {
        bind: ([127, 0, 0, 1], 0).into(),
        username: "alice".into(),
        password: "sesame".into(),
        api_key: None,
        default_max_kbps: Some(128),
    };
    Server::start(config, Arc::new(Bare { file })).unwrap()
}

#[test]
fn scrobbling_and_now_playing_succeed_when_the_source_ignores_them() {
    let server = start();
    for query in ["id=x&time=1000", "id=x&submission=false", "id=unknown"] {
        let reply = get(&server, "scrobble", query);
        let doc = parse_xml(&reply);
        assert_eq!(
            doc.root_element().attribute("status"),
            Some("ok"),
            "{query}"
        );
    }
    server.stop();
}

#[test]
fn a_source_that_never_converts_gets_the_original_even_under_a_cap() {
    let server = start();
    for query in ["id=x", "id=x&format=mp3", "id=x&maxBitRate=64"] {
        let reply = get(&server, "stream", query);
        assert_eq!(reply.status, 200, "{query}");
        assert_eq!(reply.body, b"flac bytes", "{query}");
        assert_eq!(reply.header("content-type"), Some("audio/flac"));
    }
    let missing = get(&server, "stream", "id=nope&format=mp3");
    assert_eq!((missing.status, error_of(&missing).0), (404, 70));
    server.stop();
}
