use std::hash::Hasher;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::models::{self, IpodModel, SyncSupport};
use crate::{itunesdb, sysinfo, usb};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DatabaseFormat {
    ITunesDb,
    ITunesCdb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ipod {
    /// FireWire GUID if known, else a stable hash of the mount path and volume name.
    pub id: String,
    /// The master playlist's title, which is what iTunes showed; the volume name if unreadable.
    pub name: String,
    pub mount_path: PathBuf,
    pub model: Option<IpodModel>,
    pub capacity_bytes: u64,
    pub free_bytes: u64,
    pub firewire_guid: Option<String>,
    /// The GUID is the USB serial number, because the iPod's own files do not give it. Nothing
    /// then vouches for it, so a sync checks it against the database already on the iPod.
    pub guid_from_usb: bool,
    pub serial_number: Option<String>,
    pub database: DatabaseFormat,
    pub sync: SyncSupport,
}

pub(crate) fn itunes_dir(mount: &Path) -> PathBuf {
    mount.join("iPod_Control").join("iTunes")
}

pub(crate) fn database_path(mount: &Path, format: DatabaseFormat) -> PathBuf {
    let file = match format {
        DatabaseFormat::ITunesDb => "iTunesDB",
        DatabaseFormat::ITunesCdb => "iTunesCDB",
    };
    itunes_dir(mount).join(file)
}

/// libgpod's `itdb_parse` prefers iTunesCDB when it exists. An empty file (left behind by an
/// interrupted sync or fsck) loses to a non-empty one of the other format.
pub(crate) fn locate_database(mount: &Path) -> Option<DatabaseFormat> {
    let size = |format| {
        std::fs::metadata(database_path(mount, format))
            .ok()
            .map(|meta| meta.len())
    };
    match (
        size(DatabaseFormat::ITunesCdb),
        size(DatabaseFormat::ITunesDb),
    ) {
        (Some(len), _) if len > 0 => Some(DatabaseFormat::ITunesCdb),
        (_, Some(len)) if len > 0 => Some(DatabaseFormat::ITunesDb),
        (Some(_), _) => Some(DatabaseFormat::ITunesCdb),
        (_, Some(_)) => Some(DatabaseFormat::ITunesDb),
        (None, None) => None,
    }
}

/// Whether the iPod has a database file yet. One that was never synced, or was just restored, has
/// none, and its first sync writes one.
pub(crate) fn database_exists(ipod: &Ipod) -> bool {
    database_path(&ipod.mount_path, ipod.database).is_file()
}

/// The database format of the iPod at `mount`, or `None` if it is not an iPod. An iPod without a
/// database yet gets the iTunesDB its first sync writes.
///
/// `iPod_Control` alone proves nothing, since any disk can hold a folder of that name. What an
/// iPod without a database has is the `Device/SysInfo` its firmware writes, or the `iTunes`
/// folder the database goes in. The names are the ones the iPod and iTunes write. A
/// case-insensitive volume (FAT32, or HFS+ unless formatted case-sensitive) matches any case; on
/// a case-sensitive one the rest of this crate would not find these files under another case
/// either, so looking for them there would only detect an iPod it could not then use.
pub(crate) fn recognise(mount: &Path) -> Option<DatabaseFormat> {
    let control = mount.join("iPod_Control");
    let device_info = |name| control.join("Device").join(name).is_file();
    let blank =
        control.join("iTunes").is_dir() || device_info("SysInfo") || device_info("SysInfoExtended");
    locate_database(mount).or_else(|| blank.then_some(DatabaseFormat::ITunesDb))
}

pub fn open(mount: &Path) -> Result<Ipod> {
    let database = recognise(mount).ok_or_else(|| Error::NotAnIpod(mount.to_path_buf()))?;
    let info = sysinfo::read(&mount.join("iPod_Control").join("Device"));
    let model = models::identify(info.serial_number.as_deref(), info.model_num_str.as_deref());
    let sync = models::sync_support(model.as_ref(), info.db_version);
    // An iPod that iTunes never wrote SysInfoExtended to may not say its GUID anywhere on its disk,
    // but its USB serial number is the same number.
    let usb_guid = match info.firewire_guid {
        Some(_) => None,
        None => (sync == SyncSupport::NeedsHash58)
            .then(|| usb::serial_number(mount))
            .flatten(),
    };
    let guid_from_usb = usb_guid.is_some();
    let firewire_guid = info.firewire_guid.or(usb_guid);
    let stats = fs4::statvfs(mount).map_err(|source| Error::io(mount, source))?;
    let volume_name = volume_name(mount);
    // A database we cannot read must not stop the device from showing up, so the name falls back.
    let name = itunesdb::load(&database_path(mount, database), database)
        .and_then(|data| itunesdb::master_playlist_name(&data))
        .ok()
        .flatten()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| volume_name.clone());
    let id = firewire_guid
        .clone()
        .unwrap_or_else(|| stable_id(mount, &volume_name));

    Ok(Ipod {
        id,
        name,
        mount_path: mount.to_path_buf(),
        model,
        capacity_bytes: stats.total_space(),
        free_bytes: stats.available_space(),
        firewire_guid,
        guid_from_usb,
        serial_number: info.serial_number,
        database,
        sync,
    })
}

/// `/Volumes/IPOD` gives `IPOD`; a Windows drive root has no file name, so the drive itself is used.
fn volume_name(mount: &Path) -> String {
    mount.file_name().map_or_else(
        || mount.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// FNV-1a rather than `DefaultHasher`, whose output may change between Rust releases and would
/// orphan any per-device settings stored under the id.
fn stable_id(mount: &Path, volume_name: &str) -> String {
    let mut hasher = Fnv1a(0xcbf2_9ce4_8422_2325);
    hasher.write(mount.as_os_str().as_encoded_bytes());
    hasher.write(&[0]);
    hasher.write(volume_name.as_bytes());
    format!("{:016x}", hasher.finish())
}

struct Fnv1a(u64);

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
