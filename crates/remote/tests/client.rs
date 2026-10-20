mod common;

use common::{FakeServer, PASSWORD, USER};
use md5::{Digest, Md5};
use serde_json::json;
use unsub_remote::{Album, AlbumListKind, Artist, Client, Error, Playlist, Song, SongListing};

fn connect(server: &FakeServer) -> Client {
    Client::new(&server.url, USER, PASSWORD)
}

fn song_json(id: &str, title: &str, track: u32) -> serde_json::Value {
    json!({
        "id": id, "parent": "al-1", "title": title, "album": "OK Computer", "artist": "Radiohead",
        "track": track, "discNumber": 1, "year": 1997, "genre": "Rock", "size": 11_520_000,
        "contentType": "audio/flac", "suffix": "flac", "duration": 287, "bitRate": 900,
        "path": "Radiohead/OK Computer/01.flac", "albumId": "al-1", "artistId": "ar-1",
        "coverArt": "al-1", "playCount": 3, "created": "2024-05-06T07:08:09Z",
        "isDir": false, "isVideo": false, "type": "music", "somethingNew": [1, 2, 3],
    })
}

#[test]
fn signs_every_request_with_a_fresh_token_and_never_sends_the_password() {
    let server = FakeServer::start();
    let client = connect(&server);

    client.ping().unwrap();
    client.ping().unwrap();

    let requests = server.requests_to("ping");
    assert_eq!(requests.len(), 2);
    let mut salts = Vec::new();
    for request in &requests {
        let params = &request.params;
        assert_eq!(params["u"], USER);
        assert_eq!(params["f"], "json");
        assert_eq!(params["v"], "1.16.1");
        assert_eq!(params["c"], "Unsub");
        assert!(!params.contains_key("p"));
        assert!(!request.raw_url.contains(PASSWORD));
        let expected = hex::encode(Md5::digest(format!("{PASSWORD}{}", params["s"])));
        assert_eq!(params["t"], expected);
        assert!(params["s"].len() >= 16);
        salts.push(params["s"].clone());
    }
    assert_ne!(salts[0], salts[1], "each request gets its own salt");
}

#[test]
fn a_wrong_password_is_a_typed_error() {
    let server = FakeServer::start();
    let client = Client::new(&server.url, USER, "not the password");

    assert!(matches!(client.ping(), Err(Error::WrongCredentials)));
    assert!(matches!(client.artists(), Err(Error::WrongCredentials)));
}

#[test]
fn trailing_slashes_in_the_base_url_are_ignored() {
    let server = FakeServer::start();
    Client::new(&format!("{}//", server.url), USER, PASSWORD)
        .ping()
        .unwrap();
}

#[test]
fn artists_come_back_flattened_in_the_servers_order() {
    let server = FakeServer::start();
    server.respond(
        "getArtists",
        json!({ "artists": { "ignoredArticles": "The", "index": [
            { "name": "A", "artist": [{ "id": "ar-2", "name": "Aphex Twin", "albumCount": 4, "coverArt": "ar-2" }] },
            { "name": "R", "artist": [
                { "id": "ar-1", "name": "Radiohead", "albumCount": 9 },
                { "id": "ar-3", "name": "Röyksopp" },
            ]},
        ]}}),
    );

    let artists = connect(&server).artists().unwrap();

    assert_eq!(
        artists,
        [
            Artist {
                id: "ar-2".into(),
                name: "Aphex Twin".into(),
                album_count: 4,
                cover_art: Some("ar-2".into()),
            },
            Artist {
                id: "ar-1".into(),
                name: "Radiohead".into(),
                album_count: 9,
                cover_art: None,
            },
            Artist {
                id: "ar-3".into(),
                name: "Röyksopp".into(),
                ..Artist::default()
            },
        ]
    );
}

#[test]
fn an_artist_comes_with_its_albums() {
    let server = FakeServer::start();
    server.respond(
        "getArtist",
        json!({ "artist": { "id": "ar-1", "name": "Radiohead", "albumCount": 2, "album": [
            { "id": "al-1", "name": "OK Computer", "artist": "Radiohead", "artistId": "ar-1",
              "year": 1997, "genre": "Rock", "songCount": 12, "duration": 3200,
              "created": "2024-05-06T07:08:09Z" },
            { "id": "al-2", "name": "Kid A", "songCount": 10, "duration": 2900 },
        ]}}),
    );

    let detail = connect(&server).artist("ar-1").unwrap();

    assert_eq!(server.requests_to("getArtist")[0].params["id"], "ar-1");
    assert_eq!(detail.artist.name, "Radiohead");
    assert_eq!(detail.artist.album_count, 2);
    assert_eq!(
        detail.albums,
        [
            Album {
                id: "al-1".into(),
                name: "OK Computer".into(),
                artist: Some("Radiohead".into()),
                artist_id: Some("ar-1".into()),
                year: Some(1997),
                genre: Some("Rock".into()),
                song_count: 12,
                duration: 3200,
                cover_art: None,
                created: Some("2024-05-06T07:08:09Z".into()),
            },
            Album {
                id: "al-2".into(),
                name: "Kid A".into(),
                song_count: 10,
                duration: 2900,
                ..Album::default()
            },
        ]
    );
}

#[test]
fn an_album_comes_with_its_songs_and_ignores_fields_it_does_not_know() {
    let server = FakeServer::start();
    server.respond(
        "getAlbum",
        json!({ "album": { "id": "al-1", "name": "OK Computer", "songCount": 2, "duration": 574,
            "song": [song_json("s-1", "Airbag", 1), song_json("s-2", "Paranoid Android", 2)] }}),
    );

    let detail = connect(&server).album("al-1").unwrap();

    assert_eq!(detail.album.name, "OK Computer");
    assert_eq!(detail.songs.len(), 2);
    assert_eq!(
        detail.songs[1],
        Song {
            id: "s-2".into(),
            title: "Paranoid Android".into(),
            album: Some("OK Computer".into()),
            artist: Some("Radiohead".into()),
            track: Some(2),
            disc_number: Some(1),
            year: Some(1997),
            genre: Some("Rock".into()),
            size: Some(11_520_000),
            content_type: Some("audio/flac".into()),
            suffix: Some("flac".into()),
            duration: Some(287),
            bit_rate: Some(900),
            album_id: Some("al-1".into()),
            artist_id: Some("ar-1".into()),
            cover_art: Some("al-1".into()),
            play_count: Some(3),
        }
    );
}

#[test]
fn album_lists_pass_their_kind_size_and_offset() {
    let server = FakeServer::start();
    server.respond(
        "getAlbumList2",
        json!({ "albumList2": { "album": [{ "id": "al-9", "name": "Newest" }] }}),
    );

    let albums = connect(&server)
        .album_list(AlbumListKind::AlphabeticalByArtist, 25, 50)
        .unwrap();

    assert_eq!(albums[0].name, "Newest");
    let params = &server.requests_to("getAlbumList2")[0].params;
    assert_eq!(params["type"], "alphabeticalByArtist");
    assert_eq!(params["size"], "25");
    assert_eq!(params["offset"], "50");
    for (kind, text) in [
        (AlbumListKind::Random, "random"),
        (AlbumListKind::Newest, "newest"),
        (AlbumListKind::Frequent, "frequent"),
        (AlbumListKind::Recent, "recent"),
        (AlbumListKind::AlphabeticalByName, "alphabeticalByName"),
    ] {
        connect(&server).album_list(kind, 1, 0).unwrap();
        assert_eq!(
            server.requests_to("getAlbumList2").last().unwrap().params["type"],
            text
        );
    }
}

#[test]
fn search_returns_artists_albums_and_songs() {
    let server = FakeServer::start();
    server.respond(
        "search3",
        json!({ "searchResult3": {
            "artist": [{ "id": "ar-1", "name": "Radiohead" }],
            "album": [{ "id": "al-1", "name": "OK Computer" }],
            "song": [song_json("s-1", "Airbag", 1)],
        }}),
    );

    let results = connect(&server).search("radio head \"ok\"").unwrap();

    assert_eq!(
        server.requests_to("search3")[0].params["query"],
        "radio head \"ok\""
    );
    assert_eq!(results.artists.len(), 1);
    assert_eq!(results.albums[0].id, "al-1");
    assert_eq!(results.songs[0].title, "Airbag");
}

#[test]
fn playlists_and_their_songs() {
    let server = FakeServer::start();
    server.respond(
        "getPlaylists",
        json!({ "playlists": { "playlist": [
            { "id": "pl-1", "name": "Road Trip", "songCount": 2, "duration": 600, "owner": "alice",
              "public": false, "created": "2024-01-01T00:00:00Z" },
        ]}}),
    );
    server.respond(
        "getPlaylist",
        json!({ "playlist": { "id": "pl-1", "name": "Road Trip", "songCount": 2, "duration": 600,
            "entry": [song_json("s-2", "Paranoid Android", 2), song_json("s-1", "Airbag", 1)] }}),
    );
    let client = connect(&server);

    assert_eq!(
        client.playlists().unwrap(),
        [Playlist {
            id: "pl-1".into(),
            name: "Road Trip".into(),
            song_count: 2,
            duration: 600,
            owner: Some("alice".into()),
        }]
    );
    let detail = client.playlist("pl-1").unwrap();
    let titles: Vec<_> = detail
        .songs
        .iter()
        .map(|song| song.title.as_str())
        .collect();
    assert_eq!(
        titles,
        ["Paranoid Android", "Airbag"],
        "playlist order is kept"
    );
    assert_eq!(detail.playlist.name, "Road Trip");
}

#[test]
fn listings_the_server_leaves_empty_are_empty() {
    let server = FakeServer::start();
    server.respond(
        "getArtists",
        json!({ "artists": { "ignoredArticles": "" } }),
    );
    server.respond("getAlbumList2", json!({ "albumList2": {} }));
    // No member at all, which some servers send for an empty result.
    server.respond("search3", json!({}));
    server.respond("getPlaylists", json!({ "playlists": {} }));
    let client = connect(&server);

    assert!(client.artists().unwrap().is_empty());
    assert!(
        client
            .album_list(AlbumListKind::Newest, 10, 0)
            .unwrap()
            .is_empty()
    );
    let found = client.search("nothing").unwrap();
    assert!(found.artists.is_empty() && found.albums.is_empty() && found.songs.is_empty());
    assert!(client.playlists().unwrap().is_empty());
}

#[test]
fn subsonic_errors_become_typed_errors() {
    let server = FakeServer::start();
    server.fail("getAlbum", 70, "Album not found");
    server.fail("getArtist", 50, "User is not authorized");
    server.fail(
        "getPlaylist",
        41,
        "Token authentication not supported for LDAP users",
    );
    server.fail(
        "getPlaylists",
        30,
        "Incompatible Subsonic REST protocol version. Server must upgrade.",
    );
    let client = connect(&server);

    assert!(matches!(client.album("x"), Err(Error::NotFound)));
    assert!(matches!(client.artist("x"), Err(Error::NotAuthorized)));
    assert!(matches!(
        client.playlist("x"),
        Err(Error::TokenAuthUnsupported)
    ));
    match client.playlists() {
        Err(Error::Server { code, message }) => {
            assert_eq!(code, 30);
            assert!(message.contains("Server must upgrade"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_response_without_the_expected_member_is_an_error() {
    let server = FakeServer::start();
    server.respond("getAlbum", json!({ "somethingElse": {} }));

    assert!(matches!(
        connect(&server).album("al-1"),
        Err(Error::Unexpected(_))
    ));
}

#[test]
fn a_server_that_is_not_subsonic_is_reported_as_such() {
    let server = FakeServer::start();
    server.reply_raw(
        "ping",
        200,
        "text/html",
        "<html>Welcome to my router</html>",
    );
    server.reply_raw("getArtists", 502, "text/html", "<html>Bad gateway</html>");
    let client = connect(&server);

    assert!(matches!(client.ping(), Err(Error::Unexpected(_))));
    assert!(matches!(client.artists(), Err(Error::Status(502))));
}

#[test]
fn a_server_that_is_not_there_is_an_http_error() {
    let client = Client::new("http://127.0.0.1:9", USER, PASSWORD);
    assert!(matches!(client.ping(), Err(Error::Http(_))));
}

#[test]
fn cover_art_returns_the_bytes_and_asks_for_the_size() {
    let server = FakeServer::start();
    server.cover("al-1", vec![0xFF, 0xD8, 0xFF, 1, 2, 3]);
    let client = connect(&server);

    assert_eq!(
        client.cover_art("al-1", Some(300)).unwrap(),
        [0xFF, 0xD8, 0xFF, 1, 2, 3]
    );
    client.cover_art("al-1", None).unwrap();

    let requests = server.requests_to("getCoverArt");
    assert_eq!(requests[0].params["size"], "300");
    assert!(!requests[1].params.contains_key("size"));
}

#[test]
fn missing_cover_art_is_not_found_however_the_server_says_so() {
    let server = FakeServer::start();
    let client = connect(&server);

    assert!(matches!(
        client.cover_art("nope", None),
        Err(Error::NotFound)
    ));
    server.mode(|mode| mode.missing_as_404 = true);
    assert!(matches!(
        client.cover_art("nope", None),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        Client::new(&server.url, USER, "wrong").cover_art("al-1", None),
        Err(Error::WrongCredentials)
    ));
}

#[test]
fn scrobble_reports_playing_and_played() {
    let server = FakeServer::start();
    let client = connect(&server);

    client.scrobble("s-1", false).unwrap();
    client.scrobble("s-1", true).unwrap();

    let requests = server.requests_to("scrobble");
    assert_eq!(requests[0].params["id"], "s-1");
    assert_eq!(requests[0].params["submission"], "false");
    assert_eq!(requests[1].params["submission"], "true");
    assert!(requests[1].params["time"].parse::<u64>().unwrap() > 1_700_000_000_000);
}

#[test]
fn stream_urls_are_complete_signed_requests() {
    let server = FakeServer::start();
    let client = connect(&server);

    let url = client.stream_url("a song&id=1", Some(192));
    let (path, query) = url.split_once('?').unwrap();
    let params: std::collections::HashMap<String, String> =
        form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

    assert_eq!(path, format!("{}/rest/stream", server.url));
    assert_eq!(
        params["id"], "a song&id=1",
        "the id is encoded, not spliced in"
    );
    assert_eq!(params["maxBitRate"], "192");
    assert_eq!(params["u"], USER);
    let expected = hex::encode(Md5::digest(format!("{PASSWORD}{}", params["s"])));
    assert_eq!(params["t"], expected);
    assert!(!url.contains(PASSWORD));
    assert_eq!(
        form_urlencoded::parse(
            client
                .stream_url("x", None)
                .split_once('?')
                .unwrap()
                .1
                .as_bytes()
        )
        .find(|(name, _)| name == "maxBitRate")
        .map(|(_, value)| value.into_owned()),
        Some("0".to_owned()),
        "no limit means as stored"
    );
}

// ---------------------------------------------------------------------------------------------
// The server's name
// ---------------------------------------------------------------------------------------------

#[test]
fn the_server_name_comes_from_its_ping() {
    let server = FakeServer::start();
    let client = connect(&server);

    server.respond(
        "ping",
        json!({ "type": "navidrome", "serverVersion": "0.52.5 (735e16b)" }),
    );
    assert_eq!(client.server_name().unwrap(), "navidrome 0.52.5 (735e16b)");

    server.respond("ping", json!({ "type": "Unsub" }));
    assert_eq!(client.server_name().unwrap(), "Unsub");

    // A classic Subsonic server says neither.
    server.respond("ping", json!({ "version": "1.16.1" }));
    assert_eq!(client.server_name().unwrap(), "Subsonic");
    server.respond("ping", json!({ "type": "", "serverVersion": "" }));
    assert_eq!(client.server_name().unwrap(), "Subsonic");
}

#[test]
fn asking_for_the_server_name_checks_the_credentials_too() {
    let server = FakeServer::start();
    assert!(matches!(
        Client::new(&server.url, USER, "wrong").server_name(),
        Err(Error::WrongCredentials)
    ));
}

#[test]
fn a_songs_cover_art_field_goes_straight_to_cover_art() {
    let server = FakeServer::start();
    server.cover("al-1", vec![1, 2, 3]);
    server.respond(
        "getAlbum",
        json!({ "album": { "id": "al-1", "name": "OK Computer", "song": [song_json("s-1", "Airbag", 1)] }}),
    );
    let client = connect(&server);

    let song = client.album("al-1").unwrap().songs.remove(0);

    assert_eq!(song.cover_art.as_deref(), Some("al-1"));
    assert_eq!(
        client
            .cover_art(song.cover_art.as_deref().unwrap(), Some(64))
            .unwrap(),
        [1, 2, 3]
    );
    assert_eq!(server.requests_to("getCoverArt")[0].params["id"], "al-1");
}

// ---------------------------------------------------------------------------------------------
// Every song
// ---------------------------------------------------------------------------------------------

fn titled(id: &str) -> serde_json::Value {
    song_json(id, id, 1)
}

fn titles(songs: &[Song]) -> Vec<&str> {
    songs.iter().map(|song| song.title.as_str()).collect()
}

/// A server whose `search3` hands out the whole library for an empty query, a page at a time.
fn serve_everything_by_search(server: &FakeServer, songs: Vec<serde_json::Value>) {
    server.respond_with("search3", move |params| {
        let offset: usize = params["songOffset"].parse().unwrap();
        let count: usize = params["songCount"].parse().unwrap();
        let page: Vec<_> = songs.iter().skip(offset).take(count).cloned().collect();
        json!({ "searchResult3": { "song": page } })
    });
}

/// A library of albums A(2 songs), B(3), C(1) and D(4), listed alphabetically and opened by id.
fn serve_albums(server: &FakeServer) {
    let albums = [("A", 2), ("B", 3), ("C", 1), ("D", 4)];
    server.respond_with("getAlbumList2", move |params| {
        let offset: usize = params["offset"].parse().unwrap();
        let size: usize = params["size"].parse().unwrap();
        assert_eq!(params["type"], "alphabeticalByName");
        let page: Vec<_> = albums
            .iter()
            .skip(offset)
            .take(size)
            .map(|(name, count)| json!({ "id": name, "name": name, "songCount": count }))
            .collect();
        json!({ "albumList2": { "album": page } })
    });
    server.respond_with("getAlbum", move |params| {
        let id = params["id"].as_str();
        let (name, count) = albums
            .iter()
            .find(|(name, _)| *name == id)
            .copied()
            .unwrap();
        let songs: Vec<_> = (1..=count).map(|n| titled(&format!("{name}{n}"))).collect();
        json!({ "album": { "id": name, "name": name, "songCount": count, "song": songs } })
    });
}

#[test]
fn every_song_is_paged_out_of_search3_with_an_empty_query() {
    let server = FakeServer::start();
    serve_everything_by_search(&server, (1..=7).map(|n| titled(&format!("s{n}"))).collect());
    let client = connect(&server);
    assert_eq!(
        client.song_listing(),
        None,
        "nothing is known before the first call"
    );

    let first = client.songs(0, 3).unwrap();
    let second = client.songs(3, 3).unwrap();
    let third = client.songs(6, 3).unwrap();
    let past_the_end = client.songs(9, 3).unwrap();

    assert_eq!(titles(&first), ["s1", "s2", "s3"]);
    assert_eq!(titles(&second), ["s4", "s5", "s6"]);
    assert_eq!(titles(&third), ["s7"]);
    assert!(past_the_end.is_empty());
    assert_eq!(client.song_listing(), Some(SongListing::Search));
    let params = &server.requests_to("search3")[0].params;
    assert_eq!(
        params["query"], "\"\"",
        "an empty phrase in quotes, which servers read as everything"
    );
    assert_eq!(params["artistCount"], "0");
    assert_eq!(params["albumCount"], "0");
    assert_eq!(params["songCount"], "3");
    assert_eq!(params["songOffset"], "0");
    assert!(server.requests_to("getAlbumList2").is_empty());
}

#[test]
fn a_server_that_refuses_the_empty_query_is_walked_album_by_album() {
    let server = FakeServer::start();
    server.fail("search3", 10, "Required parameter is missing: query");
    serve_albums(&server);
    let client = connect(&server);

    let first = client.songs(0, 4).unwrap();
    assert_eq!(titles(&first), ["A1", "A2", "B1", "B2"]);
    assert_eq!(client.song_listing(), Some(SongListing::AlbumWalk));
    let opened: Vec<_> = server
        .requests_to("getAlbum")
        .iter()
        .map(|r| r.params["id"].clone())
        .collect();
    assert_eq!(
        opened,
        ["A", "B"],
        "it stops opening albums once it has enough"
    );

    let second = client.songs(4, 4).unwrap();
    assert_eq!(titles(&second), ["B3", "C1", "D1", "D2"]);
    let opened: Vec<_> = server
        .requests_to("getAlbum")
        .iter()
        .skip(2)
        .map(|r| r.params["id"].clone())
        .collect();
    assert_eq!(
        opened,
        ["B", "C", "D"],
        "album A lies wholly before the window and is not opened"
    );

    assert_eq!(titles(&client.songs(8, 4).unwrap()), ["D3", "D4"]);
    assert!(client.songs(10, 4).unwrap().is_empty());
    assert_eq!(
        server.requests_to("search3").len(),
        1,
        "the refusal is remembered"
    );
}

#[test]
fn the_album_walk_pages_through_the_album_list_and_skips_albums_by_their_song_counts() {
    let server = FakeServer::start();
    server.fail("search3", 10, "Required parameter is missing: query");
    // 1,200 albums of one song each. The 1,101st song is the one in album al-1100.
    server.respond_with("getAlbumList2", |params| {
        let offset: usize = params["offset"].parse().unwrap();
        let size: usize = params["size"].parse().unwrap();
        let page: Vec<_> = (0..1200)
            .skip(offset)
            .take(size)
            .map(|n| json!({ "id": format!("al-{n}"), "name": format!("{n:04}"), "songCount": 1 }))
            .collect();
        json!({ "albumList2": { "album": page } })
    });
    server.respond_with("getAlbum", |params| {
        let id = params["id"].clone();
        json!({ "album": { "id": id, "name": id, "songCount": 1, "song": [titled(&id)] } })
    });

    let songs = connect(&server).songs(1100, 1).unwrap();

    assert_eq!(titles(&songs), ["al-1100"]);
    let opened: Vec<_> = server
        .requests_to("getAlbum")
        .iter()
        .map(|r| r.params["id"].clone())
        .collect();
    assert_eq!(
        opened,
        ["al-1100"],
        "the 1,100 albums before it were skipped unopened"
    );
    let pages: Vec<_> = server
        .requests_to("getAlbumList2")
        .iter()
        .map(|r| (r.params["offset"].clone(), r.params["size"].clone()))
        .collect();
    assert_eq!(
        pages,
        [
            ("0".into(), "500".into()),
            ("500".into(), "500".into()),
            ("1000".into(), "500".into())
        ]
    );
}

#[test]
fn the_album_walk_stops_at_a_short_last_page() {
    let server = FakeServer::start();
    server.fail("search3", 10, "Required parameter is missing: query");
    serve_albums(&server);

    // Asking for more than exists: the walk ends when the album list does.
    let songs = connect(&server).songs(0, 100).unwrap();

    assert_eq!(songs.len(), 10);
    assert_eq!(
        server.requests_to("getAlbumList2").len(),
        1,
        "four albums is a short page"
    );
}

#[test]
fn nothing_at_all_from_the_empty_query_also_sends_it_to_the_walk() {
    let server = FakeServer::start();
    // A server that answers "everything" with nothing.
    server.respond("search3", json!({ "searchResult3": {} }));
    serve_albums(&server);
    let client = connect(&server);

    let songs = client.songs(0, 3).unwrap();

    assert_eq!(titles(&songs), ["A1", "A2", "B1"]);
    assert_eq!(client.song_listing(), Some(SongListing::AlbumWalk));
}

#[test]
fn an_empty_library_is_just_empty() {
    let server = FakeServer::start();
    server.respond("search3", json!({ "searchResult3": {} }));
    server.respond("getAlbumList2", json!({ "albumList2": {} }));
    let client = connect(&server);

    assert!(client.songs(0, 50).unwrap().is_empty());
    assert_eq!(
        client.song_listing(),
        None,
        "no songs, so nothing was learned"
    );
}

#[test]
fn real_failures_are_not_a_reason_to_walk_albums() {
    let server = FakeServer::start();
    serve_albums(&server);

    assert!(matches!(
        Client::new(&server.url, USER, "wrong").songs(0, 10),
        Err(Error::WrongCredentials)
    ));
    assert!(matches!(
        Client::new("http://127.0.0.1:9", USER, PASSWORD).songs(0, 10),
        Err(Error::Http(_))
    ));
    assert!(server.requests_to("getAlbumList2").is_empty());
}

#[test]
fn asking_for_no_songs_asks_the_server_nothing() {
    let server = FakeServer::start();

    assert!(connect(&server).songs(0, 0).unwrap().is_empty());
    assert!(server.requests().is_empty());
}
