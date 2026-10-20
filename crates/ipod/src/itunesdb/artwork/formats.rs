//! The thumbnail formats an iPod wants for album art. A device that ships a `SysInfoExtended` lists
//! them itself (`AlbumArt`); older ones fall back to libgpod's per-model table
//! (`ipod_*_cover_art_info` and `ipod_artwork_capabilities` in itdb_device.c).

use plist::{Dictionary, Value};

use crate::models::IpodFamily;

/// How a thumbnail's pixels are laid out. The cover-art formats of every iPod this crate writes to
/// are 16-bit RGB565; the other layouts libgpod knows are for photos and the iPhone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PixelFormat {
    Rgb565Le,
    Rgb565Be,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtworkFormat {
    pub(crate) format_id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: PixelFormat,
    /// Fills the space around an image that does not have the format's aspect ratio. The first
    /// three bytes are red, green and blue.
    pub(crate) back_color: [u8; 4],
    /// Rows are padded to a multiple of this many bytes; 0 for no padding.
    pub(crate) row_bytes_alignment: u32,
}

const BYTES_PER_PIXEL: u32 = 2;

impl ArtworkFormat {
    const fn builtin(format_id: u32, width: u32, height: u32) -> Self {
        Self {
            format_id,
            width,
            height,
            pixels: PixelFormat::Rgb565Le,
            back_color: [0; 4],
            row_bytes_alignment: 0,
        }
    }

    /// Pixels per stored row, including alignment padding (libgpod's `get_aligned_width`).
    pub(crate) fn row_pixels(&self) -> u32 {
        let alignment = self.row_bytes_alignment / BYTES_PER_PIXEL;
        if alignment != 0 && !self.width.is_multiple_of(alignment) {
            self.width + alignment - self.width % alignment
        } else {
            self.width
        }
    }

    /// Bytes one thumbnail occupies in its `.ithmb` file.
    pub(crate) fn slot_len(&self) -> u32 {
        self.row_pixels() * self.height * BYTES_PER_PIXEL
    }
}

/// libgpod's `g_value_to_image_format`: the `PixelFormat` string is the hex of a four-character
/// code, and entries with a layout this crate cannot write are dropped, as libgpod drops them.
fn from_dictionary(dict: &Dictionary) -> Option<ArtworkFormat> {
    let pixels = match dict.get("PixelFormat")?.as_string()? {
        "4C353635" /* L565 */ => PixelFormat::Rgb565Le,
        "42353635" /* B565 */ => PixelFormat::Rgb565Be,
        _ => return None,
    };
    let number = |key: &str| {
        dict.get(key)
            .and_then(Value::as_signed_integer)
            .and_then(|value| u32::try_from(value).ok())
    };
    let aligned_rows = dict.get("AlignRowBytes").and_then(Value::as_boolean) == Some(true);
    // A few devices (nano 3G) carry AlignRowBytes without RowBytesAlignment, meaning 4.
    let row_bytes_alignment = number("RowBytesAlignment")
        .filter(|&alignment| alignment != 0)
        .unwrap_or(if aligned_rows { 4 } else { 0 });
    let back_color = dict
        .get("BackColor")
        .and_then(Value::as_string)
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .map_or([0; 4], u32::to_be_bytes);
    Some(ArtworkFormat {
        format_id: number("FormatId")?,
        width: number("RenderWidth").filter(|&width| width != 0)?,
        height: number("RenderHeight").filter(|&height| height != 0)?,
        pixels,
        back_color,
        row_bytes_alignment,
    })
}

/// The album-art formats listed in a `SysInfoExtended`, if it lists any this crate can write.
pub(crate) fn from_sysinfo_extended(root: &Value) -> Option<Vec<ArtworkFormat>> {
    let dict = root.as_dictionary()?;
    let list = dict
        .get("AlbumArt")
        .or_else(|| dict.get("AlbumArt2"))?
        .as_array()?;
    let formats: Vec<ArtworkFormat> = list
        .iter()
        .filter_map(|entry| from_dictionary(entry.as_dictionary()?))
        .collect();
    (!formats.is_empty()).then_some(formats)
}

const PHOTO: [ArtworkFormat; 2] = [
    ArtworkFormat::builtin(1017, 56, 56),
    ArtworkFormat::builtin(1016, 140, 140),
];
const VIDEO: [ArtworkFormat; 2] = [
    ArtworkFormat::builtin(1028, 100, 100),
    ArtworkFormat::builtin(1029, 200, 200),
];
const NANO_1_2: [ArtworkFormat; 2] = [
    ArtworkFormat::builtin(1031, 42, 42),
    ArtworkFormat::builtin(1027, 100, 100),
];
/// iPod classic, which the 3rd-generation nano shares.
const CLASSIC: [ArtworkFormat; 4] = [
    ArtworkFormat::builtin(1061, 56, 56),
    ArtworkFormat::builtin(1055, 128, 128),
    ArtworkFormat::builtin(1068, 128, 128),
    ArtworkFormat::builtin(1060, 320, 320),
];
const NANO_4: [ArtworkFormat; 6] = [
    ArtworkFormat::builtin(1055, 128, 128),
    ArtworkFormat::builtin(1068, 128, 128),
    ArtworkFormat::builtin(1071, 240, 240),
    ArtworkFormat::builtin(1074, 50, 50),
    ArtworkFormat::builtin(1078, 80, 80),
    ArtworkFormat::builtin(1084, 240, 240),
];
const NANO_5: [ArtworkFormat; 4] = [
    ArtworkFormat::builtin(1056, 128, 128),
    ArtworkFormat::builtin(1078, 80, 80),
    ArtworkFormat::builtin(1073, 240, 240),
    ArtworkFormat::builtin(1074, 50, 50),
];

/// libgpod's per-model fallback. Models without album art (mini, shuffle, the original iPods)
/// get none.
pub(crate) fn for_model(family: IpodFamily, generation: u8) -> Vec<ArtworkFormat> {
    match (family, generation) {
        (IpodFamily::Photo, _) => PHOTO.to_vec(),
        (IpodFamily::Video, _) => VIDEO.to_vec(),
        (IpodFamily::Nano, 1 | 2) => NANO_1_2.to_vec(),
        (IpodFamily::Nano, 3) | (IpodFamily::Classic, _) => CLASSIC.to_vec(),
        (IpodFamily::Nano, 4) => NANO_4.to_vec(),
        (IpodFamily::Nano, 5) => NANO_5.to_vec(),
        _ => Vec::new(),
    }
}

/// Sparse artwork lets tracks of one album share a single image entry. libgpod turns it on for
/// every model from the 3rd-generation nano on, and for any device whose `SysInfoExtended` says so.
pub(crate) fn supports_sparse(listed: bool, family: IpodFamily, generation: u8) -> bool {
    listed
        || matches!(
            (family, generation),
            (IpodFamily::Classic, _) | (IpodFamily::Nano, 3..)
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary(entries: &[(&str, Value)]) -> Value {
        let mut dict = Dictionary::new();
        for (key, value) in entries {
            dict.insert((*key).to_owned(), value.clone());
        }
        Value::Dictionary(dict)
    }

    fn entry(id: i64, pixel_format: &str) -> Value {
        dictionary(&[
            ("FormatId", Value::Integer(id.into())),
            ("RenderWidth", Value::Integer(128.into())),
            ("RenderHeight", Value::Integer(96.into())),
            ("PixelFormat", Value::String(pixel_format.into())),
            ("RowBytesAlignment", Value::Integer(64.into())),
            ("BackColor", Value::String("FF0080FF".into())),
        ])
    }

    #[test]
    fn reads_the_formats_a_device_lists_and_skips_layouts_it_cannot_write() {
        let root = dictionary(&[(
            "AlbumArt",
            Value::Array(vec![
                entry(1055, "4C353635"),
                entry(1066, "79343230"),
                entry(1067, "42353635"),
            ]),
        )]);

        let formats = from_sysinfo_extended(&root).unwrap();

        assert_eq!(formats.len(), 2);
        assert_eq!(
            (formats[0].format_id, formats[0].pixels),
            (1055, PixelFormat::Rgb565Le)
        );
        assert_eq!(
            (formats[1].format_id, formats[1].pixels),
            (1067, PixelFormat::Rgb565Be)
        );
        assert_eq!(formats[0].back_color, [0xff, 0x00, 0x80, 0xff]);
        assert_eq!(formats[0].row_bytes_alignment, 64);
    }

    #[test]
    fn a_device_without_a_usable_list_has_none() {
        assert!(from_sysinfo_extended(&dictionary(&[])).is_none());
        let only_photos = dictionary(&[("AlbumArt", Value::Array(vec![entry(1, "79343230")]))]);
        assert!(from_sysinfo_extended(&only_photos).is_none());
    }

    #[test]
    fn align_row_bytes_without_an_alignment_means_four() {
        let dict = dictionary(&[
            ("FormatId", Value::Integer(1.into())),
            ("RenderWidth", Value::Integer(42.into())),
            ("RenderHeight", Value::Integer(42.into())),
            ("PixelFormat", Value::String("4C353635".into())),
            ("AlignRowBytes", Value::Boolean(true)),
        ]);

        let format = from_dictionary(dict.as_dictionary().unwrap()).unwrap();

        assert_eq!(format.row_bytes_alignment, 4);
        assert_eq!(
            format.row_pixels(),
            42,
            "42 pixels is already a multiple of two"
        );
    }

    #[test]
    fn rows_are_padded_to_the_alignment_and_slots_include_the_padding() {
        let format = ArtworkFormat {
            row_bytes_alignment: 64,
            ..ArtworkFormat::builtin(1, 50, 10)
        };

        assert_eq!(format.row_pixels(), 64);
        assert_eq!(format.slot_len(), 64 * 10 * 2);
        assert_eq!(ArtworkFormat::builtin(1061, 56, 56).slot_len(), 56 * 56 * 2);
    }

    #[test]
    fn the_fallback_table_follows_libgpod_for_each_family() {
        let ids = |family, generation| -> Vec<u32> {
            for_model(family, generation)
                .iter()
                .map(|format| format.format_id)
                .collect()
        };

        assert_eq!(ids(IpodFamily::Classic, 3), [1061, 1055, 1068, 1060]);
        assert_eq!(ids(IpodFamily::Nano, 3), [1061, 1055, 1068, 1060]);
        assert_eq!(
            ids(IpodFamily::Nano, 4),
            [1055, 1068, 1071, 1074, 1078, 1084]
        );
        assert_eq!(ids(IpodFamily::Nano, 5), [1056, 1078, 1073, 1074]);
        assert_eq!(ids(IpodFamily::Nano, 2), [1031, 1027]);
        assert_eq!(ids(IpodFamily::Video, 2), [1028, 1029]);
        assert_eq!(ids(IpodFamily::Photo, 1), [1017, 1016]);
        assert!(ids(IpodFamily::Mini, 1).is_empty() && ids(IpodFamily::Original, 4).is_empty());
    }

    #[test]
    fn sparse_artwork_is_on_for_classic_and_later_nanos_or_when_listed() {
        assert!(supports_sparse(false, IpodFamily::Classic, 1));
        assert!(supports_sparse(false, IpodFamily::Nano, 4));
        assert!(!supports_sparse(false, IpodFamily::Nano, 2));
        assert!(!supports_sparse(false, IpodFamily::Video, 2));
        assert!(supports_sparse(true, IpodFamily::Video, 2));
    }
}
