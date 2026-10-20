//! Fixtures shared by the app's tests.

use antistreaming_library::{MediaKind, Track};

/// A complete library track with plain defaults, for tests to adjust with struct update syntax.
pub fn track(id: i64, codec: &str, kind: MediaKind) -> Track {
    Track {
        id,
        path: format!("/music/{id}.{}", codec.to_lowercase()),
        title: format!("Song {id}"),
        artist: "Artist".into(),
        album_artist: "Artist".into(),
        album: "Album".into(),
        genre: String::new(),
        composer: String::new(),
        year: None,
        track_number: Some(1),
        track_count: None,
        disc_number: None,
        disc_count: None,
        duration_ms: 180_000,
        bitrate_kbps: Some(320),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
        channels: Some(2),
        codec: codec.into(),
        file_size: 1_000_000,
        date_added: 0,
        date_modified: 0,
        play_count: 0,
        last_played: None,
        skip_count: 0,
        rating: 0,
        kind,
        enabled: true,
        in_cloud: false,
        missing: false,
        loudness: None,
        remember_position: false,
        has_lyrics: false,
        compilation: false,
        sort_title: None,
        sort_artist: None,
        sort_album: None,
        sort_album_artist: None,
    }
}
