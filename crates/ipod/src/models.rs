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
    /// How many `iPod_Control/Music/Fnn` folders the firmware looks in.
    music_dirs: u8,
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
    music_dirs: u8,
) -> Entry {
    Entry {
        number,
        capacity_gb,
        family,
        generation,
        color,
        music_dirs,
    }
}

/// libgpod's `ipod_info_table`, with its last column as `music_dirs`. The 512 MB shuffle has no
/// `capacity_gb` because it is below 1 GB.
const MODELS: &[Entry] = &[
    // Original generation 1
    entry("8513", Some(5), Original, 1, NONE, 20),
    entry("8541", Some(5), Original, 1, NONE, 20),
    entry("8697", Some(5), Original, 1, NONE, 20),
    entry("8709", Some(10), Original, 1, NONE, 20),
    // Original generation 2
    entry("8737", Some(10), Original, 2, NONE, 20),
    entry("8740", Some(10), Original, 2, NONE, 20),
    entry("8738", Some(20), Original, 2, NONE, 50),
    entry("8741", Some(20), Original, 2, NONE, 50),
    // Original generation 3
    entry("8976", Some(10), Original, 3, NONE, 20),
    entry("8946", Some(15), Original, 3, NONE, 50),
    entry("9460", Some(15), Original, 3, NONE, 50),
    entry("9244", Some(20), Original, 3, NONE, 50),
    entry("8948", Some(30), Original, 3, NONE, 50),
    entry("9245", Some(40), Original, 3, NONE, 50),
    // Original generation 4
    entry("9282", Some(20), Original, 4, NONE, 50),
    entry("9787", Some(25), Original, 4, U2, 50),
    entry("9268", Some(40), Original, 4, NONE, 50),
    // Mini generation 1
    entry("9160", Some(4), Mini, 1, SILVER, 6),
    entry("9436", Some(4), Mini, 1, BLUE, 6),
    entry("9435", Some(4), Mini, 1, PINK, 6),
    entry("9434", Some(4), Mini, 1, GREEN, 6),
    entry("9437", Some(4), Mini, 1, GOLD, 6),
    // Mini generation 2
    entry("9800", Some(4), Mini, 2, SILVER, 6),
    entry("9802", Some(4), Mini, 2, BLUE, 6),
    entry("9804", Some(4), Mini, 2, PINK, 6),
    entry("9806", Some(4), Mini, 2, GREEN, 6),
    entry("9801", Some(6), Mini, 2, SILVER, 20),
    entry("9803", Some(6), Mini, 2, BLUE, 20),
    entry("9805", Some(6), Mini, 2, PINK, 20),
    entry("9807", Some(6), Mini, 2, GREEN, 20),
    // Photo generation 1
    entry("A079", Some(20), Photo, 1, NONE, 50),
    entry("A127", Some(20), Photo, 1, U2, 50),
    entry("9829", Some(30), Photo, 1, NONE, 50),
    entry("9585", Some(40), Photo, 1, NONE, 50),
    entry("9830", Some(60), Photo, 1, NONE, 50),
    entry("9586", Some(60), Photo, 1, NONE, 50),
    // Shuffle generation 1
    entry("9724", None, Shuffle, 1, NONE, 3),
    entry("9725", Some(1), Shuffle, 1, NONE, 3),
    // Shuffle generation 2
    entry("A546", Some(1), Shuffle, 2, SILVER, 3),
    entry("A947", Some(1), Shuffle, 2, PINK, 3),
    entry("A949", Some(1), Shuffle, 2, BLUE, 3),
    entry("A951", Some(1), Shuffle, 2, GREEN, 3),
    entry("A953", Some(1), Shuffle, 2, ORANGE, 3),
    entry("C167", Some(1), Shuffle, 2, GOLD, 3),
    entry("B225", Some(1), Shuffle, 2, SILVER, 3),
    entry("B233", Some(1), Shuffle, 2, PURPLE, 3),
    entry("B231", Some(1), Shuffle, 2, RED, 3),
    entry("B227", Some(1), Shuffle, 2, BLUE, 3),
    entry("B228", Some(1), Shuffle, 2, BLUE, 3),
    entry("B229", Some(1), Shuffle, 2, GREEN, 3),
    entry("B518", Some(2), Shuffle, 2, SILVER, 3),
    entry("B520", Some(2), Shuffle, 2, BLUE, 3),
    entry("B522", Some(2), Shuffle, 2, GREEN, 3),
    entry("B524", Some(2), Shuffle, 2, RED, 3),
    entry("B526", Some(2), Shuffle, 2, PURPLE, 3),
    // Shuffle generation 3
    entry("C306", Some(2), Shuffle, 3, SILVER, 3),
    entry("C323", Some(2), Shuffle, 3, BLACK, 3),
    entry("C381", Some(2), Shuffle, 3, GREEN, 3),
    entry("C384", Some(2), Shuffle, 3, BLUE, 3),
    entry("C387", Some(2), Shuffle, 3, PINK, 3),
    entry("B867", Some(4), Shuffle, 3, SILVER, 3),
    entry("C164", Some(4), Shuffle, 3, BLACK, 3),
    entry("C303", Some(4), Shuffle, 3, STAINLESS, 3),
    entry("C307", Some(4), Shuffle, 3, GREEN, 3),
    entry("C328", Some(4), Shuffle, 3, BLUE, 3),
    entry("C331", Some(4), Shuffle, 3, PINK, 3),
    // Shuffle generation 4
    entry("C584", Some(2), Shuffle, 4, SILVER, 3),
    entry("C585", Some(2), Shuffle, 4, PINK, 3),
    entry("C749", Some(2), Shuffle, 4, ORANGE, 3),
    entry("C750", Some(2), Shuffle, 4, GREEN, 3),
    entry("C751", Some(2), Shuffle, 4, BLUE, 3),
    // Nano generation 1
    entry("A350", Some(1), Nano, 1, WHITE, 3),
    entry("A352", Some(1), Nano, 1, BLACK, 3),
    entry("A004", Some(2), Nano, 1, WHITE, 3),
    entry("A099", Some(2), Nano, 1, BLACK, 3),
    entry("A005", Some(4), Nano, 1, WHITE, 6),
    entry("A107", Some(4), Nano, 1, BLACK, 6),
    // Video generation 1
    entry("A002", Some(30), Video, 1, WHITE, 50),
    entry("A146", Some(30), Video, 1, BLACK, 50),
    entry("A003", Some(60), Video, 1, WHITE, 50),
    entry("A147", Some(60), Video, 1, BLACK, 50),
    entry("A452", Some(30), Video, 1, U2, 50),
    // Video generation 2
    entry("A444", Some(30), Video, 2, WHITE, 50),
    entry("A446", Some(30), Video, 2, BLACK, 50),
    entry("A664", Some(30), Video, 2, U2, 50),
    entry("A448", Some(80), Video, 2, WHITE, 50),
    entry("A450", Some(80), Video, 2, BLACK, 50),
    // Nano generation 2
    entry("A477", Some(2), Nano, 2, SILVER, 3),
    entry("A426", Some(4), Nano, 2, SILVER, 6),
    entry("A428", Some(4), Nano, 2, BLUE, 6),
    entry("A487", Some(4), Nano, 2, GREEN, 6),
    entry("A489", Some(4), Nano, 2, PINK, 6),
    entry("A725", Some(4), Nano, 2, RED, 6),
    entry("A726", Some(8), Nano, 2, RED, 6),
    entry("A497", Some(8), Nano, 2, BLACK, 14),
    // Original generation 4
    entry("E436", Some(40), Original, 4, NONE, 50),
    // Photo generation 1
    entry("S492", Some(30), Photo, 1, NONE, 50),
    // Classic generation 1
    entry("B029", Some(80), Classic, 1, SILVER, 50),
    entry("B147", Some(80), Classic, 1, BLACK, 50),
    entry("B145", Some(160), Classic, 1, SILVER, 50),
    entry("B150", Some(160), Classic, 1, BLACK, 50),
    // Classic generation 2
    entry("B562", Some(120), Classic, 2, SILVER, 50),
    entry("B565", Some(120), Classic, 2, BLACK, 50),
    // Classic generation 3
    entry("C293", Some(160), Classic, 3, SILVER, 50),
    entry("C297", Some(160), Classic, 3, BLACK, 50),
    // Nano generation 3
    entry("A978", Some(4), Nano, 3, SILVER, 6),
    entry("A980", Some(8), Nano, 3, SILVER, 14),
    entry("B261", Some(8), Nano, 3, BLACK, 14),
    entry("B249", Some(8), Nano, 3, BLUE, 14),
    entry("B253", Some(8), Nano, 3, GREEN, 14),
    entry("B257", Some(8), Nano, 3, RED, 14),
    // Nano generation 4
    entry("B480", Some(4), Nano, 4, SILVER, 14),
    entry("B651", Some(4), Nano, 4, BLUE, 14),
    entry("B654", Some(4), Nano, 4, PINK, 14),
    entry("B657", Some(4), Nano, 4, PURPLE, 14),
    entry("B660", Some(4), Nano, 4, ORANGE, 14),
    entry("B663", Some(4), Nano, 4, GREEN, 14),
    entry("B666", Some(4), Nano, 4, YELLOW, 14),
    entry("B598", Some(8), Nano, 4, SILVER, 14),
    entry("B732", Some(8), Nano, 4, BLUE, 14),
    entry("B735", Some(8), Nano, 4, PINK, 14),
    entry("B739", Some(8), Nano, 4, PURPLE, 14),
    entry("B742", Some(8), Nano, 4, ORANGE, 14),
    entry("B745", Some(8), Nano, 4, GREEN, 14),
    entry("B748", Some(8), Nano, 4, YELLOW, 14),
    entry("B751", Some(8), Nano, 4, RED, 14),
    entry("B754", Some(8), Nano, 4, BLACK, 14),
    entry("B903", Some(16), Nano, 4, SILVER, 14),
    entry("B905", Some(16), Nano, 4, BLUE, 14),
    entry("B907", Some(16), Nano, 4, PINK, 14),
    entry("B909", Some(16), Nano, 4, PURPLE, 14),
    entry("B911", Some(16), Nano, 4, ORANGE, 14),
    entry("B913", Some(16), Nano, 4, GREEN, 14),
    entry("B915", Some(16), Nano, 4, YELLOW, 14),
    entry("B917", Some(16), Nano, 4, RED, 14),
    entry("B918", Some(16), Nano, 4, BLACK, 14),
    // Nano generation 5
    entry("C027", Some(8), Nano, 5, SILVER, 14),
    entry("C031", Some(8), Nano, 5, BLACK, 14),
    entry("C034", Some(8), Nano, 5, PURPLE, 14),
    entry("C037", Some(8), Nano, 5, BLUE, 14),
    entry("C040", Some(8), Nano, 5, GREEN, 14),
    entry("C043", Some(8), Nano, 5, YELLOW, 14),
    entry("C046", Some(8), Nano, 5, ORANGE, 14),
    entry("C049", Some(8), Nano, 5, RED, 14),
    entry("C050", Some(8), Nano, 5, PINK, 14),
    entry("C060", Some(16), Nano, 5, SILVER, 14),
    entry("C062", Some(16), Nano, 5, BLACK, 14),
    entry("C064", Some(16), Nano, 5, PURPLE, 14),
    entry("C066", Some(16), Nano, 5, BLUE, 14),
    entry("C068", Some(16), Nano, 5, GREEN, 14),
    entry("C070", Some(16), Nano, 5, YELLOW, 14),
    entry("C072", Some(16), Nano, 5, ORANGE, 14),
    entry("C074", Some(16), Nano, 5, RED, 14),
    entry("C075", Some(16), Nano, 5, PINK, 14),
    // Nano generation 6
    entry("C525", Some(8), Nano, 6, SILVER, 14),
    entry("C688", Some(8), Nano, 6, BLACK, 14),
    entry("C689", Some(8), Nano, 6, BLUE, 14),
    entry("C690", Some(8), Nano, 6, GREEN, 14),
    entry("C691", Some(8), Nano, 6, ORANGE, 14),
    entry("C692", Some(8), Nano, 6, PINK, 14),
    entry("C693", Some(8), Nano, 6, RED, 14),
    entry("C526", Some(16), Nano, 6, SILVER, 14),
    entry("C694", Some(16), Nano, 6, BLACK, 14),
    entry("C695", Some(16), Nano, 6, BLUE, 14),
    entry("C696", Some(16), Nano, 6, GREEN, 14),
    entry("C697", Some(16), Nano, 6, ORANGE, 14),
    entry("C698", Some(16), Nano, 6, PINK, 14),
    entry("C699", Some(16), Nano, 6, RED, 14),
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

/// How many `Fnn` music folders the model has, which libgpod's `itdb_create_directories` makes
/// on a blank iPod. `None` for a model number this table does not list.
pub(crate) fn music_dirs(model_number: &str) -> Option<usize> {
    let entry = MODELS.iter().find(|entry| entry.number == model_number)?;
    Some(usize::from(entry.music_dirs))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_model_has_the_number_of_music_folders_libgpod_gives_it() {
        assert_eq!(music_dirs("C293"), Some(50), "classic");
        assert_eq!(music_dirs("A446"), Some(50), "video");
        assert_eq!(music_dirs("9800"), Some(6), "mini");
        assert_eq!(music_dirs("B518"), Some(3), "shuffle 2G");
        assert_eq!(music_dirs("A350"), Some(3), "nano 1G");
        assert_eq!(music_dirs("8513"), Some(20), "the first iPod");
        assert_eq!(music_dirs("ZZZZ"), None);
    }
}
