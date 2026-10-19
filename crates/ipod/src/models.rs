//! Model identification, ported from libgpod's `itdb_device.c`: `ipod_info_table`,
//! `serial_to_model_mapping`, `get_ipod_info_from_model_number`, `itdb_ipod_info_from_serial` and
//! `itdb_device_get_checksum_type`.
//!
//! Only devices that mount as USB disks are listed. libgpod also lists the iPod touch, iPhone, iPad
//! and the Motorola "mobile" iPod; those sync over a different protocol and never reach `discover`.

use serde::Serialize;

use IpodFamily::{Classic, Mini, Nano, Original, Photo, Shuffle, Video};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IpodFamily {
    Original,
    Mini,
    Photo,
    Video,
    Classic,
    Nano,
    Shuffle,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpodModel {
    pub family: IpodFamily,
    /// 1-based position within the family in libgpod's table. This matches the marketing generation
    /// except for Video (1 = 5th, 2 = 5.5th), Classic (1 and 2 = 6th, 3 = 7th) and Photo (always 1).
    pub generation: u8,
    pub name: String,
    pub capacity_gb: Option<u32>,
    pub color: Option<String>,
    /// libgpod's table key: the marketing part number without its leading letter, e.g. `C293` for
    /// `MC293`.
    pub model_number: String,
}

/// Whether and how the iTunesDB checksum must be written for this device to accept a sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncSupport {
    Supported,
    NeedsHash58,
    NeedsHash72,
    /// HASHAB (nano 6G and later), an unknown checksum scheme, or an unidentified device.
    Unsupported,
}

struct Entry {
    number: &'static str,
    capacity_gb: Option<u32>,
    family: IpodFamily,
    generation: u8,
    color: Option<&'static str>,
}

const NONE: Option<&str> = None;
const U2: Option<&str> = Some("U2 Special Edition");
const SILVER: Option<&str> = Some("Silver");
const BLACK: Option<&str> = Some("Black");
const WHITE: Option<&str> = Some("White");
const BLUE: Option<&str> = Some("Blue");
const PINK: Option<&str> = Some("Pink");
const GREEN: Option<&str> = Some("Green");
const GOLD: Option<&str> = Some("Gold");
const RED: Option<&str> = Some("Red");
const ORANGE: Option<&str> = Some("Orange");
const PURPLE: Option<&str> = Some("Purple");
const YELLOW: Option<&str> = Some("Yellow");
const STAINLESS: Option<&str> = Some("Stainless");

const fn entry(
    number: &'static str,
    capacity_gb: Option<u32>,
    family: IpodFamily,
    generation: u8,
    color: Option<&'static str>,
) -> Entry {
    Entry {
        number,
        capacity_gb,
        family,
        generation,
        color,
    }
}

/// libgpod's `ipod_info_table`. The 512 MB shuffle has no `capacity_gb` because it is below 1 GB.
const MODELS: &[Entry] = &[
    // Original generation 1
    entry("8513", Some(5), Original, 1, NONE),
    entry("8541", Some(5), Original, 1, NONE),
    entry("8697", Some(5), Original, 1, NONE),
    entry("8709", Some(10), Original, 1, NONE),
    // Original generation 2
    entry("8737", Some(10), Original, 2, NONE),
    entry("8740", Some(10), Original, 2, NONE),
    entry("8738", Some(20), Original, 2, NONE),
    entry("8741", Some(20), Original, 2, NONE),
    // Original generation 3
    entry("8976", Some(10), Original, 3, NONE),
    entry("8946", Some(15), Original, 3, NONE),
    entry("9460", Some(15), Original, 3, NONE),
    entry("9244", Some(20), Original, 3, NONE),
    entry("8948", Some(30), Original, 3, NONE),
    entry("9245", Some(40), Original, 3, NONE),
    // Original generation 4
    entry("9282", Some(20), Original, 4, NONE),
    entry("9787", Some(25), Original, 4, U2),
    entry("9268", Some(40), Original, 4, NONE),
    // Mini generation 1
    entry("9160", Some(4), Mini, 1, SILVER),
    entry("9436", Some(4), Mini, 1, BLUE),
    entry("9435", Some(4), Mini, 1, PINK),
    entry("9434", Some(4), Mini, 1, GREEN),
    entry("9437", Some(4), Mini, 1, GOLD),
    // Mini generation 2
    entry("9800", Some(4), Mini, 2, SILVER),
    entry("9802", Some(4), Mini, 2, BLUE),
    entry("9804", Some(4), Mini, 2, PINK),
    entry("9806", Some(4), Mini, 2, GREEN),
    entry("9801", Some(6), Mini, 2, SILVER),
    entry("9803", Some(6), Mini, 2, BLUE),
    entry("9805", Some(6), Mini, 2, PINK),
    entry("9807", Some(6), Mini, 2, GREEN),
    // Photo generation 1
    entry("A079", Some(20), Photo, 1, NONE),
    entry("A127", Some(20), Photo, 1, U2),
    entry("9829", Some(30), Photo, 1, NONE),
    entry("9585", Some(40), Photo, 1, NONE),
    entry("9830", Some(60), Photo, 1, NONE),
    entry("9586", Some(60), Photo, 1, NONE),
    // Shuffle generation 1
    entry("9724", None, Shuffle, 1, NONE),
    entry("9725", Some(1), Shuffle, 1, NONE),
    // Shuffle generation 2
    entry("A546", Some(1), Shuffle, 2, SILVER),
    entry("A947", Some(1), Shuffle, 2, PINK),
    entry("A949", Some(1), Shuffle, 2, BLUE),
    entry("A951", Some(1), Shuffle, 2, GREEN),
    entry("A953", Some(1), Shuffle, 2, ORANGE),
    entry("C167", Some(1), Shuffle, 2, GOLD),
    entry("B225", Some(1), Shuffle, 2, SILVER),
    entry("B233", Some(1), Shuffle, 2, PURPLE),
    entry("B231", Some(1), Shuffle, 2, RED),
    entry("B227", Some(1), Shuffle, 2, BLUE),
    entry("B228", Some(1), Shuffle, 2, BLUE),
    entry("B229", Some(1), Shuffle, 2, GREEN),
    entry("B518", Some(2), Shuffle, 2, SILVER),
    entry("B520", Some(2), Shuffle, 2, BLUE),
    entry("B522", Some(2), Shuffle, 2, GREEN),
    entry("B524", Some(2), Shuffle, 2, RED),
    entry("B526", Some(2), Shuffle, 2, PURPLE),
    // Shuffle generation 3
    entry("C306", Some(2), Shuffle, 3, SILVER),
    entry("C323", Some(2), Shuffle, 3, BLACK),
    entry("C381", Some(2), Shuffle, 3, GREEN),
    entry("C384", Some(2), Shuffle, 3, BLUE),
    entry("C387", Some(2), Shuffle, 3, PINK),
    entry("B867", Some(4), Shuffle, 3, SILVER),
    entry("C164", Some(4), Shuffle, 3, BLACK),
    entry("C303", Some(4), Shuffle, 3, STAINLESS),
    entry("C307", Some(4), Shuffle, 3, GREEN),
    entry("C328", Some(4), Shuffle, 3, BLUE),
    entry("C331", Some(4), Shuffle, 3, PINK),
    // Shuffle generation 4
    entry("C584", Some(2), Shuffle, 4, SILVER),
    entry("C585", Some(2), Shuffle, 4, PINK),
    entry("C749", Some(2), Shuffle, 4, ORANGE),
    entry("C750", Some(2), Shuffle, 4, GREEN),
    entry("C751", Some(2), Shuffle, 4, BLUE),
    // Nano generation 1
    entry("A350", Some(1), Nano, 1, WHITE),
    entry("A352", Some(1), Nano, 1, BLACK),
    entry("A004", Some(2), Nano, 1, WHITE),
    entry("A099", Some(2), Nano, 1, BLACK),
    entry("A005", Some(4), Nano, 1, WHITE),
    entry("A107", Some(4), Nano, 1, BLACK),
    // Video generation 1
    entry("A002", Some(30), Video, 1, WHITE),
    entry("A146", Some(30), Video, 1, BLACK),
    entry("A003", Some(60), Video, 1, WHITE),
    entry("A147", Some(60), Video, 1, BLACK),
    entry("A452", Some(30), Video, 1, U2),
    // Video generation 2
    entry("A444", Some(30), Video, 2, WHITE),
    entry("A446", Some(30), Video, 2, BLACK),
    entry("A664", Some(30), Video, 2, U2),
    entry("A448", Some(80), Video, 2, WHITE),
    entry("A450", Some(80), Video, 2, BLACK),
    // Nano generation 2
    entry("A477", Some(2), Nano, 2, SILVER),
    entry("A426", Some(4), Nano, 2, SILVER),
    entry("A428", Some(4), Nano, 2, BLUE),
    entry("A487", Some(4), Nano, 2, GREEN),
    entry("A489", Some(4), Nano, 2, PINK),
    entry("A725", Some(4), Nano, 2, RED),
    entry("A726", Some(8), Nano, 2, RED),
    entry("A497", Some(8), Nano, 2, BLACK),
    // Original generation 4
    entry("E436", Some(40), Original, 4, NONE),
    // Photo generation 1
    entry("S492", Some(30), Photo, 1, NONE),
    // Classic generation 1
    entry("B029", Some(80), Classic, 1, SILVER),
    entry("B147", Some(80), Classic, 1, BLACK),
    entry("B145", Some(160), Classic, 1, SILVER),
    entry("B150", Some(160), Classic, 1, BLACK),
    // Classic generation 2
    entry("B562", Some(120), Classic, 2, SILVER),
    entry("B565", Some(120), Classic, 2, BLACK),
    // Classic generation 3
    entry("C293", Some(160), Classic, 3, SILVER),
    entry("C297", Some(160), Classic, 3, BLACK),
    // Nano generation 3
    entry("A978", Some(4), Nano, 3, SILVER),
    entry("A980", Some(8), Nano, 3, SILVER),
    entry("B261", Some(8), Nano, 3, BLACK),
    entry("B249", Some(8), Nano, 3, BLUE),
    entry("B253", Some(8), Nano, 3, GREEN),
    entry("B257", Some(8), Nano, 3, RED),
    // Nano generation 4
    entry("B480", Some(4), Nano, 4, SILVER),
    entry("B651", Some(4), Nano, 4, BLUE),
    entry("B654", Some(4), Nano, 4, PINK),
    entry("B657", Some(4), Nano, 4, PURPLE),
    entry("B660", Some(4), Nano, 4, ORANGE),
    entry("B663", Some(4), Nano, 4, GREEN),
    entry("B666", Some(4), Nano, 4, YELLOW),
    entry("B598", Some(8), Nano, 4, SILVER),
    entry("B732", Some(8), Nano, 4, BLUE),
    entry("B735", Some(8), Nano, 4, PINK),
    entry("B739", Some(8), Nano, 4, PURPLE),
    entry("B742", Some(8), Nano, 4, ORANGE),
    entry("B745", Some(8), Nano, 4, GREEN),
    entry("B748", Some(8), Nano, 4, YELLOW),
    entry("B751", Some(8), Nano, 4, RED),
    entry("B754", Some(8), Nano, 4, BLACK),
    entry("B903", Some(16), Nano, 4, SILVER),
    entry("B905", Some(16), Nano, 4, BLUE),
    entry("B907", Some(16), Nano, 4, PINK),
    entry("B909", Some(16), Nano, 4, PURPLE),
    entry("B911", Some(16), Nano, 4, ORANGE),
    entry("B913", Some(16), Nano, 4, GREEN),
    entry("B915", Some(16), Nano, 4, YELLOW),
    entry("B917", Some(16), Nano, 4, RED),
    entry("B918", Some(16), Nano, 4, BLACK),
    // Nano generation 5
    entry("C027", Some(8), Nano, 5, SILVER),
    entry("C031", Some(8), Nano, 5, BLACK),
    entry("C034", Some(8), Nano, 5, PURPLE),
    entry("C037", Some(8), Nano, 5, BLUE),
    entry("C040", Some(8), Nano, 5, GREEN),
    entry("C043", Some(8), Nano, 5, YELLOW),
    entry("C046", Some(8), Nano, 5, ORANGE),
    entry("C049", Some(8), Nano, 5, RED),
    entry("C050", Some(8), Nano, 5, PINK),
    entry("C060", Some(16), Nano, 5, SILVER),
    entry("C062", Some(16), Nano, 5, BLACK),
    entry("C064", Some(16), Nano, 5, PURPLE),
    entry("C066", Some(16), Nano, 5, BLUE),
    entry("C068", Some(16), Nano, 5, GREEN),
    entry("C070", Some(16), Nano, 5, YELLOW),
    entry("C072", Some(16), Nano, 5, ORANGE),
    entry("C074", Some(16), Nano, 5, RED),
    entry("C075", Some(16), Nano, 5, PINK),
    // Nano generation 6
    entry("C525", Some(8), Nano, 6, SILVER),
    entry("C688", Some(8), Nano, 6, BLACK),
    entry("C689", Some(8), Nano, 6, BLUE),
    entry("C690", Some(8), Nano, 6, GREEN),
    entry("C691", Some(8), Nano, 6, ORANGE),
    entry("C692", Some(8), Nano, 6, PINK),
    entry("C693", Some(8), Nano, 6, RED),
    entry("C526", Some(16), Nano, 6, SILVER),
    entry("C694", Some(16), Nano, 6, BLACK),
    entry("C695", Some(16), Nano, 6, BLUE),
    entry("C696", Some(16), Nano, 6, GREEN),
    entry("C697", Some(16), Nano, 6, ORANGE),
    entry("C698", Some(16), Nano, 6, PINK),
    entry("C699", Some(16), Nano, 6, RED),
];

/// libgpod's `serial_to_model_mapping`, iPod rows only: the last three characters of the serial.
const SERIAL_SUFFIXES: &[(&str, &str)] = &[
    ("LG6", "8541"),
    ("NAM", "8541"),
    ("MJ2", "8541"),
    ("ML1", "8709"),
    ("MME", "8709"),
    ("MMB", "8737"),
    ("MMC", "8738"),
];

/// Serial suffix wins over `ModelNumStr`, as in libgpod's `itdb_device_get_ipod_info`.
pub(crate) fn identify(serial: Option<&str>, model_num_str: Option<&str>) -> Option<IpodModel> {
    let by_serial = serial
        .filter(|serial| serial.len() >= 3)
        .and_then(|serial| serial.get(serial.len() - 3..))
        .and_then(|suffix| SERIAL_SUFFIXES.iter().find(|(known, _)| *known == suffix))
        .and_then(|(_, number)| lookup(number));
    by_serial.or_else(|| model_num_str.and_then(lookup_model_num_str))
}

/// libgpod skips a leading letter (`xA623`, `MB147`) before comparing; the bare number is accepted
/// too so a SysInfo that already omits the prefix still resolves.
fn lookup_model_num_str(model_num_str: &str) -> Option<IpodModel> {
    let trimmed = model_num_str.trim();
    lookup(trimmed).or_else(|| lookup(trimmed.get(1..)?))
}

fn lookup(number: &str) -> Option<IpodModel> {
    MODELS
        .iter()
        .find(|entry| entry.number == number)
        .map(|entry| IpodModel {
            family: entry.family,
            generation: entry.generation,
            name: display_name(entry.family, entry.generation),
            capacity_gb: entry.capacity_gb,
            color: entry.color.map(str::to_owned),
            model_number: entry.number.to_owned(),
        })
}

fn display_name(family: IpodFamily, generation: u8) -> String {
    match (family, generation) {
        (Original, generation) => format!("iPod ({} generation)", ordinal(generation)),
        (Mini, generation) => format!("iPod mini ({} generation)", ordinal(generation)),
        (Photo, _) => "iPod photo".to_owned(),
        (Video, 1) => "iPod video (5th generation)".to_owned(),
        (Video, _) => "iPod video (5.5th generation)".to_owned(),
        (Classic, 3) => "iPod classic (7th generation)".to_owned(),
        (Classic, _) => "iPod classic (6th generation)".to_owned(),
        (Nano, generation) => format!("iPod nano ({} generation)", ordinal(generation)),
        (Shuffle, generation) => format!("iPod shuffle ({} generation)", ordinal(generation)),
        (IpodFamily::Unknown, _) => "iPod".to_owned(),
    }
}

fn ordinal(n: u8) -> String {
    let suffix = match n {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// SysInfoExtended's `DBVersion` is authoritative when present, as in libgpod; otherwise the
/// checksum follows from the model.
pub(crate) fn sync_support(model: Option<&IpodModel>, db_version: Option<i64>) -> SyncSupport {
    match (db_version, model) {
        (Some(0..=2), _) => SyncSupport::Supported,
        (Some(3), _) => SyncSupport::NeedsHash58,
        (Some(4), _) => SyncSupport::NeedsHash72,
        (Some(_), _) | (None, None) => SyncSupport::Unsupported,
        (None, Some(model)) => match (model.family, model.generation) {
            (Classic, _) | (Nano, 3 | 4) => SyncSupport::NeedsHash58,
            (Nano, 5) => SyncSupport::NeedsHash72,
            (Nano, 1 | 2) | (Original | Mini | Photo | Video | Shuffle, _) => {
                SyncSupport::Supported
            }
            (Nano, _) | (IpodFamily::Unknown, _) => SyncSupport::Unsupported,
        },
    }
}
