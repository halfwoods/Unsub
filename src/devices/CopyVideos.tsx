import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { deviceVideos, deviceVideoExportPreview, deviceVideoExport } from "../api/commands";
import type { Device, DeviceVideo } from "../api/types";
import { useApp } from "../state/store";
import { formatBytes } from "../util/format";
import { createVideoExportSession } from "./videoExportSession";
import "./CopyVideos.css";

const KIND: Record<DeviceVideo["kind"], string> = {
  movie: "Movie", musicVideo: "Music Video", tv: "TV Show", videoPodcast: "Video Podcast", other: "Video",
};
const PROTECTION: Record<DeviceVideo["protection"], string> = {
  protected: "Protected — original will be copied", unprotected: "No protection detected", unknown: "Protection unknown",
};

export function CopyVideos({ device, onClose }: { device: Device; onClose: () => void }) {
  const [session] = useState(() => createVideoExportSession(device.id, {
    list: deviceVideos, preview: deviceVideoExportPreview, start: deviceVideoExport,
  }));
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot);
  const busy = useApp(s => s.deviceSync !== null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const dialog = useRef<HTMLDivElement>(null), close = useRef(onClose), mounted = useRef(false);
  close.current = onClose;
  useEffect(() => {
    mounted.current = true;
    void session.load();
    const previous = document.activeElement as HTMLElement | null;
    dialog.current?.focus();
    return () => { mounted.current = false; session.dispose(); previous?.focus(); };
  }, [session]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault(); event.stopPropagation();
        if (!session.getSnapshot().starting && !choosing) close.current();
      }
      if (event.key !== "Tab") return;
      const controls = Array.from(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),[tabindex="0"]') ?? [])
        .filter(element => element.getClientRects().length > 0);
      const first = controls[0], last = controls[controls.length - 1];
      if (!controls.includes(document.activeElement as HTMLElement)) { event.preventDefault(); (event.shiftKey ? last : first)?.focus(); }
      else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [session, choosing]);

  const choose = async () => {
    setChoosing(true); setFolderError(null);
    try {
      const folder = await open({ title: "Copy Videos to Folder", directory: true, multiple: false });
      if (mounted.current && typeof folder === "string") session.destination(folder);
    } catch (error) { if (mounted.current) setFolderError(String(error)); }
    finally { if (mounted.current) setChoosing(false); }
  };
  const items = state.listing?.items ?? [];
  const selected = new Set(state.selected);
  const disabled = state.starting || choosing;
  const preview = state.preview;
  const status = state.loading ? "Reading videos from the iPod…" : state.counting ? "Checking the destination…"
    : preview ? `${preview.videos.toLocaleString()} videos · ${formatBytes(preview.bytes)} · ${formatBytes(preview.freeBytes)} available`
      : items.length === 0 ? "No videos were found on this iPod." : state.selected.length === 0 ? "Select videos to copy."
        : state.destination ? "Choose the folder again to retry the preview." : "Choose a destination folder.";

  return <div className="sheet-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !disabled) onClose(); }}>
    <div ref={dialog} className="sheet copy-videos" role="dialog" aria-modal="true" aria-labelledby="copy-videos-title" tabIndex={-1}>
      <h2 id="copy-videos-title">Copy Videos from “{device.name}”</h2>
      <p>Save original files and their metadata to a folder. The iPod stays unchanged; videos are not added to your music library.</p>
      {state.listing?.warning && <p role="status">{state.listing.warning}</p>}
      {state.listing?.fromFiles && !state.listing.warning && <p>Reading files directly because the iPod database is unavailable. Names and categories may be incomplete.</p>}
      <div className="copy-videos-choices">
        <button className="push-button" disabled={disabled || state.loading || items.length === 0} onClick={() => session.select(items.map(item => item.id))}>Select All</button>
        <button className="push-button" disabled={disabled || state.loading || selected.size === 0} onClick={() => session.select([])}>Select None</button>
        <span>{selected.size.toLocaleString()} selected</span>
      </div>
      <div className="copy-videos-list" aria-label="Videos to copy" tabIndex={0}>
        {items.map(item => <label key={item.id} className="copy-video-row">
          <input type="checkbox" checked={selected.has(item.id)} disabled={disabled}
            onChange={event => session.select(event.target.checked ? [...state.selected, item.id] : state.selected.filter(id => id !== item.id))} />
          <span className="copy-video-description"><strong title={item.title}>{item.title || "Untitled Video"}</strong>
            <span title={[item.artist, item.album].filter(Boolean).join(" · ")}>{[KIND[item.kind], item.artist, item.album].filter(Boolean).join(" · ")}</span>
            <span>{item.missing ? "File unavailable" : PROTECTION[item.protection]}</span>
          </span>
          <span>{item.bytes === null ? "" : formatBytes(item.bytes)}</span>
        </label>)}
        {!state.loading && items.length === 0 && <p>No movies or music videos to copy.</p>}
      </div>
      <div className="copy-videos-destination">
        <button className="push-button" disabled={disabled || state.loading} onClick={() => void choose()}>Choose Folder…</button>
        <span title={preview?.folder ?? state.destination}>{preview?.folder ?? (state.destination || "No folder selected")}</span>
      </div>
      <p className="copy-videos-status" role="status" aria-live="polite">{status}</p>
      {preview && !preview.fits && <p role="alert">There is not enough room for this export. Choose another folder or fewer videos.</p>}
      {!!preview?.missing && <p>{preview.missing.toLocaleString()} unavailable files will be reported and skipped.</p>}
      <p className="prefs-note">Protected files are copied unchanged. Copying does not remove protection; playback may still require authorization. Unreadable protection information is marked unknown.</p>
      {(state.error || folderError) && <p role="alert" className="copy-videos-error">{state.error ?? folderError}
        {!state.listing && <button className="push-button" onClick={() => void session.load()}>Retry</button>}
      </p>}
      {busy && <p role="status">Another sync or copy is running. Wait for it to finish before starting this export.</p>}
      <div className="sheet-buttons">
        <button className="push-button" disabled={disabled} onClick={onClose}>Cancel</button>
        <button className="push-button push-button--default" disabled={disabled || busy || !session.canStart()}
          onClick={() => { void session.start().then(started => { if (started && mounted.current) close.current(); }); }}>{state.starting ? "Starting…" : "Copy Videos"}</button>
      </div>
    </div>
  </div>;
}
