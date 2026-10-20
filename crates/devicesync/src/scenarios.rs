//! End-to-end sync scenarios against fake players in temporary folders and in memory.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    Device, DeviceKind, DeviceRoot, Error, FsStorage, MtpBackend, MtpDevice, MtpStorageInfo,
    PlaylistFormat, Profile, RESERVE_BYTES, Result, Storage, SyncItem, SyncPlan, SyncPlaylist,
    SyncProgress, SyncStage, SyncSummary, Target, apply, apply_with,
    manifest::Manifest,
    plan, plan_with,
    testing::{Faulty, Library, MemoryStorage},
};

/// A mounted player in a temporary folder.
struct Player {
    dir: tempfile::TempDir,
    storage: FsStorage,
}

impl Player {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            storage: FsStorage::new(dir.path()),
            dir,
        }
    }

    /// A file the user put there, which a sync must leave alone.
    fn user_file(&self, path: &str, bytes: &[u8]) {
        let full = self.dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, bytes).unwrap();
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        fs::read(self.dir.path().join(path)).ok()
    }

    /// Every file on the player.
    fn tree(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    let relative = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.insert(relative, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(self.dir.path(), self.dir.path(), &mut out);
        out
    }

    /// Paths of the files below `prefix`.
    fn files(&self, prefix: &str) -> Vec<String> {
        self.tree()
            .into_keys()
            .filter(|path| path.starts_with(prefix))
            .collect()
    }

    fn manifest(&self) -> Manifest {
        Manifest::load(&self.storage).unwrap()
    }

    fn folders(&self) -> Vec<String> {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    out.push(
                        path.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                    walk(root, &path, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(self.dir.path(), self.dir.path(), &mut out);
        out.sort();
        out
    }
}

fn target<'a>(storage: &'a dyn Storage, profile: &'a Profile) -> Target<'a> {
    Target { storage, profile }
}

fn copy_source(item: &SyncItem, _: &Profile) -> io::Result<PathBuf> {
    Ok(item.source.clone())
}

struct Run {
    plan: SyncPlan,
    events: Vec<SyncProgress>,
    summary: SyncSummary,
}

/// Plans and applies, copying sources as they are.
fn sync(
    storage: &dyn Storage,
    profile: &Profile,
    items: &[SyncItem],
    playlists: &[SyncPlaylist],
) -> Run {
    try_sync(
        storage,
        profile,
        items,
        playlists,
        &mut copy_source,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap()
}

fn try_sync(
    storage: &dyn Storage,
    profile: &Profile,
    items: &[SyncItem],
    playlists: &[SyncPlaylist],
    prepare: &mut dyn FnMut(&SyncItem, &Profile) -> io::Result<PathBuf>,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(SyncProgress),
) -> Result<Run> {
    let plan = plan_with(target(storage, profile), items, playlists)?;
    let mut events = Vec::new();
    let summary = apply_with(
        target(storage, profile),
        &plan,
        items,
        playlists,
        prepare,
        &mut |event| {
            events.push(event.clone());
            on_progress(event);
        },
        cancel,
    )?;
    Ok(Run {
        plan,
        events,
        summary,
    })
}

fn rockbox() -> Profile {
    Profile::rockbox()
}

fn paths(files: &[crate::PlannedFile]) -> Vec<&str> {
    files.iter().map(|file| file.path.as_str()).collect()
}

mod first_sync {
    use super::*;

    #[test]
    fn copies_every_track_to_the_layout_and_records_it() {
        let (player, library) = (Player::new(), Library::new());
        let items = [
            library.track(1, "The Beatles", "Abbey Road", "Come Together", 1, "flac"),
            library.track(2, "The Beatles", "Abbey Road", "Something", 2, "flac"),
            library.track(3, "Daft Punk", "Discovery", "One More Time", 1, "mp3"),
        ];
        let run = sync(&player.storage, &rockbox(), &items, &[]);

        assert_eq!(
            paths(&run.plan.add),
            [
                "Music/Daft Punk/Discovery/1-01 One More Time.mp3",
                "Music/The Beatles/Abbey Road/1-01 Come Together.flac",
                "Music/The Beatles/Abbey Road/1-02 Something.flac",
            ]
        );
        assert_eq!(
            (
                run.plan.update.len(),
                run.plan.remove.len(),
                run.plan.unchanged
            ),
            (0, 0, 0)
        );
        assert_eq!(
            run.summary,
            SyncSummary {
                copied: 3,
                removed: 0,
                skipped: vec![]
            }
        );
        assert!(run.plan.fits);
        assert_eq!(
            run.plan.bytes_needed,
            items.iter().map(|item| item.size).sum::<u64>()
        );

        for item in &items {
            let entry = &player.manifest().items[&item.library_id];
            assert_eq!(
                player.read(&entry.path).unwrap(),
                fs::read(&item.source).unwrap()
            );
            assert_eq!(
                (entry.size, entry.source_size, entry.source_mtime),
                (item.size, item.size, item.mtime)
            );
        }
        // Only the music and the manifest are on the player; no temp files, nothing pending.
        assert_eq!(player.tree().len(), 4);
        assert!(player.manifest().pending.is_empty() && player.manifest().trash.is_empty());
        assert!(player.tree().keys().all(|path| !path.contains("as-part")));
    }

    #[test]
    fn reports_progress_per_track() {
        let (player, library) = (Player::new(), Library::new());
        let items = [
            library.track(1, "A", "B", "One", 1, "mp3"),
            library.track(2, "A", "B", "Two", 2, "mp3"),
        ];
        let run = sync(&player.storage, &rockbox(), &items, &[]);
        let copying: Vec<_> = run
            .events
            .iter()
            .filter(|event| event.stage == SyncStage::Copying)
            .collect();
        assert_eq!(
            copying
                .iter()
                .map(|event| (event.done, event.total))
                .collect::<Vec<_>>(),
            [(0, 2), (1, 2)]
        );
        assert_eq!(copying[0].current, "One");
    }

    #[test]
    fn duplicate_library_ids_are_one_track() {
        let (player, library) = (Player::new(), Library::new());
        let item = library.track(1, "A", "B", "One", 1, "mp3");
        let run = sync(&player.storage, &rockbox(), &[item.clone(), item], &[]);
        assert_eq!(run.plan.add.len(), 1);
        assert_eq!(player.files("Music/").len(), 1);
    }

    #[test]
    fn a_custom_layout_and_music_folder_apply() {
        let (player, library) = (Player::new(), Library::new());
        let profile = Profile {
            music_dir: "Audio".to_owned(),
            layout: "{album}/{track} - {title}".to_owned(),
            ..Profile::default()
        };
        sync(
            &player.storage,
            &profile,
            &[library.track(1, "A", "Album", "Song", 7, "flac")],
            &[],
        );
        assert_eq!(player.files("Audio/"), ["Audio/Album/07 - Song.flac"]);
    }
}

mod resync {
    use super::*;

    fn synced(player: &Player, library: &Library) -> Vec<SyncItem> {
        let items = vec![
            library.track(1, "Artist", "Album", "One", 1, "flac"),
            library.track(2, "Artist", "Album", "Two", 2, "flac"),
            library.track(3, "Other", "Record", "Three", 1, "mp3"),
        ];
        sync(&player.storage, &rockbox(), &items, &[]);
        items
    }

    #[test]
    fn a_second_sync_of_the_same_library_changes_nothing() {
        let (player, library) = (Player::new(), Library::new());
        let items = synced(&player, &library);
        let before = player.tree();
        let run = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!(run.summary, SyncSummary::default());
        assert_eq!(
            (
                run.plan.add.len(),
                run.plan.update.len(),
                run.plan.remove.len(),
                run.plan.unchanged
            ),
            (0, 0, 0, 3)
        );
        assert_eq!(run.plan.bytes_needed, 0);
        assert_eq!(player.tree(), before);
    }

    #[test]
    fn a_changed_source_is_copied_again_in_place() {
        let (player, library) = (Player::new(), Library::new());
        let mut items = synced(&player, &library);
        items[1] = library.modified(&items[1]);
        let run = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!((run.summary.copied, run.summary.removed), (1, 0));
        assert_eq!(
            paths(&run.plan.update),
            ["Music/Artist/Album/1-02 Two.flac"]
        );
        assert!(run.plan.add.is_empty() && run.plan.remove.is_empty());
        assert_eq!(run.plan.unchanged, 2);
        assert_eq!(
            player.read("Music/Artist/Album/1-02 Two.flac").unwrap(),
            vec![0xEE; items[1].size as usize]
        );
        assert_eq!(player.manifest().items[&2].source_mtime, items[1].mtime);
        assert_eq!(
            sync(&player.storage, &rockbox(), &items, &[])
                .plan
                .unchanged,
            3
        );
    }

    #[test]
    fn a_new_mtime_alone_is_enough() {
        let (player, library) = (Player::new(), Library::new());
        let mut items = synced(&player, &library);
        items[0].mtime += 5;
        let plan = plan_with(target(&player.storage, &rockbox()), &items, &[]).unwrap();
        assert_eq!(plan.update.len(), 1);
    }

    #[test]
    fn dropped_tracks_are_removed_with_their_empty_folders_and_nothing_else() {
        let (player, library) = (Player::new(), Library::new());
        let items = synced(&player, &library);
        player.user_file("Music/Other/Record/cover.jpg", b"the user's cover");
        player.user_file("Music/Artist/notes.txt", b"mine");
        player.user_file("Podcasts/episode.mp3", b"mine too");

        // Track 1 and track 3 leave the library.
        let run = sync(&player.storage, &rockbox(), &items[1..2], &[]);
        assert_eq!((run.summary.copied, run.summary.removed), (0, 2));
        assert_eq!(
            run.plan
                .remove
                .iter()
                .map(|file| file.library_id)
                .collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(run.plan.bytes_freed, items[0].size + items[2].size);

        assert_eq!(
            player.files("Music/"),
            [
                "Music/Artist/Album/1-02 Two.flac",
                "Music/Artist/notes.txt",
                "Music/Other/Record/cover.jpg",
            ]
        );
        assert_eq!(
            player.read("Music/Other/Record/cover.jpg").unwrap(),
            b"the user's cover"
        );
        assert_eq!(player.read("Podcasts/episode.mp3").unwrap(), b"mine too");
        assert_eq!(
            player.manifest().items.keys().copied().collect::<Vec<_>>(),
            [2]
        );
    }

    #[test]
    fn emptying_the_library_removes_only_what_we_added() {
        let (player, library) = (Player::new(), Library::new());
        player.user_file("Music/My Own/Album/track.mp3", b"user");
        player.user_file("Music/loose.flac", b"user");
        let before_user = player.tree();
        let items = synced(&player, &library);
        assert!(items.len() == 3);
        sync(&player.storage, &rockbox(), &[], &[]);

        let after = player.tree();
        for (path, bytes) in &before_user {
            assert_eq!(after.get(path), Some(bytes), "{path} changed");
        }
        // Our folders are gone; the user's remain.
        assert!(
            !player
                .folders()
                .iter()
                .any(|folder| folder.contains("Artist") || folder.contains("Other"))
        );
        assert!(player.folders().contains(&"Music/My Own/Album".to_owned()));
        assert!(player.folders().contains(&"Music".to_owned()));
        assert!(player.manifest().items.is_empty());
    }

    #[test]
    fn a_renamed_track_is_copied_to_its_new_name_and_the_old_file_deleted() {
        let (player, library) = (Player::new(), Library::new());
        let mut items = synced(&player, &library);
        items[0].title = "One (Remastered)".to_owned();
        let run = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!((run.summary.copied, run.summary.removed), (1, 0));
        assert_eq!(run.plan.update.len(), 1);
        assert_eq!(
            run.plan.update[0].replaces.as_deref(),
            Some("Music/Artist/Album/1-01 One.flac")
        );
        assert_eq!(
            player.files("Music/Artist/"),
            [
                "Music/Artist/Album/1-01 One (Remastered).flac",
                "Music/Artist/Album/1-02 Two.flac",
            ]
        );
        assert_eq!(
            player.manifest().items[&1].path,
            "Music/Artist/Album/1-01 One (Remastered).flac"
        );
        assert_eq!(
            sync(&player.storage, &rockbox(), &items, &[])
                .plan
                .unchanged,
            3
        );
    }

    #[test]
    fn changing_the_layout_moves_everything_and_leaves_no_old_folders() {
        let (player, library) = (Player::new(), Library::new());
        let items = synced(&player, &library);
        let flat = Profile {
            layout: "{album_artist} - {title}".to_owned(),
            ..rockbox()
        };
        let run = sync(&player.storage, &flat, &items, &[]);
        assert_eq!(run.plan.update.len(), 3);
        assert_eq!(
            player.files("Music/"),
            [
                "Music/Artist - One.flac",
                "Music/Artist - Two.flac",
                "Music/Other - Three.mp3",
            ]
        );
        assert_eq!(player.folders(), [".antistreaming", "Music"]);
    }

    /// In memory, because the real thing flushes every file to disk, which is the right call for a
    /// removable drive and slow for thousands of test files.
    #[test]
    fn a_big_library_syncs_resyncs_and_shrinks() {
        let (device, library) = (MemoryStorage::new(1 << 30), Library::new());
        let items: Vec<SyncItem> = (1..=3_000)
            .map(|id| {
                let (artist, album) = (format!("Artist {}", id % 40), format!("Album {}", id % 97));
                library.track_of_size(id, &artist, &album, &format!("Song {id}"), 1, "mp3", 8)
            })
            .collect();
        let music = |device: &MemoryStorage| {
            device
                .paths()
                .iter()
                .filter(|path| path.starts_with("Music/"))
                .count()
        };
        let first = sync(&device, &rockbox(), &items, &[]);
        assert_eq!(first.plan.add.len(), 3_000);
        assert_eq!(Manifest::load(&device).unwrap().items.len(), 3_000);

        let second = sync(&device, &rockbox(), &items, &[]);
        assert_eq!(
            (
                second.plan.unchanged,
                second.plan.add.len(),
                second.plan.update.len()
            ),
            (3_000, 0, 0)
        );

        let third = sync(&device, &rockbox(), &items[..1_000], &[]);
        assert_eq!(
            (third.plan.remove.len(), third.plan.unchanged),
            (2_000, 1_000)
        );
        assert_eq!(music(&device), 1_000);
    }

    #[test]
    fn a_file_the_user_deleted_is_copied_again() {
        let (player, library) = (Player::new(), Library::new());
        let items = synced(&player, &library);
        fs::remove_file(player.dir.path().join("Music/Artist/Album/1-01 One.flac")).unwrap();
        let run = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!(paths(&run.plan.add), ["Music/Artist/Album/1-01 One.flac"]);
        assert_eq!(
            player.read("Music/Artist/Album/1-01 One.flac").unwrap(),
            fs::read(&items[0].source).unwrap()
        );
    }

    #[test]
    fn a_truncated_copy_is_replaced() {
        let (player, library) = (Player::new(), Library::new());
        let items = synced(&player, &library);
        fs::write(
            player.dir.path().join("Music/Artist/Album/1-02 Two.flac"),
            b"cut",
        )
        .unwrap();
        let run = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!(
            paths(&run.plan.update),
            ["Music/Artist/Album/1-02 Two.flac"]
        );
        assert_eq!(
            player.read("Music/Artist/Album/1-02 Two.flac").unwrap(),
            fs::read(&items[1].source).unwrap()
        );
    }
}

mod names {
    use super::*;

    fn twins(library: &Library) -> Vec<SyncItem> {
        vec![
            library.track(7, "Band", "Live", "Intro", 1, "flac"),
            library.track(3, "Band", "Live", "Intro", 1, "flac"),
            library.track(5, "Band", "Live", "Intro", 1, "flac"),
        ]
    }

    #[test]
    fn identical_names_are_numbered_in_library_id_order_whatever_the_input_order() {
        let library = Library::new();
        let mut items = twins(&library);
        let mut results = Vec::new();
        for _ in 0..2 {
            let player = Player::new();
            sync(&player.storage, &rockbox(), &items, &[]);
            results.push(
                player
                    .manifest()
                    .items
                    .iter()
                    .map(|(id, entry)| (*id, entry.path.clone()))
                    .collect::<Vec<_>>(),
            );
            items.reverse();
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(
            results[0],
            [
                (3, "Music/Band/Live/1-01 Intro.flac".to_owned()),
                (5, "Music/Band/Live/1-01 Intro (2).flac".to_owned()),
                (7, "Music/Band/Live/1-01 Intro (3).flac".to_owned()),
            ]
        );
    }

    #[test]
    fn names_differing_only_in_case_collide_as_they_do_on_fat() {
        let (player, library) = (Player::new(), Library::new());
        let items = [
            library.track(1, "band", "live", "intro", 1, "flac"),
            library.track(2, "Band", "Live", "Intro", 1, "flac"),
        ];
        sync(&player.storage, &rockbox(), &items, &[]);
        let manifest = player.manifest();
        assert_eq!(manifest.items[&1].path, "Music/band/live/1-01 intro.flac");
        assert_eq!(
            manifest.items[&2].path,
            "Music/Band/Live/1-01 Intro (2).flac"
        );
    }

    #[test]
    fn a_numbered_name_is_kept_for_as_long_as_it_derives_from_the_same_base() {
        let (player, library) = (Player::new(), Library::new());
        let items = twins(&library);
        sync(&player.storage, &rockbox(), &items, &[]);
        // The track that held the plain name leaves; the others must not shuffle.
        let remaining: Vec<SyncItem> = items
            .iter()
            .filter(|item| item.library_id != 3)
            .cloned()
            .collect();
        let run = sync(&player.storage, &rockbox(), &remaining, &[]);
        assert_eq!(
            (
                run.plan.add.len(),
                run.plan.update.len(),
                run.plan.unchanged
            ),
            (0, 0, 2)
        );
        assert_eq!(
            player.files("Music/"),
            [
                "Music/Band/Live/1-01 Intro (2).flac",
                "Music/Band/Live/1-01 Intro (3).flac"
            ]
        );
    }

    #[test]
    fn a_file_the_user_already_has_at_the_target_path_is_never_overwritten() {
        let (player, library) = (Player::new(), Library::new());
        player.user_file("Music/Band/Live/1-01 Intro.flac", b"the user's own copy");
        player.user_file("music/band/live/1-02 outro (2).flac", b"case differs");
        let items = [
            library.track(1, "Band", "Live", "Intro", 1, "flac"),
            library.track(2, "Band", "Live", "Outro", 2, "flac"),
        ];
        let run = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!(
            paths(&run.plan.add),
            [
                "Music/Band/Live/1-01 Intro (2).flac",
                "Music/Band/Live/1-02 Outro.flac"
            ]
        );
        assert_eq!(
            player.read("Music/Band/Live/1-01 Intro.flac").unwrap(),
            b"the user's own copy"
        );
        // And on the next sync neither the user's file nor ours moves.
        let again = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!(again.plan.unchanged, 2);
        assert_eq!(
            player.read("Music/Band/Live/1-01 Intro.flac").unwrap(),
            b"the user's own copy"
        );
    }

    #[test]
    fn unsafe_names_are_made_safe_in_the_folders_on_the_player() {
        let (player, library) = (Player::new(), Library::new());
        let item = library.track(
            1,
            "AC/DC",
            "Who Made Who: \"Live\"?",
            "Hells Bells. ",
            4,
            "flac",
        );
        sync(&player.storage, &rockbox(), &[item], &[]);
        assert_eq!(
            player.files("Music/"),
            ["Music/AC_DC/Who Made Who_ _Live__/1-04 Hells Bells.flac"]
        );
    }
}

mod conversion {
    use super::*;

    #[test]
    fn a_format_the_player_cannot_play_gets_the_preferred_extension_and_goes_through_prepare() {
        let (player, library) = (Player::new(), Library::new());
        let profile = Profile {
            formats: vec!["mp3".to_owned(), "flac".to_owned()],
            ..rockbox()
        };
        let items = [
            library.track(1, "A", "B", "Wma Song", 1, "wma"),
            library.track(2, "A", "B", "Flac Song", 2, "flac"),
        ];
        let converted = library.scratch("converted.mp3", 700);
        let mut asked = Vec::new();
        let run = try_sync(
            &player.storage,
            &profile,
            &items,
            &[],
            &mut |item, profile| {
                asked.push((item.library_id, profile.formats.clone()));
                Ok(if item.library_id == 1 {
                    converted.clone()
                } else {
                    item.source.clone()
                })
            },
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();

        assert_eq!(
            paths(&run.plan.add),
            [
                "Music/A/B/1-01 Wma Song.mp3",
                "Music/A/B/1-02 Flac Song.flac"
            ]
        );
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].1, ["mp3", "flac"]);
        // The converted file is what is on the player, and its size is what is recorded.
        assert_eq!(
            player.read("Music/A/B/1-01 Wma Song.mp3").unwrap(),
            vec![0xAB; 700]
        );
        assert_eq!(player.manifest().items[&1].size, 700);
        assert_eq!(player.manifest().items[&1].source_size, items[0].size);
        assert!(
            converted.exists(),
            "prepare's file is the caller's to delete"
        );
        assert_eq!(
            sync(&player.storage, &profile, &items, &[]).plan.unchanged,
            2
        );
    }

    #[test]
    fn if_prepare_picks_another_type_the_extension_follows_the_file() {
        let (player, library) = (Player::new(), Library::new());
        let profile = Profile {
            formats: vec!["mp3".to_owned(), "m4a".to_owned()],
            ..rockbox()
        };
        let items = [library.track(1, "A", "B", "Song", 1, "wma")];
        let aac = library.scratch("converted.m4a", 300);
        try_sync(
            &player.storage,
            &profile,
            &items,
            &[],
            &mut |_, _| Ok(aac.clone()),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(player.files("Music/"), ["Music/A/B/1-01 Song.m4a"]);
        assert!(player.manifest().pending.is_empty());
    }

    #[test]
    fn formats_in_a_hand_edited_profile_match_whatever_their_case() {
        let (player, library) = (Player::new(), Library::new());
        let profile = Profile {
            formats: vec!["MP3".to_owned(), "FLAC".to_owned()],
            ..rockbox()
        };
        let items = [
            library.track(1, "A", "B", "Kept", 1, "Flac"),
            library.track(2, "A", "B", "Converted", 2, "ogg"),
        ];
        let converted = library.scratch("converted.mp3", 40);
        try_sync(
            &player.storage,
            &profile,
            &items,
            &[],
            &mut |item, _| {
                Ok(if item.library_id == 2 {
                    converted.clone()
                } else {
                    item.source.clone()
                })
            },
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(
            player.files("Music/"),
            ["Music/A/B/1-01 Kept.flac", "Music/A/B/1-02 Converted.mp3"]
        );
    }

    #[test]
    fn an_empty_format_list_means_everything_plays() {
        let (player, library) = (Player::new(), Library::new());
        let profile = Profile {
            formats: Vec::new(),
            ..rockbox()
        };
        sync(
            &player.storage,
            &profile,
            &[library.track(1, "A", "B", "Song", 1, "weird")],
            &[],
        );
        assert_eq!(player.files("Music/"), ["Music/A/B/1-01 Song.weird"]);
    }

    #[test]
    fn a_track_prepare_fails_on_is_skipped_and_the_rest_carry_on() {
        let (player, library) = (Player::new(), Library::new());
        let items = [
            library.track(1, "A", "B", "Good", 1, "flac"),
            library.track(2, "A", "B", "Broken", 2, "flac"),
            library.track(3, "A", "B", "Also good", 3, "flac"),
        ];
        let mut events = Vec::new();
        let run = try_sync(
            &player.storage,
            &rockbox(),
            &items,
            &[],
            &mut |item, _| {
                if item.library_id == 2 {
                    Err(io::Error::other("cannot decode"))
                } else {
                    Ok(item.source.clone())
                }
            },
            &AtomicBool::new(false),
            &mut |event| events.push(event),
        )
        .unwrap();

        assert_eq!(run.plan.add.len(), 3);
        assert_eq!(
            run.summary,
            SyncSummary {
                copied: 2,
                removed: 0,
                skipped: vec![(2, "cannot decode".to_owned())]
            }
        );
        assert_eq!(
            player.manifest().items.keys().copied().collect::<Vec<_>>(),
            [1, 3]
        );
        let skipped: Vec<_> = events
            .iter()
            .filter(|event| event.stage == SyncStage::Skipped)
            .collect();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].current, "Broken: cannot decode");
        // The next sync tries it again.
        let next = plan_with(target(&player.storage, &rockbox()), &items, &[]).unwrap();
        assert_eq!(paths(&next.add), ["Music/A/B/1-02 Broken.flac"]);
        assert_eq!(next.unchanged, 2);
    }

    #[test]
    fn a_prepared_file_that_vanished_is_skipped_too() {
        let (player, library) = (Player::new(), Library::new());
        let items = [library.track(1, "A", "B", "Song", 1, "flac")];
        let mut events = Vec::new();
        try_sync(
            &player.storage,
            &rockbox(),
            &items,
            &[],
            &mut |_, _| Ok("/nonexistent/file.flac".into()),
            &AtomicBool::new(false),
            &mut |event| events.push(event),
        )
        .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.stage == SyncStage::Skipped)
                .count(),
            1
        );
        assert!(player.manifest().items.is_empty());
    }
}

mod space {
    use super::*;

    fn items(library: &Library) -> Vec<SyncItem> {
        vec![
            library.track_of_size(1, "A", "B", "One", 1, "flac", 600),
            library.track_of_size(2, "A", "B", "Two", 2, "flac", 400),
        ]
    }

    #[test]
    fn a_sync_that_does_not_fit_is_refused_before_anything_is_written() {
        let library = Library::new();
        let items = items(&library);
        let device = MemoryStorage::new(RESERVE_BYTES + 999);
        let plan = plan_with(target(&device, &rockbox()), &items, &[]).unwrap();
        assert!(!plan.fits);
        assert_eq!(
            (plan.bytes_needed, plan.free_bytes),
            (1_000, RESERVE_BYTES + 999)
        );

        let err = apply_with(
            target(&device, &rockbox()),
            &plan,
            &items,
            &[],
            &mut copy_source,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(
            matches!(err, Error::NotEnoughSpace { needed: 1_000, available } if available == RESERVE_BYTES + 999),
            "{err}"
        );
        assert!(device.paths().is_empty(), "{:?}", device.paths());
    }

    #[test]
    fn the_reserve_is_what_is_left_over() {
        let library = Library::new();
        let items = items(&library);
        let fits = |capacity: u64| {
            plan_with(
                target(&MemoryStorage::new(capacity), &rockbox()),
                &items,
                &[],
            )
            .unwrap()
            .fits
        };
        assert!(fits(RESERVE_BYTES + 1_000));
        assert!(!fits(RESERVE_BYTES + 999));
        assert!(fits(RESERVE_BYTES * 10));
    }

    /// Fills the device with a user's file until exactly `target` bytes are free.
    fn fill_to_free(device: &MemoryStorage, target: u64) {
        let now = device.free_bytes().unwrap();
        device
            .write(
                "Other/ballast.bin",
                &vec![0; usize::try_from(now - target).unwrap()],
            )
            .unwrap();
    }

    const MIB: u64 = 1 << 20;

    #[test]
    fn removals_make_room_for_what_replaces_them() {
        let library = Library::new();
        let device = MemoryStorage::new(RESERVE_BYTES * 3);
        let old = items(&library);
        sync(&device, &rockbox(), &old, &[]);
        fill_to_free(&device, RESERVE_BYTES + 1_100);

        // The new album is bigger than the room left over, so it cannot be added to the old one...
        let new = [library.track_of_size(10, "C", "D", "Big", 1, "flac", 1_800)];
        let both = [old.clone(), new.to_vec()].concat();
        assert!(
            !plan_with(target(&device, &rockbox()), &both, &[])
                .unwrap()
                .fits
        );
        // ...but it fits once the old one leaves, which happens first.
        let run = sync(&device, &rockbox(), &new, &[]);
        assert!(run.plan.fits);
        assert_eq!(run.plan.remove.len(), 2);
        assert_eq!(
            device
                .paths()
                .iter()
                .filter(|path| path.starts_with("Music/"))
                .count(),
            1
        );
    }

    #[test]
    fn replacing_files_in_place_needs_room_only_for_the_largest() {
        let library = Library::new();
        let changed = |free: u64| {
            let device = MemoryStorage::new(RESERVE_BYTES * 3);
            let items = items(&library);
            sync(&device, &rockbox(), &items, &[]);
            fill_to_free(&device, free);
            let items: Vec<SyncItem> = items.iter().map(|item| library.modified(item)).collect();
            (device, items)
        };

        // Both files grow by 100 bytes: 1 200 are rewritten, more than the 1 100 free, but each file
        // is swapped in turn and the old copy given back.
        let (device, items) = changed(RESERVE_BYTES + 1_100);
        let run = sync(&device, &rockbox(), &items, &[]);
        assert_eq!(run.plan.update.len(), 2);
        assert!(run.plan.fits);
        assert_eq!(run.plan.bytes_needed, 1_200);
        assert_eq!(run.plan.bytes_freed, 1_000);

        // With less than the largest new file (700 bytes) to spare there is nowhere to put it.
        let (device, items) = changed(RESERVE_BYTES + 650);
        assert!(
            !plan_with(target(&device, &rockbox()), &items, &[])
                .unwrap()
                .fits
        );
    }

    #[test]
    fn space_is_checked_again_at_copy_time() {
        let library = Library::new();
        let device = MemoryStorage::new(RESERVE_BYTES * 2);
        let items = items(&library);
        let plan = plan_with(target(&device, &rockbox()), &items, &[]).unwrap();
        assert!(plan.fits);
        // Something else fills the device after the plan was made, leaving room for bookkeeping only.
        fill_to_free(&device, 500 * 1024);
        let err = apply_with(
            target(&device, &rockbox()),
            &plan,
            &items,
            &[],
            &mut copy_source,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(
            matches!(err, Error::NotEnoughSpace { needed: 600, .. }),
            "{err}"
        );
        assert!(device.get("Other/ballast.bin").is_some());
        assert!(!device.paths().iter().any(|path| path.starts_with("Music/")));
        assert!(Manifest::load(&device).unwrap().pending.is_empty());
    }

    #[test]
    fn a_device_that_fills_up_mid_sync_stops_it_with_the_manifest_matching_what_was_copied() {
        let library = Library::new();
        let items: Vec<SyncItem> = (1..=3)
            .map(|id| {
                library.track_of_size(
                    id,
                    "A",
                    "B",
                    &format!("Song {id}"),
                    u32::try_from(id).unwrap(),
                    "flac",
                    5 * MIB as usize,
                )
            })
            .collect();
        let device = MemoryStorage::new(RESERVE_BYTES + 15 * MIB + MIB / 2);
        let plan = plan_with(target(&device, &rockbox()), &items, &[]).unwrap();
        assert!(plan.fits);
        // Other files take the room the plan counted on: two tracks and a megabyte will fit, three won't.
        fill_to_free(&device, 12 * MIB);

        let err = apply_with(
            target(&device, &rockbox()),
            &plan,
            &items,
            &[],
            &mut copy_source,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, Error::NotEnoughSpace { .. }), "{err}");
        let manifest = Manifest::load(&device).unwrap();
        let on_device: Vec<_> = device
            .paths()
            .into_iter()
            .filter(|path| path.starts_with("Music/"))
            .collect();
        let recorded: Vec<_> = manifest
            .items
            .values()
            .map(|entry| entry.path.clone())
            .collect();
        assert_eq!(manifest.items.keys().copied().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(on_device, recorded);
        assert!(manifest.pending.is_empty());
    }
}

mod safety {
    use super::*;

    #[test]
    fn a_plan_is_applied_only_once() {
        let (player, library) = (Player::new(), Library::new());
        let items = [library.track(1, "A", "B", "One", 1, "flac")];
        let plan = plan_with(target(&player.storage, &rockbox()), &items, &[]).unwrap();
        let apply_it = || {
            apply_with(
                target(&player.storage, &rockbox()),
                &plan,
                &items,
                &[],
                &mut copy_source,
                &mut |_| {},
                &AtomicBool::new(false),
            )
        };
        apply_it().unwrap();
        let before = player.tree();
        assert!(matches!(apply_it(), Err(Error::Conflict(_))));
        assert_eq!(player.tree(), before);
    }

    #[test]
    fn a_plan_made_before_another_sync_is_stale() {
        let (player, library) = (Player::new(), Library::new());
        let items = [
            library.track(1, "A", "B", "One", 1, "flac"),
            library.track(2, "A", "B", "Two", 2, "flac"),
        ];
        let stale = plan_with(target(&player.storage, &rockbox()), &items, &[]).unwrap();
        sync(&player.storage, &rockbox(), &items[..1], &[]);
        let err = apply_with(
            target(&player.storage, &rockbox()),
            &stale,
            &items,
            &[],
            &mut copy_source,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err}");
        assert_eq!(player.manifest().items.len(), 1);
    }

    #[test]
    fn a_plan_that_names_a_file_we_did_not_add_deletes_nothing() {
        let (player, library) = (Player::new(), Library::new());
        let items = [library.track(1, "A", "B", "One", 1, "flac")];
        sync(&player.storage, &rockbox(), &items, &[]);
        player.user_file("Music/precious.flac", b"irreplaceable");

        let mut forged = plan_with(target(&player.storage, &rockbox()), &[], &[]).unwrap();
        forged.remove[0].path = "Music/precious.flac".to_owned();
        let err = apply_with(
            target(&player.storage, &rockbox()),
            &forged,
            &[],
            &[],
            &mut copy_source,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err}");
        assert_eq!(
            player.read("Music/precious.flac").unwrap(),
            b"irreplaceable"
        );
        assert!(player.read("Music/A/B/1-01 One.flac").is_some());
    }

    #[test]
    fn a_plan_cannot_write_outside_the_music_folder_or_over_another_track() {
        let (player, library) = (Player::new(), Library::new());
        let items = [
            library.track(1, "A", "B", "One", 1, "flac"),
            library.track(2, "A", "B", "Two", 2, "flac"),
        ];
        sync(&player.storage, &rockbox(), &items[..1], &[]);
        player.user_file("Documents/thesis.txt", b"thesis");
        for bad in [
            "Documents/thesis.txt",
            "Music/../Documents/thesis.txt",
            "Music/A/B/1-01 One.flac",
        ] {
            let mut plan = plan_with(target(&player.storage, &rockbox()), &items, &[]).unwrap();
            plan.add[0].path = bad.to_owned();
            let err = apply_with(
                target(&player.storage, &rockbox()),
                &plan,
                &items,
                &[],
                &mut copy_source,
                &mut |_| {},
                &AtomicBool::new(false),
            )
            .unwrap_err();
            assert!(matches!(err, Error::Conflict(_)), "{bad}: {err}");
        }
        assert_eq!(player.read("Documents/thesis.txt").unwrap(), b"thesis");
    }

    #[test]
    fn an_unreadable_manifest_stops_everything() {
        let (player, library) = (Player::new(), Library::new());
        let items = [library.track(1, "A", "B", "One", 1, "flac")];
        player.user_file(".antistreaming/manifest.json", b"{ this is not json");
        assert!(matches!(
            plan_with(target(&player.storage, &rockbox()), &items, &[]),
            Err(Error::Manifest(_))
        ));
        player.user_file(
            ".antistreaming/manifest.json",
            br#"{"version":99,"revision":0,"items":{},"playlists":{},"pending":[],"trash":[]}"#,
        );
        assert!(matches!(
            plan_with(target(&player.storage, &rockbox()), &items, &[]),
            Err(Error::Manifest(_))
        ));
        assert_eq!(player.files("Music/").len(), 0);
    }

    #[test]
    fn a_bad_profile_is_refused_by_plan_and_apply() {
        let (player, library) = (Player::new(), Library::new());
        let bad = Profile {
            music_dir: "../..".to_owned(),
            ..rockbox()
        };
        let items = [library.track(1, "A", "B", "One", 1, "flac")];
        assert!(matches!(
            plan_with(target(&player.storage, &bad), &items, &[]),
            Err(Error::InvalidProfile(_))
        ));
        let plan = plan_with(target(&player.storage, &rockbox()), &items, &[]).unwrap();
        let err = apply_with(
            target(&player.storage, &bad),
            &plan,
            &items,
            &[],
            &mut copy_source,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidProfile(_)));
        assert!(player.tree().is_empty());
    }

    #[test]
    fn a_full_lifecycle_never_touches_a_single_user_byte() {
        let (player, library) = (Player::new(), Library::new());
        let user = [
            ("Music/Favourites/track.mp3", b"user 1".to_vec()),
            ("Music/Band/Live/1-01 Intro.flac", b"user 2".to_vec()),
            ("Playlists/Mix.m3u8", b"#EXTM3U\r\nuser".to_vec()),
            ("Playlists/Old/readme.txt", b"user 3".to_vec()),
            (".rockbox/config.cfg", b"user 4".to_vec()),
            ("Notes.txt", b"user 5".to_vec()),
            ("Music/.hidden/x", b"user 6".to_vec()),
        ];
        for (path, bytes) in &user {
            player.user_file(path, bytes);
        }
        let items = [
            library.track(1, "Band", "Live", "Intro", 1, "flac"),
            library.track(2, "Band", "Live", "Outro", 2, "flac"),
            library.track(3, "Favourites", "", "track", 1, "mp3"),
        ];
        let playlists = [SyncPlaylist {
            name: "Mix".to_owned(),
            items: vec![3, 1, 2],
        }];
        let check = |step: &str| {
            for (path, bytes) in &user {
                assert_eq!(
                    player.read(path).as_ref(),
                    Some(bytes),
                    "{path} after {step}"
                );
            }
        };
        sync(&player.storage, &rockbox(), &items, &playlists);
        check("first sync");
        let mut changed = items.to_vec();
        changed[0].title = "Intro (Live)".to_owned();
        changed[1] = library.modified(&changed[1]);
        sync(&player.storage, &rockbox(), &changed, &playlists);
        check("changes");
        sync(&player.storage, &rockbox(), &[], &[]);
        check("emptying");
        // What the sync added is gone.
        assert!(player.manifest().items.is_empty() && player.manifest().playlists.is_empty());
        assert_eq!(
            player
                .tree()
                .keys()
                .filter(|path| !path.starts_with(".antistreaming/"))
                .count(),
            user.len()
        );
    }
}

mod interruptions {
    use super::*;

    fn album(library: &Library) -> Vec<SyncItem> {
        (1..=5)
            .map(|id| {
                library.track(
                    id,
                    "Band",
                    "Album",
                    &format!("Song {id}"),
                    u32::try_from(id).unwrap(),
                    "flac",
                )
            })
            .collect()
    }

    #[test]
    fn a_failure_mid_way_leaves_the_manifest_matching_the_disk_and_the_next_sync_finishes() {
        let (player, library) = (Player::new(), Library::new());
        let items = album(&library);
        let flaky = Faulty::failing_put(&player.storage, 3, false);
        let err = try_sync(
            &flaky,
            &rockbox(),
            &items,
            &[],
            &mut copy_source,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .map(drop)
        .unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "{err}");

        // Two tracks made it; the manifest says exactly that, and nothing half-written remains.
        let manifest = player.manifest();
        assert_eq!(manifest.items.keys().copied().collect::<Vec<_>>(), [1, 2]);
        assert!(manifest.pending.is_empty() && manifest.trash.is_empty());
        assert_eq!(player.files("Music/").len(), 2);
        assert!(player.tree().keys().all(|path| !path.contains("as-part")));

        let run = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!((run.plan.add.len(), run.plan.unchanged), (3, 2));
        assert_eq!(player.files("Music/").len(), 5);
        assert_eq!(player.manifest().items.len(), 5);
    }

    #[test]
    fn a_pulled_cable_leaves_a_state_the_next_sync_cleans_up_without_duplicates() {
        let (player, library) = (Player::new(), Library::new());
        let items = album(&library);
        player.user_file("Music/Band/Album/keep me.txt", b"user");

        // The third copy dies with the cable: nothing on the player can be reached afterwards, not
        // even to record the two tracks that were already copied.
        let unplugged = Faulty::failing_put(&player.storage, 3, true);
        try_sync(
            &unplugged,
            &rockbox(),
            &items,
            &[],
            &mut copy_source,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .map(drop)
        .unwrap_err();
        let stranded = player.tree();
        assert!(
            stranded.keys().any(|path| path.ends_with(".as-part")),
            "a half-written file was left"
        );
        assert_eq!(
            player.manifest().items.len(),
            0,
            "the batch was never recorded"
        );
        assert!(!player.manifest().pending.is_empty());

        // Plugged back in: the leftovers are recognised as ours and cleared, then the sync completes.
        let run = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!(run.plan.add.len(), 5);
        assert_eq!(
            player.files("Music/"),
            [
                "Music/Band/Album/1-01 Song 1.flac",
                "Music/Band/Album/1-02 Song 2.flac",
                "Music/Band/Album/1-03 Song 3.flac",
                "Music/Band/Album/1-04 Song 4.flac",
                "Music/Band/Album/1-05 Song 5.flac",
                "Music/Band/Album/keep me.txt",
            ],
            "no duplicates, no leftovers"
        );
        assert_eq!(
            player.read("Music/Band/Album/keep me.txt").unwrap(),
            b"user"
        );
        let manifest = player.manifest();
        assert!(manifest.pending.is_empty());
        assert_eq!(manifest.items.len(), 5);
    }

    #[test]
    fn leftovers_of_a_dead_sync_never_include_a_user_file() {
        let (player, library) = (Player::new(), Library::new());
        let items = album(&library);
        // The user's file sits where our first track would go.
        player.user_file("Music/Band/Album/1-01 Song 1.flac", b"user's");
        let unplugged = Faulty::failing_put(&player.storage, 2, true);
        try_sync(
            &unplugged,
            &rockbox(),
            &items,
            &[],
            &mut copy_source,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .map(drop)
        .unwrap_err();
        sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!(
            player.read("Music/Band/Album/1-01 Song 1.flac").unwrap(),
            b"user's"
        );
        assert_eq!(
            player.manifest().items[&1].path,
            "Music/Band/Album/1-01 Song 1 (2).flac"
        );
    }

    #[test]
    fn cancelling_stops_cleanly_and_the_next_sync_carries_on() {
        let (player, library) = (Player::new(), Library::new());
        let items = album(&library);
        let cancel = AtomicBool::new(false);
        let err = try_sync(
            &player.storage,
            &rockbox(),
            &items,
            &[],
            &mut copy_source,
            &cancel,
            &mut |event| {
                if event.stage == SyncStage::Copying && event.done == 2 {
                    cancel.store(true, Ordering::SeqCst);
                }
            },
        )
        .map(drop)
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled));
        assert_eq!(
            player.manifest().items.keys().copied().collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(player.tree().keys().all(|path| !path.contains("as-part")));
        assert!(player.manifest().pending.is_empty());

        let run = sync(&player.storage, &rockbox(), &items, &[]);
        assert_eq!((run.plan.add.len(), run.plan.unchanged), (3, 2));
    }

    #[test]
    fn cancelling_before_starting_changes_nothing() {
        let (player, library) = (Player::new(), Library::new());
        let items = album(&library);
        let err = try_sync(
            &player.storage,
            &rockbox(),
            &items,
            &[],
            &mut copy_source,
            &AtomicBool::new(true),
            &mut |_| {},
        )
        .map(drop)
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled));
        assert!(player.files("Music/").is_empty());
    }

    #[test]
    fn a_removal_cut_off_before_deleting_is_finished_by_the_next_sync() {
        let (player, library) = (Player::new(), Library::new());
        let items = album(&library);
        sync(&player.storage, &rockbox(), &items, &[]);
        // The manifest dropped track 5 and listed its file for deletion, then the cable went.
        let mut manifest = player.manifest();
        let gone = manifest.items.remove(&5).unwrap();
        manifest.trash.push(gone.path.clone());
        manifest.save(&player.storage, true).unwrap();
        assert!(player.read(&gone.path).is_some());

        let run = sync(&player.storage, &rockbox(), &items[..4], &[]);
        assert_eq!(run.plan.unchanged, 4);
        assert!(player.read(&gone.path).is_none());
        assert!(player.manifest().trash.is_empty());
    }

    #[test]
    fn trash_that_is_wanted_again_is_not_deleted() {
        let (player, library) = (Player::new(), Library::new());
        let items = album(&library);
        sync(&player.storage, &rockbox(), &items, &[]);
        let mut manifest = player.manifest();
        let path = manifest.items[&2].path.clone();
        manifest.trash.push(path.clone());
        manifest.save(&player.storage, false).unwrap();
        sync(&player.storage, &rockbox(), &items, &[]);
        assert!(player.read(&path).is_some());
        assert_eq!(player.manifest().items.len(), 5);
    }

    #[test]
    fn many_tracks_are_recorded_in_batches() {
        let (player, library) = (Player::new(), Library::new());
        let items: Vec<SyncItem> = (1..=250)
            .map(|id| library.track_of_size(id, "A", "B", &format!("T{id}"), 1, "mp3", 10))
            .collect();
        let mut saves = 0;
        try_sync(
            &player.storage,
            &rockbox(),
            &items,
            &[],
            &mut copy_source,
            &AtomicBool::new(false),
            &mut |event| {
                if event.stage == SyncStage::Copying && (event.done == 100 || event.done == 200) {
                    // At each batch boundary the manifest already holds everything before it.
                    saves += 1;
                    assert_eq!(
                        player.manifest().items.len(),
                        usize::try_from(event.done).unwrap()
                    );
                }
            },
        )
        .unwrap();
        assert_eq!(saves, 2);
        assert_eq!(player.manifest().items.len(), 250);
    }
}

mod playlists {
    use super::*;

    fn library_of_three(library: &Library) -> Vec<SyncItem> {
        vec![
            library.track(1, "The Beatles", "Abbey Road", "Come Together", 1, "flac"),
            library.track(2, "The Beatles", "Abbey Road", "Something", 2, "flac"),
            library.track(3, "Daft Punk", "Discovery", "One More Time", 1, "mp3"),
        ]
    }

    #[test]
    fn written_as_m3u8_with_relative_paths_in_the_playlist_folder() {
        let (player, library) = (Player::new(), Library::new());
        let items = library_of_three(&library);
        let playlists = [SyncPlaylist {
            name: "Road Trip".to_owned(),
            items: vec![3, 1],
        }];
        let run = sync(&player.storage, &rockbox(), &items, &playlists);
        assert_eq!(run.plan.write_playlists, ["Road Trip"]);
        assert_eq!(
            String::from_utf8(player.read("Playlists/Road Trip.m3u8").unwrap()).unwrap(),
            "#EXTM3U\r\n#EXTINF:-1,Daft Punk - One More Time\r\n../Music/Daft Punk/Discovery/1-01 One More Time.mp3\r\n\
             #EXTINF:-1,The Beatles - Come Together\r\n../Music/The Beatles/Abbey Road/1-01 Come Together.flac\r\n"
        );
        assert_eq!(
            player.manifest().playlists["Road Trip"],
            "Playlists/Road Trip.m3u8"
        );
    }

    #[test]
    fn tracks_that_are_not_on_the_device_are_left_out() {
        let (player, library) = (Player::new(), Library::new());
        let items = library_of_three(&library);
        let playlists = [SyncPlaylist {
            name: "Mixed".to_owned(),
            items: vec![1, 99, 3],
        }];
        // Track 3 fails to prepare, so it never reaches the player.
        try_sync(
            &player.storage,
            &rockbox(),
            &items,
            &playlists,
            &mut |item, _| {
                if item.library_id == 3 {
                    Err(io::Error::other("no"))
                } else {
                    Ok(item.source.clone())
                }
            },
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        let text = String::from_utf8(player.read("Playlists/Mixed.m3u8").unwrap()).unwrap();
        assert!(text.contains("Come Together") && !text.contains("One More Time"));
        assert_eq!(text.matches("#EXTINF").count(), 1);
    }

    #[test]
    fn follow_renames_and_disappear_when_no_longer_wanted() {
        let (player, library) = (Player::new(), Library::new());
        let mut items = library_of_three(&library);
        let playlists = [
            SyncPlaylist {
                name: "A".to_owned(),
                items: vec![1],
            },
            SyncPlaylist {
                name: "B".to_owned(),
                items: vec![2],
            },
        ];
        sync(&player.storage, &rockbox(), &items, &playlists);

        items[0].title = "Come Together (2019 Mix)".to_owned();
        let run = sync(&player.storage, &rockbox(), &items, &playlists[..1]);
        assert_eq!(run.plan.remove_playlists, ["B"]);
        let text = String::from_utf8(player.read("Playlists/A.m3u8").unwrap()).unwrap();
        assert!(
            text.contains("../Music/The Beatles/Abbey Road/1-01 Come Together (2019 Mix).flac"),
            "{text}"
        );
        assert!(player.read("Playlists/B.m3u8").is_none());
        assert_eq!(
            player
                .manifest()
                .playlists
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["A"]
        );
    }

    #[test]
    fn a_playlist_the_user_made_is_never_overwritten() {
        let (player, library) = (Player::new(), Library::new());
        player.user_file("Playlists/Favourites.m3u8", b"#EXTM3U\r\nmine.mp3\r\n");
        let items = library_of_three(&library);
        let playlists = [SyncPlaylist {
            name: "Favourites".to_owned(),
            items: vec![1],
        }];
        sync(&player.storage, &rockbox(), &items, &playlists);
        assert_eq!(
            player.read("Playlists/Favourites.m3u8").unwrap(),
            b"#EXTM3U\r\nmine.mp3\r\n"
        );
        assert_eq!(
            player.manifest().playlists["Favourites"],
            "Playlists/Favourites (2).m3u8"
        );
        // A second sync keeps writing to the same file.
        sync(&player.storage, &rockbox(), &items, &playlists);
        assert_eq!(
            player.files("Playlists/"),
            ["Playlists/Favourites (2).m3u8", "Playlists/Favourites.m3u8"]
        );
    }

    #[test]
    fn pls_is_written_when_the_profile_asks_and_a_format_change_moves_the_file() {
        let (player, library) = (Player::new(), Library::new());
        let items = library_of_three(&library);
        let playlists = [SyncPlaylist {
            name: "Mix".to_owned(),
            items: vec![3],
        }];
        sync(&player.storage, &rockbox(), &items, &playlists);
        assert!(player.read("Playlists/Mix.m3u8").is_some());

        let pls = Profile {
            playlist_format: PlaylistFormat::Pls,
            ..rockbox()
        };
        sync(&player.storage, &pls, &items, &playlists);
        let text = String::from_utf8(player.read("Playlists/Mix.pls").unwrap()).unwrap();
        assert!(
            text.starts_with("[playlist]")
                && text.contains("File1=../Music/Daft Punk/Discovery/1-01 One More Time.mp3")
        );
        assert!(
            player.read("Playlists/Mix.m3u8").is_none(),
            "the old format's file is ours to remove"
        );
        assert_eq!(player.manifest().playlists["Mix"], "Playlists/Mix.pls");
    }

    #[test]
    fn unsafe_playlist_names_are_made_safe() {
        let (player, library) = (Player::new(), Library::new());
        let items = library_of_three(&library);
        sync(
            &player.storage,
            &rockbox(),
            &items,
            &[SyncPlaylist {
                name: "Rock/Pop: 90s?".to_owned(),
                items: vec![1],
            }],
        );
        assert_eq!(
            player.files("Playlists/"),
            ["Playlists/Rock_Pop_ 90s_.m3u8"]
        );
    }

    #[test]
    fn an_empty_library_leaves_valid_empty_playlists() {
        let player = Player::new();
        sync(
            &player.storage,
            &rockbox(),
            &[],
            &[SyncPlaylist {
                name: "Empty".to_owned(),
                items: vec![],
            }],
        );
        assert_eq!(player.read("Playlists/Empty.m3u8").unwrap(), b"#EXTM3U\r\n");
    }
}

mod devices {
    use super::*;

    #[test]
    fn a_rockbox_player_syncs_to_music_and_playlists() {
        let (root, library) = (tempfile::tempdir().unwrap(), Library::new());
        let mount = root.path().join("PLAYER");
        fs::create_dir_all(mount.join(".rockbox")).unwrap();
        let device =
            crate::discover::discover_in([mount.clone()], &*crate::mtp::system()).remove(0);
        assert_eq!(device.kind, DeviceKind::Rockbox);

        let items = [library.track(1, "A", "B", "Song", 1, "flac")];
        let playlists = [SyncPlaylist {
            name: "Mix".to_owned(),
            items: vec![1],
        }];
        let plan = plan(&device, &items, &playlists).unwrap();
        apply(
            &device,
            &plan,
            &items,
            &playlists,
            &mut copy_source,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();

        assert!(mount.join("Music/A/B/1-01 Song.flac").is_file());
        assert!(mount.join("Playlists/Mix.m3u8").is_file());
        assert!(mount.join(".antistreaming/manifest.json").is_file());
        // The .rockbox folder was not touched.
        assert_eq!(fs::read_dir(mount.join(".rockbox")).unwrap().count(), 0);
    }

    #[test]
    fn a_marked_stick_uses_its_own_profile() {
        let (root, library) = (tempfile::tempdir().unwrap(), Library::new());
        let mount = root.path().join("STICK");
        fs::create_dir(&mount).unwrap();
        let profile = Profile {
            music_dir: "Songs".to_owned(),
            playlist_dir: "Songs/Lists".to_owned(),
            formats: vec!["mp3".to_owned()],
            ..Profile::default()
        };
        crate::mark_device(&mount, "Stick", &profile).unwrap();
        let device =
            crate::discover::discover_in([mount.clone()], &*crate::mtp::system()).remove(0);
        let items = [library.track(1, "A", "B", "Song", 1, "flac")];
        let playlists = [SyncPlaylist {
            name: "Mix".to_owned(),
            items: vec![1],
        }];
        let plan = plan(&device, &items, &playlists).unwrap();
        // The flac cannot play on this stick, so it is converted to the preferred format.
        let converted = library.scratch("song.mp3", 50);
        apply(
            &device,
            &plan,
            &items,
            &playlists,
            &mut |_, _| Ok(converted.clone()),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(mount.join("Songs/A/B/1-01 Song.mp3").is_file());
        let text = fs::read_to_string(mount.join("Songs/Lists/Mix.m3u8")).unwrap();
        assert!(text.contains("\r\n../A/B/1-01 Song.mp3\r\n"), "{text}");
        assert!(mount.join(crate::MARKER_FILE).is_file());
    }

    struct Phone(std::sync::Arc<MemoryStorage>);

    impl MtpBackend for Phone {
        fn devices(&self) -> Result<Vec<MtpDevice>> {
            Ok(vec![MtpDevice {
                id: "SER".to_owned(),
                name: "Pixel".to_owned(),
                storages: vec![MtpStorageInfo {
                    id: 1,
                    description: "Internal".to_owned(),
                    capacity_bytes: 1 << 30,
                    free_bytes: 1 << 29,
                }],
            }])
        }

        fn open(&self, _: &str, _: u32) -> Result<Box<dyn Storage>> {
            Ok(Box::new(self.0.clone()))
        }
    }

    #[test]
    fn a_phone_syncs_through_the_same_planner_and_applier() {
        let library = Library::new();
        let memory = std::sync::Arc::new(MemoryStorage::new(1 << 30));
        let backend = Phone(memory.clone());
        let device = crate::discover::discover_in([], &backend).remove(0);
        assert_eq!(
            (device.kind, &device.root),
            (
                DeviceKind::Mtp,
                &DeviceRoot::Mtp {
                    device: "SER".to_owned(),
                    storage: 1
                }
            )
        );
        assert_eq!(device.profile, Profile::android());

        let storage = backend.open("SER", 1).unwrap();
        let mut items = vec![
            library.track(1, "A", "B", "One", 1, "flac"),
            library.track(2, "A", "B", "Two", 2, "ogg"),
        ];
        let playlists = [SyncPlaylist {
            name: "Mix".to_owned(),
            items: vec![2, 1],
        }];
        sync(&*storage, &device.profile, &items, &playlists);
        assert_eq!(
            memory.paths(),
            [
                ".antistreaming/manifest.json",
                "Music/A/B/1-01 One.flac",
                "Music/A/B/1-02 Two.ogg",
                "Playlists/Mix.m3u8",
            ]
        );

        // Re-syncing and removing work the same way.
        items.remove(0);
        let run = sync(&*storage, &device.profile, &items, &playlists);
        assert_eq!((run.plan.remove.len(), run.plan.unchanged), (1, 1));
        assert!(memory.get("Music/A/B/1-01 One.flac").is_none());
    }

    #[test]
    fn this_build_has_no_mtp_backend_so_a_phone_cannot_be_opened() {
        let device = Device {
            id: "x".to_owned(),
            name: "Phone".to_owned(),
            kind: DeviceKind::Mtp,
            root: DeviceRoot::Mtp {
                device: "SER".to_owned(),
                storage: 1,
            },
            capacity_bytes: 0,
            free_bytes: 0,
            profile: Profile::android(),
        };
        assert!(matches!(
            plan(&device, &[], &[]),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            apply(
                &device,
                &plan_for_nothing(),
                &[],
                &[],
                &mut copy_source,
                &mut |_| {},
                &AtomicBool::new(false)
            ),
            Err(Error::Unsupported(_))
        ));
    }

    fn plan_for_nothing() -> SyncPlan {
        let storage = MemoryStorage::new(1 << 30);
        plan_with(target(&storage, &Profile::android()), &[], &[]).unwrap()
    }
}
