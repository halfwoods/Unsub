import { useEffect, useState } from "react";
import { deviceRescue, deviceRescuePreview } from "../api/commands";
import type { Device, RescueOptions, RescuePlaylist, RescuePreview, RescueScope } from "../api/types";
import { SourceIcon } from "../sidebar/icons";
import { useApp } from "../state/store";
import { formatBytes } from "../util/format";

/** The preview as one line: what is there, what would be copied, and what is left out and why. */
function describe(preview: RescuePreview) {
  const count = (n: number, what: string) => `${n.toLocaleString()} ${what}`;
  const parts = [
    count(preview.songs, preview.songs === 1 ? "song" : "songs"),
    count(preview.new, "new"),
    preview.restored > 0 && count(preview.restored, "to restore"),
    preview.similar > 0 && count(preview.similar, "other versions"),
    preview.known > 0 && count(preview.known, "in your library"),
    preview.offline > 0 && count(preview.offline, "on a disk that isn't connected"),
    preview.protected > 0 && count(preview.protected, "copy-protected"),
    preview.unsure > 0 && count(preview.unsure, "not sure"),
    preview.missing > 0 && count(preview.missing, "missing"),
    formatBytes(preview.bytes),
  ];
  return parts.filter(Boolean).join(" · ");
}

/** The notes under the preview for the songs it treats out of the ordinary. */
function notes(preview: RescuePreview): string[] {
  const found: string[] = [];
  if (preview.protected > 0) {
    found.push(
      "Songs bought from the iTunes Store before 2009 are still copy-protected. They play only where they were authorised, so they can't be copied.",
    );
  }
  if (preview.unsure > 0) {
    found.push(
      "Songs marked “not sure” could be more than one of the songs you already have, so Unsub leaves them alone rather than guess.",
    );
  }
  if (preview.similar > 0) {
    found.push("Songs marked “other versions” are a few seconds longer or shorter than songs you have, so they may be other cuts. They are copied too.");
  }
  if (preview.restored > 0) {
    found.push("Songs marked “to restore” are in your library but their files have gone. Their copies on the iPod take the files' place, and the songs keep their play counts and playlists.");
  }
  return found;
}

const scopeOf = (selected: boolean, keys: string[]): RescueScope => (selected ? { type: "playlists", keys } : { type: "all" });

/** Copies songs picked in an iPod's list, as the menu and a drop on Music do: not the ones the library has, and without playlists. */
export function copySongsToLibrary(deviceId: string, ids: number[]) {
  const options: RescueOptions = { skipKnown: true, playCounts: true, playlists: false, restoreOffline: false };
  void deviceRescue(deviceId, { type: "songs", ids }, options).catch((e: unknown) => useApp.getState().showNotice(String(e)));
}

/** The sheet behind Copy to Library… on an iPod's Summary: rescues its music into the library. */
export function CopyToLibrary({ device, onClose }: { device: Device; onClose: () => void }) {
  const showNotice = useApp((s) => s.showNotice);
  const [playlists, setPlaylists] = useState<RescuePlaylist[]>([]);
  const [selected, setSelected] = useState(false);
  const [chosen, setChosen] = useState<string[]>([]);
  const [options, setOptions] = useState<RescueOptions>({ skipKnown: true, playCounts: true, playlists: true, restoreOffline: false });
  const [preview, setPreview] = useState<RescuePreview | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Counting means reading the iPod and matching its songs against the library, so it waits for the choices to settle. It
  // depends only on what is copied: the other two options change what is done with the songs, not which. A count that is
  // overtaken is dropped here, and the one behind it stops itself.
  const { skipKnown, restoreOffline } = options;
  useEffect(() => {
    let current = true;
    setPreview(null);
    const timer = setTimeout(() => {
      void deviceRescuePreview(device.id, scopeOf(selected, chosen), { skipKnown, restoreOffline, playCounts: true, playlists: true }).then(
        (p) => {
          if (!current) return;
          setPreview(p);
          setPlaylists(p.playlists);
          setError(null);
        },
        (e: unknown) => current && setError(String(e)),
      );
    }, 250);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [device.id, selected, chosen, skipKnown, restoreOffline]);

  const toggle = (key: string, on: boolean) => setChosen(on ? [...chosen, key] : chosen.filter((c) => c !== key));
  const option = (patch: Partial<RescueOptions>) => setOptions({ ...options, ...patch });

  const start = () => {
    onClose();
    void deviceRescue(device.id, scopeOf(selected, chosen), options).catch((e: unknown) => showNotice(String(e)));
  };

  const short = preview !== null && !preview.fits;
  const status =
    error ??
    (preview === null
      ? "Counting the songs…"
      : short
        ? `Not enough room: this copy needs ${formatBytes(preview.bytes)} and ${formatBytes(preview.freeBytes)} is free.`
        : describe(preview));
  const warning = error !== null || short;

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet copy-to-library" role="dialog" aria-label="Copy to Library">
        <h2 className="first-sync-title">Copy “{device.name}” to Your Library</h2>
        <p className="prefs-note">
          Copies the music on this iPod into your library, with its song info, play counts and playlists. The iPod itself isn't changed.
        </p>
        <fieldset className="device-choices">
          <label>
            <input type="radio" checked={!selected} onChange={() => setSelected(false)} />
            All songs
          </label>
          <label>
            <input type="radio" checked={selected} onChange={() => setSelected(true)} />
            Selected playlists
          </label>
        </fieldset>
        {selected && (
          <ul className="device-playlists" aria-label="Playlists">
            {playlists.map((p) => (
              <li key={p.key}>
                <label>
                  <input type="checkbox" checked={chosen.includes(p.key)} onChange={(e) => toggle(p.key, e.target.checked)} />
                  <SourceIcon name={p.smart ? "smart" : "playlist"} />
                  {p.name}
                </label>
              </li>
            ))}
          </ul>
        )}
        <div className="copy-to-library-options">
          <label className="prefs-check">
            <input type="checkbox" checked={options.skipKnown} onChange={(e) => option({ skipKnown: e.target.checked })} />
            Skip songs already in my library
          </label>
          <label className="prefs-check">
            <input type="checkbox" checked={options.playCounts} onChange={(e) => option({ playCounts: e.target.checked })} />
            Bring play counts and ratings
          </label>
          <label className="prefs-check">
            <input type="checkbox" checked={options.playlists} onChange={(e) => option({ playlists: e.target.checked })} />
            Recreate playlists
          </label>
        </div>
        {preview && preview.offlineVolumes.length > 0 && (
          <>
            <p className="prefs-note">
              Songs on {preview.offlineVolumes.map((volume) => `“${volume}”`).join(" and ")} are in your library, but that disk isn't connected, so
              they are left alone in case it is only unplugged.
            </p>
            <label className="prefs-check">
              <input type="checkbox" checked={options.restoreOffline} onChange={(e) => option({ restoreOffline: e.target.checked })} />
              Restore them from the iPod anyway (if that disk is gone for good)
            </label>
          </>
        )}
        <p className={warning ? "copy-to-library-status copy-to-library-status--warning" : "copy-to-library-status"}>{status}</p>
        {preview &&
          notes(preview).map((note) => (
            <p key={note} className="prefs-note">
              {note}
            </p>
          ))}
        <div className="sheet-buttons">
          <button className="push-button" onClick={onClose}>
            Cancel
          </button>
          <button className="push-button push-button--default" disabled={!preview?.fits || preview.songs === 0} onClick={start}>
            Copy
          </button>
        </div>
      </div>
    </div>
  );
}
