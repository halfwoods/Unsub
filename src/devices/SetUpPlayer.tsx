import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { playerSetUp, type PlayerPreset } from "../api/commands";
import { useApp } from "../state/store";

const PRESETS: { value: PlayerPreset; label: string }[] = [
  { value: "massStorage", label: "USB music player" },
  { value: "android", label: "Android phone (as a USB drive)" },
  { value: "rockbox", label: "Rockbox player" },
];

/** Makes a USB player's drive a sync target, so it appears under DEVICES. */
export function SetUpPlayer() {
  const close = () => useApp.setState({ settingUpPlayer: false });
  const showNotice = useApp((s) => s.showNotice);
  const [path, setPath] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [preset, setPreset] = useState<PlayerPreset>("massStorage");
  const [busy, setBusy] = useState(false);

  const choose = async () => {
    const picked = await open({ title: "Choose the Player's Drive", directory: true, multiple: false, defaultPath: "/Volumes" });
    if (typeof picked !== "string") return;
    setPath(picked);
    if (!name) setName(picked.split(/[\\/]/).filter(Boolean).pop() ?? "");
  };

  const setUp = async () => {
    if (!path) return;
    setBusy(true);
    try {
      await playerSetUp(path, name.trim() || "Music Player", preset);
      close();
    } catch (e) {
      showNotice(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="sheet set-up-player" role="dialog" aria-label="Set Up Music Player">
        <h2 className="first-sync-title">Set Up a Music Player</h2>
        <p className="prefs-note">
          Choose the drive of a USB music player or phone. AntiStreaming syncs your music to it as files and playlists, and never
          touches anything else on it.
        </p>
        <div className="prefs-grid">
          <span className="prefs-label">Drive:</span>
          <span className="prefs-inline">
            <span className="set-up-path">{path ?? "None chosen"}</span>
            <button className="push-button" onClick={() => void choose()}>
              Choose…
            </button>
          </span>
          <label className="prefs-label" htmlFor="player-name">
            Name:
          </label>
          <input id="player-name" className="smart-input" value={name} onChange={(e) => setName(e.target.value)} />
          <label className="prefs-label" htmlFor="player-kind">
            Kind:
          </label>
          <select id="player-kind" className="smart-popup" value={preset} onChange={(e) => setPreset(e.target.value as PlayerPreset)}>
            {PRESETS.map((p) => (
              <option key={p.value} value={p.value}>
                {p.label}
              </option>
            ))}
          </select>
        </div>
        <div className="sheet-buttons">
          <button className="push-button" onClick={close}>
            Cancel
          </button>
          <button className="push-button push-button--default" disabled={!path || busy} onClick={() => void setUp()}>
            Set Up
          </button>
        </div>
      </div>
    </div>
  );
}
