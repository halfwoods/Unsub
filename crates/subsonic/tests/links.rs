//! `Server::start_links`: audio at secret links, and nothing else.

use std::{
    io::{Cursor, Write},
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use unsub_subsonic::{LinkSource, Server, Stream};

const FILE_LEN: usize = 5000;

fn file_bytes() -> Vec<u8> {
    (0..FILE_LEN).map(|i| (i % 251) as u8).collect()
}

struct Links {
    file: tempfile::NamedTempFile,
}

impl LinkSource for Links {
    fn open(&self, secret: &str) -> Option<Stream> {
        match secret {
            "0123456789abcdef" => Some(Stream::File {
                path: self.file.path().to_owned(),
                content_type: "audio/flac".to_owned(),
            }),
            "feedfeedfeedfeed" => Some(Stream::Reader {
                reader: Box::new(Cursor::new(b"cover bytes".to_vec())),
                content_type: "image/jpeg".to_owned(),
                length: Some(11),
            }),
            "gone" => Some(Stream::File {
                path: "/definitely/not/here.flac".into(),
                content_type: "audio/flac".to_owned(),
            }),
            _ => None,
        }
    }
}

struct Rig {
    server: Server,
    agent: ureq::Agent,
}

impl Rig {
    fn start() -> Self {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&file_bytes()).unwrap();
        let bind = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let server = Server::start_links(bind, Arc::new(Links { file })).unwrap();
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        Self { server, agent }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.server.local_addr())
    }

    fn get(&self, path: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
        self.finish(self.agent.get(self.url(path)).call())
    }

    fn finish(
        &self,
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let mut response = result.unwrap();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
            .collect();
        let status = response.status().as_u16();
        (status, headers, response.body_mut().read_to_vec().unwrap())
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[test]
fn a_secret_opens_its_file_with_a_type_a_length_and_range_support() {
    let rig = Rig::start();
    let (status, headers, body) = rig.get("/link/0123456789abcdef");
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-type"), Some("audio/flac"));
    assert_eq!(header(&headers, "content-length"), Some("5000"));
    assert_eq!(header(&headers, "accept-ranges"), Some("bytes"));
    assert_eq!(body, file_bytes());
}

#[test]
fn the_extension_players_want_is_ignored() {
    let rig = Rig::start();
    let (status, _, body) = rig.get("/link/0123456789abcdef.flac");
    assert_eq!((status, body), (200, file_bytes()));
}

#[test]
fn ranges_are_honoured_so_speakers_can_seek() {
    let rig = Rig::start();
    let response = rig
        .agent
        .get(rig.url("/link/0123456789abcdef.flac"))
        .header("Range", "bytes=100-199")
        .call();
    let (status, headers, body) = rig.finish(response);
    assert_eq!(status, 206);
    assert_eq!(
        header(&headers, "content-range"),
        Some("bytes 100-199/5000")
    );
    assert_eq!(body, file_bytes()[100..200]);
}

#[test]
fn a_generated_stream_is_sent_with_its_length() {
    let rig = Rig::start();
    let (status, headers, body) = rig.get("/link/feedfeedfeedfeed.jpg");
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-type"), Some("image/jpeg"));
    assert_eq!(body, b"cover bytes");
}

#[test]
fn head_answers_with_the_headers_and_no_body() {
    let rig = Rig::start();
    let response = rig
        .agent
        .head(rig.url("/link/0123456789abcdef.flac"))
        .call();
    let (status, headers, body) = rig.finish(response);
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-length"), Some("5000"));
    assert!(body.is_empty());
}

#[test]
fn a_secret_that_opens_nothing_is_not_found() {
    let rig = Rig::start();
    for path in [
        "/link/0123456789abcdee",
        "/link/",
        "/link/.flac",
        "/link/..%2f..%2fetc%2fpasswd",
    ] {
        assert_eq!(rig.get(path).0, 404, "{path}");
    }
}

#[test]
fn a_file_that_has_gone_is_not_found() {
    let rig = Rig::start();
    assert_eq!(rig.get("/link/gone.flac").0, 404);
}

#[test]
fn only_links_are_served_not_the_subsonic_api_or_anything_else() {
    let rig = Rig::start();
    for path in [
        "/rest/ping?u=a&p=b&v=1.16.1&c=t",
        "/",
        "/link/a/b",
        "/secret",
    ] {
        assert_eq!(rig.get(path).0, 404, "{path}");
    }
}

#[test]
fn writes_are_refused() {
    let rig = Rig::start();
    let response = rig.agent.post(rig.url("/link/0123456789abcdef")).send("x");
    assert_eq!(rig.finish(response).0, 405);
}

#[test]
fn it_stops_like_the_other_server() {
    let rig = Rig::start();
    let addr = rig.server.local_addr();
    rig.server.stop();
    assert!(std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err());
}
