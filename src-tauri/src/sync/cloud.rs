use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::{SyncState, WORK_DIR, lock, to_error};
use crate::error::CommandResult;

/// How a sync reaches songs that are only in iCloud; a test stands in for the real thing.
pub(super) trait Cloud {
    /// Whether the file is a placeholder whose contents aren't on this Mac.
    fn is_placeholder(&self, path: &Path) -> bool;
    /// Downloads it, reporting (bytes so far, total) and giving up once `stop` is set.
    fn download(&self, path: &Path, progress: &mut dyn FnMut(u64, u64), stop: &AtomicBool) -> unsub_library::Result<()>;
    /// Gives the downloaded copy back to iCloud, freeing the space.
    fn release(&self, path: &Path) -> unsub_library::Result<()>;
}

pub(super) struct ICloud;

impl Cloud for ICloud {
    fn is_placeholder(&self, path: &Path) -> bool {
        unsub_library::is_dataless(path)
    }

    fn download(&self, path: &Path, progress: &mut dyn FnMut(u64, u64), stop: &AtomicBool) -> unsub_library::Result<()> {
        unsub_library::ensure_local(path, progress, stop)
    }

    fn release(&self, path: &Path) -> unsub_library::Result<()> {
        unsub_library::evict(path)
    }
}

const LEFTOVERS_FILE: &str = "Device Sync Downloads.json";

/// A download a sync asked iCloud for and then stopped waiting for, because it was stopped or
/// gave up. The request is still out there, so the song lands on this Mac whenever iCloud gets to
/// it, and nothing else would give it back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Leftover {
    path: PathBuf,
    /// The iPod whose sync asked, whose "remove downloaded songs" setting decides.
    device: String,
    /// Unix seconds.
    at: i64,
}

/// How long a song is waited for: a request iCloud has not answered in a day never will be, and
/// the song may have been downloaded and wanted since.
const LEFTOVER_LIFE_SECS: i64 = 24 * 60 * 60;

/// The [`Leftover`]s, kept in a small file in the app cache until a later sync or the next launch
/// finds them finished and gives them back.
pub(super) struct Leftovers {
    file: PathBuf,
}

/// Held while the file is read and written back, since a sync, playback and launch all use it.
static LEFTOVERS: Mutex<()> = Mutex::new(());

impl Leftovers {
    fn load(&self, now: i64) -> Vec<Leftover> {
        let all: Vec<Leftover> = std::fs::read(&self.file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        all.into_iter().filter(|left| now - left.at < LEFTOVER_LIFE_SECS).collect()
    }

    /// A list that can't be saved is a download that stays on the Mac, not a failed sync.
    fn save(&self, all: &[Leftover]) {
        if all.is_empty() {
            let _ = std::fs::remove_file(&self.file);
        } else if let Ok(json) = serde_json::to_vec(all) {
            let _ = self.file.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(&self.file, json);
        }
    }

    pub(super) fn add(&self, paths: &[PathBuf], device: &str, now: i64) {
        if paths.is_empty() {
            return;
        }
        let _held = lock(&LEFTOVERS);
        let mut all = self.load(now);
        all.retain(|left| !paths.contains(&left.path));
        all.extend(paths.iter().map(|path| Leftover { path: path.clone(), device: device.to_owned(), at: now }));
        self.save(&all);
    }

    /// The song is wanted on this Mac after all, as when it is played.
    fn forget(&self, path: &Path, now: i64) {
        let _held = lock(&LEFTOVERS);
        let mut all = self.load(now);
        let before = all.len();
        all.retain(|left| left.path != path);
        if all.len() != before {
            self.save(&all);
        }
    }

    /// Gives back the downloads that have finished since, where their iPod's setting says to,
    /// forgets those whose file is gone, and keeps waiting for the rest.
    pub(super) fn release(&self, cloud: &dyn Cloud, giving_back: &dyn Fn(&str) -> bool, now: i64) {
        let _held = lock(&LEFTOVERS);
        let waiting: Vec<Leftover> = self
            .load(now)
            .into_iter()
            .filter(|left| {
                if !left.path.exists() {
                    return false;
                }
                if cloud.is_placeholder(&left.path) {
                    return true;
                }
                if giving_back(&left.device) {
                    let _ = cloud.release(&left.path);
                }
                false
            })
            .collect();
        self.save(&waiting);
    }
}

pub(super) fn leftovers_of(app: &AppHandle) -> CommandResult<Leftovers> {
    Ok(Leftovers { file: app.path().app_cache_dir().map_err(to_error)?.join(LEFTOVERS_FILE) })
}

/// Whether the iPod's sync settings say to give downloaded songs back.
pub(super) fn giving_back(app: &AppHandle) -> impl Fn(&str) -> bool + use<> {
    let app = app.clone();
    move |device| app.state::<SyncState>().settings_for(device).remove_downloaded
}

/// A song that was a leftover is wanted on this Mac now, so it is not to be given back.
pub fn forget_leftover(app: &AppHandle, path: &Path) {
    if let Ok(leftovers) = leftovers_of(app) {
        leftovers.forget(path, crate::playback::now_unix());
    }
}

/// Cleans up after a sync the app did not live to finish: clears the cache of converted songs and
/// covers, and gives back the downloads that finished after it stopped waiting for them.
pub fn tidy(app: &AppHandle) {
    let Ok(cache) = app.path().app_cache_dir() else { return };
    let _ = std::fs::remove_dir_all(cache.join(WORK_DIR));
    let giving_back = giving_back(app);
    // Each song is a call to the system, so not while the app is starting.
    std::thread::spawn(move || {
        Leftovers { file: cache.join(LEFTOVERS_FILE) }.release(&ICloud, &giving_back, crate::playback::now_unix());
    });
}

/// The covers of songs that came down from iCloud, read while the song is here: it is given back
/// before the iPod's artwork is written, and a cover can't be read from a placeholder. Songs of an
/// album mostly share one picture, so each distinct picture is stored once.
pub(super) struct Covers {
    dir: PathBuf,
    by_song: RefCell<HashMap<i64, u64>>,
}

impl Covers {
    pub(super) fn new(dir: PathBuf) -> Self {
        Self { dir, by_song: RefCell::default() }
    }

    fn file(&self, key: u64) -> PathBuf {
        self.dir.join(format!("{key:016x}"))
    }

    /// A cover that can't be kept costs the song its picture, not the sync.
    pub(super) fn keep(&self, song: i64, picture: &[u8]) {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        picture.hash(&mut hasher);
        let key = hasher.finish();
        let file = self.file(key);
        if file.exists() || std::fs::create_dir_all(&self.dir).and_then(|()| std::fs::write(&file, picture)).is_ok() {
            self.by_song.borrow_mut().insert(song, key);
        }
    }

    pub(super) fn get(&self, song: i64) -> Option<Vec<u8>> {
        let key = *self.by_song.borrow().get(&song)?;
        std::fs::read(self.file(key)).ok()
    }

    pub(super) fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
pub(super) mod fake {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::Cloud;

    /// A cloud whose songs are the files in `placeholders` until they are downloaded.
    #[derive(Default)]
    pub(crate) struct FakeCloud<'a> {
        pub(crate) placeholders: RefCell<HashSet<PathBuf>>,
        pub(crate) downloads: RefCell<Vec<PathBuf>>,
        pub(crate) released: RefCell<Vec<PathBuf>>,
        /// Downloads of these fail, as when the file is gone or the account signed out.
        pub(crate) failing: HashSet<PathBuf>,
        /// Downloads of these never make progress, as when the Mac is offline.
        pub(crate) hanging: HashSet<PathBuf>,
        /// A download that makes no progress for this long before it completes, as when the Mac is
        /// offline (for ever, with `Duration::MAX`) or the file is big and slow.
        pub(crate) slow: Option<Duration>,
        /// The size the download reports, 100 bytes if not given.
        pub(crate) total: Option<u64>,
        /// Sets this flag as soon as a download starts, as a click on Stop would.
        pub(crate) cancels: Option<&'a AtomicBool>,
    }

    impl Cloud for FakeCloud<'_> {
        fn is_placeholder(&self, path: &Path) -> bool {
            self.placeholders.borrow().contains(path)
        }

        fn download(&self, path: &Path, progress: &mut dyn FnMut(u64, u64), stop: &AtomicBool) -> unsub_library::Result<()> {
            self.downloads.borrow_mut().push(path.to_owned());
            if let Some(cancel) = self.cancels {
                cancel.store(true, Ordering::Relaxed);
            }
            if self.failing.contains(path) {
                return Err(io::Error::other("offline").into());
            }
            let total = self.total.unwrap_or(100);
            let started = Instant::now();
            let slow = if self.hanging.contains(path) { Some(Duration::MAX) } else { self.slow };
            while slow.is_some_and(|slow| started.elapsed() < slow) {
                progress(0, total);
                if stop.load(Ordering::Relaxed) {
                    return Err(unsub_library::Error::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            self.placeholders.borrow_mut().remove(path);
            progress(total, total);
            Ok(())
        }

        fn release(&self, path: &Path) -> unsub_library::Result<()> {
            self.released.borrow_mut().push(path.to_owned());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashSet;

    use super::fake::FakeCloud;
    use super::*;

    #[test]
    fn downloads_left_unfinished_are_given_back_once_they_finish_and_forgotten_once_gone() {
        let dir = tempfile::tempdir().unwrap();
        let (done, waiting, gone) = (dir.path().join("done.mp3"), dir.path().join("waiting.mp3"), dir.path().join("gone.mp3"));
        std::fs::write(&done, b"x").unwrap();
        std::fs::write(&waiting, b"x").unwrap();
        let leftovers = Leftovers { file: dir.path().join("cache").join("leftovers.json") };
        let always = |_: &str| true;
        leftovers.add(&[done.clone(), waiting.clone(), gone], "ipod", 1_000);
        leftovers.add(std::slice::from_ref(&waiting), "ipod", 1_010);
        assert_eq!(leftovers.load(1_020).len(), 3, "a path is listed once");
        let cloud = FakeCloud { placeholders: RefCell::new(HashSet::from([waiting.clone()])), ..FakeCloud::default() };

        leftovers.release(&cloud, &always, 1_030);

        assert_eq!(*cloud.released.borrow(), [done]);
        let kept = leftovers.load(1_030);
        assert_eq!(kept.iter().map(|left| &left.path).collect::<Vec<_>>(), [&waiting]);
        // Nothing more has finished, so another go does nothing.
        leftovers.release(&cloud, &always, 1_040);
        assert_eq!(cloud.released.borrow().len(), 1);
        cloud.placeholders.borrow_mut().clear();
        leftovers.release(&cloud, &always, 1_050);
        assert_eq!(cloud.released.borrow().len(), 2);
        assert!(!leftovers.file.exists(), "an empty list is no file");
    }

    #[test]
    fn a_leftover_is_given_back_only_where_its_ipods_setting_says_so_and_only_for_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let (on, off, old) = (dir.path().join("on.mp3"), dir.path().join("off.mp3"), dir.path().join("old.mp3"));
        for file in [&on, &off, &old] {
            std::fs::write(file, b"x").unwrap();
        }
        let leftovers = Leftovers { file: dir.path().join("leftovers.json") };
        leftovers.add(std::slice::from_ref(&old), "on", 0);
        leftovers.add(std::slice::from_ref(&on), "on", 100);
        leftovers.add(std::slice::from_ref(&off), "off", 100);
        let cloud = FakeCloud::default();

        leftovers.release(&cloud, &|device: &str| device == "on", LEFTOVER_LIFE_SECS + 50);

        assert_eq!(
            *cloud.released.borrow(),
            std::slice::from_ref(&on),
            "not the other iPod's, and not the one nobody has asked about in a day"
        );
        assert!(leftovers.load(LEFTOVER_LIFE_SECS + 50).is_empty());
    }

    #[test]
    fn a_song_that_is_played_is_taken_off_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let (played, other) = (dir.path().join("played.mp3"), dir.path().join("other.mp3"));
        let leftovers = Leftovers { file: dir.path().join("leftovers.json") };
        leftovers.add(&[played.clone(), other.clone()], "ipod", 5);

        leftovers.forget(&played, 6);

        let kept = leftovers.load(6);
        assert_eq!(kept.iter().map(|left| &left.path).collect::<Vec<_>>(), [&other]);
        leftovers.forget(&other, 7);
        assert!(!leftovers.file.exists());
    }
}
