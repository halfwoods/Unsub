//! `iPod_Control/Device/SysInfo` (`Key: value` lines) and `SysInfoExtended` (XML plist), read the
//! way libgpod's `itdb_device_read_sysinfo` and `itdb_sysinfo_extended_parse` do. Both files are
//! optional and best-effort: an iPod without them still opens, just without a model.

use std::fs;
use std::path::Path;

use plist::Value;

#[derive(Debug, Default)]
pub(crate) struct SysInfo {
    pub(crate) model_num_str: Option<String>,
    pub(crate) firewire_guid: Option<String>,
    pub(crate) serial_number: Option<String>,
    pub(crate) db_version: Option<i64>,
}

pub(crate) fn read(device_dir: &Path) -> SysInfo {
    let mut info = SysInfo::default();
    if let Ok(bytes) = fs::read(device_dir.join("SysInfo")) {
        apply_text(&mut info, &String::from_utf8_lossy(&bytes));
    }
    if let Ok(plist) = Value::from_file(device_dir.join("SysInfoExtended")) {
        apply_plist(&mut info, &plist);
    }
    info
}

fn apply_text(info: &mut SysInfo, text: &str) {
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "ModelNumStr" => info.model_num_str = non_empty(value),
            "FirewireGuid" => info.firewire_guid = non_empty(strip_hex_prefix(value)),
            _ => {}
        }
    }
}

/// SysInfoExtended wins over SysInfo for the GUID, as in libgpod.
fn apply_plist(info: &mut SysInfo, plist: &Value) {
    let Some(dict) = plist.as_dictionary() else {
        return;
    };
    let string = |key: &str| dict.get(key).and_then(Value::as_string).and_then(non_empty);
    if let Some(guid) = string("FireWireGUID") {
        info.firewire_guid = non_empty(strip_hex_prefix(&guid));
    }
    info.serial_number = string("SerialNumber");
    info.db_version = dict.get("DBVersion").and_then(Value::as_signed_integer);
}

fn non_empty(value: &str) -> Option<String> {
    Some(value.to_owned()).filter(|value| !value.is_empty())
}

/// SysInfo writes `0x000A27...`, SysInfoExtended the bare digits; libgpod accepts either.
fn strip_hex_prefix(guid: &str) -> &str {
    guid.strip_prefix("0x")
        .or_else(|| guid.strip_prefix("0X"))
        .unwrap_or(guid)
}
