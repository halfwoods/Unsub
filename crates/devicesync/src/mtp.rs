//! The seam for MTP devices (Android phones). macOS has no built-in MTP and nothing here talks to
//! one yet: a backend (libmtp, or Windows' WPD) implements [`MtpBackend`] and the planner and
//! applier work on it unchanged, through [`Storage`].

use crate::{Error, Result, Storage};

/// One storage of an MTP device, such as "Internal shared storage".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MtpStorageInfo {
    pub id: u32,
    pub description: String,
    pub capacity_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MtpDevice {
    /// Stable across plugging, e.g. the USB serial number.
    pub id: String,
    pub name: String,
    pub storages: Vec<MtpStorageInfo>,
}

pub trait MtpBackend: Send + Sync {
    fn devices(&self) -> Result<Vec<MtpDevice>>;

    /// Opens `storage` of `device`. Paths on the returned [`Storage`] are relative to the root of
    /// that storage.
    fn open(&self, device: &str, storage: u32) -> Result<Box<dyn Storage>>;
}

/// The backend this build has. There is none yet, so MTP devices are never discovered and cannot be
/// opened.
pub fn system() -> Box<dyn MtpBackend> {
    Box::new(Unsupported)
}

struct Unsupported;

const NO_MTP: &str = "MTP devices are not supported on this platform yet";

impl MtpBackend for Unsupported {
    fn devices(&self) -> Result<Vec<MtpDevice>> {
        Err(Error::Unsupported(NO_MTP))
    }

    fn open(&self, _: &str, _: u32) -> Result<Box<dyn Storage>> {
        Err(Error::Unsupported(NO_MTP))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_backend_finds_nothing_and_refuses_to_open() {
        let backend = system();
        assert!(matches!(backend.devices(), Err(Error::Unsupported(_))));
        assert!(matches!(
            backend.open("phone", 1),
            Err(Error::Unsupported(_))
        ));
    }
}
