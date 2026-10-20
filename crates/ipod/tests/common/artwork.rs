//! Test-side reading and writing of ArtworkDB and `.ithmb` files. It shares no code with the
//! writer under test, so a layout mistake cannot cancel out between the two.

use std::io::Cursor;

use image::{ImageFormat, Rgb, RgbImage};

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn i16_at(bytes: &[u8], at: usize) -> i16 {
    i16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumb {
    pub format_id: u32,
    pub file: String,
    pub offset: u32,
    pub size: u32,
    pub width: i16,
    pub height: i16,
    pub horizontal_padding: i16,
    pub vertical_padding: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub id: u32,
    pub song_id: u64,
    pub source_size: u32,
    pub thumbs: Vec<Thumb>,
    /// The mhii and everything inside it.
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Parsed {
    pub next_id: u32,
    pub sections: Vec<u16>,
    pub images: Vec<Image>,
    /// Format ids listed in the file list (mhsd 3), if any.
    pub listed_formats: Vec<u32>,
}

pub fn parse(bytes: &[u8]) -> Parsed {
    assert_eq!(&bytes[..4], b"mhfd");
    assert_eq!(
        u32_at(bytes, 8) as usize,
        bytes.len(),
        "mhfd length covers the file"
    );
    let mut parsed = Parsed {
        next_id: u32_at(bytes, 28),
        sections: vec![],
        images: vec![],
        listed_formats: vec![],
    };
    let mut at = u32_at(bytes, 4) as usize;
    while at < bytes.len() {
        assert_eq!(&bytes[at..at + 4], b"mhsd", "offset {at}");
        let end = at + u32_at(bytes, at + 8) as usize;
        let index = u16_at(bytes, at + 12);
        parsed.sections.push(index);
        let list = at + u32_at(bytes, at + 4) as usize;
        let count = u32_at(bytes, list + 8);
        let mut item = list + u32_at(bytes, list + 4) as usize;
        match index {
            1 => {
                for _ in 0..count {
                    let image = parse_image(bytes, item);
                    item += image.bytes.len();
                    parsed.images.push(image);
                }
            }
            3 => {
                for _ in 0..count {
                    assert_eq!(&bytes[item..item + 4], b"mhif");
                    parsed.listed_formats.push(u32_at(bytes, item + 16));
                    item += u32_at(bytes, item + 8) as usize;
                }
            }
            _ => {}
        }
        assert_eq!(item, end, "section {index} is tiled exactly by its items");
        at = end;
    }
    parsed
}

fn parse_image(bytes: &[u8], at: usize) -> Image {
    assert_eq!(&bytes[at..at + 4], b"mhii");
    let end = at + u32_at(bytes, at + 8) as usize;
    let children = u32_at(bytes, at + 12);
    let mut thumbs = Vec::new();
    let mut child = at + u32_at(bytes, at + 4) as usize;
    for _ in 0..children {
        assert_eq!(&bytes[child..child + 4], b"mhod");
        assert_eq!(u16_at(bytes, child + 12), 2, "a thumbnail container");
        let mhni = child + u32_at(bytes, child + 4) as usize;
        assert_eq!(&bytes[mhni..mhni + 4], b"mhni");
        let name_at = mhni + u32_at(bytes, mhni + 4) as usize;
        assert_eq!(&bytes[name_at..name_at + 4], b"mhod");
        assert_eq!(u16_at(bytes, name_at + 12), 3, "a file name");
        let text_len = u32_at(bytes, name_at + 24) as usize;
        assert_eq!(bytes[name_at + 28], 2, "UTF-16LE");
        let units: Vec<u16> = bytes[name_at + 36..name_at + 36 + text_len]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(
            u32_at(bytes, name_at + 8) as usize % 4,
            0,
            "name mhods are padded to 4 bytes"
        );
        thumbs.push(Thumb {
            format_id: u32_at(bytes, mhni + 16),
            file: String::from_utf16(&units).unwrap(),
            offset: u32_at(bytes, mhni + 20),
            size: u32_at(bytes, mhni + 24),
            vertical_padding: i16_at(bytes, mhni + 28),
            horizontal_padding: i16_at(bytes, mhni + 30),
            height: i16_at(bytes, mhni + 32),
            width: i16_at(bytes, mhni + 34),
        });
        child += u32_at(bytes, child + 8) as usize;
    }
    assert_eq!(child, end);
    Image {
        id: u32_at(bytes, at + 16),
        song_id: u64_at(bytes, at + 20),
        source_size: u32_at(bytes, at + 48),
        thumbs,
        bytes: bytes[at..end].to_vec(),
    }
}

/// Artwork as iTunes (not this crate) might have left it: odd values in the fields this crate
/// never interprets, which must survive.
pub struct ForeignImage {
    pub id: u32,
    pub song_id: u64,
    pub thumbs: Vec<Thumb>,
}

fn header(tag: &[u8; 4], len: usize) -> Vec<u8> {
    let mut header = vec![0; len];
    header[..4].copy_from_slice(tag);
    header[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    header
}

fn finish(mut chunk: Vec<u8>) -> Vec<u8> {
    let len = chunk.len() as u32;
    chunk[8..12].copy_from_slice(&len.to_le_bytes());
    chunk
}

pub fn foreign_database(images: &[ForeignImage], formats: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut list = header(b"mhli", 0x5c);
    list[8..12].copy_from_slice(&(images.len() as u32).to_le_bytes());
    let mut image_section = header(b"mhsd", 0x60);
    image_section[12] = 1;
    image_section.extend_from_slice(&list);
    for image in images {
        let mut mhii = header(b"mhii", 0x98);
        mhii[12..16].copy_from_slice(&(image.thumbs.len() as u32).to_le_bytes());
        mhii[16..20].copy_from_slice(&image.id.to_le_bytes());
        mhii[20..28].copy_from_slice(&image.song_id.to_le_bytes());
        // Fields this crate does not interpret, set to recognisable values.
        mhii[28..32].copy_from_slice(&0x1111_1111u32.to_le_bytes());
        mhii[32..36].copy_from_slice(&0x2222_2222u32.to_le_bytes());
        mhii[36..40].copy_from_slice(&0x3333_3333u32.to_le_bytes());
        mhii[40..44].copy_from_slice(&0x4444_4444u32.to_le_bytes());
        mhii[48..52].copy_from_slice(&0x5555_5555u32.to_le_bytes());
        for thumb in &image.thumbs {
            let name: Vec<u8> = thumb
                .file
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect();
            let padding = (4 - (36 + name.len()) % 4) % 4;
            let mut string = header(b"mhod", 36);
            string[4..8].copy_from_slice(&24u32.to_le_bytes());
            string[12] = 3;
            string[15] = padding as u8;
            string[24..28].copy_from_slice(&(name.len() as u32).to_le_bytes());
            string[28] = 2;
            string.extend_from_slice(&name);
            string.extend(std::iter::repeat_n(0u8, padding));
            let string = finish(string);

            let mut mhni = header(b"mhni", 0x4c);
            mhni[12..16].copy_from_slice(&1u32.to_le_bytes());
            mhni[16..20].copy_from_slice(&thumb.format_id.to_le_bytes());
            mhni[20..24].copy_from_slice(&thumb.offset.to_le_bytes());
            mhni[24..28].copy_from_slice(&thumb.size.to_le_bytes());
            mhni[28..30].copy_from_slice(&thumb.vertical_padding.to_le_bytes());
            mhni[30..32].copy_from_slice(&thumb.horizontal_padding.to_le_bytes());
            mhni[32..34].copy_from_slice(&thumb.height.to_le_bytes());
            mhni[34..36].copy_from_slice(&thumb.width.to_le_bytes());
            mhni.extend_from_slice(&string);
            let mhni = finish(mhni);

            let mut container = header(b"mhod", 24);
            container[12] = 2;
            container.extend_from_slice(&mhni);
            mhii.extend_from_slice(&finish(container));
        }
        image_section.extend_from_slice(&finish(mhii));
    }
    let image_section = finish(image_section);

    let mut albums = header(b"mhsd", 0x60);
    albums[12] = 2;
    albums.extend_from_slice(&header(b"mhla", 0x5c));
    let albums = finish(albums);

    let mut files_list = header(b"mhlf", 0x5c);
    files_list[8..12].copy_from_slice(&(formats.len() as u32).to_le_bytes());
    let mut files = header(b"mhsd", 0x60);
    files[12] = 3;
    files.extend_from_slice(&files_list);
    for &(format_id, width, height) in formats {
        let mut mhif = header(b"mhif", 0x7c);
        mhif[8..12].copy_from_slice(&0x7cu32.to_le_bytes());
        mhif[16..20].copy_from_slice(&format_id.to_le_bytes());
        mhif[20..24].copy_from_slice(&(width * height * 2).to_le_bytes());
        files.extend_from_slice(&mhif);
    }
    let files = finish(files);

    let mut root = header(b"mhfd", 0x84);
    root[16..20].copy_from_slice(&2u32.to_le_bytes());
    root[20..24].copy_from_slice(&3u32.to_le_bytes());
    let next = images
        .iter()
        .map(|image| image.id)
        .max()
        .map_or(0x64, |max| max + 1);
    root[28..32].copy_from_slice(&next.to_le_bytes());
    root[48] = 2;
    root.extend_from_slice(&image_section);
    root.extend_from_slice(&albums);
    root.extend_from_slice(&files);
    finish(root)
}

pub fn png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    encode(width, height, color, ImageFormat::Png)
}

pub fn jpeg(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    encode(width, height, color, ImageFormat::Jpeg)
}

fn encode(width: u32, height: u32, color: [u8; 3], format: ImageFormat) -> Vec<u8> {
    let mut bytes = Vec::new();
    RgbImage::from_pixel(width, height, Rgb(color))
        .write_to(&mut Cursor::new(&mut bytes), format)
        .unwrap();
    bytes
}
