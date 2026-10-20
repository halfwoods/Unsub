//! The `ArtworkDB` file: which `.ithmb` thumbnails belong to which track. Record layouts follow
//! libgpod's db-artwork-writer.c (`write_mhfd`, `write_mhii`, `write_mhni`, `write_mhod_type_3`,
//! `write_mhlf`) and the structs in db-itunes-parser.h.
//!
//! Existing images are kept as the bytes they were read as, so artwork this crate did not create
//! survives a sync unchanged.

use std::collections::HashSet;

use super::super::chunks::{Chunk, ListHead, MHLI, tag_name};
use super::formats::ArtworkFormat;
use crate::bytes::{put, u32_at};
use crate::error::{Error, Result};
use crate::itunesdb::mhod::u32_from;

const MHFD: [u8; 4] = *b"mhfd";
const MHSD: [u8; 4] = *b"mhsd";
const MHII: [u8; 4] = *b"mhii";

const MHFD_HEADER_LEN: usize = 0x84;
const MHSD_HEADER_LEN: usize = 0x60;
const LIST_HEADER_LEN: usize = 0x5c;
const MHII_HEADER_LEN: usize = 0x98;
const MHNI_HEADER_LEN: usize = 0x4c;
const MHIF_HEADER_LEN: usize = 0x7c;
/// An ArtworkDB mhod puts only its 24-byte base in `header_len`; its string payload follows the
/// larger 36-byte struct libgpod writes.
const MHOD_BASE_LEN: usize = 24;
const MHOD_STRING_HEADER_LEN: usize = 36;

const MHFD_NEXT_ID_AT: usize = 28;
const MHSD_INDEX_AT: usize = 12;
const IMAGE_ID_AT: usize = 16;
const IMAGE_SONG_ID_AT: usize = 20;

/// mhsd indexes: images, albums (photos only), and the list of thumbnail files.
const SECTION_IMAGES: u16 = 1;
const SECTION_ALBUMS: u16 = 2;
const SECTION_FILES: u16 = 3;

pub(super) struct Image {
    pub(super) id: u32,
    bytes: Vec<u8>,
}

/// Where one thumbnail of an existing image lives.
pub(super) struct ThumbRef {
    pub(super) file_name: String,
    pub(super) offset: u32,
    pub(super) size: u32,
}

impl Image {
    /// The thumbnails an image record points at, for checking them against the `.ithmb` files.
    pub(super) fn thumbs(&self) -> Vec<ThumbRef> {
        let data = &self.bytes;
        let mut thumbs = Vec::new();
        let Ok(image) = Chunk::read(data, 0, data.len()) else {
            return thumbs;
        };
        let mut at = image.header_end();
        while at < image.end {
            let Ok(container) = Chunk::read(data, at, image.end) else {
                break;
            };
            at = container.end;
            let Ok(mhni) = Chunk::read(data, container.header_end(), container.end) else {
                continue;
            };
            let name = mhni.header_end();
            let size = u32_at(data, name + 24).unwrap_or(0) as usize;
            let text =
                data.get(name + MHOD_STRING_HEADER_LEN..name + MHOD_STRING_HEADER_LEN + size);
            let units: Vec<u16> = text
                .unwrap_or_default()
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            thumbs.push(ThumbRef {
                file_name: String::from_utf16_lossy(&units),
                offset: mhni.u32(20).unwrap_or(0),
                size: mhni.u32(24).unwrap_or(0),
            });
        }
        thumbs
    }
}

enum Section {
    Images {
        mhsd: Vec<u8>,
        list: Vec<u8>,
        images: Vec<Image>,
    },
    /// The photo album and file lists; carried over untouched.
    Other(Vec<u8>),
}

pub(super) struct ArtworkDb {
    header: Vec<u8>,
    sections: Vec<Section>,
}

impl ArtworkDb {
    pub(super) fn parse(data: &[u8]) -> Result<Self> {
        let root = Chunk::read(data, 0, data.len())?;
        if root.tag != MHFD {
            return Err(Error::corrupt(
                0,
                format!("not an ArtworkDB: found {}", tag_name(root.tag)),
            ));
        }
        let mut sections = Vec::new();
        let mut offset = root.header_end();
        while offset < root.end {
            let section = Chunk::read(data, offset, root.end)?;
            offset = section.end;
            if section.tag != MHSD {
                continue;
            }
            let bytes = data
                .get(section.offset..section.end)
                .unwrap_or_default()
                .to_vec();
            if section.u16(MHSD_INDEX_AT) != Some(SECTION_IMAGES) {
                sections.push(Section::Other(bytes));
                continue;
            }
            let list = ListHead::find(data, &section, MHLI)?;
            let mut images = Vec::new();
            let mut at = list.items_offset;
            while images.len() < list.count as usize && at < section.end {
                let image = Chunk::read(data, at, section.end)?;
                at = image.end;
                if image.tag == MHII {
                    let id = image.u32(IMAGE_ID_AT).unwrap_or(0);
                    images.push(Image {
                        id,
                        bytes: data
                            .get(image.offset..image.end)
                            .unwrap_or_default()
                            .to_vec(),
                    });
                }
            }
            sections.push(Section::Images {
                mhsd: section.header().to_vec(),
                list: list.header(data).to_vec(),
                images,
            });
        }
        if !sections
            .iter()
            .any(|section| matches!(section, Section::Images { .. }))
        {
            return Err(Error::corrupt(0, "ArtworkDB has no image list"));
        }
        Ok(Self {
            header: root.header().to_vec(),
            sections,
        })
    }

    /// A new, empty ArtworkDB for a device that has none (libgpod's `write_mhfd`).
    pub(super) fn fresh(formats: &[ArtworkFormat]) -> Self {
        let mut header = chunk_header(b"mhfd", MHFD_HEADER_LEN);
        // iTunes 7 deletes an ArtworkDB whose field at +16 is 1; iTunes 4.9 and later write 2.
        put(&mut header, 16, &2u32.to_le_bytes());
        put(&mut header, 20, &3u32.to_le_bytes());
        put(&mut header, 48, &[2]);

        let section = |index: u16| {
            let mut mhsd = chunk_header(b"mhsd", MHSD_HEADER_LEN);
            put(&mut mhsd, MHSD_INDEX_AT, &index.to_le_bytes());
            mhsd
        };
        let mut files = chunk_header(b"mhlf", LIST_HEADER_LEN);
        put(&mut files, 8, &u32_from(formats.len()).to_le_bytes());
        let mut files_section = section(SECTION_FILES);
        files_section.append(&mut files);
        for format in formats {
            let mut mhif = chunk_header(b"mhif", MHIF_HEADER_LEN);
            put(&mut mhif, 8, &u32_from(MHIF_HEADER_LEN).to_le_bytes());
            put(&mut mhif, 16, &format.format_id.to_le_bytes());
            put(
                &mut mhif,
                20,
                &(format.width * format.height * 2).to_le_bytes(),
            );
            files_section.extend_from_slice(&mhif);
        }
        let length = u32_from(files_section.len());
        put(&mut files_section, 8, &length.to_le_bytes());

        let mut albums_section = section(SECTION_ALBUMS);
        albums_section.extend_from_slice(&chunk_header(b"mhla", LIST_HEADER_LEN));
        let length = u32_from(albums_section.len());
        put(&mut albums_section, 8, &length.to_le_bytes());

        Self {
            header,
            sections: vec![
                Section::Images {
                    mhsd: section(SECTION_IMAGES),
                    list: chunk_header(b"mhli", LIST_HEADER_LEN),
                    images: Vec::new(),
                },
                Section::Other(albums_section),
                Section::Other(files_section),
            ],
        }
    }

    pub(super) fn images(&self) -> impl Iterator<Item = &Image> {
        self.sections.iter().flat_map(|section| match section {
            Section::Images { images, .. } => images.as_slice(),
            Section::Other(_) => &[],
        })
    }

    pub(super) fn image_ids(&self) -> HashSet<u32> {
        self.sections
            .iter()
            .filter_map(|section| match section {
                Section::Images { images, .. } => Some(images),
                Section::Other(_) => None,
            })
            .flatten()
            .map(|image| image.id)
            .collect()
    }

    /// The database with `remove`d images dropped and `add`ed ones appended. Everything else is
    /// copied as it was; only lengths, the image count and the next free id are updated.
    pub(super) fn emit(&self, add: &[Vec<u8>], remove: &HashSet<u32>, next_id: u32) -> Vec<u8> {
        let mut out = self.header.clone();
        for section in &self.sections {
            match section {
                Section::Other(bytes) => out.extend_from_slice(bytes),
                Section::Images { mhsd, list, images } => {
                    let start = out.len();
                    out.extend_from_slice(mhsd);
                    let list_at = out.len();
                    out.extend_from_slice(list);
                    let kept = images.iter().filter(|image| !remove.contains(&image.id));
                    let mut count = 0u32;
                    for bytes in kept
                        .map(|image| image.bytes.as_slice())
                        .chain(add.iter().map(Vec::as_slice))
                    {
                        out.extend_from_slice(bytes);
                        count += 1;
                    }
                    put(&mut out, list_at + 8, &count.to_le_bytes());
                    let length = u32_from(out.len() - start);
                    put(&mut out, start + 8, &length.to_le_bytes());
                }
            }
        }
        let length = u32_from(out.len());
        put(&mut out, 8, &length.to_le_bytes());
        let stored = u32_at(&self.header, MHFD_NEXT_ID_AT).unwrap_or(0);
        put(
            &mut out,
            MHFD_NEXT_ID_AT,
            &stored.max(next_id).to_le_bytes(),
        );
        out
    }
}

/// Where one rendered thumbnail was stored.
pub(super) struct ThumbRecord {
    pub(super) format_id: u32,
    /// `:F1055_1.ithmb`
    pub(super) file_name: String,
    pub(super) offset: u32,
    pub(super) size: u32,
    pub(super) width: i16,
    pub(super) height: i16,
    pub(super) horizontal_padding: i16,
    pub(super) vertical_padding: i16,
}

/// An mhii and its thumbnails (libgpod's `write_mhii`). `source_len` is the size of the cover as
/// it came from the library; the image record stores it plus one, as libgpod does "for some
/// reason".
pub(super) fn encode_image(
    id: u32,
    song_id: u64,
    source_len: u32,
    created_mac: u32,
    thumbs: &[ThumbRecord],
) -> Vec<u8> {
    let mut image = chunk_header(b"mhii", MHII_HEADER_LEN);
    put(&mut image, 12, &u32_from(thumbs.len()).to_le_bytes());
    put(&mut image, IMAGE_ID_AT, &id.to_le_bytes());
    put(&mut image, IMAGE_SONG_ID_AT, &song_id.to_le_bytes());
    put(&mut image, 40, &created_mac.to_le_bytes());
    put(&mut image, 48, &source_len.saturating_add(1).to_le_bytes());
    for thumb in thumbs {
        image.extend_from_slice(&encode_thumb(thumb));
    }
    let length = u32_from(image.len());
    put(&mut image, 8, &length.to_le_bytes());
    image
}

/// An mhod (type 2) holding an mhni, which names the thumbnail file and where in it the pixels are.
fn encode_thumb(thumb: &ThumbRecord) -> Vec<u8> {
    let name = encode_file_name(&thumb.file_name);
    let mut mhni = chunk_header(b"mhni", MHNI_HEADER_LEN);
    put(&mut mhni, 12, &1u32.to_le_bytes());
    put(&mut mhni, 16, &thumb.format_id.to_le_bytes());
    put(&mut mhni, 20, &thumb.offset.to_le_bytes());
    put(&mut mhni, 24, &thumb.size.to_le_bytes());
    put(&mut mhni, 28, &thumb.vertical_padding.to_le_bytes());
    put(&mut mhni, 30, &thumb.horizontal_padding.to_le_bytes());
    put(&mut mhni, 32, &thumb.height.to_le_bytes());
    put(&mut mhni, 34, &thumb.width.to_le_bytes());
    mhni.extend_from_slice(&name);
    let length = u32_from(mhni.len());
    put(&mut mhni, 8, &length.to_le_bytes());

    let mut container = chunk_header(b"mhod", MHOD_BASE_LEN);
    put(&mut container, 12, &2u16.to_le_bytes());
    container.extend_from_slice(&mhni);
    let length = u32_from(container.len());
    put(&mut container, 8, &length.to_le_bytes());
    container
}

/// An mhod (type 3) holding the file name as UTF-16LE, padded to a multiple of four bytes.
fn encode_file_name(name: &str) -> Vec<u8> {
    let text: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let padding = (4 - (MHOD_STRING_HEADER_LEN + text.len()) % 4) % 4;
    let mut mhod = chunk_header(b"mhod", MHOD_STRING_HEADER_LEN);
    put(&mut mhod, 4, &u32_from(MHOD_BASE_LEN).to_le_bytes());
    put(&mut mhod, 12, &3u16.to_le_bytes());
    put(&mut mhod, 15, &[u8::try_from(padding).unwrap_or(0)]);
    put(&mut mhod, 24, &u32_from(text.len()).to_le_bytes());
    // Encoding 2 is UTF-16LE.
    put(&mut mhod, 28, &[2]);
    mhod.extend_from_slice(&text);
    mhod.extend(std::iter::repeat_n(0u8, padding));
    let length = u32_from(mhod.len());
    put(&mut mhod, 8, &length.to_le_bytes());
    mhod
}

fn chunk_header(tag: &[u8; 4], len: usize) -> Vec<u8> {
    let mut header = vec![0; len];
    put(&mut header, 0, tag);
    put(&mut header, 4, &u32_from(len).to_le_bytes());
    header
}
