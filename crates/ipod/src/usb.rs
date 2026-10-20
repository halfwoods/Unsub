//! The USB serial number of the iPod a volume belongs to. For the iPods whose database checksum is
//! keyed by the FireWire GUID (classic, nano 3G/4G), that serial number is the GUID, and it is the
//! only place to find it on an iPod that iTunes never wrote `SysInfoExtended` to.
//!
//! Only macOS is implemented. Windows would map the drive letter to its disk number, walk up to the
//! `USB\VID_05AC&PID_xxxx\<serial>` device instance with SetupAPI, and so needs `windows-sys` as a
//! runtime dependency; elsewhere there is no lookup, and the GUID stays unknown.

use std::path::Path;

/// `None` when the volume is not behind an Apple USB device or the lookup fails. The device is
/// found through the volume's own disk, never as "the only iPod plugged in", so with several
/// connected a wrong number is not returned.
pub(crate) fn serial_number(mount: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos::serial_number(mount)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = mount;
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::io::Cursor;
    use std::path::Path;
    use std::process::Command;

    use plist::Value;

    const APPLE_VENDOR_ID: u64 = 0x05ac;
    /// The product ids of the iPods that mount as disks, 0x1200 for the third generation up to the
    /// shuffles' 0x1300s.
    const IPOD_PRODUCTS: std::ops::RangeInclusive<u64> = 0x1200..=0x13ff;

    pub(super) fn serial_number(mount: &Path) -> Option<String> {
        let info = Command::new("diskutil")
            .arg("info")
            .arg("-plist")
            .arg(mount)
            .output()
            .ok()?;
        let disk = whole_disk(&info.stdout)?;
        let tree = Command::new("ioreg")
            .args(["-r", "-c", "IOUSBHostDevice", "-l", "-w0"])
            .output()
            .ok()?;
        serial_behind(&String::from_utf8_lossy(&tree.stdout), &disk)
    }

    /// The disk the volume's partition belongs to (`disk4` for `disk4s3`), which is the name the
    /// USB device's storage is registered under.
    fn whole_disk(diskutil_plist: &[u8]) -> Option<String> {
        let info = Value::from_reader(Cursor::new(diskutil_plist)).ok()?;
        let disk = info.as_dictionary()?.get("ParentWholeDisk")?.as_string()?;
        Some(disk.to_owned())
    }

    /// One object in the `ioreg -l` tree and what has been read of its properties so far.
    struct Node {
        /// Where its `+-o` sits in the line: children are indented further than their parent.
        column: usize,
        is_usb_device: bool,
        vendor: Option<u64>,
        product: Option<u64>,
        serial: Option<String>,
    }

    /// The serial number of the iPod that has the disk `disk` below it, from the output of
    /// `ioreg -r -c IOUSBHostDevice -l`. A node's properties follow its own line and precede its
    /// children's, so by the time the disk's `BSD Name` is reached every ancestor has been read.
    ///
    /// Only the USB device the disk sits directly under is looked at, and it has to be an Apple
    /// iPod: the serial number of a card reader, or of a hub with an iPod's drive behind it, is
    /// never taken for the iPod's. Only a 16-digit hex serial number is a GUID.
    fn serial_behind(tree: &str, disk: &str) -> Option<String> {
        let mut path: Vec<Node> = Vec::new();
        for line in tree.lines() {
            if let Some((column, class)) = node_line(line) {
                path.retain(|node| node.column < column);
                path.push(Node {
                    column,
                    is_usb_device: class == "IOUSBHostDevice",
                    vendor: None,
                    product: None,
                    serial: None,
                });
            } else if let (Some((key, value)), Some(node)) = (property(line), path.last_mut()) {
                match key {
                    "idVendor" => node.vendor = value.parse().ok(),
                    "idProduct" => node.product = value.parse().ok(),
                    "USB Serial Number" | "kUSBSerialNumberString" => node.serial = unquote(value),
                    "BSD Name" if unquote(value).as_deref() == Some(disk) => {
                        let device = path.iter().rev().find(|node| node.is_usb_device)?;
                        let is_ipod = device.vendor == Some(APPLE_VENDOR_ID)
                            && device
                                .product
                                .is_some_and(|product| IPOD_PRODUCTS.contains(&product));
                        return device.serial.clone().filter(|serial| {
                            is_ipod
                                && serial.len() == 16
                                && serial.bytes().all(|b| b.is_ascii_hexdigit())
                        });
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// `  | +-o iPod@14100000  <class IOUSBHostDevice, id 0x1, ...>` gives the column of the `+-o`
    /// and the class.
    fn node_line(line: &str) -> Option<(usize, &str)> {
        let column = line.find("+-o ")?;
        if !line[..column].bytes().all(|b| b == b' ' || b == b'|') {
            return None;
        }
        let class = line[column..].split_once("<class ")?.1.split(',').next()?;
        Some((column, class))
    }

    /// `  |   "idVendor" = 1452` gives `idVendor` and `1452`.
    fn property(line: &str) -> Option<(&str, &str)> {
        let line = line.trim_start_matches([' ', '|']).strip_prefix('"')?;
        let (key, value) = line.split_once("\" = ")?;
        Some((key, value.trim()))
    }

    fn unquote(value: &str) -> Option<String> {
        Some(value.strip_prefix('"')?.strip_suffix('"')?.to_owned())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Shaped like `ioreg -r -c IOUSBHostDevice -l`: a keyboard and two iPods, each with its
        /// storage nested several levels down, as on a real machine.
        const TREE: &str = r#"+-o IOUSBHostDevice@00100000  <class IOUSBHostDevice, id 0x1000aa272, registered, matched, active, busy 0 (63 ms), retain 39>
  | {
  |   "idProduct" = 4112
  |   "USB Serial Number" = "KEYBOARD-1"
  |   "idVendor" = 1539
  | }
  |
  +-o IOUSBHostInterface@0  <class IOUSBHostInterface, id 0x1000aa277, registered, matched, active, busy 0 (55 ms), retain 11>
  | | {
  | |   "idVendor" = 1539
  | | }
  | |
  | +-o AppleUserUSBHostHIDDevice  <class AppleUserHIDDevice, id 0x1000aa27f, registered, matched, active, busy 0 (6 ms), retain 15>
+-o iPod@14100000  <class IOUSBHostDevice, id 0x1000aa300, registered, matched, active, busy 0 (63 ms), retain 39>
  | {
  |   "idProduct" = 4705
  |   "USB Product Name" = "iPod"
  |   "idVendor" = 1452
  |   "USB Serial Number" = "000A27001A2B3C4D"
  | }
  |
  +-o IOUSBHostInterface@0  <class IOUSBHostInterface, id 0x1000aa301, registered, matched, active, busy 0 (55 ms), retain 11>
    | {
    |   "bInterfaceClass" = 8
    | }
    |
    +-o IOUSBMassStorageDriverNub  <class IOUSBMassStorageDriverNub, id 0x1000aa302, registered, matched, active, busy 0, retain 8>
      +-o IOBlockStorageServices  <class IOBlockStorageServices, id 0x1000aa303, registered, matched, active, busy 0, retain 9>
        +-o IOBlockStorageDriver  <class IOBlockStorageDriver, id 0x1000aa304, registered, matched, active, busy 0, retain 8>
          +-o Apple iPod Media  <class IOMedia, id 0x1000aa305, registered, matched, active, busy 0, retain 12>
            | {
            |   "Whole" = Yes
            |   "BSD Name" = "disk4"
            | }
            |
            +-o IOMedia@3  <class IOMedia, id 0x1000aa306, registered, matched, active, busy 0, retain 8>
              | {
              |   "BSD Name" = "disk4s3"
              | }
+-o iPod@14200000  <class IOUSBHostDevice, id 0x1000aa400, registered, matched, active, busy 0 (63 ms), retain 39>
  | {
  |   "idProduct" = 4707
  |   "idVendor" = 1452
  |   "USB Serial Number" = "000A27009999AAAA"
  | }
  |
  +-o IOUSBHostInterface@0  <class IOUSBHostInterface, id 0x1000aa401, registered, matched, active, busy 0 (55 ms), retain 11>
    +-o Apple iPod Media  <class IOMedia, id 0x1000aa405, registered, matched, active, busy 0, retain 12>
      | {
      |   "BSD Name" = "disk5"
      | }
"#;

        #[test]
        fn each_disk_belongs_to_the_device_it_sits_under() {
            assert_eq!(
                serial_behind(TREE, "disk4").as_deref(),
                Some("000A27001A2B3C4D")
            );
            assert_eq!(
                serial_behind(TREE, "disk5").as_deref(),
                Some("000A27009999AAAA")
            );
        }

        #[test]
        fn a_disk_nothing_usb_holds_has_no_serial() {
            assert_eq!(serial_behind(TREE, "disk9"), None);
            assert_eq!(serial_behind("", "disk4"), None);
        }

        #[test]
        fn only_an_apple_device_with_a_guid_shaped_serial_counts() {
            let other_maker = TREE.replacen("\"idVendor\" = 1452", "\"idVendor\" = 1234", 1);
            assert_eq!(serial_behind(&other_maker, "disk4"), None);
            let not_a_guid = TREE.replacen("000A27001A2B3C4D", "9C9123456XYZ", 1);
            assert_eq!(serial_behind(&not_a_guid, "disk4"), None);
        }

        /// An Apple hub (with a GUID-shaped serial number of its own) with a third-party card reader
        /// below it, and an Apple device that is not an iPod.
        const HUB_AND_READER: &str = r#"+-o USB Hub@14000000  <class IOUSBHostDevice, id 0x1, registered, matched, active, busy 0, retain 9>
  | {
  |   "idProduct" = 4705
  |   "idVendor" = 1452
  |   "USB Serial Number" = "000A27001A2B3C4D"
  | }
  | 
  +-o Card Reader@14100000  <class IOUSBHostDevice, id 0x2, registered, matched, active, busy 0, retain 9>
    | {
    |   "idProduct" = 21829
    |   "idVendor" = 1507
    |   "USB Serial Number" = "READER0000000001"
    | }
    | 
    +-o Reader Media  <class IOMedia, id 0x3, registered, matched, active, busy 0, retain 12>
      | {
      |   "BSD Name" = "disk6"
      | }
+-o Magic Keyboard@14200000  <class IOUSBHostDevice, id 0x4, registered, matched, active, busy 0, retain 9>
  | {
  |   "idProduct" = 619
  |   "idVendor" = 1452
  |   "USB Serial Number" = "000A27001234ABCD"
  | }
  | 
  +-o Keyboard Media  <class IOMedia, id 0x5, registered, matched, active, busy 0, retain 12>
    | {
    |   "BSD Name" = "disk7"
    | }
"#;

        #[test]
        fn the_device_the_disk_sits_under_is_the_only_one_that_counts() {
            // The reader is third-party, and the hub above it is not skipped to.
            assert_eq!(serial_behind(HUB_AND_READER, "disk6"), None);
            // Apple's, but not an iPod's product id.
            assert_eq!(serial_behind(HUB_AND_READER, "disk7"), None);
        }

        #[test]
        fn lines_are_taken_apart_by_their_shape() {
            let node = "  | +-o iPod@1  <class IOUSBHostDevice, id 0x1, registered>";
            assert_eq!(node_line(node), Some((4, "IOUSBHostDevice")));
            assert_eq!(node_line("  |   \"BSD Name\" = \"disk4\""), None);
            assert_eq!(node_line("some text +-o not a tree line <class X,"), None);
            assert_eq!(
                property("  |   \"idVendor\" = 1452"),
                Some(("idVendor", "1452"))
            );
            assert_eq!(property("  | {"), None);
            assert_eq!(unquote("\"disk4\"").as_deref(), Some("disk4"));
            assert_eq!(unquote("Yes"), None);
        }

        #[test]
        fn the_whole_disk_comes_from_diskutils_plist() {
            let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>DeviceIdentifier</key>
	<string>disk4s3</string>
	<key>ParentWholeDisk</key>
	<string>disk4</string>
</dict>
</plist>"#;
            assert_eq!(whole_disk(plist).as_deref(), Some("disk4"));
            assert_eq!(whole_disk(b"not a plist"), None);
        }
    }
}
