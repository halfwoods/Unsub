use antistreaming_library::{Error, Library, MediaKind, SortKey, Track, TrackQuery};

use crate::common::{all_kinds, scanned_library, titles, track_titled};

/// Shaped like a device database: foreign ids (order-preserving, so tie-breaks
/// match), paths that do not exist on this machine, and stored play data.
fn device_tracks(source: &Library) -> Vec<Track> {
    source
        .tracks(&all_kinds())
        .unwrap()
        .into_iter()
        .map(|mut track| {
            track.id = track.id * 3 + 1_000;
            track.path = format!(":iPod_Control:Music:F00:{}.m4a", track.id);
            track
        })
        .collect()
}

fn imported(source: &Library) -> Library {
    let mut device = Library::open_in_memory().unwrap();
    device.import_tracks(&device_tracks(source)).unwrap();
    device
}

#[test]
fn imported_rows_keep_their_ids_and_every_field() {
    let (source, _dir) = scanned_library();
    let mut tracks = device_tracks(&source);
    tracks[0].play_count = 7;
    tracks[0].skip_count = 2;
    tracks[0].rating = 80;
    tracks[0].last_played = Some(1_700_000_000);
    tracks[0].date_added = 42;
    tracks[0].sort_artist = Some("Zzz Sort Artist".into());
    tracks[1].kind = MediaKind::Podcast;
    tracks[1].compilation = true;
    let mut device = Library::open_in_memory().unwrap();

    device.import_tracks(&tracks).unwrap();

    assert_eq!(device.tracks(&all_kinds()).unwrap().len(), tracks.len());
    for track in &tracks {
        assert_eq!(device.track(track.id).unwrap().as_ref(), Some(track));
    }
}

#[test]
fn imported_rows_sort_and_search_like_scanned_rows() {
    let (mut source, _dir) = scanned_library();
    let halo = track_titled(&source, "Halo").id;
    let zombie = track_titled(&source, "Zombie").id;
    source.set_rating(halo, 60).unwrap();
    source.record_play(zombie, 5).unwrap();
    source.record_play(halo, 9).unwrap();
    source.record_play(halo, 10).unwrap();
    let device = imported(&source);

    let keys = [
        SortKey::Name,
        SortKey::Time,
        SortKey::Artist,
        SortKey::Album,
        SortKey::AlbumArtist,
        SortKey::Genre,
        SortKey::Year,
        SortKey::Rating,
        SortKey::Plays,
        SortKey::LastPlayed,
        SortKey::DateAdded,
        SortKey::Kind,
        SortKey::Bitrate,
        SortKey::SampleRate,
        SortKey::Size,
        SortKey::PlaylistOrder,
    ];
    let searches = [
        None,
        Some("beyonce"),
        Some("abbey beatles"),
        Some("sasha pop"),
        Some("nope"),
    ];
    for key in keys {
        for descending in [false, true] {
            for search in searches {
                for kind in [None, Some(MediaKind::Music), Some(MediaKind::Podcast)] {
                    let query = TrackQuery {
                        kind,
                        search: search.map(Into::into),
                        sort: key,
                        descending,
                        ids: None,
                        playlist: None,
                    };
                    assert_eq!(
                        titles(&device, &query),
                        titles(&source, &query),
                        "{key:?} descending={descending} search={search:?} kind={kind:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn imported_sort_fields_are_honoured_and_blank_ones_ignored() {
    let (source, _dir) = scanned_library();
    let mut tracks = device_tracks(&source);
    for track in &mut tracks {
        match track.title.as_str() {
            "Halo" => track.sort_artist = Some("Zzz".into()),
            "Zombie" => track.sort_title = Some(String::new()),
            _ => {}
        }
    }
    let mut device = Library::open_in_memory().unwrap();
    device.import_tracks(&tracks).unwrap();

    let by_artist = titles(&device, &TrackQuery::default());
    assert_eq!(by_artist[by_artist.len() - 2], "Halo");
    let by_name = titles(
        &device,
        &TrackQuery {
            sort: SortKey::Name,
            ..TrackQuery::default()
        },
    );
    assert_eq!(by_name.last().map(String::as_str), Some("Zombie"));
}

#[test]
fn reimporting_an_id_replaces_the_row() {
    let (source, _dir) = scanned_library();
    let mut tracks = device_tracks(&source);
    let mut device = Library::open_in_memory().unwrap();
    device.import_tracks(&tracks).unwrap();

    tracks[0].title = "Renamed".into();
    tracks[0].rating = 100;
    device.import_tracks(&tracks[..1]).unwrap();

    assert_eq!(device.tracks(&all_kinds()).unwrap().len(), tracks.len());
    assert_eq!(
        device.track(tracks[0].id).unwrap().as_ref(),
        Some(&tracks[0])
    );
    assert_eq!(
        titles(
            &device,
            &TrackQuery {
                search: Some("renamed".into()),
                ..all_kinds()
            }
        ),
        ["Renamed"]
    );
}

#[test]
fn import_is_atomic_and_rejects_a_path_held_by_another_id() {
    let (source, _dir) = scanned_library();
    let mut tracks = device_tracks(&source);
    tracks[1].path = tracks[0].path.clone();
    let mut device = Library::open_in_memory().unwrap();

    let result = device.import_tracks(&tracks);

    assert!(matches!(result, Err(Error::Database(_))), "{result:?}");
    assert_eq!(device.stats(&all_kinds()).unwrap().count, 0);
}

#[test]
fn importing_nothing_is_a_no_op() {
    let mut device = Library::open_in_memory().unwrap();

    device.import_tracks(&[]).unwrap();

    assert_eq!(device.stats(&all_kinds()).unwrap().count, 0);
}
