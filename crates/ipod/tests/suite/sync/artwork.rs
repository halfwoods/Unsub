use std::collections::BTreeMap;
use std::fs::{self, File};

use unsub_ipod::{Error, SyncTrack, apply_sync, artwork_supported, plan_sync, read_database};

use super::*;
use crate::common::artwork::{self as art, ForeignImage, Thumb, foreign_database, jpeg, png};

const GUID: &str = "0x000A27001A2B3C4D";
/// Format id, width, height.
type Format = (u32, u32, u32);
const CLASSIC: [Format; 4] = [
    (1061, 56, 56),
    (1055, 128, 128),
    (1068, 128, 128),
    (1060, 320, 320),
];
const RED_565: u16 = 0xf800;

pub fn art_dir(fake: &FakeIpod) -> PathBuf {
    fake.mount.join("iPod_Control/Artwork")
}

pub fn artwork_db_bytes(fake: &FakeIpod) -> Vec<u8> {
    fs::read(art_dir(fake).join("ArtworkDB")).unwrap()
}

fn artwork_db(fake: &FakeIpod) -> art::Parsed {
    art::parse(&artwork_db_bytes(fake))
}

fn ithmb(fake: &FakeIpod, name: &str) -> Vec<u8> {
    fs::read(art_dir(fake).join(name)).unwrap()
}

fn ithmb_files(fake: &FakeIpod) -> BTreeSet<String> {
    match fs::read_dir(art_dir(fake)) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => BTreeSet::new(),
    }
}

pub fn covered(library: &Path, id: i64, title: &str, cover: Vec<u8>) -> SyncTrack {
    let mut track = library_track(library, id, title);
    register_cover(&mut track, cover);
    track
}

fn dbid_of(fake: &FakeIpod, title: &str) -> u64 {
    read_database(&opened(fake))
        .unwrap()
        .tracks
        .iter()
        .find(|t| t.title == title)
        .unwrap()
        .dbid
}

/// The image the track's mhit points at (0 for none).
fn link_of(fake: &FakeIpod, title: &str) -> u32 {
    let (header, _) = db::track_record(&db_bytes(fake), dbid_of(fake, title)).unwrap();
    u32::from_le_bytes(header[352..356].try_into().unwrap())
}

fn slot(format_id: u32) -> u32 {
    CLASSIC
        .iter()
        .find(|f| f.0 == format_id)
        .map(|f| f.1 * f.2 * 2)
        .unwrap()
}

fn ipod_model(model: &str) -> FakeIpod {
    let fake = FakeIpod::new("POD");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo(model, GUID);
    for folder in ["F01", "F02"] {
        fs::create_dir_all(fake.mount.join("iPod_Control/Music").join(folder)).unwrap();
    }
    fake
}

#[test]
fn tracks_of_one_album_with_one_cover_share_a_single_image() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let red = png(300, 300, [255, 0, 0]);
    let tracks = [
        covered(library.path(), 1, "One", red.clone()),
        covered(library.path(), 2, "Two", red.clone()),
        covered(library.path(), 3, "Three", jpeg(200, 200, [0, 255, 0])),
    ];
    let mut other_album = tracks[2].clone();
    other_album.album = "Vespertine".to_owned();
    let tracks = [tracks[0].clone(), tracks[1].clone(), other_album];

    sync(&fake, &tracks, &[]);

    let db = artwork_db(&fake);
    assert_eq!(db.sections, [1, 2, 3]);
    assert_eq!(db.images.len(), 2);
    assert_eq!((db.images[0].id, db.images[1].id), (0x64, 0x65));
    assert_eq!(
        db.images[0].song_id,
        dbid_of(&fake, "One"),
        "the first track of the album owns the image"
    );
    assert_eq!(db.images[1].song_id, dbid_of(&fake, "Three"));
    assert_eq!(db.listed_formats, [1061, 1055, 1068, 1060]);
    assert!(db.next_id > 0x65);
    assert_eq!(
        (
            link_of(&fake, "One"),
            link_of(&fake, "Two"),
            link_of(&fake, "Three")
        ),
        (0x64, 0x64, 0x65)
    );
    for foreign in ["Crazy in Love", "戦場のメリークリスマス", "Episode 42"] {
        assert_eq!(link_of(&fake, foreign), 0, "iTunes' tracks have none");
    }
    assert_eq!(
        db.images[0].source_size as usize,
        red.len() + 1,
        "the original size plus one, as libgpod writes it"
    );
    let (header, _) = db::track_record(&db_bytes(&fake), dbid_of(&fake, "One")).unwrap();
    assert_eq!(
        u16::from_le_bytes([header[124], header[125]]),
        1,
        "artwork count"
    );
    assert_eq!(
        u32::from_le_bytes(header[128..132].try_into().unwrap()) as usize,
        red.len(),
        "artwork size"
    );
    assert_eq!(header[164], 1, "has artwork");
}

#[test]
fn every_format_gets_a_thumbnail_in_its_own_file_at_slot_offsets() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        covered(library.path(), 1, "One", png(300, 300, [255, 0, 0])),
        {
            let mut other = covered(library.path(), 2, "Two", png(300, 300, [0, 0, 255]));
            other.album = "Other".to_owned();
            other
        },
    ];

    sync(&fake, &tracks, &[]);

    let db = artwork_db(&fake);
    for (index, image) in db.images.iter().enumerate() {
        let formats: Vec<u32> = image.thumbs.iter().map(|t| t.format_id).collect();
        assert_eq!(formats, [1061, 1055, 1068, 1060]);
        for thumb in &image.thumbs {
            assert_eq!(thumb.file, format!(":F{}_1.ithmb", thumb.format_id));
            assert_eq!(thumb.size, slot(thumb.format_id));
            assert_eq!(
                thumb.offset,
                index as u32 * thumb.size,
                "the second image follows the first"
            );
            let side = CLASSIC.iter().find(|f| f.0 == thumb.format_id).unwrap().1 as i16;
            assert_eq!(
                (
                    thumb.width,
                    thumb.height,
                    thumb.horizontal_padding,
                    thumb.vertical_padding
                ),
                (side, side, 0, 0)
            );
        }
    }
    for &(format_id, _, _) in &CLASSIC {
        assert_eq!(
            ithmb(&fake, &format!("F{format_id}_1.ithmb")).len() as u32,
            2 * slot(format_id)
        );
    }
}

#[test]
fn thumbnail_pixels_are_little_endian_rgb565_with_the_picture_centred() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    // 112x56 into 56x56: scaled to 56x28, centred with 14 rows of background above and below.
    let track = covered(library.path(), 1, "Wide", png(112, 56, [255, 0, 0]));

    sync(&fake, &[track], &[]);

    let image = &artwork_db(&fake).images[0];
    let small = image.thumbs.iter().find(|t| t.format_id == 1061).unwrap();
    assert_eq!((small.horizontal_padding, small.vertical_padding), (0, 14));
    assert_eq!(
        (small.width, small.height),
        (56, 14 + 28),
        "sizes include the leading padding"
    );
    let pixels = ithmb(&fake, "F1061_1.ithmb");
    let pixel = |x: usize, y: usize| {
        u16::from_le_bytes([pixels[(y * 56 + x) * 2], pixels[(y * 56 + x) * 2 + 1]])
    };
    assert_eq!(pixel(28, 0), 0, "background above");
    assert_eq!(pixel(28, 28), RED_565, "the picture");
    assert_eq!(pixel(28, 55), 0, "background below");
}

#[test]
fn each_model_gets_the_thumbnail_formats_libgpod_lists_for_it() {
    let cases: [(&str, &[Format], bool); 6] = [
        ("xC293", &CLASSIC, true),
        ("xA978", &CLASSIC, true),
        (
            "xB598",
            &[
                (1055, 128, 128),
                (1068, 128, 128),
                (1071, 240, 240),
                (1074, 50, 50),
                (1078, 80, 80),
                (1084, 240, 240),
            ],
            true,
        ),
        ("xA448", &[(1028, 100, 100), (1029, 200, 200)], false),
        ("xA477", &[(1031, 42, 42), (1027, 100, 100)], false),
        ("xA079", &[(1017, 56, 56), (1016, 140, 140)], false),
    ];
    for (model, formats, sparse) in cases {
        let fake = ipod_model(model);
        assert!(artwork_supported(&opened(&fake)), "{model}");
        let library = tempfile::tempdir().unwrap();
        let red = png(300, 300, [255, 0, 0]);

        sync(
            &fake,
            &[
                covered(library.path(), 1, "One", red.clone()),
                covered(library.path(), 2, "Two", red),
            ],
            &[],
        );

        let db = artwork_db(&fake);
        assert_eq!(
            db.images.len(),
            if sparse { 1 } else { 2 },
            "{model}: sparse artwork shares, others do not"
        );
        let ids: Vec<u32> = formats.iter().map(|f| f.0).collect();
        assert_eq!(db.listed_formats, ids, "{model}");
        for image in &db.images {
            let written: Vec<(u32, i16, i16)> = image
                .thumbs
                .iter()
                .map(|t| (t.format_id, t.width, t.height))
                .collect();
            let expected: Vec<(u32, i16, i16)> = formats
                .iter()
                .map(|&(id, w, h)| (id, w as i16, h as i16))
                .collect();
            assert_eq!(written, expected, "{model}");
        }
        for &(id, width, height) in formats {
            let file = ithmb(&fake, &format!("F{id}_1.ithmb"));
            assert_eq!(
                file.len() as u32,
                db.images.len() as u32 * width * height * 2,
                "{model} F{id}"
            );
            assert_eq!(
                u16::from_le_bytes([file[0], file[1]]),
                RED_565,
                "{model} F{id}"
            );
        }
    }
}

#[test]
fn ipods_without_album_art_ignore_covers() {
    let fake = ipod_model("x9160");
    let library = tempfile::tempdir().unwrap();

    sync(
        &fake,
        &[covered(library.path(), 1, "One", png(64, 64, [1, 2, 3]))],
        &[],
    );

    assert!(!artwork_supported(&opened(&fake)));
    assert!(!art_dir(&fake).exists());
    assert_eq!(link_of(&fake, "One"), 0);
}

fn slot_bytes(marker: u8, format_id: u32) -> Vec<u8> {
    vec![marker; slot(format_id) as usize]
}

/// A classic whose first two tracks (iTunes') already have artwork, in an ArtworkDB and `.ithmb`
/// files written independently of this crate.
pub fn classic_with_foreign_artwork() -> (FakeIpod, [u64; 2]) {
    let mut tracks = sample_tracks();
    for (index, id) in [0x64u32, 0x65].into_iter().enumerate() {
        tracks[index].header_patches = vec![(352, id.to_le_bytes().to_vec()), (164, vec![1])];
    }
    let owners = [tracks[0].dbid, tracks[1].dbid];
    let fake = classic_7g(&DbSpec::new(tracks, sample_playlists()));
    for folder in ["F01", "F02"] {
        fs::create_dir_all(fake.mount.join("iPod_Control/Music").join(folder)).unwrap();
    }
    let images: Vec<ForeignImage> = [(0x64u32, owners[0]), (0x65, owners[1])]
        .into_iter()
        .enumerate()
        .map(|(index, (id, song_id))| ForeignImage {
            id,
            song_id,
            thumbs: CLASSIC
                .iter()
                .map(|&(format_id, w, h)| Thumb {
                    format_id,
                    file: format!(":F{format_id}_1.ithmb"),
                    offset: index as u32 * slot(format_id),
                    size: slot(format_id),
                    width: w as i16,
                    height: h as i16,
                    horizontal_padding: 0,
                    vertical_padding: 0,
                })
                .collect(),
        })
        .collect();
    fs::create_dir_all(art_dir(&fake)).unwrap();
    fs::write(
        art_dir(&fake).join("ArtworkDB"),
        foreign_database(&images, &CLASSIC),
    )
    .unwrap();
    for &(format_id, _, _) in &CLASSIC {
        let both = [slot_bytes(0xa1, format_id), slot_bytes(0xa2, format_id)].concat();
        fs::write(art_dir(&fake).join(format!("F{format_id}_1.ithmb")), both).unwrap();
    }
    (fake, owners)
}

#[test]
fn artwork_itunes_wrote_is_left_exactly_as_it_was() {
    let (fake, owners) = classic_with_foreign_artwork();
    let before = artwork_db(&fake);
    let thumbs_before: Vec<Vec<u8>> = CLASSIC
        .iter()
        .map(|f| ithmb(&fake, &format!("F{}_1.ithmb", f.0)))
        .collect();
    let links_before: Vec<Vec<u8>> = owners
        .iter()
        .map(|&d| db::track_record(&db_bytes(&fake), d).unwrap().0[352..356].to_vec())
        .collect();
    let library = tempfile::tempdir().unwrap();

    sync(
        &fake,
        &[covered(
            library.path(),
            7,
            "Hunter",
            png(300, 300, [255, 0, 0]),
        )],
        &[],
    );

    let after = artwork_db(&fake);
    assert_eq!(after.images.len(), 3);
    assert_eq!(
        after.images[0].bytes, before.images[0].bytes,
        "byte for byte, odd fields included"
    );
    assert_eq!(after.images[1].bytes, before.images[1].bytes);
    let added = &after.images[2];
    assert_eq!(added.id, 0x66);
    assert_eq!(added.song_id, dbid_of(&fake, "Hunter"));
    for (thumb, (format, before)) in added.thumbs.iter().zip(CLASSIC.iter().zip(&thumbs_before)) {
        assert_eq!(
            thumb.offset as usize,
            before.len(),
            "new thumbnails go after the old ones"
        );
        let file = ithmb(&fake, &format!("F{}_1.ithmb", format.0));
        assert_eq!(
            &file[..before.len()],
            &before[..],
            "F{} keeps iTunes' thumbnails",
            format.0
        );
        assert_eq!(file.len(), before.len() + slot(format.0) as usize);
    }
    let links_after: Vec<Vec<u8>> = owners
        .iter()
        .map(|&d| db::track_record(&db_bytes(&fake), d).unwrap().0[352..356].to_vec())
        .collect();
    assert_eq!(
        links_after, links_before,
        "iTunes' tracks still point at their images"
    );
    assert_eq!(link_of(&fake, "Hunter"), 0x66);
    assert!(after.next_id >= 0x67);
}

#[test]
fn the_previous_artwork_database_is_kept_beside_the_new_one() {
    let (fake, _) = classic_with_foreign_artwork();
    let original = artwork_db_bytes(&fake);
    let library = tempfile::tempdir().unwrap();

    sync(
        &fake,
        &[covered(library.path(), 7, "Hunter", png(64, 64, [9, 9, 9]))],
        &[],
    );

    assert_eq!(
        fs::read(art_dir(&fake).join("ArtworkDB.unsub-backup")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(art_dir(&fake).join("ArtworkDB.unsub-original")).unwrap(),
        original
    );
    assert!(!ithmb_files(&fake).iter().any(|n| n.ends_with(".unsub-tmp")));
}

#[test]
fn a_later_sync_reuses_the_image_made_for_the_same_cover() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let red = png(300, 300, [255, 0, 0]);
    let one = covered(library.path(), 1, "One", red.clone());
    sync(&fake, std::slice::from_ref(&one), &[]);
    let size_after_first = ithmb(&fake, "F1060_1.ithmb").len();

    sync(&fake, &[one, covered(library.path(), 2, "Two", red)], &[]);

    assert_eq!(artwork_db(&fake).images.len(), 1);
    assert_eq!(
        ithmb(&fake, "F1060_1.ithmb").len(),
        size_after_first,
        "no second copy of the thumbnails"
    );
    assert_eq!(link_of(&fake, "Two"), link_of(&fake, "One"));
}

#[test]
fn an_image_no_track_uses_any_more_is_dropped_one_sync_later() {
    let (fake, _) = classic_with_foreign_artwork();
    let library = tempfile::tempdir().unwrap();
    let one = covered(library.path(), 1, "One", png(300, 300, [255, 0, 0]));
    sync(&fake, std::slice::from_ref(&one), &[]);
    assert_eq!(artwork_db(&fake).images.len(), 3);

    sync(&fake, &[], &[]);
    assert_eq!(
        artwork_db(&fake).images.len(),
        3,
        "the removed track's image is still referenced by the database it replaced"
    );

    sync(&fake, &[], &[]);
    let db = artwork_db(&fake);
    assert_eq!(
        db.images.iter().map(|i| i.id).collect::<Vec<_>>(),
        [0x64, 0x65],
        "only iTunes' images remain"
    );
    assert!(
        ithmb(&fake, "F1060_1.ithmb").len() > 2 * slot(1060) as usize,
        "thumbnail bytes are not reclaimed"
    );
}

#[test]
fn a_track_that_had_no_cover_gains_one_on_a_later_sync() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let plain = library_track(library.path(), 1, "One");
    sync(&fake, std::slice::from_ref(&plain), &[]);
    assert_eq!(link_of(&fake, "One"), 0);
    let mut with_cover = plain.clone();
    register_cover(&mut with_cover, png(100, 100, [0, 0, 255]));

    let plan = plan(&fake, std::slice::from_ref(&with_cover), &[]);
    assert_eq!(plan.update, [1]);
    assert!(
        plan.bytes_needed >= (56 * 56 + 128 * 128 * 2 + 320 * 320) * 2,
        "thumbnail space is counted"
    );
    apply_sync(
        &opened(&fake),
        &plan,
        std::slice::from_ref(&with_cover),
        &[],
        &mut copy_as_is,
        &mut cover_provider,
        &mut |_| {},
    )
    .unwrap();

    assert_ne!(link_of(&fake, "One"), 0);
    assert_eq!(artwork_db(&fake).images.len(), 1);
    let again = self::plan(&fake, &[with_cover], &[]);
    assert!(again.update.is_empty(), "and then nothing more to do");
}

#[test]
fn a_cover_that_cannot_be_decoded_just_leaves_the_track_without_art() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();

    sync(
        &fake,
        &[covered(library.path(), 1, "One", b"not an image".to_vec())],
        &[],
    );

    assert_eq!(link_of(&fake, "One"), 0);
    assert!(
        read_database(&opened(&fake))
            .unwrap()
            .tracks
            .iter()
            .any(|t| t.title == "One")
    );
}

#[test]
fn thumbnails_roll_over_to_a_new_file_after_256_megabytes() {
    let fake = fake_classic();
    fs::create_dir_all(art_dir(&fake)).unwrap();
    for &(format_id, _, _) in &CLASSIC {
        File::create(art_dir(&fake).join(format!("F{format_id}_1.ithmb")))
            .unwrap()
            .set_len(256_000_000)
            .unwrap();
    }
    let library = tempfile::tempdir().unwrap();

    sync(
        &fake,
        &[covered(library.path(), 1, "One", png(64, 64, [1, 1, 1]))],
        &[],
    );

    let image = &artwork_db(&fake).images[0];
    for thumb in &image.thumbs {
        assert_eq!(thumb.file, format!(":F{}_2.ithmb", thumb.format_id));
        assert_eq!(thumb.offset, 0);
    }
    assert_eq!(ithmb(&fake, "F1061_2.ithmb").len() as u32, slot(1061));
}

struct Untouched {
    itunes: Vec<u8>,
    artwork: Option<Vec<u8>>,
    thumbs: BTreeMap<String, u64>,
    music: BTreeSet<PathBuf>,
}

impl Untouched {
    fn capture(fake: &FakeIpod) -> Self {
        let thumbs = ithmb_files(fake)
            .into_iter()
            .map(|name| {
                let len = fs::metadata(art_dir(fake).join(&name)).unwrap().len();
                (name, len)
            })
            .collect();
        Self {
            itunes: db_bytes(fake),
            artwork: fs::read(art_dir(fake).join("ArtworkDB")).ok(),
            thumbs,
            music: music_files(fake),
        }
    }

    fn assert_holds(&self, fake: &FakeIpod) {
        assert_eq!(db_bytes(fake), self.itunes, "iTunesDB");
        assert_eq!(
            fs::read(art_dir(fake).join("ArtworkDB")).ok(),
            self.artwork,
            "ArtworkDB"
        );
        let now = Self::capture(fake);
        assert_eq!(now.thumbs, self.thumbs, "thumbnail files and their lengths");
        assert_eq!(now.music, self.music, "music files");
    }
}

fn apply_covered(fake: &FakeIpod, tracks: &[SyncTrack]) -> Result<unsub_ipod::Database, Error> {
    let plan = plan(fake, tracks, &[]);
    apply_sync(
        &opened(fake),
        &plan,
        tracks,
        &[],
        &mut copy_as_is,
        &mut cover_provider,
        &mut |_| {},
    )
}

#[test]
fn a_failure_writing_the_itunesdb_takes_the_appended_thumbnails_back_out() {
    let (fake, _) = classic_with_foreign_artwork();
    let library = tempfile::tempdir().unwrap();
    let tracks = [covered(
        library.path(),
        7,
        "Hunter",
        png(300, 300, [255, 0, 0]),
    )];
    fs::create_dir(itunes_file(&fake, "iTunesDB.unsub-tmp")).unwrap();
    let untouched = Untouched::capture(&fake);

    let result = apply_covered(&fake, &tracks);

    assert!(result.is_err());
    untouched.assert_holds(&fake);
}

#[test]
fn a_failure_staging_the_artwork_database_changes_nothing_either() {
    let (fake, _) = classic_with_foreign_artwork();
    let library = tempfile::tempdir().unwrap();
    let tracks = [covered(
        library.path(),
        7,
        "Hunter",
        png(300, 300, [255, 0, 0]),
    )];
    fs::create_dir(art_dir(&fake).join("ArtworkDB.unsub-tmp")).unwrap();
    let untouched = Untouched::capture(&fake);

    let result = apply_covered(&fake, &tracks);

    assert!(result.is_err());
    untouched.assert_holds(&fake);
    assert!(!itunes_file(&fake, "iTunesDB.unsub-tmp").exists());
}

#[test]
fn a_fresh_device_leaves_no_thumbnail_files_behind_when_the_sync_fails() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [covered(
        library.path(),
        7,
        "Hunter",
        png(300, 300, [255, 0, 0]),
    )];
    fs::create_dir(itunes_file(&fake, "iTunesDB.unsub-tmp")).unwrap();

    let result = apply_covered(&fake, &tracks);

    assert!(result.is_err());
    assert!(ithmb_files(&fake).is_empty(), "{:?}", ithmb_files(&fake));
}

#[test]
fn thumbnail_files_that_are_not_a_whole_number_of_slots_stop_the_sync() {
    let (fake, _) = classic_with_foreign_artwork();
    let path = art_dir(&fake).join("F1055_1.ithmb");
    let mut bytes = fs::read(&path).unwrap();
    bytes.truncate(bytes.len() - 100);
    fs::write(&path, bytes).unwrap();
    let library = tempfile::tempdir().unwrap();
    let tracks = [covered(
        library.path(),
        7,
        "Hunter",
        png(300, 300, [255, 0, 0]),
    )];
    let untouched = Untouched::capture(&fake);

    let result = apply_covered(&fake, &tracks);

    assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
    untouched.assert_holds(&fake);
}

#[test]
fn a_damaged_artwork_database_blocks_only_syncs_that_need_it() {
    let (fake, _) = classic_with_foreign_artwork();
    let damaged = b"this is not an ArtworkDB".to_vec();
    fs::write(art_dir(&fake).join("ArtworkDB"), &damaged).unwrap();
    let library = tempfile::tempdir().unwrap();

    let needs_it = [covered(
        library.path(),
        7,
        "Hunter",
        png(300, 300, [255, 0, 0]),
    )];
    let untouched = Untouched::capture(&fake);
    let result = apply_covered(&fake, &needs_it);
    assert!(matches!(result, Err(Error::Corrupt { .. })), "{result:?}");
    untouched.assert_holds(&fake);

    sync(&fake, &[library_track(library.path(), 8, "Plain")], &[]);
    assert_eq!(artwork_db_bytes(&fake), damaged, "left alone");
    assert!(
        read_database(&opened(&fake))
            .unwrap()
            .tracks
            .iter()
            .any(|t| t.title == "Plain")
    );
}

#[test]
fn plan_sync_refuses_unsupported_devices_whatever_the_covers() {
    let fake = ipod_model("xC525");
    fake.write_sysinfo_extended("000A27001A2B3C4D", "SERIAL00000", 5);
    let library = tempfile::tempdir().unwrap();
    let ipod = opened(&fake);
    let current = read_database(&ipod).unwrap();

    let result = plan_sync(
        &ipod,
        &current,
        &[covered(library.path(), 1, "One", png(8, 8, [1, 1, 1]))],
        &[],
    );

    assert!(matches!(result, Err(Error::SyncRefused(_))));
}

#[test]
fn a_device_that_lists_its_own_formats_is_believed_over_the_table() {
    let fake = FakeIpod::new("UNKNOWN");
    fake.write_itunesdb(&sample_database().build());
    fs::create_dir_all(fake.mount.join("iPod_Control/Music/F01")).unwrap();
    // No model number: only the device's own list says what it wants. 50 pixels is 100 bytes, which
    // 64-byte row alignment pads to 128 bytes, i.e. 64 pixels per row.
    fake.write_sysinfo_extended_with_album_art("000A27001A2B3C4D", 2, (1999, 50, 50), 64);
    let library = tempfile::tempdir().unwrap();
    let red = png(100, 100, [255, 0, 0]);

    sync(
        &fake,
        &[
            covered(library.path(), 1, "One", red.clone()),
            covered(library.path(), 2, "Two", red),
        ],
        &[],
    );

    let db = artwork_db(&fake);
    assert_eq!(
        db.images.len(),
        1,
        "it asked for sparse artwork, so the two tracks share an image"
    );
    let thumb = &db.images[0].thumbs[0];
    assert_eq!(
        (thumb.format_id, thumb.size, thumb.width, thumb.height),
        (1999, 64 * 50 * 2, 50, 50)
    );
    let file = ithmb(&fake, "F1999_1.ithmb");
    assert_eq!(file.len(), 64 * 50 * 2);
    let pixel = |x: usize, y: usize| {
        u16::from_le_bytes([file[(y * 64 + x) * 2], file[(y * 64 + x) * 2 + 1]])
    };
    assert_eq!(pixel(10, 10), RED_565);
    assert_eq!(
        pixel(55, 10),
        0xffff,
        "the alignment padding takes the format's background colour"
    );
}

#[test]
fn covers_are_fetched_one_at_a_time_and_only_for_the_tracks_that_need_them() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let album = png(300, 300, [255, 0, 0]);
    let mut tracks = vec![
        covered(library.path(), 1, "One", album.clone()),
        covered(library.path(), 2, "Two", album.clone()),
        covered(library.path(), 3, "Three", album.clone()),
        library_track(library.path(), 4, "No cover"),
    ];
    let mut other = covered(library.path(), 5, "Other album", png(300, 300, [0, 255, 0]));
    other.album = "Elsewhere".to_owned();
    tracks.push(other);

    sync(&fake, &tracks, &[]);

    let asked = covers_asked_for();
    assert_eq!(
        asked.len(),
        2,
        "one fetch per album cover, none for the track without one: {asked:?}"
    );
    assert!(asked.contains(&1) && asked.contains(&5));
    assert_eq!(artwork_db(&fake).images.len(), 2);
    assert_eq!(link_of(&fake, "No cover"), 0);

    // Nothing changed, so nothing is fetched and nothing is planned.
    let again = plan(&fake, &tracks, &[]);
    assert!(again.add.is_empty() && again.update.is_empty());
    sync(&fake, &tracks, &[]);
    assert!(covers_asked_for().is_empty());
}

#[test]
fn a_changed_cover_key_replaces_the_cover_and_a_changed_cover_with_the_same_key_does_not() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let mut track = covered(library.path(), 1, "One", png(300, 300, [255, 0, 0]));
    sync(&fake, std::slice::from_ref(&track), &[]);
    let first = link_of(&fake, "One");
    covers_asked_for();

    // New artwork under the same key: the library says nothing changed, so nothing is loaded.
    register_cover(&mut track, png(300, 300, [0, 0, 255]));
    let same_key = SyncTrack {
        artwork_key: plan_key(&fake, 1),
        ..track.clone()
    };
    assert!(
        plan(&fake, std::slice::from_ref(&same_key), &[])
            .update
            .is_empty()
    );

    // A new key: the track is updated, the new cover fetched once, and the track repointed.
    let plan = plan(&fake, std::slice::from_ref(&track), &[]);
    assert_eq!(plan.update, [1]);
    apply_sync(
        &opened(&fake),
        &plan,
        std::slice::from_ref(&track),
        &[],
        &mut copy_as_is,
        &mut cover_provider,
        &mut |_| {},
    )
    .unwrap();

    assert_eq!(covers_asked_for(), [1]);
    let second = link_of(&fake, "One");
    assert_ne!(second, first);
    assert_eq!(
        artwork_db(&fake).images.len(),
        2,
        "the old image goes one sync later"
    );
    sync(&fake, std::slice::from_ref(&track), &[]);
    assert_eq!(
        artwork_db(&fake)
            .images
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>(),
        [second]
    );
}

/// The cover key the bookkeeping file holds for a library track.
fn plan_key(fake: &FakeIpod, library_id: i64) -> Option<u64> {
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(itunes_file(fake, "Unsub.json")).unwrap()).unwrap();
    json["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["libraryId"] == library_id)
        .and_then(|t| t["artworkKey"].as_u64())
}

#[test]
fn a_key_with_no_cover_behind_it_is_handled_once_not_every_sync() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let mut track = library_track(library.path(), 1, "One");
    track.artwork_key = Some(42);

    sync(&fake, std::slice::from_ref(&track), &[]);
    assert_eq!(covers_asked_for(), [1], "asked once, and nothing came back");
    assert_eq!(link_of(&fake, "One"), 0);

    let again = plan(&fake, std::slice::from_ref(&track), &[]);
    assert!(
        again.update.is_empty(),
        "the key was recorded, so there is nothing left to do"
    );
}

#[test]
fn identical_cover_bytes_under_different_keys_share_one_set_of_thumbnails() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let red = png(300, 300, [255, 0, 0]);
    let one = covered(library.path(), 1, "One", red.clone());
    let mut two = covered(library.path(), 2, "Two", red);
    two.artwork_key = Some(one.artwork_key.unwrap() ^ 1);

    sync(&fake, &[one, two], &[]);

    assert_eq!(
        covers_asked_for().len(),
        2,
        "different keys mean the library is asked twice"
    );
    let db = artwork_db(&fake);
    assert_eq!(
        db.images.len(),
        1,
        "the bytes are the same, so is the image"
    );
    assert_eq!(ithmb(&fake, "F1060_1.ithmb").len() as u32, slot(1060));
    assert_eq!(link_of(&fake, "One"), link_of(&fake, "Two"));
}
