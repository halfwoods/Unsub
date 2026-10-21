import { useEffect, useRef, useState } from "react";
import { onUpdateResult, updatesDismiss, updatesInstall, updatesSetAutoCheck, updatesStatus } from "../api/commands";
import type { UpdateInfo, UpdatesStatus } from "../api/types";
import { useApp } from "../state/store";
import "./UpdateSheet.css";

interface Progress {
  done: number;
  total: number | null;
}

const megabytes = (bytes: number) => (bytes / 1_000_000).toFixed(1);

/**
 * iTunes' "A new version is available" sheet. The backend decides when to look (at launch, once a
 * day while the app is open, or from the app menu) and reports every outcome through one event; this
 * only reacts to it, and tells the backend when an offer is waved away so it isn't repeated at once.
 */
export function UpdateSheet() {
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A ref because the listener below outlives every render; `progress` would be stale inside it.
  const downloading = useRef(false);

  useEffect(() => {
    const offer = (found: UpdateInfo) => {
      // Resetting here mid-download would bring the buttons back and allow a second install.
      if (downloading.current) return;
      setUpdate(found);
      setError(null);
      setProgress(null);
    };
    const subscription = onUpdateResult(({ manual, update: found, error: failure }) => {
      const { showNotice } = useApp.getState();
      if (found) {
        offer(found);
      } else if (manual) {
        showNotice(failure ? `Couldn't check for updates: ${failure}` : "Unsub is up to date");
      }
    });
    // Events sent before the listener existed (launch races the mount, and a reload starts over) are
    // gone, so ask for the offer the backend is still holding.
    void subscription.then(() => updatesStatus()).then(({ update: waiting }) => waiting && offer(waiting));
    return () => void subscription.then((unlisten) => unlisten());
  }, []);

  if (!update) return null;
  const installing = progress !== null;
  const remindLater = () => {
    if (installing) return;
    setUpdate(null);
    void updatesDismiss();
  };
  const install = async () => {
    let done = 0;
    setError(null);
    setProgress({ done, total: null });
    downloading.current = true;
    try {
      // On success the app restarts itself, so there is nothing to do afterwards.
      await updatesInstall((event) => {
        if (event.event === "started") setProgress({ done, total: event.data.contentLength });
        else if (event.event === "progress") {
          done += event.data.chunkLength;
          setProgress((p) => ({ done, total: p?.total ?? null }));
        }
      });
    } catch (e) {
      downloading.current = false;
      setError(String(e));
      setProgress(null);
    }
  };

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && remindLater()}>
      <div className="sheet update-sheet" role="dialog" aria-label="Software Update">
        <h2 className="first-sync-title">A new version of Unsub is available.</h2>
        <p className="prefs-note">
          Unsub {update.version} is now available—you have {update.currentVersion}. Would you like to download it now?
        </p>
        {update.notes && (
          <>
            <div className="update-notes-title">Release Notes:</div>
            <div className="update-notes" tabIndex={0}>
              {update.notes}
            </div>
          </>
        )}
        {progress && (
          <div className="update-progress">
            <progress value={progress.total ? progress.done : undefined} max={progress.total ?? undefined} />
            <span>
              {progress.total && progress.done >= progress.total
                ? "Installing…"
                : progress.total
                  ? `Downloading… ${megabytes(progress.done)} of ${megabytes(progress.total)} MB`
                  : "Downloading…"}
            </span>
          </div>
        )}
        {error && <p className="smart-error">{error}</p>}
        <div className="sheet-buttons">
          <button className="push-button" disabled={installing} onClick={remindLater}>
            Remind Me Later
          </button>
          <button className="push-button push-button--default" disabled={installing} autoFocus onClick={() => void install()}>
            Download and Install
          </button>
        </div>
      </div>
    </div>
  );
}

/** Preferences › General: whether Unsub looks for updates by itself. Hidden in builds without update keys. */
export function AutoUpdateSetting() {
  const [status, setStatus] = useState<UpdatesStatus | null>(null);
  useEffect(() => {
    void updatesStatus().then(setStatus);
  }, []);
  if (!status?.configured) return null;
  return (
    <>
      <label className="prefs-check">
        <input
          type="checkbox"
          checked={status.autoCheck}
          onChange={(e) => {
            setStatus({ ...status, autoCheck: e.target.checked });
            void updatesSetAutoCheck(e.target.checked);
          }}
        />
        Check for updates automatically
      </label>
      <p className="prefs-note">You have version {status.currentVersion}. Updates are checked at most once a day.</p>
    </>
  );
}
