//! Offline experimental data, never an installer or a firmware compatibility claim.
use std::collections::BTreeMap;
use std::io::{self, Cursor, Seek, SeekFrom, Write};

use image::{ImageFormat, Rgb, RgbImage};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use unsub_ipod::{ChecksumCheck, IpodFamily, verify_roundtrip};

use super::*;
use crate::common::artwork as art;

const MODEL: &str = "xC293";
const GUID: &str = "0x000A27001A2B3C4D";
const VIRTUAL_OWNER: u64 = 0x4152_5450_524f_4245;
const FORMATS: [(u32, [u8; 3]); 4] = [
    (1061, [255, 0, 0]),
    (1055, [0, 255, 0]),
    (1068, [0, 0, 255]),
    (1060, [255, 255, 0]),
];

fn new_output(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other(
            "use an absolute path to a new offline directory",
        ));
    }
    let parent = fs::canonicalize(
        path.parent()
            .ok_or_else(|| io::Error::other("output has no parent"))?,
    )?;
    if parent.ancestors().any(|p| p.join("iPod_Control").exists()) {
        return Err(io::Error::other(
            "probe output must be outside every iPod directory",
        ));
    }
    // create_dir refuses existing files/directories/symlinks; nothing there can be overwritten.
    fs::create_dir(path)
}

fn copy_fixture(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text.ends_with(".unsub-backup") || name_text.ends_with(".unsub-original") {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::other("unexpected symlink in generated fixture"));
        }
        if kind.is_dir() {
            copy_fixture(&entry.path(), &to.join(name))?;
        } else {
            fs::copy(entry.path(), to.join(name))?;
        }
    }
    Ok(())
}

fn hashes(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|p| p.as_os_str().to_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(
                    relative,
                    format!("{:x}", Sha256::digest(fs::read(path).unwrap())),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn wav(frequency: f64) -> Vec<u8> {
    let rate = 44_100u32;
    let samples = rate * 3;
    let data_size = samples * 2;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + data_size).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_size.to_le_bytes());
    for sample in 0..samples {
        let phase = f64::from(sample) * frequency * std::f64::consts::TAU / f64::from(rate);
        bytes.extend(((phase.sin() * 1500.0) as i16).to_le_bytes());
    }
    bytes
}

fn pattern(format: u32, width: u32, height: u32) -> RgbImage {
    let color = FORMATS.iter().find(|(id, _)| *id == format).unwrap().1;
    let mut image = RgbImage::from_pixel(width, height, Rgb(color));
    // Format numbers are labels, not guessed screen roles. A tiny bitmap font avoids a new dependency.
    const DIGITS: [[u8; 7]; 10] = [
        [14, 17, 19, 21, 25, 17, 14],
        [4, 12, 4, 4, 4, 4, 14],
        [14, 17, 1, 2, 4, 8, 31],
        [30, 1, 1, 14, 1, 1, 30],
        [2, 6, 10, 18, 31, 2, 2],
        [31, 16, 16, 30, 1, 1, 30],
        [14, 16, 16, 30, 17, 17, 14],
        [31, 1, 2, 4, 8, 8, 8],
        [14, 17, 17, 14, 17, 17, 14],
        [14, 17, 17, 15, 1, 1, 14],
    ];
    let label = format.to_string();
    let scale = (width / (label.len() as u32 * 6 + 2)).max(1);
    let left = (width - label.len() as u32 * 6 * scale) / 2;
    let top = (height - 7 * scale) / 2;
    let ink = Rgb(if format == 1060 || format == 1055 {
        [0, 0, 0]
    } else {
        [255, 255, 255]
    });
    for (digit, character) in label.bytes().enumerate() {
        for (y, row) in DIGITS[(character - b'0') as usize].iter().enumerate() {
            for x in 0..5u32 {
                if row & (1 << (4 - x)) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            image.put_pixel(
                                left + (digit as u32 * 6 + x) * scale + dx,
                                top + y as u32 * scale + dy,
                                ink,
                            );
                        }
                    }
                }
            }
        }
    }
    image
}

fn rgb565(image: &RgbImage) -> Vec<u8> {
    image
        .pixels()
        .flat_map(|pixel| {
            let [r, g, b] = pixel.0;
            (((u16::from(r) >> 3) << 11) | ((u16::from(g) >> 2) << 5) | (u16::from(b) >> 3))
                .to_le_bytes()
        })
        .collect()
}

fn art_path(fake: &FakeIpod, thumb: &art::Thumb) -> PathBuf {
    let name = thumb.file.trim_start_matches(':');
    assert_eq!(
        Path::new(name).components().count(),
        1,
        "generated filename only"
    );
    artwork::art_dir(fake).join(name)
}

fn coded_formats(fake: &FakeIpod) {
    let parsed = art::parse(&artwork::artwork_db_bytes(fake));
    for thumb in &parsed.images[0].thumbs {
        let pixels = rgb565(&pattern(
            thumb.format_id,
            thumb.width as u32,
            thumb.height as u32,
        ));
        assert_eq!(pixels.len(), thumb.size as usize);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(art_path(fake, thumb))
            .unwrap();
        file.seek(SeekFrom::Start(u64::from(thumb.offset))).unwrap();
        file.write_all(&pixels).unwrap();
    }
}

fn album_supplier(fake: &FakeIpod, owner: u64) {
    let mut bytes = db_bytes(fake);
    let albums = db::chunks(&bytes, b"mhia", 88);
    assert_eq!(albums.len(), 1);
    bytes[albums[0] + 32..albums[0] + 40].copy_from_slice(&owner.to_le_bytes());
    fs::write(fake.mount.join(DB_FILE), bytes).unwrap();
}

fn virtual_artwork(fake: &FakeIpod) {
    let mut bytes = artwork::artwork_db_bytes(fake);
    let parsed = art::parse(&bytes);
    let mut extra = parsed.images[0].bytes.clone();
    extra[16..20].copy_from_slice(&parsed.next_id.to_le_bytes());
    extra[20..28].copy_from_slice(&VIRTUAL_OWNER.to_le_bytes());
    for at in db::chunks(&extra, b"mhni", 0x4c) {
        let format = db::u32_at(&extra, at + 16);
        let thumb = parsed.images[0]
            .thumbs
            .iter()
            .find(|t| t.format_id == format)
            .unwrap();
        let path = art_path(fake, thumb);
        let offset = fs::metadata(&path).unwrap().len();
        let pixels = vec![0xf81fu16.to_le_bytes(); (thumb.size / 2) as usize].concat();
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(&pixels)
            .unwrap();
        extra[at + 20..at + 24].copy_from_slice(&u32::try_from(offset).unwrap().to_le_bytes());
    }
    let section = db::chunks(&bytes, b"mhsd", 0x60)
        .into_iter()
        .find(|at| db::u32_at(&bytes, at + 12) == 1)
        .unwrap();
    let section_size = db::u32_at(&bytes, section + 8);
    let list = section + 0x60;
    let count = db::u32_at(&bytes, list + 8);
    let end = section + section_size as usize;
    let added = extra.len() as u32;
    bytes.splice(end..end, extra);
    let total = bytes.len() as u32;
    bytes[8..12].copy_from_slice(&total.to_le_bytes());
    bytes[28..32].copy_from_slice(&(parsed.next_id + 1).to_le_bytes());
    bytes[section + 8..section + 12].copy_from_slice(&(section_size + added).to_le_bytes());
    bytes[list + 8..list + 12].copy_from_slice(&(count + 1).to_le_bytes());
    fs::write(artwork::art_dir(fake).join("ArtworkDB"), bytes).unwrap();
    album_supplier(fake, VIRTUAL_OWNER);
}

fn export_bundle(output: &Path, model: &str, guid: &str) -> io::Result<Value> {
    let hex = guid.strip_prefix("0x").unwrap_or(guid);
    if model.is_empty()
        || !model.bytes().all(|c| c.is_ascii_alphanumeric())
        || hex.len() != 16
        || !hex.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(io::Error::other(
            "use a Classic model number and a 16-digit FireWire GUID",
        ));
    }
    let baseline = FakeIpod::new("Offline Classic");
    baseline.write_sysinfo(model, guid);
    if opened(&baseline)
        .model
        .as_ref()
        .is_none_or(|m| m.family != IpodFamily::Classic)
    {
        return Err(io::Error::other("this probe targets Classic models only"));
    }
    new_output(output)?;
    let library = tempfile::tempdir()?;
    let tracks: Vec<_> = [
        ([255, 0, 0], "Red"),
        ([0, 255, 0], "Green"),
        ([0, 0, 255], "Blue"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (color, title))| {
        let id = index as i64 + 1;
        let mut track = library_track(library.path(), id, &format!("Artwork probe {id} — {title}"));
        track.source_path = library.path().join(format!("{id}.wav"));
        fs::write(&track.source_path, wav(220.0 + index as f64 * 110.0)).unwrap();
        track.artist = "Unsub diagnostic".into();
        track.album_artist = track.artist.clone();
        track.album = "Classic Cover Flow probe".into();
        track.duration_ms = 3000;
        track.bitrate_kbps = Some(706);
        track.track_count = Some(3);
        register_cover(&mut track, art::png(320, 320, color));
        track
    })
    .collect();
    let (_, database) = sync(&baseline, &tracks, &[]);
    assert_eq!(database.tracks.len(), 3);
    assert!(!database.tracks.iter().any(|t| t.dbid == VIRTUAL_OWNER));
    let mut cases = Vec::new();
    for name in ["control", "format-coded", "live-supplier", "virtual-owner"] {
        let fake = FakeIpod::new("Offline Classic");
        copy_fixture(&baseline.mount, &fake.mount)?;
        match name {
            "format-coded" => coded_formats(&fake),
            "live-supplier" => album_supplier(&fake, database.tracks[1].dbid),
            "virtual-owner" => virtual_artwork(&fake),
            _ => {}
        }
        // The ordinary writer reseals our OWN generated data and proves the selector survives
        // another sync. No real mount or user-supplied database is accepted by this builder.
        if name != "control" {
            sync(&fake, &tracks, &[]);
        }
        let report = verify_roundtrip(&opened(&fake)).unwrap();
        assert_eq!(report.checksum, ChecksumCheck::Verified);
        assert!(
            report.is_lossless(),
            "{name} failed the read-only roundtrip check"
        );
        let case_dir = output.join(name);
        copy_fixture(&fake.mount, &case_dir)?;
        // Fixture library IDs have no relationship to a tester's real desktop library.
        fs::remove_file(case_dir.join("iPod_Control/iTunes/Unsub.json"))?;
        cases.push(json!({ "name": name, "files": hashes(&case_dir), "checksumVerified": true }));
    }
    let legend = output.join("legend");
    fs::create_dir(&legend)?;
    for (format, _) in FORMATS {
        let mut bytes = Cursor::new(Vec::new());
        pattern(format, 128, 128)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        fs::write(legend.join(format!("F{format}.png")), bytes.into_inner())?;
    }
    fs::write(
        output.join("README.txt"),
        "OFFLINE DIAGNOSTIC ONLY — NOT AN INSTALLER\n\nNo connected iPod or pre-existing music file was modified; all music here was generated. The default identity is synthetic. Do not copy these databases onto a populated iPod. Hardware testing needs a spare/empty device, its actual model/GUID, a full backup and a controlled installation/recovery procedure. Nothing here installs automatically. Unsub sync-ownership mappings and generated backup files are excluded.\n\nEach case has exactly three playable WAV tracks with red, green and blue song art.\ncontrol: baseline generated by normal Unsub sync.\nformat-coded: only the first song's thumbnail pixels change; colors/numbers identify format IDs, not screen roles. Compare Cover Flow, Now Playing, expanded artwork and album menus.\nlive-supplier: MHIA points at the green song. Establish whether this firmware honors the supplier pointer before interpreting a failed virtual-owner case.\nvirtual-owner: MHIA points at a purple artwork item with no Track. If it displays as the album cover while all three songs keep their own art, this candidate works on that tested firmware only. Failure alone does not establish impossibility.\n\nNative 2.0.4 analysis found that its importer skips the iTunesDB album dataset; live-supplier and virtual-owner are therefore expected to be ignored on that inspected path. See docs/CLASSIC_COVER_FLOW_ARTWORK.md for the exact carrier hash and limits.\n\nUse a clean test dataset and a cold boot between cases so cached artwork cannot be mistaken for a result. Prevent iTunes/other sync tools from rewriting a case; hash the readback before interpreting it. Record exact model, firmware/modification/version, screen photos and every observation. No firmware consumer behavior has been verified by this offline fixture.\n",
    )?;
    let manifest = json!({
        "version": 1, "installable": false, "firmwareDisplayVerified": false,
        "model": model, "firewireGuid": guid, "virtualOwner": format!("{VIRTUAL_OWNER:016x}"),
        "trackCount": 3, "cases": cases,
        "tracks": database.tracks.iter().map(|track| json!({ "title": track.title, "persistentId": format!("{:016x}", track.dbid), "file": track.path.strip_prefix(&baseline.mount).unwrap().to_string_lossy().replace('\\', "/") })).collect::<Vec<_>>()
    });
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )?;
    Ok(manifest)
}

#[test]
fn offline_probe_has_no_extra_track_and_preserves_all_audio() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("probe");
    let manifest = export_bundle(&output, MODEL, GUID).unwrap();
    assert_eq!(manifest["installable"], false);
    let media: Vec<_> = manifest["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| track["file"].as_str().unwrap())
        .collect();
    let control = hashes(&output.join("control"));
    let control_art =
        art::parse(&fs::read(output.join("control/iPod_Control/Artwork/ArtworkDB")).unwrap());
    let slot_bytes = |case: &str, thumb: &art::Thumb| {
        let bytes = fs::read(
            output
                .join(case)
                .join("iPod_Control/Artwork")
                .join(thumb.file.trim_start_matches(':')),
        )
        .unwrap();
        bytes[thumb.offset as usize..(thumb.offset + thumb.size) as usize].to_vec()
    };
    for case in manifest["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let files = hashes(&output.join(name));
        assert!(
            !output
                .join(name)
                .join("iPod_Control/iTunes/Unsub.json")
                .exists()
        );
        for path in &media {
            assert_eq!(files[*path], control[*path], "{name} changed audio");
        }
        let parsed = art::parse(
            &fs::read(output.join(name).join("iPod_Control/Artwork/ArtworkDB")).unwrap(),
        );
        assert_eq!(
            parsed.images.len(),
            if name == "virtual-owner" { 4 } else { 3 }
        );
        for original in &control_art.images {
            let current = parsed
                .images
                .iter()
                .find(|image| image.song_id == original.song_id)
                .unwrap();
            assert_eq!(current.thumbs, original.thumbs);
            for thumb in &current.thumbs {
                if name == "format-coded" && original.id == control_art.images[0].id {
                    let expected: u16 = match thumb.format_id {
                        1061 => 0xf800,
                        1055 => 0x07e0,
                        1068 => 0x001f,
                        1060 => 0xffe0,
                        _ => panic!("unexpected generated format"),
                    };
                    let pixels = slot_bytes(name, thumb);
                    assert_eq!(pixels.len(), thumb.size as usize);
                    assert_eq!(pixels[..2], expected.to_le_bytes());
                    assert_ne!(pixels, slot_bytes("control", thumb));
                } else {
                    assert_eq!(
                        slot_bytes(name, thumb),
                        slot_bytes("control", thumb),
                        "{name} changed a song's art"
                    );
                }
            }
        }

        let ipod = unsub_ipod::open(&output.join(name)).unwrap();
        assert_eq!(read_database(&ipod).unwrap().tracks.len(), 3);
        let bytes = fs::read(output.join(name).join(DB_FILE)).unwrap();
        let album = db::chunks(&bytes, b"mhia", 88)[0];
        let supplier = db::u64_at(&bytes, album + 32);
        if name == "virtual-owner" {
            assert_eq!(supplier, VIRTUAL_OWNER);
            assert!(parsed.images.iter().any(|image| image.song_id == supplier));
            assert!(
                !read_database(&ipod)
                    .unwrap()
                    .tracks
                    .iter()
                    .any(|track| track.dbid == supplier)
            );
        }
        if name == "live-supplier" {
            assert_eq!(supplier, read_database(&ipod).unwrap().tracks[1].dbid);
        }
    }
    assert_eq!(
        fs::read(output.join("control").join(DB_FILE)).unwrap(),
        fs::read(output.join("format-coded").join(DB_FILE)).unwrap()
    );
}

#[test]
fn offline_probe_refuses_existing_outputs_and_every_ipod_ancestor() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("existing");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("sentinel"), b"keep").unwrap();
    assert!(export_bundle(&existing, MODEL, GUID).is_err());
    assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"keep");
    let device = FakeIpod::new("Device");
    assert!(export_bundle(&device.mount.join("probe"), MODEL, GUID).is_err());
    assert!(!device.mount.join("probe").exists());
    #[cfg(unix)]
    {
        let alias = dir.path().join("device-alias");
        std::os::unix::fs::symlink(&device.mount, &alias).unwrap();
        assert!(export_bundle(&alias.join("probe"), MODEL, GUID).is_err());
        assert!(!device.mount.join("probe").exists());
    }
}

#[test]
#[ignore = "exports an offline experiment only when an explicit new output path is supplied"]
fn export_classic_artwork_probe() {
    let output = std::env::var_os("UNSUB_CLASSIC_ARTWORK_PROBE_OUT")
        .expect("set UNSUB_CLASSIC_ARTWORK_PROBE_OUT to a new absolute offline directory");
    let model = std::env::var("UNSUB_CLASSIC_ARTWORK_PROBE_MODEL").unwrap_or_else(|_| MODEL.into());
    let guid = std::env::var("UNSUB_CLASSIC_ARTWORK_PROBE_GUID").unwrap_or_else(|_| GUID.into());
    export_bundle(Path::new(&output), &model, &guid).unwrap();
    println!(
        "Offline probe written to {}. No device was modified; no firmware behavior is verified.",
        Path::new(&output).display()
    );
}
