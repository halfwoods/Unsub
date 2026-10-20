//! What phone apps do beyond browsing and playing: rate, star, edit playlists, share a play queue,
//! read lyrics, and the endpoints they probe on the way.

mod common;

use common::*;
use serde_json::json;
use unsub_subsonic::{Capabilities, PlaylistEdit, Targets};

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|&id| id.to_owned()).collect()
}

fn playlist_songs(h: &Harness, id: &str) -> Vec<String> {
    let store = h.source.playlists.lock().unwrap();
    store
        .iter()
        .find(|(playlist, _)| playlist.id == id)
        .map(|(_, songs)| songs.clone())
        .unwrap()
}

mod ratings {
    use super::*;

    #[test]
    fn set_rating_passes_the_stars_in_both_formats() {
        let h = Harness::start();
        xml_ok(&h.get("setRating", "id=s-1&rating=4"));
        h.json("setRating", "id=al-1&rating=1");
        h.json("setRating", "id=ar-2&rating=0");
        assert_eq!(
            h.source.calls.lock().unwrap().ratings,
            [
                ("s-1".to_owned(), 4),
                ("al-1".to_owned(), 1),
                ("ar-2".to_owned(), 0)
            ]
        );
    }

    #[test]
    fn set_rating_over_post() {
        let h = Harness::start();
        let reply = h.post(
            "setRating.view",
            &[
                ("u", USER),
                ("p", PASSWORD),
                ("v", "1.16.1"),
                ("c", "t"),
                ("f", "json"),
                ("id", "s-1"),
                ("rating", "3"),
            ],
        );
        assert_eq!(reply.json()["status"], "ok");
        assert_eq!(
            h.source.calls.lock().unwrap().ratings,
            [("s-1".to_owned(), 3)]
        );
    }

    #[test]
    fn set_rating_checks_its_parameters() {
        let h = Harness::start();
        assert_eq!(error_of(&h.get("setRating", "rating=3")).0, 10);
        assert_eq!(error_of(&h.get("setRating", "id=s-1")).0, 10);
        for bad in ["6", "-1", "five", "255"] {
            let (code, message) = error_of(&h.get("setRating", &format!("id=s-1&rating={bad}")));
            assert_eq!(code, 0, "{bad}");
            assert!(message.contains("rating"), "{message}");
        }
        assert_eq!(error_of(&h.get("setRating", "id=nope&rating=3")).0, 70);
        assert!(h.source.calls.lock().unwrap().ratings.is_empty());
    }

    #[test]
    fn star_and_unstar_sort_ids_into_songs_albums_and_artists() {
        let h = Harness::start();
        xml_ok(&h.get(
            "star",
            "id=s-1&id=s-2&albumId=al-1&artistId=ar-1&artistId=ar-2",
        ));
        h.json("unstar", "albumId=al-3");
        h.json("unstar", "id=s-4&artistId=ar-3");
        let calls = h.source.calls.lock().unwrap();
        assert_eq!(
            calls.stars,
            [Targets {
                songs: ids(&["s-1", "s-2"]),
                albums: ids(&["al-1"]),
                artists: ids(&["ar-1", "ar-2"])
            }]
        );
        assert_eq!(
            calls.unstars,
            [
                Targets {
                    songs: vec![],
                    albums: ids(&["al-3"]),
                    artists: vec![]
                },
                Targets {
                    songs: ids(&["s-4"]),
                    albums: vec![],
                    artists: ids(&["ar-3"])
                }
            ]
        );
    }

    #[test]
    fn star_needs_something_to_star_and_ignores_empty_ids() {
        let h = Harness::start();
        for endpoint in ["star", "unstar"] {
            assert_eq!(error_of(&h.get(endpoint, "")).0, 10, "{endpoint}");
            assert_eq!(
                error_of(&h.get(endpoint, "id=&albumId=")).0,
                10,
                "{endpoint}"
            );
        }
        assert_eq!(error_of(&h.get("star", "id=nope")).0, 70);
        assert!(h.source.calls.lock().unwrap().stars.is_empty());
    }

    #[test]
    fn songs_carry_their_star_and_rating_in_both_formats() {
        let h = Harness::start();
        let song = &h.json("getSong", "id=s-1")["song"];
        assert_eq!(song["starred"], "2023-11-14T22:13:25Z");
        assert_eq!(song["userRating"], 5);
        let doc = xml_ok(&h.get("getSong", "id=s-1"));
        let song = child(doc.root_element(), "song");
        assert_eq!(
            (song.attribute("starred"), song.attribute("userRating")),
            (Some("2023-11-14T22:13:25Z"), Some("5"))
        );
        // A song nobody starred or rated has neither attribute.
        let song = &h.json("getSong", "id=s-2")["song"];
        assert!(song.get("starred").is_none() && song.get("userRating").is_none());
    }
}

mod playlist_editing {
    use super::*;

    #[test]
    fn create_playlist_makes_one_and_answers_with_it() {
        let h = Harness::start();
        let doc = xml_ok(&h.get(
            "createPlaylist",
            "name=Road+Trip&songId=s-3&songId=s-1&songId=s-3",
        ));
        let playlist = child(doc.root_element(), "playlist");
        assert_eq!(playlist.attribute("name"), Some("Road Trip"));
        assert_eq!(playlist.attribute("songCount"), Some("3"));
        let entries: Vec<_> = children(playlist, "entry")
            .iter()
            .map(|e| e.attribute("id").unwrap())
            .collect();
        assert_eq!(entries, ["s-3", "s-1", "s-3"]);

        let body = h.json("createPlaylist", "name=Empty");
        assert_eq!(body["playlist"]["name"], "Empty");
        assert_eq!(body["playlist"]["songCount"], 0);
        assert_eq!(
            h.source.calls.lock().unwrap().created,
            [
                ("Road Trip".to_owned(), ids(&["s-3", "s-1", "s-3"])),
                ("Empty".to_owned(), vec![])
            ]
        );
        // And it is a playlist like any other.
        let listed = h.json("getPlaylists", "");
        assert_eq!(listed["playlists"]["playlist"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn create_playlist_needs_a_name() {
        let h = Harness::start();
        assert_eq!(error_of(&h.get("createPlaylist", "songId=s-1")).0, 10);
        assert_eq!(error_of(&h.get("createPlaylist", "name=")).0, 10);
        assert!(h.source.calls.lock().unwrap().created.is_empty());
    }

    #[test]
    fn create_playlist_with_an_id_replaces_the_songs() {
        let h = Harness::start();
        let body = h.json("createPlaylist", "playlistId=pl-1&songId=s-4&songId=s-5");
        let entries: Vec<_> = body["playlist"]["entry"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap())
            .collect();
        assert_eq!(entries, ["s-4", "s-5"]);
        assert_eq!(playlist_songs(&h, "pl-1"), ids(&["s-4", "s-5"]));
        // One edit, so a source can make the swap atomic.
        let edits = h.source.calls.lock().unwrap().edits.clone();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].1.remove, [0, 1]);
        assert_eq!(edits[0].1.add, ids(&["s-4", "s-5"]));
    }

    #[test]
    fn create_playlist_with_an_unknown_or_smart_id() {
        let h = Harness::start();
        assert_eq!(
            error_of(&h.get("createPlaylist", "playlistId=nope&songId=s-1")).0,
            70
        );
        assert_eq!(
            error_of(&h.get("createPlaylist", "playlistId=pl-smart&songId=s-1")).0,
            50
        );
        assert_eq!(playlist_songs(&h, "pl-smart"), ids(&["s-2"]));
    }

    #[test]
    fn update_playlist_renames_describes_and_publishes() {
        let h = Harness::start();
        xml_ok(&h.get(
            "updatePlaylist",
            "playlistId=pl-1&name=Best+Of&comment=For+the+car&public=true",
        ));
        let edits = h.source.calls.lock().unwrap().edits.clone();
        assert_eq!(
            edits,
            [(
                "pl-1".to_owned(),
                PlaylistEdit {
                    name: Some("Best Of".into()),
                    comment: Some("For the car".into()),
                    public: Some(true),
                    ..PlaylistEdit::default()
                }
            )]
        );
        let body = h.json("getPlaylist", "id=pl-1");
        assert_eq!(body["playlist"]["name"], "Best Of");
        assert_eq!(body["playlist"]["comment"], "For the car");
        assert_eq!(body["playlist"]["public"], true);

        h.json("updatePlaylist", "playlistId=pl-1&public=false&comment=");
        let body = h.json("getPlaylist", "id=pl-1");
        assert_eq!(body["playlist"]["public"], false);
        // An empty comment is a comment cleared, not a comment left alone.
        assert_eq!(body["playlist"]["comment"], "");
        assert_eq!(body["playlist"]["name"], "Best Of");
    }

    #[test]
    fn update_playlist_removes_by_position_then_adds() {
        let h = Harness::start();
        h.json(
            "updatePlaylist",
            "playlistId=pl-1&songIdToAdd=s-4&songIdToAdd=s-5",
        );
        assert_eq!(
            playlist_songs(&h, "pl-1"),
            ids(&["s-1", "s-3", "s-4", "s-5"])
        );

        // Positions refer to the list as it stood, so removing 0 and 2 takes s-1 and s-4.
        h.json(
            "updatePlaylist",
            "playlistId=pl-1&songIndexToRemove=0&songIndexToRemove=2&songIdToAdd=s-9",
        );
        assert_eq!(playlist_songs(&h, "pl-1"), ids(&["s-3", "s-5", "s-9"]));
        let edits = h.source.calls.lock().unwrap().edits.clone();
        assert_eq!(edits[1].1.remove, [0, 2]);
        assert_eq!(edits[1].1.add, ids(&["s-9"]));
        assert_eq!(edits[1].1.name, None);
    }

    #[test]
    fn update_playlist_checks_its_parameters() {
        let h = Harness::start();
        assert_eq!(error_of(&h.get("updatePlaylist", "name=x")).0, 10);
        let (code, message) =
            error_of(&h.get("updatePlaylist", "playlistId=pl-1&songIndexToRemove=first"));
        assert_eq!(code, 0);
        assert!(message.contains("songIndexToRemove"));
        assert_eq!(
            error_of(&h.get("updatePlaylist", "playlistId=nope&name=x")).0,
            70
        );
        assert_eq!(
            error_of(&h.get("updatePlaylist", "playlistId=pl-smart&name=x")).0,
            50
        );
        assert_eq!(playlist_songs(&h, "pl-1"), ids(&["s-1", "s-3"]));
    }

    #[test]
    fn delete_playlist_removes_regular_playlists_only() {
        let h = Harness::start();
        xml_ok(&h.get("deletePlaylist", "id=pl-1"));
        assert_eq!(h.source.calls.lock().unwrap().deleted, ["pl-1"]);
        assert_eq!(error_of(&h.get("getPlaylist", "id=pl-1")).0, 70);
        assert_eq!(error_of(&h.get("deletePlaylist", "id=pl-1")).0, 70);
        assert_eq!(error_of(&h.get("deletePlaylist", "id=pl-smart")).0, 50);
        assert_eq!(error_of(&h.get("deletePlaylist", "")).0, 10);
        assert!(
            h.json("getPlaylist", "id=pl-smart")["playlist"]["readonly"]
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn get_user_says_whether_playlists_can_be_edited() {
        let h = Harness::start();
        let user = &h.json("getUser", &format!("username={USER}"))["user"];
        assert_eq!(user["playlistRole"], true);
        // Still not an administrator, whatever the source can do.
        assert_eq!(user["adminRole"], false);
        *h.source.capabilities.lock().unwrap() = Capabilities::default();
        let doc = xml_ok(&h.get("getUser", &format!("username={USER}")));
        assert_eq!(
            child(doc.root_element(), "user").attribute("playlistRole"),
            Some("false")
        );
    }
}

mod play_queue {
    use super::*;

    #[test]
    fn nothing_saved_is_an_empty_answer() {
        let h = Harness::start();
        let doc = xml_ok(&h.get("getPlayQueue", ""));
        assert_eq!(
            doc.root_element()
                .children()
                .filter(|c| c.is_element())
                .count(),
            0
        );
        assert!(h.json("getPlayQueue", "").get("playQueue").is_none());
    }

    #[test]
    fn a_saved_queue_comes_back_with_full_songs() {
        let h = Harness::start();
        xml_ok(&h.get(
            "savePlayQueue",
            "id=s-3&id=s-1&id=s-2&current=s-1&position=61500",
        ));

        let doc = xml_ok(&h.get("getPlayQueue", ""));
        let queue = child(doc.root_element(), "playQueue");
        assert_eq!(queue.attribute("current"), Some("s-1"));
        assert_eq!(queue.attribute("position"), Some("61500"));
        assert_eq!(queue.attribute("username"), Some(USER));
        assert_eq!(queue.attribute("changedBy"), Some("tests"));
        assert!(queue.attribute("changed").unwrap().ends_with('Z'));
        let entries = children(queue, "entry");
        assert_eq!(
            entries
                .iter()
                .map(|e| e.attribute("id").unwrap())
                .collect::<Vec<_>>(),
            ["s-3", "s-1", "s-2"]
        );
        assert_eq!(entries[1].attribute("title"), Some("Come Together"));
        assert_eq!(entries[1].attribute("type"), Some("music"));

        let body = h.json("getPlayQueue", "");
        assert_eq!(body["playQueue"]["current"], "s-1");
        assert_eq!(body["playQueue"]["position"], 61_500);
        assert_eq!(body["playQueue"]["entry"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn saving_again_replaces_it_and_saving_nothing_clears_it() {
        let h = Harness::start();
        h.json("savePlayQueue", "id=s-1&id=s-2&current=s-2");
        h.json("savePlayQueue", "id=s-4&current=s-4&position=5");
        let body = h.json("getPlayQueue", "");
        assert_eq!(body["playQueue"]["entry"].as_array().unwrap().len(), 1);
        assert_eq!(body["playQueue"]["current"], "s-4");
        h.json("savePlayQueue", "");
        assert!(h.json("getPlayQueue", "").get("playQueue").is_none());
    }

    #[test]
    fn missing_position_and_current_are_fine_and_a_negative_position_is_zero() {
        let h = Harness::start();
        h.json("savePlayQueue", "id=s-1&id=s-2");
        let body = h.json("getPlayQueue", "");
        assert_eq!(body["playQueue"]["position"], 0);
        assert!(body["playQueue"].get("current").is_none());
        h.json("savePlayQueue", "id=s-1&current=s-1&position=-40");
        assert_eq!(h.json("getPlayQueue", "")["playQueue"]["position"], 0);
        assert_eq!(
            error_of(&h.get("savePlayQueue", "id=s-1&position=soon")).0,
            0
        );
    }

    #[test]
    fn songs_deleted_since_are_dropped_and_so_is_a_current_song_that_went_with_them() {
        let h = Harness::start();
        h.json(
            "savePlayQueue",
            "id=s-1&id=gone&id=s-2&current=gone&position=9",
        );
        let body = h.json("getPlayQueue", "");
        let entries: Vec<_> = body["playQueue"]["entry"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap())
            .collect();
        assert_eq!(entries, ["s-1", "s-2"]);
        assert!(body["playQueue"].get("current").is_none());
    }

    #[test]
    fn it_is_shared_between_clients_and_kept_in_memory_only() {
        let h = Harness::start();
        h.json("savePlayQueue", "id=s-1&current=s-1&c=phone");
        let url = format!(
            "{}/rest/getPlayQueue?u={USER}&p={PASSWORD}&v=1.16.1&c=tablet&f=json",
            h.base
        );
        let body = h.get_url(&url).json();
        assert_eq!(body["playQueue"]["changedBy"], "tests");
        assert_eq!(body["playQueue"]["entry"][0]["id"], "s-1");
        // A fresh server starts with none.
        assert!(
            Harness::start()
                .json("getPlayQueue", "")
                .get("playQueue")
                .is_none()
        );
    }

    #[test]
    fn a_source_failure_while_loading_is_reported() {
        let h = Harness::start();
        h.json("savePlayQueue", "id=s-1&id=broken");
        assert_eq!(
            error_of(&h.get("getPlayQueue", "")),
            (0, "database is locked".to_owned())
        );
    }
}

mod reading {
    use super::*;

    #[test]
    fn songs_by_genre_pages_and_filters() {
        let h = Harness::start();
        let doc = xml_ok(&h.get("getSongsByGenre", "genre=Rock&count=3&offset=1"));
        let songs = children(child(doc.root_element(), "songsByGenre"), "song");
        assert_eq!(
            songs
                .iter()
                .map(|s| s.attribute("id").unwrap())
                .collect::<Vec<_>>(),
            ["s-2", "s-3", "s-4"]
        );
        assert_eq!(songs[0].attribute("type"), Some("music"));

        let body = h.json("getSongsByGenre", "genre=Rock");
        assert_eq!(body["songsByGenre"]["song"].as_array().unwrap().len(), 7);
        let none = h.json("getSongsByGenre", "genre=Polka");
        assert_eq!(none["songsByGenre"]["song"], json!([]));

        h.json("getSongsByGenre", "genre=Rock&count=99999");
        assert_eq!(
            h.source.calls.lock().unwrap().songs_by_genre,
            [
                ("Rock".to_owned(), 3, 1),
                ("Rock".to_owned(), 10, 0),
                ("Polka".to_owned(), 10, 0),
                ("Rock".to_owned(), 500, 0)
            ]
        );
    }

    #[test]
    fn songs_by_genre_needs_a_genre() {
        let h = Harness::start();
        assert_eq!(error_of(&h.get("getSongsByGenre", "")).0, 10);
        assert_eq!(
            error_of(&h.get("getSongsByGenre", "genre=Rock&count=lots")).0,
            0
        );
    }

    #[test]
    fn lyrics_by_song_id_are_one_unsynced_block_a_line_per_line() {
        let h = Harness::start();
        let doc = xml_ok(&h.get("getLyricsBySongId", "id=s-1"));
        let lyrics = child(child(doc.root_element(), "lyricsList"), "structuredLyrics");
        assert_eq!(lyrics.attribute("displayArtist"), Some("The Beatles"));
        assert_eq!(lyrics.attribute("displayTitle"), Some("Come Together"));
        assert_eq!(lyrics.attribute("lang"), Some("eng"));
        assert_eq!(lyrics.attribute("synced"), Some("false"));
        let lines: Vec<_> = children(lyrics, "line")
            .iter()
            .map(|l| (l.text().unwrap_or(""), l.attribute("start")))
            .collect();
        assert_eq!(
            lines,
            [
                ("Here comes the sun", None),
                ("Little darling", None),
                ("", None),
                ("It's alright", None)
            ]
        );

        let body = h.json("getLyricsBySongId", "id=s-1");
        let block = &body["lyricsList"]["structuredLyrics"][0];
        assert_eq!(block["synced"], false);
        assert_eq!(block["lang"], "eng");
        assert_eq!(block["line"][0], json!({ "value": "Here comes the sun" }));
        assert_eq!(block["line"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn lyrics_of_unknown_language_say_xxx() {
        let h = Harness::start();
        let body = h.json("getLyricsBySongId", "id=s-3");
        assert_eq!(body["lyricsList"]["structuredLyrics"][0]["lang"], "xxx");
        assert_eq!(
            body["lyricsList"]["structuredLyrics"][0]["displayArtist"],
            "Daft Punk"
        );
    }

    #[test]
    fn a_song_without_lyrics_gets_an_empty_list_and_an_unknown_song_an_error() {
        let h = Harness::start();
        let doc = xml_ok(&h.get("getLyricsBySongId", "id=s-2"));
        assert_eq!(
            children(child(doc.root_element(), "lyricsList"), "structuredLyrics").len(),
            0
        );
        assert_eq!(
            h.json("getLyricsBySongId", "id=s-2")["lyricsList"]["structuredLyrics"],
            json!([])
        );
        assert_eq!(error_of(&h.get("getLyricsBySongId", "id=nope")).0, 70);
        assert_eq!(error_of(&h.get("getLyricsBySongId", "")).0, 10);
        assert_eq!(error_of(&h.get("getSong", "id=broken")).0, 0);
    }

    #[test]
    fn legacy_get_lyrics_finds_the_song_by_title_and_artist() {
        let h = Harness::start();
        let doc = xml_ok(&h.get("getLyrics", "artist=the+beatles&title=COME+TOGETHER"));
        let lyrics = child(doc.root_element(), "lyrics");
        assert_eq!(
            (lyrics.attribute("artist"), lyrics.attribute("title")),
            (Some("The Beatles"), Some("Come Together"))
        );
        assert!(lyrics.text().unwrap().starts_with("Here comes the sun"));

        let body = h.json("getLyrics", "title=One+More+Time");
        assert_eq!(body["lyrics"]["artist"], "Daft Punk");
        assert_eq!(body["lyrics"]["value"], "Around the world");
    }

    #[test]
    fn legacy_get_lyrics_without_a_match_is_an_empty_element() {
        let h = Harness::start();
        for query in [
            "title=Nonexistent",
            "artist=Nobody&title=Come+Together",
            "title=Something",
        ] {
            assert_eq!(h.json("getLyrics", query)["lyrics"], json!({}), "{query}");
        }
        assert_eq!(error_of(&h.get("getLyrics", "artist=The+Beatles")).0, 10);
    }

    #[test]
    fn music_directory_lists_an_artists_albums_and_an_albums_songs() {
        let h = Harness::start();
        let doc = xml_ok(&h.get("getMusicDirectory", "id=ar-1"));
        let directory = child(doc.root_element(), "directory");
        assert_eq!(
            (
                directory.attribute("id"),
                directory.attribute("name"),
                directory.attribute("parent")
            ),
            (Some("ar-1"), Some("The Beatles"), None)
        );
        let albums = children(directory, "child");
        assert_eq!(
            albums
                .iter()
                .map(|c| c.attribute("id").unwrap())
                .collect::<Vec<_>>(),
            ["al-1", "al-2"]
        );
        assert_eq!(
            (albums[0].attribute("isDir"), albums[0].attribute("parent")),
            (Some("true"), Some("ar-1"))
        );

        let doc = xml_ok(&h.get("getMusicDirectory", "id=al-1"));
        let directory = child(doc.root_element(), "directory");
        assert_eq!(
            (directory.attribute("name"), directory.attribute("parent")),
            (Some("Abbey Road"), Some("ar-1"))
        );
        let songs = children(directory, "child");
        assert_eq!(
            songs
                .iter()
                .map(|c| c.attribute("id").unwrap())
                .collect::<Vec<_>>(),
            ["s-1", "s-2"]
        );
        assert_eq!(
            (songs[0].attribute("isDir"), songs[0].attribute("type")),
            (Some("false"), Some("music"))
        );

        let body = h.json("getMusicDirectory", "id=al-3");
        assert_eq!(body["directory"]["child"][0]["title"], "One More Time");
        assert_eq!(body["directory"]["parent"], "ar-2");
    }

    #[test]
    fn music_directory_needs_a_known_id() {
        let h = Harness::start();
        assert_eq!(error_of(&h.get("getMusicDirectory", "id=nope")).0, 70);
        assert_eq!(error_of(&h.get("getMusicDirectory", "")).0, 10);
    }

    #[test]
    fn token_info_names_the_account() {
        let h = Harness::start();
        let reply = h.get_url(&format!(
            "{}/rest/tokenInfo?apiKey={API_KEY}&v=1.16.1&c=t&f=json",
            h.base
        ));
        assert_eq!(reply.json()["tokenInfo"], json!({ "username": USER }));
    }

    #[test]
    fn scan_status_is_never_scanning() {
        let h = Harness::start();
        for endpoint in ["getScanStatus", "startScan"] {
            assert_eq!(
                h.json(endpoint, "")["scanStatus"],
                json!({ "scanning": false }),
                "{endpoint}"
            );
            let doc = xml_ok(&h.get(endpoint, ""));
            assert_eq!(
                child(doc.root_element(), "scanStatus").attribute("scanning"),
                Some("false")
            );
        }
    }
}

mod now_playing {
    use super::*;

    fn url(h: &Harness, endpoint: &str, client: &str, query: &str) -> String {
        format!(
            "{}/rest/{endpoint}?u={USER}&p={PASSWORD}&v=1.16.1&c={client}&f=json&{query}",
            h.base
        )
    }

    #[test]
    fn it_reports_what_clients_say_they_are_playing() {
        let h = Harness::start();
        assert_eq!(
            h.json("getNowPlaying", "")["nowPlaying"],
            json!({ "entry": [] })
        );

        h.get_url(&url(&h, "scrobble", "phone", "id=s-1&submission=false"));
        h.get_url(&url(&h, "scrobble", "tablet", "id=s-3&submission=false"));
        let body = h.json("getNowPlaying", "");
        let entries = body["nowPlaying"]["entry"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            (entries[0]["id"].as_str(), entries[0]["playerName"].as_str()),
            (Some("s-1"), Some("phone"))
        );
        assert_eq!(
            (
                entries[1]["title"].as_str(),
                entries[1]["playerName"].as_str()
            ),
            (Some("One More Time"), Some("tablet"))
        );
        assert_eq!(entries[0]["username"], USER);
        assert_eq!(entries[0]["minutesAgo"], 0);
        assert_eq!(entries[0]["type"], "music");

        let doc = xml_ok(&h.get("getNowPlaying", ""));
        assert_eq!(
            children(child(doc.root_element(), "nowPlaying"), "entry").len(),
            2
        );
    }

    #[test]
    fn a_client_moving_on_replaces_its_song() {
        let h = Harness::start();
        h.get_url(&url(&h, "scrobble", "phone", "id=s-1&submission=false"));
        h.get_url(&url(&h, "scrobble", "phone", "id=s-2&submission=false"));
        let body = h.json("getNowPlaying", "");
        let entries = body["nowPlaying"]["entry"].as_array().unwrap();
        assert_eq!((entries.len(), entries[0]["id"].as_str()), (1, Some("s-2")));
    }

    #[test]
    fn a_play_that_counts_ends_the_entry_but_only_for_that_client_and_song() {
        let h = Harness::start();
        h.get_url(&url(&h, "scrobble", "phone", "id=s-1&submission=false"));
        h.get_url(&url(&h, "scrobble", "phone", "id=s-2"));
        h.get_url(&url(&h, "scrobble", "tablet", "id=s-1"));
        assert_eq!(
            h.json("getNowPlaying", "")["nowPlaying"]["entry"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        h.get_url(&url(&h, "scrobble", "phone", "id=s-1"));
        assert_eq!(
            h.json("getNowPlaying", "")["nowPlaying"]["entry"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn a_song_that_no_longer_exists_is_not_listed() {
        let h = Harness::start();
        h.get_url(&url(&h, "scrobble", "phone", "id=ghost&submission=false"));
        assert_eq!(
            h.json("getNowPlaying", "")["nowPlaying"]["entry"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn a_ping_the_server_rejects_is_not_remembered() {
        let h = Harness::start();
        h.get_url(&url(
            &h,
            "scrobble",
            "phone",
            "id=s-1&time=soon&submission=false",
        ));
        assert_eq!(
            h.json("getNowPlaying", "")["nowPlaying"]["entry"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }
}

mod probes {
    use super::*;

    #[test]
    fn endpoints_for_features_this_server_lacks_answer_with_an_empty_element() {
        let h = Harness::start();
        for (endpoint, query, root) in [
            ("getInternetRadioStations", "", "internetRadioStations"),
            ("getPodcasts", "", "podcasts"),
            ("getNewestPodcasts", "", "newestPodcasts"),
            ("getBookmarks", "", "bookmarks"),
            ("getShares", "", "shares"),
            ("getChatMessages", "", "chatMessages"),
            ("getVideos", "", "videos"),
            ("getArtistInfo", "id=ar-1", "artistInfo"),
            ("getArtistInfo2", "id=ar-1", "artistInfo2"),
            ("getAlbumInfo", "id=al-1", "albumInfo"),
            ("getAlbumInfo2", "id=al-1", "albumInfo"),
            ("getSimilarSongs", "id=s-1", "similarSongs"),
            ("getSimilarSongs2", "id=s-1", "similarSongs2"),
            ("getTopSongs", "artist=The+Beatles", "topSongs"),
        ] {
            assert_eq!(h.json(endpoint, query)[root], json!({}), "{endpoint}");
            let doc = xml_ok(&h.get(endpoint, query));
            let element = child(doc.root_element(), root);
            assert_eq!(
                element.children().filter(|c| c.is_element()).count(),
                0,
                "{endpoint}"
            );
        }
    }

    #[test]
    fn those_that_need_an_id_still_ask_for_one() {
        let h = Harness::start();
        for (endpoint, missing) in [
            ("getArtistInfo2", "id"),
            ("getAlbumInfo2", "id"),
            ("getSimilarSongs2", "id"),
            ("getTopSongs", "artist"),
        ] {
            let (code, message) = error_of(&h.get(endpoint, ""));
            assert_eq!(code, 10, "{endpoint}");
            assert!(message.contains(missing), "{message}");
        }
    }

    #[test]
    fn operations_that_need_a_role_the_account_lacks_are_not_authorised() {
        let h = Harness::start();
        for endpoint in [
            "jukeboxControl",
            "createShare",
            "updateShare",
            "deleteShare",
            "createPodcastChannel",
            "deletePodcastChannel",
            "refreshPodcasts",
            "downloadPodcastEpisode",
            "deletePodcastEpisode",
            "createUser",
            "updateUser",
            "deleteUser",
            "changePassword",
        ] {
            for suffix in ["", "&f=json"] {
                assert_eq!(
                    error_of(&h.get(endpoint, suffix.trim_start_matches('&'))).0,
                    50,
                    "{endpoint}"
                );
            }
        }
        // Other things nobody implemented stay "unsupported", not "forbidden".
        assert_eq!(error_of(&h.get("createBookmark", "")).0, 0);
    }
}
