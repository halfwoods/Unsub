//! Finding players: mounted volumes with Rockbox on them or a marker the user put there, and the
//! MTP devices a backend reports.

use std::{
    fs,
    hash::Hasher,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    Device, DeviceKind, DeviceRoot, Error, FsStorage, Profile, Result, Storage, mtp,
    mtp::MtpBackend,
};

/// Written to the root of a volume to make it a sync target.
pub const MARKER_FILE: &str = ".unsub-device.json";
const MARKER_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marker {
    version: u32,
    /// Chosen when the volume was first marked, so settings kept under it survive a rename or
    /// a new mount point.
    id: String,
    name: String,
    profile: Profile,
}

/// Makes the volume at `path` a sync target by writing the marker file, so a plain USB stick or a
/// player with no recognisable firmware shows up in [`discover`]. Marking again changes the name or
/// profile and keeps the id.
pub fn mark_device(path: &Path, name: &str, profile: &Profile) -> Result<()> {
    profile.validate()?;
    if !path.is_dir() {
        return Err(Error::io(
            path.display().to_string(),
            std::io::Error::new(std::io::ErrorKind::NotFound, "not a folder"),
        ));
    }
    let id = read_marker(path).map_or_else(|| new_id(path), |marker| marker.id);
    let marker = Marker {
        version: MARKER_VERSION,
        id,
        name: name.to_owned(),
        profile: profile.clone(),
    };
    let bytes =
        serde_json::to_vec_pretty(&marker).map_err(|error| Error::Manifest(error.to_string()))?;
    FsStorage::new(path)
        .write(MARKER_FILE, &bytes)
        .map_err(|source| Error::io(path.join(MARKER_FILE).display().to_string(), source))
}

/// Every player that is plugged in: marked volumes, volumes with a `.rockbox` folder, and the MTP
/// devices the platform's backend can see. Apple-firmware iPods (an `iPod_Control` folder and
/// nothing else) are left to the iPod crate.
pub fn discover() -> Vec<Device> {
    discover_in(candidate_mounts(), mtp::system().as_ref())
}

pub(crate) fn discover_in(
    mounts: impl IntoIterator<Item = PathBuf>,
    mtp: &dyn MtpBackend,
) -> Vec<Device> {
    let mut devices: Vec<Device> = mounts
        .into_iter()
        .filter_map(|mount| inspect(&mount))
        .collect();
    devices.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    // No backend, or none that can see a phone right now, is simply no phones.
    for phone in mtp.devices().unwrap_or_default() {
        let single = phone.storages.len() == 1;
        for storage in phone.storages.clone() {
            devices.push(Device {
                id: format!("mtp:{}:{}", phone.id, storage.id),
                name: if single {
                    phone.name.clone()
                } else {
                    format!("{} ({})", phone.name, storage.description)
                },
                kind: DeviceKind::Mtp,
                root: DeviceRoot::Mtp {
                    device: phone.id.clone(),
                    storage: storage.id,
                },
                capacity_bytes: storage.capacity_bytes,
                free_bytes: storage.free_bytes,
                profile: Profile::android(),
            });
        }
    }
    devices
}

/// `None` for anything that is not a sync target, including volumes that cannot be read.
fn inspect(mount: &Path) -> Option<Device> {
    let marker = read_marker(mount);
    let rockbox = mount.join(".rockbox").is_dir();
    // A volume with neither is an ordinary disk, or an Apple iPod: iPod_Control alone is not enough
    // here, since those are synced by the iPod crate. Rockbox on an iPod keeps iPod_Control, so the
    // Rockbox folder is checked first and wins.
    if marker.is_none() && !rockbox {
        return None;
    }
    let stats = fs4::statvfs(mount).ok()?;
    let volume = volume_name(mount);
    let (id, name, profile) = match marker {
        Some(marker) => (marker.id, marker.name, marker.profile),
        None => (
            stable_id(&volume, stats.total_space()),
            volume,
            Profile::rockbox(),
        ),
    };
    Some(Device {
        id,
        name,
        kind: if rockbox {
            DeviceKind::Rockbox
        } else {
            DeviceKind::MassStorage
        },
        root: DeviceRoot::Path(mount.to_path_buf()),
        capacity_bytes: stats.total_space(),
        free_bytes: stats.available_space(),
        profile,
    })
}

/// A marker that cannot be read counts as no marker: the volume is then found only if it is a
/// Rockbox player, with Rockbox's profile.
fn read_marker(mount: &Path) -> Option<Marker> {
    let bytes = fs::read(mount.join(MARKER_FILE)).ok()?;
    let marker: Marker = serde_json::from_slice(&bytes).ok()?;
    (marker.version <= MARKER_VERSION && marker.profile.validate().is_ok()).then_some(marker)
}

/// `/Volumes/PLAYER` gives `PLAYER`; a Windows drive root has no file name, so the drive itself is
/// used.
fn volume_name(mount: &Path) -> String {
    mount.file_name().map_or_else(
        || mount.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// FNV-1a rather than `DefaultHasher`, whose output may change between Rust releases and would
/// orphan any per-device settings stored under the id. The mount point is left out so the same
/// player keeps its id when it mounts somewhere else.
fn stable_id(volume_name: &str, capacity: u64) -> String {
    let mut hasher = Fnv1a::new();
    hasher.write(volume_name.as_bytes());
    hasher.write(&[0]);
    hasher.write(&capacity.to_le_bytes());
    format!("{:016x}", hasher.finish())
}

/// An id for a newly marked volume that no other volume shares.
fn new_id(mount: &Path) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Fnv1a::new();
    hasher.write(&nanos.to_le_bytes());
    hasher.write(&u64::from(std::process::id()).to_le_bytes());
    hasher.write(&COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hasher.write(mount.as_os_str().as_encoded_bytes());
    format!("{:016x}", hasher.finish())
}

struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv1a {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// Scans macOS `/Volumes/*`, Windows drive letters `A:` to `Z:`, and on Linux `/media/$USER/*`,
/// `/run/media/$USER/*`, `/media/*` and `/mnt/*`.
#[cfg(windows)]
fn candidate_mounts() -> Vec<PathBuf> {
    (b'A'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", char::from(letter))))
        .collect()
}

#[cfg(not(windows))]
fn candidate_mounts() -> Vec<PathBuf> {
    children_of(&mount_parents())
}

#[cfg(target_os = "macos")]
fn mount_parents() -> Vec<PathBuf> {
    vec![PathBuf::from("/Volumes")]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn mount_parents() -> Vec<PathBuf> {
    let mut parents = Vec::new();
    if let Some(user) = std::env::var_os("USER") {
        parents.push(PathBuf::from("/media").join(&user));
        parents.push(PathBuf::from("/run/media").join(&user));
    }
    parents.push(PathBuf::from("/media"));
    parents.push(PathBuf::from("/mnt"));
    parents
}

/// The mounted volumes below `parents`, without symlinks (macOS lists the boot disk in `/Volumes`
/// as a link to `/`).
#[cfg(not(windows))]
fn children_of(parents: &[PathBuf]) -> Vec<PathBuf> {
    parents
        .iter()
        .filter_map(|parent| fs::read_dir(parent).ok())
        .flat_map(|entries| entries.filter_map(|entry| entry.ok()))
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::mtp::{MtpDevice, MtpStorageInfo};

    struct Phones(Vec<MtpDevice>);

    impl MtpBackend for Phones {
        fn devices(&self) -> Result<Vec<MtpDevice>> {
            Ok(self.0.clone())
        }

        fn open(&self, _: &str, _: u32) -> Result<Box<dyn Storage>> {
            Err(Error::Unsupported("not needed"))
        }
    }

    fn no_phones() -> Phones {
        Phones(Vec::new())
    }

    fn rockbox(root: &Path, name: &str) -> PathBuf {
        let mount = root.join(name);
        fs::create_dir_all(mount.join(".rockbox")).unwrap();
        mount
    }

    #[test]
    fn a_rockbox_player_is_found_with_rockbox_defaults() {
        let root = tempfile::tempdir().unwrap();
        let mount = rockbox(root.path(), "CLIPPLUS");
        let found = discover_in([mount.clone()], &no_phones());
        assert_eq!(found.len(), 1);
        let device = &found[0];
        assert_eq!(
            (device.name.as_str(), device.kind),
            ("CLIPPLUS", DeviceKind::Rockbox)
        );
        assert_eq!(device.root, DeviceRoot::Path(mount));
        assert_eq!(device.profile, Profile::rockbox());
        assert_eq!(device.profile.music_dir, "Music");
        assert_eq!(device.profile.playlist_dir, "Playlists");
        assert!(
            device.capacity_bytes > 0
                && device.free_bytes > 0
                && device.free_bytes <= device.capacity_bytes
        );
        assert_eq!(device.id.len(), 16);
    }

    #[test]
    fn the_id_follows_the_volume_not_the_mount_point() {
        let root = tempfile::tempdir().unwrap();
        let first = discover_in([rockbox(root.path(), "PLAYER")], &no_phones());
        let other = tempfile::tempdir().unwrap();
        let moved = discover_in([rockbox(other.path(), "PLAYER")], &no_phones());
        assert_eq!(first[0].id, moved[0].id);
        let renamed = discover_in([rockbox(other.path(), "PLAYER 2")], &no_phones());
        assert_ne!(first[0].id, renamed[0].id);
    }

    #[test]
    fn an_apple_ipod_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        let ipod = root.path().join("IPOD");
        fs::create_dir_all(ipod.join("iPod_Control/iTunes")).unwrap();
        fs::write(ipod.join("iPod_Control/iTunes/iTunesDB"), b"mhbd").unwrap();
        let bare = root.path().join("BARE_IPOD");
        fs::create_dir_all(bare.join("iPod_Control")).unwrap();
        assert!(discover_in([ipod, bare], &no_phones()).is_empty());
    }

    #[test]
    fn an_ipod_running_rockbox_is_found() {
        let root = tempfile::tempdir().unwrap();
        let mount = rockbox(root.path(), "IPOD");
        fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        let found = discover_in([mount], &no_phones());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, DeviceKind::Rockbox);
    }

    #[test]
    fn ordinary_volumes_are_ignored() {
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("BACKUP");
        fs::create_dir_all(plain.join("Music")).unwrap();
        assert!(discover_in([plain, root.path().join("missing")], &no_phones()).is_empty());
    }

    #[test]
    fn a_marked_volume_becomes_a_target_with_its_own_profile() {
        let root = tempfile::tempdir().unwrap();
        let stick = root.path().join("STICK");
        fs::create_dir(&stick).unwrap();
        assert!(discover_in([stick.clone()], &no_phones()).is_empty());

        let profile = Profile {
            music_dir: "Tunes".to_owned(),
            formats: vec!["mp3".to_owned()],
            ..Profile::default()
        };
        mark_device(&stick, "Walkman", &profile).unwrap();
        let found = discover_in([stick.clone()], &no_phones());
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].name.as_str(), found[0].kind),
            ("Walkman", DeviceKind::MassStorage)
        );
        assert_eq!(found[0].profile, profile);
        assert!(stick.join(MARKER_FILE).is_file());
        // Nothing but the marker was written, and no temp file is left.
        assert_eq!(fs::read_dir(&stick).unwrap().count(), 1);
    }

    #[test]
    fn marking_again_keeps_the_id() {
        let root = tempfile::tempdir().unwrap();
        let stick = root.path().join("STICK");
        fs::create_dir(&stick).unwrap();
        mark_device(&stick, "Old name", &Profile::default()).unwrap();
        let first = discover_in([stick.clone()], &no_phones())[0].clone();
        mark_device(&stick, "New name", &Profile::android()).unwrap();
        let second = discover_in([stick], &no_phones())[0].clone();
        assert_eq!(first.id, second.id);
        assert_eq!(
            (second.name.as_str(), &second.profile),
            ("New name", &Profile::android())
        );
    }

    #[test]
    fn two_marked_volumes_get_different_ids() {
        let root = tempfile::tempdir().unwrap();
        let ids: Vec<String> = ["A", "B"]
            .iter()
            .map(|name| {
                let stick = root.path().join(name);
                fs::create_dir(&stick).unwrap();
                mark_device(&stick, "Same name", &Profile::default()).unwrap();
                discover_in([stick], &no_phones())[0].id.clone()
            })
            .collect();
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn a_marker_overrides_rockbox_defaults_but_the_kind_stays_rockbox() {
        let root = tempfile::tempdir().unwrap();
        let mount = rockbox(root.path(), "PLAYER");
        let profile = Profile {
            music_dir: "Songs".to_owned(),
            ..Profile::rockbox()
        };
        mark_device(&mount, "My Rockbox", &profile).unwrap();
        let found = discover_in([mount], &no_phones());
        assert_eq!(
            (
                found[0].kind,
                found[0].profile.music_dir.as_str(),
                found[0].name.as_str()
            ),
            (DeviceKind::Rockbox, "Songs", "My Rockbox")
        );
    }

    #[test]
    fn a_broken_marker_is_ignored_rather_than_trusted() {
        let root = tempfile::tempdir().unwrap();
        let stick = root.path().join("STICK");
        fs::create_dir(&stick).unwrap();
        fs::write(stick.join(MARKER_FILE), "{ nope").unwrap();
        assert!(discover_in([stick.clone()], &no_phones()).is_empty());
        // A marker whose profile is invalid is as good as none.
        fs::write(
            stick.join(MARKER_FILE),
            r#"{"version":1,"id":"x","name":"n","profile":{"musicDir":"../up"}}"#,
        )
        .unwrap();
        assert!(discover_in([stick], &no_phones()).is_empty());
    }

    #[test]
    fn marking_refuses_bad_profiles_and_missing_folders() {
        let root = tempfile::tempdir().unwrap();
        let bad = Profile {
            music_dir: "..".to_owned(),
            ..Profile::default()
        };
        assert!(matches!(
            mark_device(root.path(), "x", &bad),
            Err(Error::InvalidProfile(_))
        ));
        assert!(matches!(
            mark_device(&root.path().join("absent"), "x", &Profile::default()),
            Err(Error::Io { .. })
        ));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn phones_come_after_volumes_one_device_per_storage() {
        let root = tempfile::tempdir().unwrap();
        let mount = rockbox(root.path(), "PLAYER");
        let phone = MtpDevice {
            id: "SERIAL1".to_owned(),
            name: "Pixel 8".to_owned(),
            storages: vec![
                MtpStorageInfo {
                    id: 65537,
                    description: "Internal shared storage".to_owned(),
                    capacity_bytes: 128,
                    free_bytes: 64,
                },
                MtpStorageInfo {
                    id: 65538,
                    description: "SD card".to_owned(),
                    capacity_bytes: 32,
                    free_bytes: 8,
                },
            ],
        };
        let found = discover_in([mount], &Phones(vec![phone]));
        assert_eq!(
            found.iter().map(|d| d.kind).collect::<Vec<_>>(),
            [DeviceKind::Rockbox, DeviceKind::Mtp, DeviceKind::Mtp]
        );
        assert_eq!(found[1].name, "Pixel 8 (Internal shared storage)");
        assert_eq!(
            found[1].root,
            DeviceRoot::Mtp {
                device: "SERIAL1".to_owned(),
                storage: 65537
            }
        );
        assert_eq!((found[1].capacity_bytes, found[1].free_bytes), (128, 64));
        assert_eq!(found[1].profile, Profile::android());
        assert_ne!(found[1].id, found[2].id);
    }

    #[test]
    fn a_phone_with_one_storage_keeps_its_plain_name() {
        let phone = MtpDevice {
            id: "S".to_owned(),
            name: "Galaxy".to_owned(),
            storages: vec![MtpStorageInfo {
                id: 1,
                description: "Phone".to_owned(),
                capacity_bytes: 1,
                free_bytes: 1,
            }],
        };
        assert_eq!(discover_in([], &Phones(vec![phone]))[0].name, "Galaxy");
    }

    #[test]
    fn the_real_discover_runs_without_a_backend() {
        // Whatever is mounted on the machine running the tests, it must not panic or invent phones.
        assert!(
            discover()
                .iter()
                .all(|device| device.kind != DeviceKind::Mtp)
        );
    }
}
