//! Every endpoint, in XML and JSON, against a fake library.

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use antistreaming_subsonic::{AlbumListKind, Capabilities};
use common::*;
use serde_json::json;

#[test]
fn ping_xml_has_namespace_and_envelope_attributes() {
    let h = Harness::start();
    let reply = h.get("ping", "");
    assert_eq!(reply.status, 200);
    let doc = xml_ok(&reply);
    let root = doc.root_element();
    assert_eq!(root.tag_name().namespace(), Some(NS));
    assert_eq!(root.attribute("version"), Some("1.16.1"));
    assert_eq!(root.attribute("type"), Some("AntiStreaming"));
    assert_eq!(
        root.attribute("serverVersion"),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(root.attribute("openSubsonic"), Some("true"));
    assert!(
        reply
            .text()
            .starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>")
    );
}

#[test]
fn ping_json_has_envelope_fields() {
    let h = Harness::start();
    let body = h.json("ping", "");
    assert_eq!(body["version"], "1.16.1");
    assert_eq!(body["type"], "AntiStreaming");
    assert_eq!(body["serverVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["openSubsonic"], true);
}

#[test]
fn view_suffix_works_like_the_bare_name() {
    let h = Harness::start();
    for endpoint in ["ping.view", "getLicense.view"] {
        let reply = h.get(endpoint, "");
        assert_eq!(
            xml_ok(&reply).root_element().attribute("status"),
            Some("ok"),
            "{endpoint}"
        );
    }
}

#[test]
fn post_form_carries_credentials_and_parameters() {
    let h = Harness::start();
    let form = [
        ("u", USER),
        ("p", PASSWORD),
        ("v", "1.16.1"),
        ("c", "tests"),
        ("f", "json"),
        ("id", "al-1"),
    ];
    let reply = h.post("getAlbum", &form);
    let body = reply.json();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["album"]["id"], "al-1");

    let reply = h.post(
        "getAlbum.view",
        &[("u", USER), ("p", "wrong"), ("v", "1.16.1"), ("c", "t")],
    );
    assert_eq!(error_of(&reply).0, 40);
}

#[test]
fn query_string_and_form_body_combine_on_post() {
    let h = Harness::start();
    let url = h.url("getAlbum", "f=json");
    let reply = h.post_url_with_form(&url, &[("id", "al-3")]);
    assert_eq!(reply.json()["album"]["id"], "al-3");
}

#[test]
fn get_license_is_valid() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getLicense", ""));
    assert_eq!(
        child(doc.root_element(), "license").attribute("valid"),
        Some("true")
    );
    assert_eq!(h.json("getLicense", "")["license"]["valid"], true);
}

#[test]
fn get_music_folders_lists_one_folder() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getMusicFolders", ""));
    let folders = children(child(doc.root_element(), "musicFolders"), "musicFolder");
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].attribute("id"), Some("1"));
    assert_eq!(
        h.json("getMusicFolders", "")["musicFolders"]["musicFolder"],
        json!([{ "id": 1, "name": "Music" }])
    );
}

#[test]
fn get_artists_xml_groups_alphabetically_ignoring_articles() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getArtists", ""));
    let artists = child(doc.root_element(), "artists");
    assert!(
        artists
            .attribute("ignoredArticles")
            .unwrap()
            .split(' ')
            .any(|a| a == "The")
    );
    let groups: Vec<_> = children(artists, "index")
        .into_iter()
        .map(|index| {
            let names: Vec<_> = children(index, "artist")
                .iter()
                .map(|a| a.attribute("name").unwrap())
                .collect();
            (index.attribute("name").unwrap(), names)
        })
        .collect();
    assert_eq!(
        groups,
        [
            ("#", vec!["10cc"]),
            ("B", vec!["The Beatles"]),
            ("D", vec!["Daft Punk"])
        ]
    );
    let beatles = children(children(artists, "index")[1], "artist")[0];
    assert_eq!(beatles.attribute("id"), Some("ar-1"));
    assert_eq!(beatles.attribute("albumCount"), Some("2"));
    assert_eq!(beatles.attribute("coverArt"), Some("ar-1-art"));
}

#[test]
fn get_artists_json_matches_the_subsonic_shape() {
    let h = Harness::start();
    let body = h.json("getArtists", "");
    let index = body["artists"]["index"].as_array().unwrap();
    assert_eq!(
        index
            .iter()
            .map(|i| i["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["#", "B", "D"]
    );
    assert_eq!(
        index[1]["artist"][0],
        json!({
            "id": "ar-1", "name": "The Beatles", "albumCount": 2, "coverArt": "ar-1-art",
            "starred": "2023-11-14T22:13:25Z"
        })
    );
}

#[test]
fn get_indexes_has_last_modified_and_same_groups() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getIndexes", ""));
    let indexes = child(doc.root_element(), "indexes");
    assert!(
        indexes
            .attribute("lastModified")
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    assert_eq!(children(indexes, "index").len(), 3);
    let body = h.json("getIndexes", "");
    assert_eq!(body["indexes"]["index"].as_array().unwrap().len(), 3);
    assert!(body["indexes"]["ignoredArticles"].is_string());
}

#[test]
fn get_artist_lists_albums() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getArtist", "id=ar-1"));
    let artist = child(doc.root_element(), "artist");
    assert_eq!(artist.attribute("name"), Some("The Beatles"));
    let albums = children(artist, "album");
    assert_eq!(
        albums
            .iter()
            .map(|a| a.attribute("id").unwrap())
            .collect::<Vec<_>>(),
        ["al-1", "al-2"]
    );
    assert_eq!(albums[0].attribute("artistId"), Some("ar-1"));

    let body = h.json("getArtist", "id=ar-1");
    assert_eq!(body["artist"]["album"].as_array().unwrap().len(), 2);
    assert_eq!(body["artist"]["album"][1]["name"], "Revolver");
}

#[test]
fn get_album_lists_songs_with_every_standard_field() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getAlbum", "id=al-1"));
    let album = child(doc.root_element(), "album");
    for (name, value) in [
        ("id", "al-1"),
        ("name", "Abbey Road"),
        ("artist", "The Beatles"),
        ("artistId", "ar-1"),
        ("year", "1969"),
        ("genre", "Rock"),
        ("songCount", "2"),
        ("duration", "400"),
        ("coverArt", "al-1"),
        ("created", "2023-11-14T22:13:20Z"),
    ] {
        assert_eq!(album.attribute(name), Some(value), "album {name}");
    }
    let songs = children(album, "song");
    assert_eq!(songs.len(), 2);
    for (name, value) in [
        ("id", "s-1"),
        ("parent", "al-1"),
        ("title", "Come Together"),
        ("album", "Abbey Road"),
        ("artist", "The Beatles"),
        ("track", "1"),
        ("discNumber", "1"),
        ("year", "1969"),
        ("genre", "Rock"),
        ("size", "10000"),
        ("contentType", "audio/mpeg"),
        ("suffix", "mp3"),
        ("duration", "259"),
        ("bitRate", "320"),
        ("path", "The Beatles/Abbey Road/Come Together.mp3"),
        ("albumId", "al-1"),
        ("artistId", "ar-1"),
        ("coverArt", "al-1"),
        ("playCount", "7"),
        ("created", "2023-11-14T22:13:20Z"),
        ("type", "music"),
        ("isDir", "false"),
        ("isVideo", "false"),
    ] {
        assert_eq!(songs[0].attribute(name), Some(value), "song {name}");
    }
}

#[test]
fn get_album_json_nests_songs_as_an_array() {
    let h = Harness::start();
    let body = h.json("getAlbum", "id=al-1");
    assert_eq!(body["album"]["songCount"], 2);
    assert_eq!(body["album"]["created"], "2023-11-14T22:13:20Z");
    let songs = body["album"]["song"].as_array().unwrap();
    assert_eq!(songs.len(), 2);
    assert_eq!(songs[0]["track"], 1);
    assert_eq!(songs[0]["type"], "music");
    assert_eq!(songs[0]["isDir"], false);
    assert_eq!(songs[0]["size"], 10_000);
}

#[test]
fn get_song_returns_one_song() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getSong", "id=s-3"));
    let song = child(doc.root_element(), "song");
    assert_eq!(
        (song.attribute("title"), song.attribute("artist")),
        (Some("One More Time"), Some("Daft Punk"))
    );
    assert_eq!(h.json("getSong", "id=s-3")["song"]["albumId"], "al-3");
}

#[test]
fn fields_the_source_leaves_out_are_omitted_rather_than_null() {
    let h = Harness::start();
    let body = h.json("getSong", "id=s-8");
    let mut keys: Vec<_> = body["song"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["id", "isDir", "isVideo", "title", "type"]);

    let doc = xml_ok(&h.get("getSong", "id=s-8"));
    let song = child(doc.root_element(), "song");
    assert_eq!(song.attributes().count(), 5);
    assert_eq!(h.json("getArtist", "id=ar-3")["artist"]["album"], json!([]));
}

#[test]
fn unknown_ids_are_error_70() {
    let h = Harness::start();
    for (endpoint, query) in [
        ("getArtist", "id=nope"),
        ("getAlbum", "id=nope"),
        ("getSong", "id=nope"),
        ("getPlaylist", "id=nope"),
    ] {
        let xml = error_of(&h.get(endpoint, query));
        let json = error_of(&h.get(endpoint, &format!("{query}&f=json")));
        assert_eq!(xml.0, 70, "{endpoint}");
        assert_eq!(json.0, 70, "{endpoint}");
        assert_eq!(
            h.get(endpoint, query).status,
            200,
            "{endpoint}: Subsonic errors are HTTP 200"
        );
    }
}

#[test]
fn missing_required_parameters_are_error_10() {
    let h = Harness::start();
    for (endpoint, query, missing) in [
        ("getArtist", "", "id"),
        ("getAlbum", "", "id"),
        ("getSong", "", "id"),
        ("getPlaylist", "", "id"),
        ("getAlbumList", "", "type"),
        ("getAlbumList2", "", "type"),
        ("getAlbumList2", "type=byYear&fromYear=1960", "toYear"),
        ("getAlbumList2", "type=byGenre", "genre"),
        ("search3", "", "query"),
        ("search2", "", "query"),
        ("getCoverArt", "", "id"),
        ("stream", "", "id"),
        ("download", "", "id"),
        ("scrobble", "", "id"),
        ("getUser", "", "username"),
    ] {
        for suffix in ["", "&f=json"] {
            let reply = h.get(endpoint, &format!("{query}{suffix}"));
            let (code, message) = error_of(&reply);
            assert_eq!(code, 10, "{endpoint}");
            assert!(message.contains(missing), "{endpoint}: {message}");
        }
    }
}

#[test]
fn malformed_numbers_are_a_generic_error_naming_the_parameter() {
    let h = Harness::start();
    let (code, message) = error_of(&h.get("getAlbumList2", "type=newest&size=lots"));
    assert_eq!(code, 0);
    assert!(message.contains("size"));
}

#[test]
fn album_list_passes_kind_paging_and_filters_to_the_source() {
    let h = Harness::start();
    let kinds = [
        ("random", AlbumListKind::Random),
        ("newest", AlbumListKind::Newest),
        ("frequent", AlbumListKind::Frequent),
        ("recent", AlbumListKind::Recent),
        ("alphabeticalByName", AlbumListKind::AlphabeticalByName),
        ("alphabeticalByArtist", AlbumListKind::AlphabeticalByArtist),
    ];
    for (name, kind) in kinds {
        h.json(
            "getAlbumList2",
            &format!("type={name}&size=2&offset=1&genre=ignored&fromYear=1&toYear=2"),
        );
        let call = h.source.calls.lock().unwrap().album_list.pop().unwrap();
        assert_eq!(call, (kind, 2, 1, None, None), "{name}");
    }
    h.json("getAlbumList2", "type=byGenre&genre=Rock");
    h.json(
        "getAlbumList2",
        "type=byYear&fromYear=2000&toYear=1960&size=999",
    );
    let calls = &h.source.calls.lock().unwrap().album_list;
    assert_eq!(
        calls[0],
        (AlbumListKind::ByGenre, 10, 0, Some("Rock".into()), None)
    );
    assert_eq!(
        calls[1],
        (AlbumListKind::ByYear, 500, 0, None, Some((2000, 1960)))
    );
}

#[test]
fn album_list2_uses_id3_albums_and_album_list_uses_directory_albums() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getAlbumList2", "type=newest&size=2"));
    let list = child(doc.root_element(), "albumList2");
    let albums = children(list, "album");
    assert_eq!(albums.len(), 2);
    assert_eq!(albums[0].attribute("name"), Some("Abbey Road"));
    assert_eq!(albums[0].attribute("isDir"), None);

    let doc = xml_ok(&h.get("getAlbumList", "type=newest&size=2"));
    let albums = children(child(doc.root_element(), "albumList"), "album");
    assert_eq!(albums[0].attribute("isDir"), Some("true"));
    assert_eq!(albums[0].attribute("title"), Some("Abbey Road"));
    assert_eq!(albums[0].attribute("parent"), Some("ar-1"));

    let body = h.json("getAlbumList", "type=newest&size=1");
    assert_eq!(body["albumList"]["album"][0]["isDir"], true);
    let body = h.json("getAlbumList2", "type=newest&size=1&offset=2");
    assert_eq!(body["albumList2"]["album"][0]["id"], "al-3");
}

#[test]
fn the_starred_album_list_asks_the_source_and_highest_is_always_empty() {
    let h = Harness::start();
    for endpoint in ["getAlbumList", "getAlbumList2"] {
        let body = h.json(endpoint, "type=starred&size=5&offset=1");
        let list = body
            .as_object()
            .unwrap()
            .iter()
            .find(|(k, _)| k.starts_with("albumList"))
            .unwrap()
            .1;
        assert_eq!(list["album"].as_array().unwrap().len(), 2, "{endpoint}");
        let call = h.source.calls.lock().unwrap().album_list.pop().unwrap();
        assert_eq!(
            call,
            (AlbumListKind::Starred, 5, 1, None, None),
            "{endpoint}"
        );

        let body = h.json(endpoint, "type=highest");
        let list = body
            .as_object()
            .unwrap()
            .iter()
            .find(|(k, _)| k.starts_with("albumList"))
            .unwrap()
            .1;
        assert_eq!(list["album"], json!([]), "{endpoint}");
    }
    assert!(h.source.calls.lock().unwrap().album_list.is_empty());
}

#[test]
fn unknown_album_list_type_is_rejected() {
    let h = Harness::start();
    assert_eq!(error_of(&h.get("getAlbumList2", "type=sideways")).0, 0);
}

#[test]
fn get_random_songs_in_both_formats_and_with_filters() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getRandomSongs", "size=3"));
    assert_eq!(
        children(child(doc.root_element(), "randomSongs"), "song").len(),
        3
    );
    let body = h.json(
        "getRandomSongs",
        "size=2&genre=Rock&fromYear=2000&toYear=1960",
    );
    assert_eq!(body["randomSongs"]["song"].as_array().unwrap().len(), 2);
    let calls = &h.source.calls.lock().unwrap().random_songs;
    assert_eq!(calls[0], (3, None, None));
    assert_eq!(calls[1], (2, Some("Rock".into()), Some((1960, 2000))));
}

#[test]
fn random_songs_with_one_year_bound_is_open_ended() {
    let h = Harness::start();
    h.json("getRandomSongs", "fromYear=1990");
    h.json("getRandomSongs", "toYear=1970&size=600");
    let calls = &h.source.calls.lock().unwrap().random_songs;
    assert_eq!(calls[0], (10, None, Some((1990, i32::MAX))));
    assert_eq!(calls[1], (500, None, Some((i32::MIN, 1970))));
}

#[test]
fn search3_returns_id3_results_and_forwards_counts_and_offsets() {
    let h = Harness::start();
    let doc = xml_ok(&h.get(
        "search3",
        "query=beatles&artistCount=5&albumCount=1&albumOffset=1&songCount=7&songOffset=0",
    ));
    let result = child(doc.root_element(), "searchResult3");
    assert_eq!(children(result, "artist").len(), 1);
    assert_eq!(children(result, "album")[0].attribute("id"), Some("al-2"));
    assert!(!children(result, "song").is_empty());
    let call = h.source.calls.lock().unwrap().search.pop().unwrap();
    assert_eq!(call, ("beatles".into(), (5, 0), (1, 1), (7, 0)));

    let body = h.json("search3", "query=beatles");
    assert_eq!(body["searchResult3"]["artist"][0]["name"], "The Beatles");
    let call = h.source.calls.lock().unwrap().search.pop().unwrap();
    assert_eq!(call, ("beatles".into(), (20, 0), (20, 0), (20, 0)));
}

#[test]
fn search2_returns_directory_style_albums() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("search2", "query=beatles"));
    let result = child(doc.root_element(), "searchResult2");
    assert_eq!(
        children(result, "album")[0].attribute("isDir"),
        Some("true")
    );
    assert_eq!(
        h.json("search2", "query=beatles")["searchResult2"]["album"][0]["title"],
        "Abbey Road"
    );
}

#[test]
fn search_with_no_matches_is_an_empty_result() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("search3", "query=zzzz"));
    assert!(children(child(doc.root_element(), "searchResult3"), "song").is_empty());
    assert_eq!(
        h.json("search3", "query=zzzz")["searchResult3"]["song"],
        json!([])
    );
}

#[test]
fn empty_and_quoted_empty_queries_reach_the_source_as_empty() {
    let h = Harness::start();
    for query in ["query=", "query=%22%22"] {
        let body = h.json("search3", &format!("{query}&songCount=500&songOffset=3"));
        assert!(
            !body["searchResult3"]["song"].as_array().unwrap().is_empty(),
            "{query}"
        );
        let call = h.source.calls.lock().unwrap().search.pop().unwrap();
        assert_eq!(call.0, "", "{query}");
        assert_eq!((call.3.0, call.3.1), (500, 3));
    }
}

#[test]
fn get_genres_uses_element_text_in_xml_and_value_in_json() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getGenres", ""));
    let genres = children(child(doc.root_element(), "genres"), "genre");
    assert_eq!(genres[0].text(), Some("Rock"));
    assert_eq!(genres[0].attribute("songCount"), Some("6"));
    assert_eq!(genres[0].attribute("albumCount"), Some("2"));
    assert_eq!(genres[1].text(), Some("R&B <soul>"));

    let body = h.json("getGenres", "");
    assert_eq!(
        body["genres"]["genre"][0],
        json!({ "value": "Rock", "songCount": 6, "albumCount": 2 })
    );
}

#[test]
fn get_playlists_and_get_playlist() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getPlaylists", ""));
    let lists = children(child(doc.root_element(), "playlists"), "playlist");
    assert_eq!(lists.len(), 2);
    assert_eq!(lists[0].attribute("readonly"), None);
    assert_eq!(lists[1].attribute("id"), Some("pl-smart"));
    assert_eq!(lists[1].attribute("readonly"), Some("true"));
    for (name, value) in [
        ("id", "pl-1"),
        ("name", "Favourites"),
        ("songCount", "2"),
        ("duration", "518"),
        ("created", "2023-11-14T22:13:20Z"),
        ("changed", "2023-11-14T22:14:20Z"),
        ("owner", USER),
    ] {
        assert_eq!(lists[0].attribute(name), Some(value), "{name}");
    }

    let doc = xml_ok(&h.get("getPlaylist", "id=pl-1"));
    let playlist = child(doc.root_element(), "playlist");
    let entries = children(playlist, "entry");
    assert_eq!(
        entries
            .iter()
            .map(|e| e.attribute("id").unwrap())
            .collect::<Vec<_>>(),
        ["s-1", "s-3"]
    );
    assert_eq!(entries[0].attribute("type"), Some("music"));

    assert_eq!(
        h.json("getPlaylists", "")["playlists"]["playlist"][0]["name"],
        "Favourites"
    );
    let body = h.json("getPlaylist", "id=pl-1");
    assert_eq!(body["playlist"]["entry"].as_array().unwrap().len(), 2);
    assert_eq!(body["playlist"]["entry"][1]["title"], "One More Time");
}

#[test]
fn get_starred2_lists_what_the_source_says_is_starred_in_id3_form() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getStarred2", ""));
    let starred = child(doc.root_element(), "starred2");
    let artist = child(starred, "artist");
    assert_eq!(
        (artist.attribute("id"), artist.attribute("starred")),
        (Some("ar-1"), Some("2023-11-14T22:13:25Z"))
    );
    let album = child(starred, "album");
    assert_eq!(
        (album.attribute("name"), album.attribute("userRating")),
        (Some("Abbey Road"), Some("5"))
    );
    assert_eq!(album.attribute("isDir"), None);
    let song = child(starred, "song");
    assert_eq!(
        (song.attribute("id"), song.attribute("userRating")),
        (Some("s-1"), Some("5"))
    );
    assert_eq!(song.attribute("starred"), Some("2023-11-14T22:13:25Z"));

    let body = h.json("getStarred2", "");
    assert_eq!(body["starred2"]["artist"][0]["id"], "ar-1");
    assert_eq!(body["starred2"]["album"][0]["id"], "al-1");
    assert_eq!(body["starred2"]["song"][0]["id"], "s-1");
}

#[test]
fn get_starred_uses_directory_style_albums() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getStarred", ""));
    let starred = child(doc.root_element(), "starred");
    assert_eq!(child(starred, "album").attribute("isDir"), Some("true"));
    assert_eq!(child(starred, "song").attribute("type"), Some("music"));
    assert_eq!(
        h.json("getStarred", "")["starred"]["album"][0]["title"],
        "Abbey Road"
    );
}

#[test]
fn get_user_describes_a_streaming_only_account() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getUser", &format!("username={USER}")));
    let user = child(doc.root_element(), "user");
    assert_eq!(user.attribute("username"), Some(USER));
    assert_eq!(user.attribute("adminRole"), Some("false"));
    assert_eq!(user.attribute("streamRole"), Some("true"));
    assert_eq!(user.attribute("downloadRole"), Some("true"));
    let body = h.json("getUser", &format!("username={USER}"));
    assert_eq!(body["user"]["adminRole"], false);
    assert_eq!(body["user"]["streamRole"], true);
}

#[test]
fn get_user_for_someone_else_is_not_authorized() {
    let h = Harness::start();
    assert_eq!(error_of(&h.get("getUser", "username=bob")).0, 50);
}

#[test]
fn open_subsonic_extensions_follow_the_configuration() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getOpenSubsonicExtensions", ""));
    let names: Vec<_> = children(doc.root_element(), "openSubsonicExtensions")
        .iter()
        .map(|e| e.attribute("name").unwrap())
        .collect();
    assert_eq!(names, ["formPost", "apiKeyAuthentication", "songLyrics"]);
    let first = children(doc.root_element(), "openSubsonicExtensions")[0];
    assert_eq!(child(first, "versions").text(), Some("1"));

    let body = h.json("getOpenSubsonicExtensions", "");
    assert_eq!(
        body["openSubsonicExtensions"][1],
        json!({ "name": "apiKeyAuthentication", "versions": [1] })
    );

    let without_key = Harness::start_with(None);
    let body = without_key.json("getOpenSubsonicExtensions", "");
    assert_eq!(body["openSubsonicExtensions"].as_array().unwrap().len(), 2);

    // Lyrics are advertised only by a source that has them.
    *without_key.source.capabilities.lock().unwrap() = Capabilities::default();
    let body = without_key.json("getOpenSubsonicExtensions", "");
    assert_eq!(
        body["openSubsonicExtensions"],
        json!([{ "name": "formPost", "versions": [1] }])
    );
}

#[test]
fn every_response_says_open_subsonic() {
    let h = Harness::start();
    for endpoint in [
        "ping",
        "getLicense",
        "getGenres",
        "getArtists",
        "getStarred",
    ] {
        assert_eq!(h.json(endpoint, "")["openSubsonic"], true, "{endpoint}");
    }
    let (code, _) = error_of(&h.get("getAlbum", "id=nope&f=json"));
    assert_eq!(code, 70);
    assert_eq!(
        h.get("getAlbum", "id=nope&f=json").json()["openSubsonic"],
        true
    );
    assert_eq!(
        parse_xml(&h.get("getAlbum", "id=nope"))
            .root_element()
            .attribute("openSubsonic"),
        Some("true")
    );
}

#[test]
fn json_is_chosen_by_f_and_everything_else_is_xml() {
    let h = Harness::start();
    assert!(
        h.get("ping", "f=json")
            .header("content-type")
            .unwrap()
            .starts_with("application/json")
    );
    assert!(
        h.get("ping", "f=JSON")
            .header("content-type")
            .unwrap()
            .starts_with("application/json")
    );
    for f in ["f=xml", "f=", "f=yaml"] {
        assert!(
            h.get("ping", f)
                .header("content-type")
                .unwrap()
                .starts_with("text/xml"),
            "{f}"
        );
    }
}

#[test]
fn unknown_endpoint_is_a_generic_error_but_only_after_authentication() {
    let h = Harness::start();
    let (code, message) = error_of(&h.get("createBookmark", ""));
    assert_eq!(code, 0);
    assert!(message.contains("createBookmark"));
    let unauthenticated = h.get_url(&format!(
        "{}/rest/createBookmark?u={USER}&p=wrong&v=1.16.1&c=t",
        h.base
    ));
    assert_eq!(error_of(&unauthenticated).0, 40);
}

#[test]
fn paths_outside_rest_are_plain_404s() {
    let h = Harness::start();
    assert_eq!(h.get_url(&format!("{}/", h.base)).status, 404);
    assert_eq!(h.get_url(&format!("{}/rest/a/b", h.base)).status, 404);
}

#[test]
fn source_errors_become_generic_errors_with_their_message() {
    let h = Harness::start();
    let (code, message) = error_of(&h.get("getSong", "id=broken"));
    assert_eq!(code, 0);
    assert_eq!(message, "database is locked");
}

#[test]
fn a_panicking_source_does_not_take_the_server_down() {
    let h = Harness::start();
    let (code, _) = error_of(&h.get("getSong", "id=boom&f=json"));
    assert_eq!(code, 0);
    assert_eq!(h.json("ping", "")["status"], "ok");
    assert_eq!(h.json("getSong", "id=s-1")["song"]["id"], "s-1");
}

#[test]
fn markup_in_metadata_is_escaped_and_forbidden_characters_dropped() {
    let h = Harness::start();
    let doc = xml_ok(&h.get("getSong", "id=s-5"));
    assert_eq!(
        child(doc.root_element(), "song").attribute("title"),
        Some("Tom & Jerry <live> \"x\"")
    );
    assert_eq!(
        h.json("getSong", "id=s-5")["song"]["title"],
        "Tom & Jerry <live> \"x\"\u{1}"
    );
}

#[test]
fn cover_art_returns_image_bytes_and_forwards_size() {
    let h = Harness::start();
    let reply = h.get("getCoverArt", "id=al-1&size=300");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some("image/png"));
    assert_eq!(reply.body, b"\x89PNG fake image");
    h.get("getCoverArt", "id=al-1");
    let calls = &h.source.calls.lock().unwrap().cover_art;
    assert_eq!(calls[0], ("al-1".to_owned(), Some(300)));
    assert_eq!(calls[1], ("al-1".to_owned(), None));
}

#[test]
fn unknown_cover_art_is_404_with_a_subsonic_error() {
    let h = Harness::start();
    let reply = h.get("getCoverArt", "id=nope&f=json");
    assert_eq!(reply.status, 404);
    assert_eq!(error_of(&reply).0, 70);
}

#[test]
fn scrobble_submission_defaults_to_true_and_passes_the_time_in_milliseconds() {
    let h = Harness::start();
    xml_ok(&h.get("scrobble", "id=s-1&time=1700000123456"));
    h.json("scrobble", "id=s-3&time=5000&submission=true");
    let calls = h.source.calls.lock().unwrap();
    assert_eq!(
        calls.scrobbles,
        [
            ("s-1".to_owned(), 1_700_000_123_456),
            ("s-3".to_owned(), 5_000)
        ]
    );
    assert!(calls.now_playing.is_empty());
}

#[test]
fn scrobble_without_a_time_means_now() {
    let h = Harness::start();
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    h.json("scrobble", "id=s-1");
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let scrobbles = h.source.calls.lock().unwrap().scrobbles.clone();
    assert_eq!(scrobbles.len(), 1);
    assert!((before..=after).contains(&scrobbles[0].1), "{scrobbles:?}");
}

#[test]
fn submission_false_is_a_now_playing_ping_naming_the_client() {
    let h = Harness::start();
    xml_ok(&h.get("scrobble", "id=s-2&submission=false&time=1234"));
    h.json("scrobble", "id=s-3&id=s-1&submission=FALSE");
    let calls = h.source.calls.lock().unwrap();
    assert_eq!(
        calls.now_playing,
        [
            ("s-2".to_owned(), "tests".to_owned()),
            ("s-3".to_owned(), "tests".to_owned()),
            ("s-1".to_owned(), "tests".to_owned())
        ]
    );
    assert!(calls.scrobbles.is_empty(), "a ping is not a play");
}

#[test]
fn now_playing_without_a_client_name_says_unknown() {
    let h = Harness::start();
    let url = format!(
        "{}/rest/scrobble?u={USER}&p={PASSWORD}&v=1.16.1&id=s-1&submission=false",
        h.base
    );
    xml_ok(&h.get_url(&url));
    assert_eq!(
        h.source.calls.lock().unwrap().now_playing,
        [("s-1".to_owned(), "unknown".to_owned())]
    );
}

#[test]
fn scrobble_pairs_repeated_ids_with_repeated_times_and_fills_in_the_rest() {
    let h = Harness::start();
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    h.json("scrobble", "id=s-1&time=1000&id=s-2&time=2000&id=s-3");
    let scrobbles = h.source.calls.lock().unwrap().scrobbles.clone();
    assert_eq!(
        scrobbles[..2],
        [("s-1".to_owned(), 1_000), ("s-2".to_owned(), 2_000)]
    );
    assert_eq!(scrobbles[2].0, "s-3");
    assert!(scrobbles[2].1 >= before);
}

#[test]
fn scrobble_rejects_a_malformed_time() {
    let h = Harness::start();
    assert_eq!(error_of(&h.get("scrobble", "id=s-1&time=yesterday")).0, 0);
    assert!(h.source.calls.lock().unwrap().scrobbles.is_empty());
}
