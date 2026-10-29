/** Development-only interaction fixture. All paths, devices, dialogs and copies are simulated. */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Device, VideoAddPreview } from "../../api/types";
import "../../theme/base.css";
import "../../smart/SmartPlaylistEditor.css";
import "../../preferences/Preferences.css";

const device: Device = { id: "fixture", name: "Fixture iPod with a long name", model: null,
  capacityBytes: 160000000000, freeBytes: 1000000000, serialNumber: null, sync: "supported",
  usage: { audioBytes: 0, videoBytes: 0, podcastBytes: 0, audiobookBytes: 0 }, counts: { music: 0, podcast: 0, audiobook: 0 }, playlists: [] };
let mode = "ready", selection = "mixed";
const requests: { command: string; payload: unknown }[] = [];
mockIPC(async (command, raw) => {
  const payload = (raw ?? {}) as Record<string, unknown>;
  if (command === "plugin:dialog|open") {
    if (selection === "cancel") return null;
    if (selection === "many") return Array.from({ length: 500 }, (_, index) => `/fixture/Video ${index + 1}.mp4`);
    return ["/fixture/Ready movie.mp4", "/fixture/Already present.m4v", "/fixture/Incompatible profile.mov",
      `/fixture/${"A long video title and filename ".repeat(7)}.mp4`];
  }
  if (command === "device_video_add_preview") {
    await new Promise(resolve => setTimeout(resolve, 150));
    if (mode === "error") throw new Error("Fixture iPod is temporarily unavailable.");
    const paths = payload.paths as string[];
    const items = paths.map(path => ({ path, title: path.split("/").pop()!, bytes: 100000000, durationMs: 125000,
      reason: path.includes("Incompatible") ? "H.264 High profile is not supported. Convert this video to an iPod-compatible H.264 Baseline MP4 first." : null,
      alreadyPresent: path.includes("Already") }));
    const bytes = items.filter(item => !item.reason && !item.alreadyPresent).length * 100000000;
    return { items, bytes, freeBytes: mode === "full" ? 1000 : 100000000000, fits: mode !== "full", supported: mode !== "unsupported",
      warning: mode === "unsupported" ? "This iPod model does not support video playback." : null } satisfies VideoAddPreview;
  }
  if (command === "device_video_add") requests.push({ command, payload });
  return null;
});

async function start() {
  const { AddVideos } = await import("../AddVideos");
  function Fixture() {
    const [opened, setOpened] = useState(false);
    const [copies, setCopies] = useState(0);
    return <main style={{ padding: 24, background: "#ededed", height: "100%" }}>
      <h1>Add Videos interaction fixture</h1>
      <p>No files are read or copied. These controls affect the simulated device.</p>
      <p><label>Device: <select aria-label="Fixture device state" onChange={event => { mode = event.target.value; }}>
        <option value="ready">Ready</option><option value="unsupported">Unsupported model</option><option value="full">Not enough space</option><option value="error">Check fails</option>
      </select></label></p>
      <p><label>File picker: <select aria-label="Fixture selection" onChange={event => { selection = event.target.value; }}>
        <option value="mixed">Mixed compatibility and long names</option><option value="many">500 eligible files</option><option value="cancel">Cancel</option>
      </select></label></p>
      <button className="push-button" onClick={() => setOpened(true)}>Open Add Videos…</button>
      <p role="status">{copies} simulated copy requests</p>
      {opened && <AddVideos device={device} onClose={() => { setOpened(false); setCopies(requests.length); }} />}
    </main>;
  }
  createRoot(document.getElementById("root")!).render(<Fixture />);
}
void start();
