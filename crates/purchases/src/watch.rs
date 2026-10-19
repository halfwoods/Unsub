use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::Result;

/// How long the directories must be quiet before the callback fires. It matches the age a file
/// needs before `Inbox::scan` will look at it, so a scan started by the callback sees a finished
/// download as settled.
const QUIET: Duration = Duration::from_secs(5);

/// Keeps the watch alive; dropping it stops watching and ends the callback thread.
pub struct WatchHandle {
    _watcher: RecommendedWatcher,
}

/// Calls `on_change` once after each burst of file changes in `dirs` (and anything inside
/// them), once the burst has been quiet for a few seconds. Directories that do not exist are
/// skipped. The callback runs on a background thread; the app answers it with `Inbox::scan`.
pub fn watch(dirs: Vec<PathBuf>, on_change: impl Fn() + Send + 'static) -> Result<WatchHandle> {
    watch_after(dirs, QUIET, on_change)
}

pub(crate) fn watch_after(
    dirs: Vec<PathBuf>,
    quiet: Duration,
    on_change: impl Fn() + Send + 'static,
) -> Result<WatchHandle> {
    let (changes, burst) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        // Reading a file raises access events on some platforms, and scanning reads files, so
        // reacting to them would make every scan trigger the next one.
        let is_access = matches!(&event, Ok(event) if matches!(event.kind, EventKind::Access(_)));
        if !is_access {
            // The receiver is gone only once the handle is dropped.
            let _ = changes.send(());
        }
    })?;
    for dir in dirs.iter().filter(|dir| dir.is_dir()) {
        watcher.watch(dir, RecursiveMode::Recursive)?;
    }

    thread::spawn(move || {
        while burst.recv().is_ok() {
            loop {
                match burst.recv_timeout(quiet) {
                    Ok(()) => {}
                    Err(RecvTimeoutError::Timeout) => break,
                    // The handle was dropped mid-burst: stop without calling back.
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            on_change();
        }
    });
    Ok(WatchHandle { _watcher: watcher })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use super::*;

    const QUIET_FOR_TESTS: Duration = Duration::from_millis(400);

    #[test]
    fn a_burst_of_changes_calls_back_once() {
        let dir = tempfile::tempdir().unwrap();
        let (fired, calls) = channel();
        let _handle = watch_after(vec![dir.path().to_owned()], QUIET_FOR_TESTS, move || {
            fired.send(()).unwrap();
        })
        .unwrap();

        for number in 0..5 {
            fs::write(dir.path().join(format!("track-{number}.flac")), b"audio").unwrap();
        }

        calls
            .recv_timeout(Duration::from_secs(10))
            .expect("a callback after the burst");
        assert!(
            calls.recv_timeout(QUIET_FOR_TESTS * 4).is_err(),
            "one burst must not call back twice"
        );
    }

    #[test]
    fn changes_in_subfolders_count() {
        let dir = tempfile::tempdir().unwrap();
        let album = dir.path().join("Album");
        fs::create_dir(&album).unwrap();
        let (fired, calls) = channel();
        let _handle = watch_after(vec![dir.path().to_owned()], QUIET_FOR_TESTS, move || {
            fired.send(()).unwrap();
        })
        .unwrap();

        fs::write(album.join("01 Song.flac"), b"audio").unwrap();

        calls
            .recv_timeout(Duration::from_secs(10))
            .expect("a callback for the nested file");
    }

    #[test]
    fn dropping_the_handle_stops_the_callbacks() {
        let dir = tempfile::tempdir().unwrap();
        let (fired, calls) = channel();
        let handle = watch_after(vec![dir.path().to_owned()], QUIET_FOR_TESTS, move || {
            let _ = fired.send(());
        })
        .unwrap();
        drop(handle);

        fs::write(dir.path().join("late.flac"), b"audio").unwrap();

        assert!(calls.recv_timeout(QUIET_FOR_TESTS * 5).is_err());
    }

    #[test]
    fn missing_directories_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no such folder");
        assert!(watch_after(vec![missing], QUIET_FOR_TESTS, || {}).is_ok());
    }
}
