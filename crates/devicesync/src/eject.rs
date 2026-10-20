//! Safely removing a player: unmounting its volume so the cable can be pulled.

use std::path::Path;

use crate::{Device, DeviceRoot, Error, Result};

/// Asks the system to eject `device`, which flushes it and unmounts it. On macOS this is
/// `diskutil eject`; on Windows it is the request the "Safely Remove Hardware" tray icon makes, for
/// the USB device behind the drive letter. Fails, naming the reason, if something still has a file
/// open on it.
///
/// MTP devices are not mounted by this app, so there is nothing to eject: `Unsupported`.
pub fn eject(device: &Device) -> Result<()> {
    match &device.root {
        DeviceRoot::Path(mount) => eject_volume(mount),
        DeviceRoot::Mtp { .. } => Err(Error::Unsupported("MTP devices cannot be ejected yet")),
    }
}

#[cfg(target_os = "macos")]
fn eject_volume(mount: &Path) -> Result<()> {
    use std::process::Command;

    let eject_error = |message: String| Error::Eject {
        device: mount.display().to_string(),
        message,
    };
    let output = Command::new("/usr/sbin/diskutil")
        .arg("eject")
        .arg(mount)
        .output()
        .map_err(|source| eject_error(source.to_string()))?;
    if output.status.success() {
        return Ok(());
    }
    // diskutil explains itself on stderr, or on stdout for some failures.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    Err(eject_error(message.trim().to_owned()))
}

#[cfg(windows)]
fn eject_volume(mount: &Path) -> Result<()> {
    let eject_error = |message: String| Error::Eject {
        device: mount.display().to_string(),
        message,
    };
    let letter = drive_letter(mount)
        .ok_or_else(|| eject_error("only a drive letter such as E:\\ can be ejected".to_owned()))?;
    windows::eject(letter).map_err(eject_error)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn eject_volume(mount: &Path) -> Result<()> {
    let _ = mount;
    Err(Error::Unsupported(
        "ejecting is only implemented on macOS and Windows",
    ))
}

/// `E` from `E:\`, `E:` or `\\?\E:\`; `None` for anything that is not a drive root, such as a
/// folder a volume is mounted on.
#[cfg(any(windows, test))]
fn drive_letter(root: &Path) -> Option<char> {
    let text = root.to_str()?;
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    let mut chars = text.chars();
    let (letter, colon) = (chars.next()?, chars.next()?);
    let rest = chars.as_str();
    (letter.is_ascii_alphabetic() && colon == ':' && matches!(rest, "" | "\\" | "/"))
        .then(|| letter.to_ascii_uppercase())
}

/// Windows has no one-call eject for a drive letter. This follows the long-established recipe of
/// the "Safely Remove Hardware" tool: find the physical disk behind the volume, take its parent in
/// the device tree (the USB device, which also covers a card reader's other slots) and ask the Plug
/// and Play manager to eject that, exactly as the tray icon does.
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use std::{
        ffi::c_void,
        io,
        mem::{size_of, zeroed},
        ptr::{addr_of, null, null_mut},
        thread,
        time::Duration,
    };

    use windows_sys::Win32::{
        Devices::DeviceAndDriverInstallation::{
            CM_Get_Parent, CM_Request_Device_EjectW, CR_SUCCESS, DIGCF_DEVICEINTERFACE,
            DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
            SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
            SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
        },
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
        System::{
            IO::DeviceIoControl,
            Ioctl::{
                GUID_DEVINTERFACE_DISK, IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER,
            },
        },
    };

    /// Windows will not always let go straight away (an indexer may still be reading), so the
    /// request is repeated a few times before giving up.
    const ATTEMPTS: usize = 3;
    const RETRY_AFTER: Duration = Duration::from_millis(500);

    /// Which physical disk a handle's device is: the pair Windows numbers disks by.
    type DiskNumber = (u32, u32);

    pub fn eject(letter: char) -> Result<(), String> {
        let volume: Vec<u16> = format!(r"\\.\{letter}:")
            .encode_utf16()
            .chain([0])
            .collect();
        // SAFETY: `volume` is a NUL-terminated wide string that outlives the call.
        let wanted = unsafe { disk_number(volume.as_ptr()) }
            .map_err(|error| format!("could not open {letter}: ({error})"))?;
        let disk = find_disk(wanted)?;
        let mut parent = 0u32;
        // SAFETY: `parent` is a valid out-pointer for the call.
        if unsafe { CM_Get_Parent(&mut parent, disk, 0) } != CR_SUCCESS {
            return Err("the drive is not attached to a device that can be ejected".to_owned());
        }
        request_eject(parent)
    }

    /// Opens `path` without asking for any access, which is enough to query it and works even
    /// while the volume is in use.
    ///
    /// # Safety
    /// `path` must point to a NUL-terminated wide string.
    unsafe fn disk_number(path: *const u16) -> io::Result<DiskNumber> {
        // SAFETY: the caller guarantees `path`; the other arguments are plain values or null.
        let handle = unsafe {
            CreateFileW(
                path,
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let handle = Handle(handle);
        // SAFETY: STORAGE_DEVICE_NUMBER is plain integers, for which all-zero is valid.
        let mut number: STORAGE_DEVICE_NUMBER = unsafe { zeroed() };
        let mut returned = 0u32;
        // SAFETY: the handle is open and the output buffer is exactly one STORAGE_DEVICE_NUMBER.
        let ok = unsafe {
            DeviceIoControl(
                handle.0,
                IOCTL_STORAGE_GET_DEVICE_NUMBER,
                null(),
                0,
                (&mut number as *mut STORAGE_DEVICE_NUMBER).cast::<c_void>(),
                size_of::<STORAGE_DEVICE_NUMBER>() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((number.DeviceType, number.DeviceNumber))
    }

    /// The device-tree node of the disk drive with this number.
    fn find_disk(wanted: DiskNumber) -> Result<u32, String> {
        let guid = GUID_DEVINTERFACE_DISK;
        // SAFETY: `guid` outlives the call; the other arguments are null or flags.
        let set = unsafe {
            SetupDiGetClassDevsW(
                &guid,
                null(),
                null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        };
        if set == -1 {
            return Err(format!(
                "could not list the disks ({})",
                io::Error::last_os_error()
            ));
        }
        let set = DeviceSet(set);

        for index in 0.. {
            // SAFETY: both structs are plain data; all-zero is valid before `cbSize` is set.
            let mut interface: SP_DEVICE_INTERFACE_DATA = unsafe { zeroed() };
            interface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            // SAFETY: `set` is open and `interface` has its size set.
            if unsafe { SetupDiEnumDeviceInterfaces(set.0, null(), &guid, index, &mut interface) }
                == 0
            {
                break;
            }

            // The first call only reports how big the detail record must be.
            let mut size = 0u32;
            // SAFETY: a null buffer of size 0 is how the size is asked for.
            unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    set.0,
                    &interface,
                    null_mut(),
                    0,
                    &mut size,
                    null_mut(),
                );
            }
            if size == 0 {
                continue;
            }
            // u32 words keep the record aligned for its 4-byte header.
            let mut record = vec![0u32; (size as usize).div_ceil(4)];
            let detail = record
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            // SAFETY: `record` is at least `size` bytes, aligned, and outlives its use below.
            unsafe { (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 };
            // SAFETY: as above for the zeroed struct.
            let mut info: SP_DEVINFO_DATA = unsafe { zeroed() };
            info.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
            // SAFETY: `detail` points at `size` writable bytes with its header set; `info` has its
            // size set.
            let got = unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    set.0,
                    &interface,
                    detail,
                    size,
                    null_mut(),
                    &mut info,
                )
            };
            if got == 0 {
                continue;
            }
            // SAFETY: on success `DevicePath` starts a NUL-terminated wide string inside `record`.
            let candidate = unsafe { disk_number(addr_of!((*detail).DevicePath).cast::<u16>()) };
            if candidate.is_ok_and(|number| number == wanted) {
                return Ok(info.DevInst);
            }
        }
        Err("could not find the disk behind the drive letter".to_owned())
    }

    fn request_eject(device: u32) -> Result<(), String> {
        let mut veto_name = [0u16; 260];
        let mut veto_type = 0;
        for attempt in 1..=ATTEMPTS {
            // SAFETY: the veto name buffer is `veto_name.len()` wide characters and writable.
            let result = unsafe {
                CM_Request_Device_EjectW(
                    device,
                    &mut veto_type,
                    veto_name.as_mut_ptr(),
                    veto_name.len() as u32,
                    0,
                )
            };
            if result == CR_SUCCESS {
                return Ok(());
            }
            if attempt < ATTEMPTS {
                thread::sleep(RETRY_AFTER);
            }
        }
        let name_len = veto_name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(veto_name.len());
        let who = String::from_utf16_lossy(&veto_name[..name_len]);
        Err(if who.is_empty() {
            "Windows would not eject the device; close anything using it and try again".to_owned()
        } else {
            format!("Windows would not eject the device because {who} is using it")
        })
    }

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by `CreateFileW` and is closed only here.
            unsafe { CloseHandle(self.0) };
        }
    }

    struct DeviceSet(HDEVINFO);

    impl Drop for DeviceSet {
        fn drop(&mut self) {
            // SAFETY: the set came from `SetupDiGetClassDevsW` and is destroyed only here.
            unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{DeviceKind, Profile};

    fn device(root: DeviceRoot) -> Device {
        Device {
            id: "x".to_owned(),
            name: "Player".to_owned(),
            kind: DeviceKind::MassStorage,
            root,
            capacity_bytes: 0,
            free_bytes: 0,
            profile: Profile::default(),
        }
    }

    #[test]
    fn drive_letters_are_read_from_drive_roots_only() {
        for (root, letter) in [
            (r"E:\", Some('E')),
            ("e:", Some('E')),
            ("D:/", Some('D')),
            (r"\\?\F:\", Some('F')),
            (r"E:\Music", None),
            ("/Volumes/PLAYER", None),
            (r"\\server\share", None),
            ("", None),
            ("E", None),
            ("1:\\", None),
        ] {
            assert_eq!(drive_letter(&PathBuf::from(root)), letter, "{root:?}");
        }
    }

    #[test]
    fn an_mtp_device_cannot_be_ejected_yet() {
        let phone = device(DeviceRoot::Mtp {
            device: "SER".to_owned(),
            storage: 1,
        });
        assert!(matches!(eject(&phone), Err(Error::Unsupported(_))));
    }

    /// There is no drive to eject in a test, but the failure path is real: the system tool is run
    /// and its complaint comes back as the error.
    #[cfg(target_os = "macos")]
    #[test]
    fn ejecting_a_volume_that_is_not_there_reports_the_systems_reason() {
        let ghost = device(DeviceRoot::Path("/Volumes/no-such-player-1234".into()));
        match eject(&ghost) {
            Err(Error::Eject { device, message }) => {
                assert_eq!(device, "/Volumes/no-such-player-1234");
                assert!(!message.is_empty());
            }
            other => panic!("expected an eject error, got {other:?}"),
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn other_platforms_say_so() {
        let usb = device(DeviceRoot::Path("/media/PLAYER".into()));
        assert!(matches!(eject(&usb), Err(Error::Unsupported(_))));
    }
}
