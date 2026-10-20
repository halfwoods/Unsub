use std::fs;
use std::io;
use std::path::Path;

use rusqlite::OptionalExtension;

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::MediaKind;
use crate::organize::prune_empty_dirs;

/// Never deletes outright: the Trash can be emptied by the user, not by us.
/// On macOS this uses the file manager's own call, which needs no permission
/// prompt, at the cost of "Put Back" not always being offered.
pub(crate) fn move_to_trash(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let context = {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut context = trash::TrashContext::default();
        context.set_delete_method(DeleteMethod::NsFileManager);
        context
    };
    #[cfg(not(target_os = "macos"))]
    let context = trash::TrashContext::default();
    context.delete(path).map_err(io::Error::other)
}

impl Library {
    /// Replaces what moves a file to the Trash; tests use this to keep their
    /// files out of the real one.
    pub fn set_trash_handler(
        &mut self,
        handler: impl Fn(&Path) -> io::Result<()> + Send + 'static,
    ) {
        self.trash = Box::new(handler);
    }

    /// Removes the tracks from the library and, with `move_files_to_trash`,
    /// their files to the Trash. A file that cannot be trashed keeps its track,
    /// and the failures are reported together once the rest are done. A file
    /// already gone is simply forgotten, and unknown ids are ignored.
    pub fn delete_tracks(&mut self, ids: &[i64], move_files_to_trash: bool) -> Result<()> {
        let media = self.media_settings()?.folder;
        let mut removable = Vec::new();
        let mut failures = Vec::new();
        for id in ids {
            let row: Option<(String, MediaKind)> = self
                .conn
                .query_row("SELECT path, kind FROM tracks WHERE id = ?1", [id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()?;
            let Some((path, kind)) = row else {
                continue;
            };
            let path = Path::new(&path);
            // A station's path is a URL: there is never a file to trash.
            if move_files_to_trash && kind != MediaKind::Radio && fs::symlink_metadata(path).is_ok()
            {
                if let Err(err) = (self.trash)(path) {
                    failures.push((*id, err.to_string()));
                    continue;
                }
                if let (Some(media), Some(parent)) = (&media, path.parent()) {
                    prune_empty_dirs(media, parent);
                }
            }
            removable.push(*id);
        }
        self.remove_tracks(&removable)?;
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::TrashFailed(failures))
        }
    }
}
