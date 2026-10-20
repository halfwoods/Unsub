//! Scenarios built from how Music names things, run over an in-memory folder.

use std::path::Path;

use super::tree::fake::Fake;
use super::*;
use crate::item::{MediaType, Ownership};

const ROOT: &str = "/music";

fn song(id: &str, title: &str, artist: &str, album: &str) -> MusicLibraryItem {
    MusicLibraryItem {
        persistent_id: id.to_owned(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album_artist: String::new(),
        album: album.to_owned(),
        genre: String::new(),
        duration_ms: None,
        track_number: None,
        disc_number: None,
        year: None,
        kind: "MPEG audio file".to_owned(),
        media: MediaType::Music,
        ownership: Ownership::Unknown,
        location: None,
        file_present: false,
        play_count: 0,
        skip_count: 0,
        rating: 0,
        date_added: None,
        last_played: None,
        last_skipped: None,
        enabled: true,
    }
}

impl MusicLibraryItem {
    fn track(mut self, track: u32) -> Self {
        self.track_number = Some(track);
        self
    }

    fn disc(mut self, disc: u32) -> Self {
        self.disc_number = Some(disc);
        self
    }

    fn by(mut self, album_artist: &str) -> Self {
        self.album_artist = album_artist.to_owned();
        self
    }

    fn lasting(mut self, seconds: u64) -> Self {
        self.duration_ms = Some(seconds * 1000);
        self
    }

    fn added(mut self, when: i64) -> Self {
        self.date_added = Some(when);
        self
    }

    fn kind(mut self, kind: &str) -> Self {
        self.kind = kind.to_owned();
        self
    }
}

/// A folder with the given files, each a cloud-only 1,000-byte file.
fn folder(files: &[&str]) -> Fake {
    let mut fake = Fake::default();
    for file in files {
        fake.file(Path::new(ROOT), file, 1_000, true);
    }
    fake
}

fn locate(fake: &Fake, items: &[MusicLibraryItem]) -> Located {
    locate_with(fake, items, Path::new(ROOT), &mut |_| {}).unwrap()
}

/// The file found for an item, relative to the folder, and how sure.
fn found(located: &Located, id: &str) -> Option<(String, Confidence)> {
    let file = located.matched.get(id)?;
    let relative = file.path.strip_prefix(ROOT).unwrap();
    Some((relative.to_string_lossy().into_owned(), file.confidence))
}

fn exact(path: &str) -> Option<(String, Confidence)> {
    Some((path.to_owned(), Confidence::Exact))
}

fn folder_match(path: &str) -> Option<(String, Confidence)> {
    Some((path.to_owned(), Confidence::Folder))
}

#[test]
fn finds_a_song_by_the_name_music_gave_it() {
    let fake = folder(&[
        "Radiohead/OK Computer/01 Airbag.m4a",
        "Radiohead/OK Computer/02 Lucky.m4a",
    ]);
    let items = [song("a", "Airbag", "Radiohead", "OK Computer").track(1)];

    let located = locate(&fake, &items);

    assert_eq!(
        found(&located, "a"),
        exact("Radiohead/OK Computer/01 Airbag.m4a")
    );
    assert!(located.matched["a"].dataless);
    assert!(located.unmatched.is_empty());
}

#[test]
fn reports_which_files_are_not_cloud_only() {
    let mut fake = Fake::default();
    fake.file(Path::new(ROOT), "A/B/01 C.mp3", 1_000, false);

    let located = locate(&fake, &[song("a", "C", "A", "B").track(1)]);

    assert!(!located.matched["a"].dataless);
}

#[test]
fn applies_music_s_rules_for_unsafe_characters() {
    let fake = folder(&[
        "AC_DC/Live_ At Donington/02 Who Made Who_.mp3",
        "AC_DC/Live_ At Donington/03 Jeff_s Boogie.mp3",
    ]);
    let items = [
        song("a", "Who Made Who?", "AC/DC", "Live: At Donington").track(2),
        song("b", "Jeff\u{2019}s Boogie", "AC/DC", "Live: At Donington").track(3),
    ];

    let located = locate(&fake, &items);

    assert_eq!(
        found(&located, "a"),
        exact("AC_DC/Live_ At Donington/02 Who Made Who_.mp3")
    );
    assert_eq!(
        found(&located, "b"),
        exact("AC_DC/Live_ At Donington/03 Jeff_s Boogie.mp3")
    );
}

#[test]
fn applies_music_s_rules_for_the_edges_of_names() {
    let fake = folder(&[
        "Aphex Twin/_..I Care Because You Do/01 Heliosphan.mp3",
        "Big K.R.I.T_/Live From The Underground/04 End..mp3",
        "Fever Ray/Fever Ray/01 If I Had A Heart.mp3",
        "Tania Maria/Páginas/_Se Quiser.mp3",
    ]);
    let items = [
        song("a", "Heliosphan", "Aphex Twin", "...I Care Because You Do").track(1),
        song("b", "End.", "Big K.R.I.T.", "Live From The Underground").track(4),
        song("c", "If I Had A Heart", "Fever Ray ", "Fever Ray ").track(1),
        song("d", " Se Quiser", "Tania Maria", "Páginas"),
    ];

    let located = locate(&fake, &items);

    assert!(located.unmatched.is_empty(), "{:?}", located.unmatched);
    for id in ["a", "b", "c", "d"] {
        assert_eq!(located.matched[id].confidence, Confidence::Exact, "{id}");
    }
}

#[test]
fn blank_names_are_filed_under_unknown() {
    let fake = folder(&[
        "Unknown Artist/Unknown Album/Untitled.mp3",
        "Pink Floyd/Unknown Album/Money.mp3",
    ]);
    let items = [song("a", "", "", ""), song("b", "Money", "Pink Floyd", "")];

    let located = locate(&fake, &items);

    assert_eq!(
        found(&located, "a"),
        exact("Unknown Artist/Unknown Album/Untitled.mp3")
    );
    assert_eq!(
        found(&located, "b"),
        exact("Pink Floyd/Unknown Album/Money.mp3")
    );
}

#[test]
fn an_album_artist_decides_the_artist_folder() {
    let fake = folder(&[
        "Daft Punk/Discovery/01 One More Time.mp3",
        "Compilations/Now 99/03 Song.mp3",
    ]);
    let items = [
        song(
            "a",
            "One More Time",
            "Daft Punk feat. Romanthony",
            "Discovery",
        )
        .by("Daft Punk")
        .track(1),
        // No album artist: the track artist's folder, and failing that Compilations.
        song("b", "Song", "Some Band", "Now 99").track(3),
    ];

    let located = locate(&fake, &items);

    assert_eq!(
        found(&located, "a"),
        exact("Daft Punk/Discovery/01 One More Time.mp3")
    );
    assert_eq!(
        found(&located, "b"),
        exact("Compilations/Now 99/03 Song.mp3")
    );
}

#[test]
fn compilations_are_found_in_the_compilations_folder() {
    let fake = folder(&["Compilations/Now 99/03 Song.mp3"]);
    let items = [song("a", "Song", "Various", "Now 99")
        .by("Various Artists")
        .track(3)];

    assert_eq!(
        found(&locate(&fake, &items), "a"),
        exact("Compilations/Now 99/03 Song.mp3")
    );
}

#[test]
fn multi_disc_albums_put_the_disc_in_front() {
    let fake = folder(&[
        "Pink Floyd/The Wall/1-01 In The Flesh_.mp3",
        "Pink Floyd/The Wall/2-03 Hey You.mp3",
        "Pink Floyd/The Wall/03 Hey You.mp3",
        "Pink Floyd/Meddle/01 One Of These Days.mp3",
    ]);
    let items = [
        song("a", "In The Flesh?", "Pink Floyd", "The Wall")
            .track(1)
            .disc(1),
        song("b", "Hey You", "Pink Floyd", "The Wall")
            .track(3)
            .disc(2),
        // A first disc of a single-disc album carries no prefix.
        song("c", "One Of These Days", "Pink Floyd", "Meddle")
            .track(1)
            .disc(1),
    ];

    let located = locate(&fake, &items);

    assert_eq!(
        found(&located, "a"),
        exact("Pink Floyd/The Wall/1-01 In The Flesh_.mp3")
    );
    assert_eq!(
        found(&located, "b"),
        exact("Pink Floyd/The Wall/2-03 Hey You.mp3")
    );
    assert_eq!(
        found(&located, "c"),
        exact("Pink Floyd/Meddle/01 One Of These Days.mp3")
    );
}

#[test]
fn files_in_disc_folders_belong_to_the_album() {
    let fake = folder(&["Artist/Box Set/Disc 2/01 Song.mp3"]);

    let located = locate(&fake, &[song("a", "Song", "Artist", "Box Set").track(1)]);

    assert_eq!(
        found(&located, "a"),
        exact("Artist/Box Set/Disc 2/01 Song.mp3")
    );
}

#[test]
fn duplicate_songs_get_the_plain_name_in_the_order_they_were_added() {
    let fake = folder(&["A/B/01 Song.mp3", "A/B/01 Song 1.mp3", "A/B/01 Song 2.mp3"]);
    let items = [
        song("third", "Song", "A", "B").track(1).added(300),
        song("first", "Song", "A", "B").track(1).added(100),
        song("second", "Song", "A", "B").track(1).added(200),
    ];

    let located = locate(&fake, &items);

    assert_eq!(found(&located, "first"), exact("A/B/01 Song.mp3"));
    assert_eq!(found(&located, "second"), exact("A/B/01 Song 1.mp3"));
    assert_eq!(found(&located, "third"), exact("A/B/01 Song 2.mp3"));
}

#[test]
fn a_folder_where_every_file_carries_a_number_is_still_found() {
    let fake = folder(&[
        "Andrew Bird/Oh! The Grandeur/04 Wait 1.mp3",
        "Andrew Bird/Oh! The Grandeur/07 Beware 1.mp3",
    ]);
    let items = [
        song("a", "Wait", "Andrew Bird", "Oh! The Grandeur").track(4),
        song("b", "Beware", "Andrew Bird", "Oh! The Grandeur").track(7),
    ];

    let located = locate(&fake, &items);

    assert_eq!(
        found(&located, "a"),
        exact("Andrew Bird/Oh! The Grandeur/04 Wait 1.mp3")
    );
    assert_eq!(
        found(&located, "b"),
        exact("Andrew Bird/Oh! The Grandeur/07 Beware 1.mp3")
    );
}

#[test]
fn a_plain_name_is_never_taken_by_a_song_that_only_looks_like_a_duplicate() {
    // "Wait 1" is a different song's own title, and the plain "Wait" exists too.
    let fake = folder(&["A/B/04 Wait.mp3", "A/B/05 Wait 1.mp3"]);
    let items = [
        song("plain", "Wait", "A", "B").track(4),
        song("one", "Wait 1", "A", "B").track(5),
    ];

    let located = locate(&fake, &items);

    assert_eq!(found(&located, "plain"), exact("A/B/04 Wait.mp3"));
    assert_eq!(found(&located, "one"), exact("A/B/05 Wait 1.mp3"));
}

#[test]
fn ignores_case_and_unicode_form_in_folder_names() {
    // The artist folder was written decomposed, the album folder in other case.
    let fake = folder(&[
        "Bjo\u{0308}rk/homogenic/01 Hunter.mp3",
        "the black keys/Brothers/01 Everlasting Light.mp3",
    ]);
    let items = [
        song("a", "Hunter", "Bj\u{00F6}rk", "Homogenic").track(1),
        song("b", "Everlasting Light", "The Black Keys", "Brothers").track(1),
    ];

    let located = locate(&fake, &items);

    assert_eq!(
        found(&located, "a"),
        exact("Bjo\u{0308}rk/homogenic/01 Hunter.mp3")
    );
    assert_eq!(
        found(&located, "b"),
        exact("the black keys/Brothers/01 Everlasting Light.mp3")
    );
}

#[test]
fn ignores_unicode_form_in_file_names() {
    let fake = folder(&["Bjork/Post/01 Hyperballad Be\u{0301}ta.mp3"]);

    let located = locate(
        &fake,
        &[song("a", "Hyperballad B\u{00E9}ta", "Bjork", "Post").track(1)],
    );

    assert_eq!(located.matched["a"].confidence, Confidence::Exact);
}

#[test]
fn long_names_are_not_cut_short_by_music() {
    let title = "A Very Long Title ".repeat(10);
    let stem = format!("01 {}", title.trim_end());
    let long_album = "L".repeat(120);
    let fake = folder(&[&format!("Artist/{long_album}/{stem}.mp3")]);

    let located = locate(&fake, &[song("a", &title, "Artist", &long_album).track(1)]);

    assert_eq!(located.matched["a"].confidence, Confidence::Exact);
}

#[test]
fn a_name_cut_short_by_an_older_version_matches_by_its_start() {
    let fake = folder(&[
        "Little Brother/And Justus For All/03 Life Of The Party (Remix) Ft Skil.mp3",
        "Little Brother/And Justus For All/04 Short.mp3",
    ]);
    let items = [song(
        "a",
        "Life Of The Party (Remix) Ft Skillz and Carlitta Durand",
        "Little Brother",
        "And Justus For All",
    )
    .track(3)];

    let located = locate(&fake, &items);

    assert_eq!(
        found(&located, "a"),
        folder_match("Little Brother/And Justus For All/03 Life Of The Party (Remix) Ft Skil.mp3")
    );
}

#[test]
fn a_shared_start_is_not_enough_when_it_is_short_and_the_numbers_differ() {
    let fake = folder(&["A/B/09 Intro Part Two.mp3"]);

    let located = locate(&fake, &[song("a", "Intro", "A", "B").track(1)]);

    assert_eq!(located.unmatched, ["a"]);
}

#[test]
fn files_named_by_other_tools_are_matched_in_the_right_album() {
    let fake = folder(&[
        "Ella Fitzgerald/Song Book/05 - a foggy day.mp3",
        "Louis Jordan/Jivin/07. My Baby Said Yes.mp3",
        "Antonin Dvorak/Masterworks/13- Serenade in D minor.mp3",
        "Frankie Valli/Collection/08_Frankie Valli & The Four Seasons - Marlena.mp3",
        "Common Sense/Borrow/02Charms Alarm.mp3",
        "Gramatik/No Shortcuts/damage_intended.mp3",
    ]);
    let items = [
        song("a", "A Foggy Day", "Ella Fitzgerald", "Song Book").track(5),
        song("b", "My Baby Said Yes", "Louis Jordan", "Jivin").track(7),
        song("c", "Serenade in D minor", "Antonin Dvorak", "Masterworks").track(13),
        song("d", "Marlena", "Frankie Valli", "Collection").track(8),
        song("e", "Charms Alarm", "Common Sense", "Borrow").track(2),
        song("f", "Damage Intended", "Gramatik", "No Shortcuts").track(13),
    ];

    let located = locate(&fake, &items);

    assert!(located.unmatched.is_empty(), "{:?}", located.unmatched);
    for id in ["a", "b", "c", "d", "e", "f"] {
        assert_eq!(located.matched[id].confidence, Confidence::Folder, "{id}");
    }
}

#[test]
fn an_album_folder_spelled_a_little_differently_still_leads_to_the_file() {
    let fake = folder(&["Artist/Greatest Hits (Remastered)/01 Song.mp3"]);

    let located = locate(
        &fake,
        &[song("a", "Song", "Artist", "Greatest Hits [Remastered]").track(1)],
    );

    assert_eq!(
        found(&located, "a"),
        folder_match("Artist/Greatest Hits (Remastered)/01 Song.mp3")
    );
}

#[test]
fn a_retitled_song_is_found_by_its_number_and_size() {
    // 3:00 at 192 kbps is 4,320,000 bytes.
    let mut fake = Fake::default();
    for (path, size) in [
        ("A/B/01 First.mp3", 4_320_000),
        ("A/B/02 Old Title.mp3", 4_320_000),
        ("A/B/03 Third.mp3", 9_000_000),
    ] {
        fake.file(Path::new(ROOT), path, size, true);
    }
    let items = [song("a", "New Title", "A", "B").track(2).lasting(180)];

    let located = locate(&fake, &items);

    assert_eq!(found(&located, "a"), folder_match("A/B/02 Old Title.mp3"));
}

#[test]
fn a_number_alone_is_not_enough_when_the_size_is_wrong() {
    let mut fake = Fake::default();
    // A 30-second clip at 192 kbps is about 720 kB; this file is 4 MB.
    fake.file(
        Path::new(ROOT),
        "A/B/02 Something Else.mp3",
        4_000_000,
        true,
    );
    fake.file(Path::new(ROOT), "A/B/05 Other.mp3", 700_000, true);

    let located = locate(
        &fake,
        &[song("a", "New Title", "A", "B").track(2).lasting(30)],
    );

    assert_eq!(located.unmatched, ["a"]);
}

#[test]
fn the_folders_own_files_set_the_rate_for_matching_by_size() {
    let mut fake = Fake::default();
    // Everything here is 256 kbps: 32,000 bytes a second.
    for (path, seconds) in [
        ("A/B/01 One.mp3", 100),
        ("A/B/02 Two.mp3", 200),
        ("A/B/03 Three.mp3", 150),
        ("A/B/mystery.mp3", 240),
        ("A/B/other mystery.mp3", 60),
    ] {
        fake.file(Path::new(ROOT), path, seconds * 32_000, true);
    }
    let items = [
        song("one", "One", "A", "B").track(1).lasting(100),
        song("two", "Two", "A", "B").track(2).lasting(200),
        song("three", "Three", "A", "B").track(3).lasting(150),
        song("long", "Unrelated Name", "A", "B")
            .track(9)
            .lasting(240),
        song("short", "Another Name", "A", "B").track(8).lasting(60),
    ];

    let located = locate(&fake, &items);

    assert_eq!(found(&located, "long"), folder_match("A/B/mystery.mp3"));
    assert_eq!(
        found(&located, "short"),
        folder_match("A/B/other mystery.mp3")
    );
}

#[test]
fn two_songs_the_same_length_cannot_be_told_apart_by_size() {
    let mut fake = Fake::default();
    for path in ["A/B/01 One.mp3", "A/B/zzz.mp3", "A/B/yyy.mp3"] {
        fake.file(Path::new(ROOT), path, 3_200_000, true);
    }
    let items = [
        song("one", "One", "A", "B").track(1).lasting(100),
        song("p", "P", "A", "B").track(5).lasting(100),
        song("q", "Q", "A", "B").track(6).lasting(100),
    ];

    let located = locate(&fake, &items);

    assert!(
        located.unmatched.contains(&"p".to_owned()) && located.unmatched.contains(&"q".to_owned())
    );
}

#[test]
fn the_last_song_and_the_last_file_of_a_folder_belong_together_if_the_size_fits() {
    let mut fake = Fake::default();
    fake.file(Path::new(ROOT), "A/B/odd name.mp3", 4_320_000, true);
    let fits = [song("a", "Nothing In Common", "A", "B").lasting(180)];
    assert_eq!(
        found(&locate(&fake, &fits), "a"),
        folder_match("A/B/odd name.mp3")
    );

    let too_long = [song("a", "Nothing In Common", "A", "B").lasting(600)];
    assert_eq!(locate(&fake, &too_long).unmatched, ["a"]);
}

#[test]
fn a_file_two_songs_want_goes_to_the_one_whose_number_it_carries() {
    let fake = folder(&["A/B/1. Intro!.mp3"]);
    let items = [
        song("ninth", "Intro", "A", "B").track(9),
        song("first", "Intro", "A", "B").track(1),
    ];

    let located = locate(&fake, &items);

    assert_eq!(found(&located, "first"), folder_match("A/B/1. Intro!.mp3"));
    assert_eq!(located.unmatched, ["ninth"]);
}

#[test]
fn a_file_two_songs_want_equally_goes_to_neither() {
    let fake = folder(&["A/B/Intro!.mp3"]);
    let items = [
        song("first", "Intro", "A", "B").track(1),
        song("ninth", "Intro", "A", "B").track(9),
    ];

    let located = locate(&fake, &items);

    assert!(located.matched.is_empty(), "{:?}", located.matched);
    assert_eq!(located.unmatched, ["first", "ninth"]);
}

#[test]
fn the_format_that_suits_the_song_is_chosen_when_both_exist() {
    let fake = folder(&["A/B/01 Song.m4a", "A/B/01 Song.mp3"]);
    let items = [
        song("aac", "Song", "A", "B")
            .track(1)
            .kind("AAC audio file"),
        song("mp3", "Song", "A", "B")
            .track(1)
            .kind("MPEG audio file"),
    ];

    let located = locate(&fake, &items);

    assert_eq!(found(&located, "aac"), exact("A/B/01 Song.m4a"));
    assert_eq!(found(&located, "mp3"), exact("A/B/01 Song.mp3"));
}

#[test]
fn a_song_whose_album_is_missing_is_found_by_a_title_only_it_has() {
    let fake = folder(&[
        "Wherever/Elsewhere/03 Unmistakable Title.mp3",
        "Other/Other/01 Common.mp3",
        "Another/Another/01 Common.mp3",
    ]);
    let items = [
        song("a", "Unmistakable Title", "Somebody", "No Such Album").track(7),
        song("b", "Common", "Somebody", "No Such Album"),
    ];

    let located = locate(&fake, &items);

    assert_eq!(
        found(&located, "a"),
        Some((
            "Wherever/Elsewhere/03 Unmistakable Title.mp3".to_owned(),
            Confidence::Fuzzy
        ))
    );
    assert_eq!(located.unmatched, ["b"], "two files share that title");
}

#[test]
fn two_songs_after_one_title_are_left_unmatched() {
    let fake = folder(&["X/Y/Only One.mp3"]);
    let items = [
        song("a", "Only One", "Q", "R"),
        song("b", "Only One", "S", "T"),
    ];

    assert_eq!(locate(&fake, &items).unmatched, ["a", "b"]);
}

#[test]
fn songs_that_cannot_have_a_file_are_in_neither_list() {
    let fake = folder(&["A/B/01 Song.mp3"]);
    let mut present = song("present", "Song", "A", "B").track(1);
    present.file_present = true;
    let mut streamed = song("streamed", "Other", "A", "B");
    streamed.ownership = Ownership::Subscription;

    let located = locate(&fake, &[present, streamed]);

    assert!(located.matched.is_empty() && located.unmatched.is_empty());
}

#[test]
fn a_song_whose_recorded_file_is_missing_is_still_looked_for() {
    let fake = folder(&["A/B/01 Song.mp3"]);
    let mut moved = song("a", "Song", "A", "B").track(1);
    moved.location = Some("/old/place/01 Song.mp3".into());

    assert_eq!(
        found(&locate(&fake, &[moved]), "a"),
        exact("A/B/01 Song.mp3")
    );
}

#[test]
fn a_file_goes_to_one_song_only() {
    let fake = folder(&["A/B/01 Song.mp3"]);
    let items = [
        song("one", "Song", "A", "B").track(1).added(1),
        song("two", "Song", "A", "B").track(1).added(2),
    ];

    let located = locate(&fake, &items);

    assert_eq!(located.matched.len(), 1);
    assert_eq!(
        found(&located, "one"),
        exact("A/B/01 Song.mp3"),
        "the earlier song keeps it"
    );
    assert_eq!(located.unmatched, ["two"]);
}

#[test]
fn only_audio_files_count() {
    let fake = folder(&[
        "A/B/01 Song.jpg",
        "A/B/.DS_Store",
        "A/B/Folder.asd",
        "A/B/Booklet.pdf",
        "A/B/02 Real.FLAC",
    ]);
    let items = [
        song("a", "Song", "A", "B").track(1),
        song("b", "Real", "A", "B").track(2),
    ];

    let located = locate(&fake, &items);

    assert_eq!(located.unmatched, ["a"]);
    assert_eq!(found(&located, "b"), exact("A/B/02 Real.FLAC"));
}

#[test]
fn the_music_folder_inside_an_itunes_media_folder_is_used() {
    let mut fake = Fake::default();
    fake.file(Path::new(ROOT), "Music/A/B/01 Song.mp3", 1_000, false);
    fake.file(Path::new(ROOT), "Podcasts/Show/Episode.mp3", 1_000, false);
    fake.file(
        Path::new(ROOT),
        "Automatically Add to Music/.keep",
        0,
        false,
    );

    let located = locate(&fake, &[song("a", "Song", "A", "B").track(1)]);

    assert_eq!(found(&located, "a"), exact("Music/A/B/01 Song.mp3"));
}

#[test]
fn an_artist_called_music_is_not_mistaken_for_the_media_folder() {
    let fake = folder(&["Music/Album/01 Song.mp3", "Other/Album/01 Song.mp3"]);

    let located = locate(&fake, &[song("a", "Song", "Music", "Album").track(1)]);

    assert_eq!(found(&located, "a"), exact("Music/Album/01 Song.mp3"));
}

#[test]
fn an_unreadable_folder_is_an_error_but_an_unreadable_subfolder_is_skipped() {
    let missing = Fake::default();
    assert!(locate_with(&missing, &[], Path::new("/nowhere"), &mut |_| {}).is_err());
}

#[test]
fn only_directory_listings_are_ever_asked_for() {
    let fake = folder(&["A/B/01 Song.mp3", "A/B/02 Song.mp3", "C/D/01 Song.mp3"]);
    let items = [
        song("a", "Song", "A", "B").track(1),
        song("b", "Song", "C", "D").track(1),
    ];

    locate(&fake, &items);

    // The tree offers nothing else to ask for; each folder is listed once, the root twice (once
    // to see whether it is a media folder).
    let mut listed: Vec<String> = fake
        .listed
        .borrow()
        .iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    listed.sort();
    assert_eq!(
        listed,
        [
            "/music",
            "/music",
            "/music/A",
            "/music/A/B",
            "/music/C",
            "/music/C/D"
        ]
    );
}

#[test]
fn reports_progress_through_both_stages() {
    let fake = folder(&["A/B/01 Song.mp3"]);
    let mut seen = Vec::new();

    locate_with(
        &fake,
        &[song("a", "Song", "A", "B").track(1)],
        Path::new(ROOT),
        &mut |progress| seen.push(progress),
    )
    .unwrap();

    assert_eq!(
        seen.first().map(|p| (p.stage, p.done, p.total)),
        Some((LocateStage::Scanning, 1, None))
    );
    let last = seen.last().unwrap();
    assert_eq!(
        (last.stage, last.done, last.total),
        (LocateStage::Matching, 1, Some(1))
    );
}

#[test]
fn a_whole_library_is_matched_in_seconds() {
    // 37,000 songs in 3,700 albums of 10, and as many files.
    let mut fake = Fake::default();
    let mut items = Vec::new();
    for album in 0..3_700 {
        for track in 1..=10 {
            let (artist, name, title) = (
                format!("Artist {}", album / 5),
                format!("Album {album}"),
                format!("Song {album}-{track}"),
            );
            fake.file(
                Path::new(ROOT),
                &format!("{artist}/{name}/{track:02} {title}.mp3"),
                1_000,
                true,
            );
            items.push(song(&format!("{album}-{track}"), &title, &artist, &name).track(track));
        }
    }
    let started = std::time::Instant::now();

    let located = locate(&fake, &items);

    assert_eq!(located.matched.len(), 37_000);
    assert!(
        located
            .matched
            .values()
            .all(|file| file.confidence == Confidence::Exact)
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}
