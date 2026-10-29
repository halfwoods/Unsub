import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { deviceVideoAdd, deviceVideoAddPreview, deviceVideoAddCancelPreview } from "../api/commands";
import type { Device, VideoAddKind } from "../api/types";
import { useApp } from "../state/store";
import { formatBytes, formatClock, pluralize } from "../util/format";
import { createVideoAddSession, MAX_VIDEO_ADD_FILES } from "./videoAddSession";
import "./CopyVideos.css";
import "./AddVideos.css";

export function AddVideos({ device, onClose }: { device: Device; onClose: () => void }) {
  const [session] = useState(() => createVideoAddSession(device.id, {
    preview: deviceVideoAddPreview, cancelPreview: deviceVideoAddCancelPreview, start: deviceVideoAdd,
  }));
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot);
  const busy = useApp(s => s.deviceSync !== null);
  const [pickerError, setPickerError] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const dialog = useRef<HTMLDivElement>(null), mounted = useRef(false), close = useRef(onClose);
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
      const controls = Array.from(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled),select:not(:disabled),[tabindex="0"]') ?? [])
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
    setChoosing(true); setPickerError(null);
    try {
      const paths = await open({ title: "Choose Videos to Add to iPod", multiple: true, directory: false,
        filters: [{ name: "Videos", extensions: ["mp4", "m4v", "mov"] }] });
      if (mounted.current && paths !== null) session.select(typeof paths === "string" ? [paths] : paths);
    } catch (error) { if (mounted.current) setPickerError(String(error)); }
    finally { if (mounted.current) setChoosing(false); }
  };
  const preview = state.preview;
  const items = new Map(preview?.items.map(item => [item.path, item]));
  const count = session.acceptedPaths().length;
  const disabled = state.starting || choosing;
  const status = state.checking ? state.paths.length ? "Checking video compatibility and space…" : "Checking this iPod…"
    : preview && !preview.supported ? "Adding videos is unavailable for this iPod."
      : state.error ? "Video checks need to be retried."
        : state.paths.length === 0 ? `Choose up to ${MAX_VIDEO_ADD_FILES} videos to add.`
          : preview ? `${pluralize(count, "video", "videos")} to add · ${formatBytes(preview.bytes)} · ${formatBytes(preview.freeBytes)} available`
            : "Choose videos or retry the compatibility check.";

  return <div className="sheet-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !disabled) onClose(); }}>
    <div ref={dialog} className="sheet copy-videos add-videos" role="dialog" aria-modal="true" aria-labelledby="add-videos-title" aria-describedby="add-videos-description" tabIndex={-1}>
      <h2 id="add-videos-title">Add Videos to “{device.name}”</h2>
      <p id="add-videos-description">Copy compatible movies or music videos to this iPod. Existing songs, videos and playlists stay in place.</p>
      <div className="add-videos-category">
        <label htmlFor="add-videos-kind">Add as:</label>
        <select id="add-videos-kind" className="smart-popup" value={state.kind} disabled={disabled || busy || state.supported !== true}
          onChange={event => session.category(event.target.value as VideoAddKind)}>
          <option value="movie">Movies</option><option value="musicVideo">Music Videos</option>
        </select>
      </div>
      {preview?.warning && <p role="status">{preview.warning}</p>}
      <div className="copy-videos-choices">
        <button className="push-button" disabled={disabled || busy || state.supported !== true} onClick={() => void choose()}>Choose Files…</button>
        <span>{pluralize(state.paths.length, "file selected", "files selected")}</span>
      </div>
      <div className="copy-videos-list" aria-label="Videos to add" tabIndex={0} aria-busy={state.checking}>
        {state.paths.map(path => {
          const item = items.get(path);
          const title = item?.title || path.split(/[/\\]/).pop() || "Untitled Video";
          const reason = item?.reason ?? (item?.alreadyPresent ? "Duplicate — skipped" : item ? "Ready to add" : state.checking ? "Checking…" : "Check unavailable");
          const details = [item?.durationMs == null ? "" : formatClock(item.durationMs), item?.bytes == null ? "" : formatBytes(item.bytes)].filter(Boolean).join(" · ");
          return <div key={path} className="copy-video-row add-video-row">
            <span className="copy-video-description"><strong title={title}>{title}</strong>
              <span title={path}>{path}</span>
              {details && <span>{details}</span>}
              <span className={item?.reason ? "add-video-reason add-video-reason--error" : "add-video-reason"}>{reason}</span>
            </span>
            <button className="push-button" disabled={disabled || busy} aria-label={`Remove ${title}`} onClick={() => session.select(state.paths.filter(selected => selected !== path))}>Remove</button>
          </div>;
        })}
        {state.paths.length === 0 && <p className="add-videos-empty">Choose MP4, M4V or MOV files. Compatibility is checked before copying.</p>}
      </div>
      <p className="copy-videos-status" role="status" aria-live="polite">{status}</p>
      {preview?.supported && !preview.fits && <p role="alert">There is not enough room on this iPod. Remove some videos and try again.</p>}
      {!!preview?.items.some(item => item.reason || item.alreadyPresent) && <p>Only files marked Ready to add will be copied.</p>}
      <p className="prefs-note">Files are copied without conversion. Incompatible videos need to be converted for your iPod first. Original files stay on this computer.</p>
      {(state.error || pickerError) && <p role="alert" className="copy-videos-error">{state.error ?? pickerError}</p>}
      {state.error && <button className="push-button" disabled={disabled || busy || state.checking} onClick={() => void session.retry()}>Retry Check</button>}
      {busy && <p role="status">Another sync or copy is running. Wait for it to finish before adding videos.</p>}
      <div className="sheet-buttons">
        <button className="push-button" disabled={disabled} onClick={onClose}>Cancel</button>
        <button className="push-button push-button--default" disabled={disabled || busy || !session.canStart()}
          onClick={() => {
            if (useApp.getState().deviceSync) return;
            void session.start().then(started => { if (started && mounted.current) close.current(); });
          }}>{state.starting ? "Starting…" : "Add Videos"}</button>
      </div>
    </div>
  </div>;
}
