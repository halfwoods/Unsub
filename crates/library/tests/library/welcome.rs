use unsub_library::{Library, MediaKind, NewStation, Track};

use crate::common::{library_of, track};

fn of_kind(kind: MediaKind) -> Track {
    Track {
        kind,
        ..track(1, "Song", "Artist", "Album", 200_000)
    }
}

#[test]
fn a_new_library_is_welcomed_until_it_has_been_answered() {
    let mut library = Library::open_in_memory().unwrap();
    assert!(library.welcome_due().unwrap());
    assert!(library.welcome_due().unwrap(), "asking doesn't answer it");

    library.set_welcomed().unwrap();

    assert!(!library.welcome_due().unwrap());
}

#[test]
fn music_podcasts_and_audiobooks_all_count_as_songs() {
    for kind in [MediaKind::Music, MediaKind::Podcast, MediaKind::Audiobook] {
        let mut library = library_of(&[of_kind(kind)]);
        assert!(!library.welcome_due().unwrap(), "{kind:?}");
    }
}

#[test]
fn stations_are_not_songs() {
    let mut library = Library::open_in_memory().unwrap();
    library
        .add_station(&NewStation {
            name: "Groove Salad".into(),
            url: "https://example.com/stream".into(),
            ..NewStation::default()
        })
        .unwrap();

    assert!(library.welcome_due().unwrap());
}

#[test]
fn a_library_with_songs_stays_unwelcomed_even_once_emptied() {
    let mut library = library_of(&[of_kind(MediaKind::Music)]);
    assert!(!library.welcome_due().unwrap());

    library.delete_tracks(&[1], false).unwrap();

    assert!(!library.welcome_due().unwrap());
}
