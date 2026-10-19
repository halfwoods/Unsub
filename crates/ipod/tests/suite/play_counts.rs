use antistreaming_ipod::read_database;

use crate::common::db::{PlayCount, play_counts_file};
use crate::{classic_7g, mac_wall_clock, opened, sample_database, unix_local};

fn untouched(rating: u32) -> PlayCount {
    PlayCount {
        plays: 0,
        last_played_mac: 0,
        rating,
        skips: 0,
    }
}

#[test]
fn adds_plays_and_skips_and_replaces_last_played_and_rating() {
    let fake = classic_7g(&sample_database());
    fake.write_play_counts(&play_counts_file(&[
        PlayCount {
            plays: 5,
            last_played_mac: mac_wall_clock(2011, 1, 10, 12),
            rating: 100,
            skips: 2,
        },
        untouched(0),
    ]));

    let tracks = read_database(&opened(&fake)).unwrap().tracks;

    assert_eq!(tracks[0].play_count, 12 + 5);
    assert_eq!(tracks[0].skip_count, 3 + 2);
    assert_eq!(tracks[0].last_played, Some(unix_local(2011, 1, 10, 12)));
    assert_eq!(tracks[0].rating, 100);
}

#[test]
fn zero_values_leave_the_database_values_alone() {
    let fake = classic_7g(&sample_database());
    fake.write_play_counts(&play_counts_file(&[untouched(0)]));

    let tracks = read_database(&opened(&fake)).unwrap().tracks;

    assert_eq!(tracks[0].play_count, 12);
    assert_eq!(tracks[0].skip_count, 3);
    assert_eq!(tracks[0].last_played, Some(unix_local(2010, 6, 15, 12)));
    assert_eq!(tracks[0].rating, 80);
}

#[test]
fn entries_apply_in_track_order() {
    let fake = classic_7g(&sample_database());
    fake.write_play_counts(&play_counts_file(&[
        untouched(0),
        PlayCount {
            plays: 1,
            last_played_mac: 0,
            rating: 60,
            skips: 0,
        },
        untouched(0),
        untouched(0),
        PlayCount {
            plays: 4,
            last_played_mac: 0,
            rating: 0,
            skips: 0,
        },
    ]));

    let tracks = read_database(&opened(&fake)).unwrap().tracks;

    assert_eq!(
        tracks
            .iter()
            .map(|track| track.play_count)
            .collect::<Vec<_>>(),
        [12, 1, 0, 0, 4]
    );
    assert_eq!(tracks[1].rating, 60);
}

#[test]
fn extra_entries_beyond_the_track_count_are_ignored() {
    let fake = classic_7g(&sample_database());
    let entries: Vec<_> = (0..20)
        .map(|_| PlayCount {
            plays: 1,
            last_played_mac: 0,
            rating: 0,
            skips: 0,
        })
        .collect();
    fake.write_play_counts(&play_counts_file(&entries));

    let tracks = read_database(&opened(&fake)).unwrap().tracks;

    assert_eq!(tracks.len(), 5);
    assert_eq!(tracks[1].play_count, 1);
}

#[test]
fn an_empty_or_damaged_play_counts_file_is_ignored() {
    let fake = classic_7g(&sample_database());
    let valid = play_counts_file(&[PlayCount {
        plays: 5,
        last_played_mac: 0,
        rating: 0,
        skips: 0,
    }]);

    for contents in [
        Vec::new(),
        b"garbage".to_vec(),
        vec![0xff; 200],
        valid[..valid.len() - 3].to_vec(),
    ] {
        fake.write_play_counts(&contents);

        let tracks = read_database(&opened(&fake)).unwrap().tracks;

        assert_eq!(tracks[0].play_count, 12);
    }
}
