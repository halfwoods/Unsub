use std::path::PathBuf;

use crate::device::recognise;

/// Mount points of iPods: those with `iPod_Control/iTunes/iTunesDB` or `iTunesCDB`, and those an
/// iPod has not written a database to yet (see [`recognise`]).
///
/// Scans macOS `/Volumes/*`, Windows drive letters `A:` to `Z:`, and on Linux `/media/$USER/*`,
/// `/run/media/$USER/*`, `/media/*` and `/mnt/*`.
pub fn discover() -> Vec<PathBuf> {
    ipod_mounts(candidate_mounts())
}

fn ipod_mounts(candidates: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut mounts: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|mount| recognise(mount).is_some())
        .collect();
    mounts.sort();
    mounts.dedup();
    mounts
}

#[cfg(windows)]
fn candidate_mounts() -> Vec<PathBuf> {
    (b'A'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", char::from(letter))))
        .collect()
}

#[cfg(not(windows))]
fn candidate_mounts() -> Vec<PathBuf> {
    children_of(&mount_parents())
}

#[cfg(target_os = "macos")]
fn mount_parents() -> Vec<PathBuf> {
    vec![PathBuf::from("/Volumes")]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn mount_parents() -> Vec<PathBuf> {
    let mut parents = Vec::new();
    if let Some(user) = std::env::var_os("USER") {
        parents.push(PathBuf::from("/media").join(&user));
        parents.push(PathBuf::from("/run/media").join(&user));
    }
    parents.push(PathBuf::from("/media"));
    parents.push(PathBuf::from("/mnt"));
    parents
}

#[cfg(not(windows))]
fn children_of(parents: &[PathBuf]) -> Vec<PathBuf> {
    parents
        .iter()
        .filter_map(|parent| std::fs::read_dir(parent).ok())
        .flat_map(|entries| entries.filter_map(|entry| entry.ok().map(|entry| entry.path())))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn make_ipod(mount: &Path, database_file: &str) {
        let itunes = mount.join("iPod_Control").join("iTunes");
        fs::create_dir_all(&itunes).unwrap();
        fs::write(itunes.join(database_file), b"mhbd").unwrap();
    }

    #[test]
    fn keeps_only_mounts_with_a_database() {
        let root = tempfile::tempdir().unwrap();
        let classic = root.path().join("CLASSIC");
        let nano = root.path().join("NANO");
        let usb_stick = root.path().join("STICK");
        make_ipod(&classic, "iTunesDB");
        make_ipod(&nano, "iTunesCDB");
        fs::create_dir_all(usb_stick.join("iPod_Control")).unwrap();

        let found = ipod_mounts([
            classic.clone(),
            nano.clone(),
            usb_stick,
            root.path().join("MISSING"),
            classic.clone(),
        ]);

        assert_eq!(found, vec![classic, nano]);
    }

    #[test]
    fn finds_an_ipod_that_has_no_database_yet_but_never_a_plain_disk() {
        let root = tempfile::tempdir().unwrap();
        let path = |name: &str| root.path().join(name);
        let with_sysinfo = path("RESTORED");
        fs::create_dir_all(with_sysinfo.join("iPod_Control/Device")).unwrap();
        fs::write(
            with_sysinfo.join("iPod_Control/Device/SysInfo"),
            b"ModelNumStr: xC293",
        )
        .unwrap();
        let with_itunes_folder = path("NEW");
        fs::create_dir_all(with_itunes_folder.join("iPod_Control/iTunes")).unwrap();
        // A disk that merely has the folder, or whose "SysInfo" is a folder, is not an iPod.
        let stick = path("STICK");
        fs::create_dir_all(stick.join("iPod_Control")).unwrap();
        let music_only = path("MUSIC");
        fs::create_dir_all(music_only.join("iPod_Control/Music/F00")).unwrap();
        let odd = path("ODD");
        fs::create_dir_all(odd.join("iPod_Control/Device/SysInfo")).unwrap();
        let not_ipod_control = path("OTHER");
        fs::create_dir_all(not_ipod_control.join("Music/iTunes")).unwrap();

        let found = ipod_mounts([
            with_sysinfo.clone(),
            with_itunes_folder.clone(),
            stick,
            music_only,
            odd,
            not_ipod_control,
        ]);

        assert_eq!(found, vec![with_itunes_folder, with_sysinfo]);
    }

    #[cfg(not(windows))]
    #[test]
    fn scans_the_children_of_each_root() {
        let root = tempfile::tempdir().unwrap();
        let ipod = root.path().join("IPOD");
        make_ipod(&ipod, "iTunesDB");
        fs::create_dir_all(root.path().join("OTHER")).unwrap();

        let found = ipod_mounts(children_of(&[
            root.path().to_path_buf(),
            root.path().join("not-a-dir"),
        ]));

        assert_eq!(found, vec![ipod]);
    }
}
