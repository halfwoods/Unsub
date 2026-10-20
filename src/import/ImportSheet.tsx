import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { importMusicCancel, importMusicRun } from "../api/commands";
import type { ImportPreview } from "../api/types";
import { useApp } from "../state/store";
import { pluralize } from "../util/format";
import "./ImportSheet.css";

/** File › Import Music Library: says what Music has and, if its files have moved, where to look for them. */
export function ImportSheet({ preview }: { preview: ImportPreview }) {
  const close = () => useApp.setState({ importSheet: null });
  const showNotice = useApp((s) => s.showNotice);
  const [folder, setFolder] = useState<string | null>(null);

  const choose = async () => {
    const picked = await open({ title: "Choose Where Your Music Files Are", directory: true, multiple: false });
    if (typeof picked === "string") setFolder(picked);
  };

  const start = () => {
    close();
    void importMusicRun(folder).catch((e: unknown) => showNotice(String(e)));
  };

  const cancel = () => {
    close();
    void importMusicCancel();
  };

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && cancel()}>
      <div className="sheet import-library" role="dialog" aria-label="Import Music Library">
        <p className="import-library-title">Import Music Library</p>
        <p>
          {pluralize(preview.songs, "song", "songs")} in your Music library.{" "}
          {preview.missing > 0
            ? `${preview.missing.toLocaleString()} of them aren't where Music expects their files.`
            : "Their files are all where Music expects them."}
        </p>
        {preview.missing > 0 && (
          <>
            <p className="import-library-title">Where are your music files now?</p>
            <div className="import-library-folder">
              <button className="push-button" onClick={() => void choose()}>
                Choose Folder…
              </button>
              <span className="import-library-path">{folder ?? "None chosen"}</span>
            </div>
            <p className="import-library-note">
              Unsub looks at the names of the files there to find your songs. Songs stored in iCloud aren't downloaded. Nothing in the
              folder is changed, moved or deleted.
            </p>
          </>
        )}
        <div className="sheet-buttons">
          <button className="push-button" onClick={cancel}>
            Cancel
          </button>
          <button className="push-button push-button--default" onClick={start}>
            {preview.missing > 0 && !folder ? "Import Without Locating" : "Import"}
          </button>
        </div>
      </div>
    </div>
  );
}
