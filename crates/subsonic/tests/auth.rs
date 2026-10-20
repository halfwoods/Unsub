//! The three authentication modes, their failures, and what must never leak.

mod common;

use common::*;
use unsub_subsonic::ServerConfig;

/// The worked example from the Subsonic API documentation: password "sesame", salt "c19b2d".
const DOC_TOKEN: &str = "26719a1196d2a940705a59634eb18eab";
const DOC_SALT: &str = "c19b2d";

fn call(h: &Harness, query: &str) -> Resp {
    h.get_url(&format!(
        "{}/rest/getSong?id=s-1&v=1.16.1&c=tests&{query}",
        h.base
    ))
}

fn code(h: &Harness, query: &str) -> u32 {
    error_of(&call(h, query)).0
}

fn assert_ok(h: &Harness, query: &str) {
    let reply = call(h, query);
    assert_eq!(
        xml_ok(&reply).root_element().attribute("status"),
        Some("ok"),
        "{query}"
    );
}

#[test]
fn plain_password_is_accepted() {
    assert_ok(&Harness::start(), "u=alice&p=sesame");
}

#[test]
fn hex_encoded_password_is_accepted() {
    // "sesame" in hex, as clients send it for `enc:`.
    assert_ok(&Harness::start(), "u=alice&p=enc:736573616d65");
}

#[test]
fn token_from_the_subsonic_docs_is_accepted() {
    assert_ok(
        &Harness::start(),
        &format!("u=alice&t={DOC_TOKEN}&s={DOC_SALT}"),
    );
}

#[test]
fn token_hex_case_does_not_matter() {
    assert_ok(
        &Harness::start(),
        &format!("u=alice&t={}&s={DOC_SALT}", DOC_TOKEN.to_uppercase()),
    );
}

#[test]
fn token_wins_over_a_plain_password_when_both_are_sent() {
    let h = Harness::start();
    assert_eq!(
        code(
            &h,
            &format!("u=alice&p=sesame&t=00000000000000000000000000000000&s={DOC_SALT}")
        ),
        40
    );
}

#[test]
fn api_key_is_accepted_without_a_username() {
    let h = Harness::start();
    assert_ok(&h, &format!("apiKey={API_KEY}"));
    let reply = h.get_url(&format!(
        "{}/rest/ping?apiKey={API_KEY}&v=1.16.1&c=t&f=json",
        h.base
    ));
    assert_eq!(reply.json()["status"], "ok");
}

#[test]
fn wrong_plain_password_is_error_40_in_both_formats() {
    let h = Harness::start();
    assert_eq!(code(&h, "u=alice&p=nope"), 40);
    assert_eq!(code(&h, "u=alice&p=nope&f=json"), 40);
    assert_eq!(code(&h, "u=alice&p="), 40);
    let reply = call(&h, "u=alice&p=nope");
    assert_eq!(reply.status, 200);
    assert_eq!(error_of(&reply).1, "Wrong username or password");
}

#[test]
fn wrong_username_is_error_40() {
    let h = Harness::start();
    assert_eq!(code(&h, "u=mallory&p=sesame"), 40);
    assert_eq!(code(&h, "u=Alice&p=sesame"), 40);
    assert_eq!(code(&h, "u=alice%20&p=sesame"), 40);
}

#[test]
fn bad_hex_and_wrong_hex_passwords_are_error_40() {
    let h = Harness::start();
    assert_eq!(code(&h, "u=alice&p=enc:zz"), 40);
    assert_eq!(code(&h, "u=alice&p=enc:736573616d6"), 40);
    assert_eq!(code(&h, "u=alice&p=enc:00"), 40);
    // Valid hex of bytes that are not UTF-8.
    assert_eq!(code(&h, "u=alice&p=enc:fffe"), 40);
    assert_eq!(code(&h, "u=alice&p=enc:"), 40);
}

#[test]
fn wrong_token_salt_or_password_is_error_40() {
    let h = Harness::start();
    assert_eq!(code(&h, &format!("u=alice&t={DOC_TOKEN}&s=otherSalt")), 40);
    assert_eq!(
        code(&h, "u=alice&t=26719a1196d2a940705a59634eb18eab0&s=c19b2d"),
        40
    );
    assert_eq!(code(&h, "u=alice&t=short&s=c19b2d"), 40);
    assert_eq!(
        code(&h, &format!("u=mallory&t={DOC_TOKEN}&s={DOC_SALT}")),
        40
    );
}

#[test]
fn missing_credentials_are_error_10() {
    let h = Harness::start();
    assert_eq!(code(&h, ""), 10);
    assert_eq!(code(&h, "p=sesame"), 10);
    assert_eq!(code(&h, "u=alice"), 10);
    assert_eq!(code(&h, &format!("u=alice&t={DOC_TOKEN}")), 10);
    assert_eq!(code(&h, "u=alice&s=salt"), 10);
}

#[test]
fn wrong_api_key_is_error_44() {
    let h = Harness::start();
    assert_eq!(code(&h, "apiKey=wrong"), 44);
    assert_eq!(code(&h, &format!("apiKey={API_KEY}x")), 44);
    assert_eq!(code(&h, "apiKey=wrong&f=json"), 44);
}

#[test]
fn api_key_combined_with_other_credentials_is_error_43() {
    let h = Harness::start();
    for extra in ["u=alice", "p=sesame", "u=alice&p=sesame", "t=abc&s=def"] {
        assert_eq!(
            code(&h, &format!("apiKey={API_KEY}&{extra}")),
            43,
            "{extra}"
        );
    }
}

#[test]
fn api_key_is_error_42_when_none_is_configured() {
    let h = Harness::start_with(None);
    assert_eq!(code(&h, "apiKey=anything"), 42);
    assert_ok(&h, "u=alice&p=sesame");
}

#[test]
fn the_configured_key_does_not_work_as_a_password() {
    let h = Harness::start();
    assert_eq!(code(&h, &format!("u=alice&p={API_KEY}")), 40);
    assert_eq!(code(&h, &format!("u={API_KEY}&p=sesame")), 40);
}

#[test]
fn every_endpoint_demands_credentials() {
    let h = Harness::start();
    for endpoint in [
        "ping",
        "getLicense",
        "getMusicFolders",
        "getIndexes",
        "getArtists",
        "getArtist",
        "getAlbum",
        "getSong",
        "getAlbumList",
        "getAlbumList2",
        "getRandomSongs",
        "search2",
        "search3",
        "getGenres",
        "getPlaylists",
        "getPlaylist",
        "getCoverArt",
        "stream",
        "download",
        "scrobble",
        "getStarred",
        "getStarred2",
        "getUser",
        "getOpenSubsonicExtensions",
        "setRating",
        "star",
        "unstar",
        "createPlaylist",
        "updatePlaylist",
        "deletePlaylist",
        "savePlayQueue",
        "getPlayQueue",
        "getSongsByGenre",
        "getLyricsBySongId",
        "getLyrics",
        "getMusicDirectory",
        "getNowPlaying",
        "getScanStatus",
        "startScan",
        "tokenInfo",
        "getBookmarks",
        "jukeboxControl",
    ] {
        let reply = h.get_url(&format!(
            "{}/rest/{endpoint}?id=s-1&type=newest&query=x&username=alice",
            h.base
        ));
        assert_eq!(error_of(&reply).0, 10, "{endpoint}");
        let reply = h.get_url(&format!(
            "{}/rest/{endpoint}?u=alice&p=bad&id=s-1&type=newest&query=x",
            h.base
        ));
        assert_eq!(error_of(&reply).0, 40, "{endpoint}");
    }
    assert!(h.source.calls.lock().unwrap().scrobbles.is_empty());
    assert!(h.source.calls.lock().unwrap().open_stream.is_empty());
}

#[test]
fn an_unauthenticated_stream_never_reaches_the_source() {
    let h = Harness::start();
    let reply = h.get_url(&format!("{}/rest/stream?id=s-1&u=alice&p=bad", h.base));
    assert!(
        reply
            .header("content-type")
            .unwrap()
            .starts_with("text/xml")
    );
    assert!(reply.body.len() < 1000);
    assert!(h.source.calls.lock().unwrap().open_stream.is_empty());
}

#[test]
fn credentials_in_a_post_form_work_for_every_mode() {
    let h = Harness::start();
    let ok = |form: &[(&str, &str)]| {
        h.post(
            "getSong.view",
            &[
                &[("id", "s-1"), ("f", "json"), ("v", "1.16.1"), ("c", "t")],
                form,
            ]
            .concat(),
        )
    };
    assert_eq!(
        ok(&[("u", "alice"), ("p", "sesame")]).json()["status"],
        "ok"
    );
    assert_eq!(
        ok(&[("u", "alice"), ("t", DOC_TOKEN), ("s", DOC_SALT)]).json()["status"],
        "ok"
    );
    assert_eq!(ok(&[("apiKey", API_KEY)]).json()["status"], "ok");
    assert_eq!(
        ok(&[("u", "alice"), ("p", "x")]).json()["error"]["code"],
        40
    );
}

#[test]
fn non_ascii_and_reserved_characters_in_the_password_work() {
    let password = "pässwörd & more=100%+";
    let h = Harness::start_with_password(None, password);
    let post = |form: &[(&str, &str)]| {
        h.post(
            "ping",
            &[&[("f", "json"), ("v", "1.16.1"), ("c", "t")], form].concat(),
        )
    };
    assert_eq!(
        post(&[("u", "alice"), ("p", password)]).json()["status"],
        "ok"
    );
    let encoded = format!("enc:{}", hex::encode(password));
    assert_eq!(
        post(&[("u", "alice"), ("p", &encoded)]).json()["status"],
        "ok"
    );
    let token = md5_hex(&format!("{password}salty"));
    assert_eq!(
        post(&[("u", "alice"), ("t", &token), ("s", "salty")]).json()["status"],
        "ok"
    );
    assert_eq!(
        post(&[("u", "alice"), ("p", "pässwörd")]).json()["error"]["code"],
        40
    );
}

fn md5_hex(text: &str) -> String {
    use md5::{Digest, Md5};
    hex::encode(Md5::digest(text))
}

#[test]
fn failed_logins_never_echo_the_credentials() {
    let h = Harness::start();
    for query in [
        "u=alice&p=hunter2-guess",
        "u=alice&t=feedfacefeedfacefeedfacefeedface&s=hunter2salt",
        "apiKey=guess-key-123",
    ] {
        for format in ["", "&f=json"] {
            let reply = call(&h, &format!("{query}{format}"));
            let text = reply.text();
            for secret in [
                "hunter2-guess",
                "feedface",
                "hunter2salt",
                "guess-key-123",
                PASSWORD,
                API_KEY,
            ] {
                assert!(!text.contains(secret), "{secret} leaked in {text}");
            }
        }
    }
}

#[test]
fn debug_output_of_the_config_redacts_secrets() {
    let config = ServerConfig {
        bind: ([127, 0, 0, 1], 4533).into(),
        username: "alice".into(),
        password: "top-secret-password".into(),
        api_key: Some("top-secret-key".into()),
        default_max_kbps: None,
    };
    let debug = format!("{config:?} {config:#?}");
    assert!(!debug.contains("top-secret"));
    assert!(debug.contains("alice") && debug.contains("4533") && debug.contains("<redacted>"));
}
