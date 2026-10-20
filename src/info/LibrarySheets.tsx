import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState, type ReactNode } from "react";
import { libraryDelete, libraryMediaSettings, libraryOrganize, libraryRelink } from "../api/commands";
import type { Track } from "../api/types";
import { useApp } from "../state/store";
import { pluralize } from "../util/format";
import { CleanUpSheet } from "./CleanUpSheet";
import { GetInfo } from "./GetInfo";
import "./LibrarySheets.css";

function Sheet({ label, children }: { label: string; children: ReactNode }) {
  const close = useApp((s) => s.openLibrarySheet);
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close(null)}>
      <div className="sheet alert-sheet" role="alertdialog" aria-label={label}>
        {children}
      </div>
    </div>
  );
}

/** iTunes' "The song could not be used because the original file could not be found." */
function LocateSheet({ track }: { track: Track }) {
  const close = useApp((s) => s.openLibrarySheet);
  const showNotice = useApp((s) => s.showNotice);
  const locate = async () => {
    const path = await open({ title: `Locate “${track.title}”`, multiple: false, directory: false });
    if (typeof path !== "string") return;
    close(null);
    try {
      // The same folder often holds the rest of what moved, so it's searched too, as iTunes offered.
      const found = await libraryRelink(track.id, path, true);
      if (found > 0) showNotice(`Found ${pluralize(found, "other missing song", "other missing songs")} in the same folder`);
      const { refresh } = useApp.getState();
      await refresh();
      const { tracks, playRow } = useApp.getState();
      const index = tracks.findIndex((t) => t.id === track.id);
      if (index >= 0) void playRow(index);
    } catch (e) {
      showNotice(String(e));
    }
  };
  return (
    <Sheet label="File not found">
      <p className="alert-title">The song “{track.title}” could not be used because the original file could not be found.</p>
      <p>Would you like to locate it? Other missing songs in the same folder are found at the same time.</p>
      <div className="sheet-buttons">
        <button className="push-button" onClick={() => close(null)}>
          Cancel
        </button>
        <button className="push-button push-button--default" onClick={() => void locate()}>
          Locate
        </button>
      </div>
    </Sheet>
  );
}

function DeleteSheet({ trackIds }: { trackIds: number[] }) {
  const close = useApp((s) => s.openLibrarySheet);
  const showNotice = useApp((s) => s.showNotice);
  const remove = (trash: boolean) => {
    close(null);
    useApp.setState({ selection: new Set() });
    void libraryDelete(trackIds, trash).catch((e: unknown) => showNotice(String(e)));
  };
  const one = trackIds.length === 1;
  return (
    <Sheet label="Delete from library">
      <p className="alert-title">Delete {one ? "this song" : `these ${trackIds.length.toLocaleString()} songs`} from your library?</p>
      <p>
        You can move {one ? "its file" : "their files"} to the Trash too, or keep {one ? "it" : "them"} where {one ? "it is" : "they are"}. Songs
        removed from the library also leave its playlists.
      </p>
      <div className="sheet-buttons">
        <button className="push-button" onClick={() => close(null)}>
          Cancel
        </button>
        <button className="push-button" onClick={() => remove(false)}>
          {one ? "Keep File" : "Keep Files"}
        </button>
        <button className="push-button push-button--default" onClick={() => remove(true)}>
          Move to Trash
        </button>
      </div>
    </Sheet>
  );
}

function OrganizeSheet() {
  const close = useApp((s) => s.openLibrarySheet);
  const [folder, setFolder] = useState<string | null>(null);
  useEffect(() => {
    void libraryMediaSettings().then((settings) => setFolder(settings.folder));
  }, []);
  return (
    <Sheet label="Organize Library">
      <p className="alert-title">Organize Library</p>
      <p>
        Moves the songs in your media folder{folder ? ` (${folder})` : ""} into Artist and Album folders, named by disc, track number and
        title. Songs kept anywhere else aren't touched.
      </p>
      <div className="sheet-buttons">
        <button className="push-button" onClick={() => close(null)}>
          Cancel
        </button>
        <button
          className="push-button push-button--default"
          onClick={() => {
            close(null);
            void libraryOrganize();
          }}
        >
          OK
        </button>
      </div>
    </Sheet>
  );
}

/** Whichever library sheet is open. */
export function LibrarySheets() {
  const sheet = useApp((s) => s.librarySheet);
  if (!sheet) return null;
  switch (sheet.type) {
    case "info":
      return <GetInfo trackIds={sheet.trackIds} />;
    case "cleanUp":
      return <CleanUpSheet trackIds={sheet.trackIds} />;
    case "locate":
      return <LocateSheet track={sheet.track} />;
    case "delete":
      return <DeleteSheet trackIds={sheet.trackIds} />;
    case "organize":
      return <OrganizeSheet />;
  }
}
