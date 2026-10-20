use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, layout::Template};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Stable across plugging: the marker's id, or a hash of the volume.
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub root: DeviceRoot,
    pub capacity_bytes: u64,
    pub free_bytes: u64,
    pub profile: Profile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceKind {
    /// A player running Rockbox (found by its `.rockbox` folder), including Rockbox-flashed iPods.
    Rockbox,
    /// A USB mass-storage player or stick the user marked as a sync target.
    MassStorage,
    /// An Android phone or other MTP device.
    Mtp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceRoot {
    /// A mounted volume.
    Path(PathBuf),
    /// A storage of an MTP device, reached through an [`MtpBackend`](crate::MtpBackend).
    Mtp { device: String, storage: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistFormat {
    /// UTF-8 `.m3u8` with `#EXTM3U` and `#EXTINF`.
    M3u8,
    /// `.pls`.
    Pls,
}

impl PlaylistFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::M3u8 => "m3u8",
            Self::Pls => "pls",
        }
    }
}

/// What a device can play and where it wants things. Stored in the device's marker file, so users
/// can edit it by hand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    /// Folder for music, relative to the device root, e.g. `Music`.
    pub music_dir: String,
    pub playlist_dir: String,
    pub playlist_format: PlaylistFormat,
    /// Playable extensions, lower case without the dot. The first is what a file the device cannot
    /// play is converted to; empty means every format plays and nothing is converted.
    pub formats: Vec<String>,
    /// Limits the device cannot play beyond. They are not enforced here (the planner does not know
    /// a track's sample rate); `prepare` receives the profile and converts what exceeds them.
    pub max_sample_rate: Option<u32>,
    pub max_bit_depth: Option<u8>,
    /// Where a file goes below `music_dir`, as a path template. The file extension is added.
    /// Placeholders: `{album_artist}`, `{album}`, `{title}`, `{disc}`, `{track}` (two digits).
    pub layout: String,
}

pub const DEFAULT_LAYOUT: &str = "{album_artist}/{album}/{disc}-{track} {title}";

impl Default for Profile {
    /// A generic USB player.
    fn default() -> Self {
        Self::mass_storage()
    }
}

fn extensions(list: &[&str]) -> Vec<String> {
    list.iter().map(|&ext| ext.to_owned()).collect()
}

impl Profile {
    /// Rockbox plays nearly everything, and reads relative playlists from `/Playlists`.
    pub fn rockbox() -> Self {
        Self {
            formats: extensions(&[
                "flac", "mp3", "m4a", "ogg", "opus", "wav", "aiff", "wv", "ape", "mpc", "wma",
                "aac",
            ]),
            ..Self::mass_storage()
        }
    }

    pub fn mass_storage() -> Self {
        Self {
            music_dir: "Music".to_owned(),
            playlist_dir: "Playlists".to_owned(),
            playlist_format: PlaylistFormat::M3u8,
            formats: extensions(&["flac", "mp3", "m4a", "aac", "wav", "ogg"]),
            max_sample_rate: None,
            max_bit_depth: None,
            layout: DEFAULT_LAYOUT.to_owned(),
        }
    }

    pub fn android() -> Self {
        Self {
            formats: extensions(&["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav"]),
            ..Self::mass_storage()
        }
    }

    /// Checks everything a hand-edited marker could get wrong, so a bad profile fails before any
    /// file is written.
    pub fn validate(&self) -> Result<()> {
        for (what, dir) in [
            ("musicDir", &self.music_dir),
            ("playlistDir", &self.playlist_dir),
        ] {
            if !is_relative_dir(dir) {
                return Err(Error::InvalidProfile(format!(
                    "{what} must be a folder below the device root, not {dir:?}"
                )));
            }
        }
        if self
            .formats
            .iter()
            .any(|ext| ext.is_empty() || ext.contains(['.', '/', '\\']))
        {
            return Err(Error::InvalidProfile(
                "formats are extensions like \"flac\", without a dot".to_owned(),
            ));
        }
        Template::parse(&self.layout).map(drop)
    }

    /// The device folder components of `music_dir` and `playlist_dir`, normalised.
    pub(crate) fn music_dir(&self) -> String {
        normalise_dir(&self.music_dir)
    }

    pub(crate) fn playlist_dir(&self) -> String {
        normalise_dir(&self.playlist_dir)
    }
}

fn normalise_dir(dir: &str) -> String {
    dir.trim_matches('/').to_owned()
}

fn is_relative_dir(dir: &str) -> bool {
    let dir = normalise_dir(dir);
    !dir.is_empty()
        && dir.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':'])
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_profiles_are_valid() {
        for profile in [
            Profile::rockbox(),
            Profile::mass_storage(),
            Profile::android(),
            Profile::default(),
        ] {
            profile.validate().unwrap();
        }
    }

    #[test]
    fn directories_must_stay_below_the_root() {
        for bad in [
            "",
            "/",
            "..",
            "../Music",
            "Music/../..",
            "C:\\Music",
            "a//b",
            "./Music",
        ] {
            let profile = Profile {
                music_dir: bad.to_owned(),
                ..Profile::default()
            };
            assert!(
                matches!(profile.validate(), Err(Error::InvalidProfile(_))),
                "{bad:?}"
            );
        }
        for fine in ["Music", "/Music/", "Audio/Music"] {
            Profile {
                music_dir: fine.to_owned(),
                ..Profile::default()
            }
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn bad_layouts_and_formats_are_rejected() {
        let profile = Profile {
            layout: "{artist}/{title}".to_owned(),
            ..Profile::default()
        };
        assert!(profile.validate().is_err());
        let profile = Profile {
            formats: vec![".flac".to_owned()],
            ..Profile::default()
        };
        assert!(profile.validate().is_err());
    }

    #[test]
    fn a_sparse_profile_fills_in_defaults_and_serialises_in_camel_case() {
        let profile: Profile =
            serde_json::from_str(r#"{"musicDir":"Tunes","formats":["mp3"]}"#).unwrap();
        assert_eq!(profile.music_dir, "Tunes");
        assert_eq!(profile.playlist_dir, "Playlists");
        assert_eq!(profile.layout, DEFAULT_LAYOUT);
        let json = serde_json::to_value(Profile::rockbox()).unwrap();
        assert_eq!(json["playlistFormat"], "m3u8");
        assert!(json.get("maxSampleRate").is_some());
        let kind = serde_json::to_value(DeviceKind::MassStorage).unwrap();
        assert_eq!(kind, "massStorage");
    }
}
