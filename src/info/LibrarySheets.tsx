import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState, type ReactNode } from "react";
import { detailsSetPaused, detailsStart, detailsStatus, jobCancel, libraryDelete, libraryMediaSettings, libraryOrganize, libraryRelink } from "../api/commands";
import type { DetailsStatus, Track } from "../api/types";
import { useApp } from "../state/store";
import { formatBytes, pluralize } from "../util/format";
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

/** Advanced › Get Song Details from iCloud: says what reading the songs that are only in iCloud involves, and once it is going, holds its controls. */
function SongDetailsSheet() {
  const close = useApp((s) => s.openLibrarySheet);
  const showNotice = useApp((s) => s.showNotice);
  const progress = useApp((s) => s.details);
  const [status, setStatus] = useState<DetailsStatus | null>(null);
  // Asked again when it starts or ends, since what is left to read has changed.
  const idle = progress === null;
  useEffect(() => {
    void detailsStatus().then(setStatus);
  }, [idle]);

  const running = !idle || status?.running === true;
  const held = progress ? progress.phase !== "running" : status?.paused === true;
  const start = () => {
    close(null);
    void detailsStart().catch((e: unknown) => showNotice(String(e)));
  };
  const stop = () => {
    close(null);
    void jobCancel("details");
  };

  if (running) {
    const done = progress ? progress.read + progress.skipped + progress.missing : 0;
    return (
      <Sheet label="Get Song Details from iCloud">
        <p className="alert-title">Getting song details from iCloud</p>
        <p>
          {progress
            ? `${done.toLocaleString()} of ${progress.total.toLocaleString()} songs done${progress.skipped > 0 ? `, ${progress.skipped.toLocaleString()} couldn't be read` : ""}.`
            : "Getting started."}
        </p>
        {progress?.phase === "lowSpace" && <p>Paused because the disk is almost full. Free up some space, then choose Resume.</p>}
        {progress?.phase === "noResponse" && <p>Paused because iCloud isn't responding. Check your connection, then choose Resume.</p>}
        {progress?.phase === "filesMissing" && <p>Paused because the songs' files can't be found. Check that iCloud Drive is on and the folder hasn't moved, then choose Resume.</p>}
        {progress?.phase === "notGivenBack" && <p>Paused because iCloud isn't taking the downloaded songs back, and they would fill the disk. Choose Resume to try again.</p>}
        {progress?.phase === "paused" && <p>Paused. It carries on from here when you choose Resume.</p>}
        <div className="sheet-buttons">
          <button className="push-button" onClick={() => close(null)}>
            Close
          </button>
          <button className="push-button" onClick={stop}>
            Stop
          </button>
          <button className="push-button push-button--default" onClick={() => void detailsSetPaused(!held)}>
            {held ? "Resume" : "Pause"}
          </button>
        </div>
      </Sheet>
    );
  }

  const songs = status?.songs ?? 0;
  return (
    <Sheet label="Get Song Details from iCloud">
      <p className="alert-title">Get Song Details from iCloud</p>
      {status && songs === 0 ? (
        <p>Every song that is only in iCloud already has its details.</p>
      ) : (
        <>
          <p>
            {status ? pluralize(songs, "song is", "songs are") : "Some songs are"} only in iCloud, so Unsub knows them just by their file names: no genre, year or cover. Unsub can read the real details by
            downloading each song, one at a time, and giving it back to iCloud once it has them. Nothing is written to your music files.
          </p>
          <p>
            {status ? `That is about ${formatBytes(status.bytes)} to download in all, so it takes a long while` : "It takes a long while"}, but it all happens in the background. It waits while a song you play is
            downloading, stops if your disk runs low on room, and carries on if you quit and open Unsub again.
          </p>
        </>
      )}
      <div className="sheet-buttons">
        <button className="push-button" onClick={() => close(null)}>
          {songs === 0 && status ? "OK" : "Cancel"}
        </button>
        {songs > 0 && (
          <button className="push-button push-button--default" onClick={start}>
            Start
          </button>
        )}
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
    case "songDetails":
      return <SongDetailsSheet />;
  }
}
