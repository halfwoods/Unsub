import { audioDir, join } from "@tauri-apps/api/path";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState, type ReactNode } from "react";
import {
  libraryMediaSettings,
  librarySetMediaSettings,
  playerSetBitPerfect,
  playerSetCrossfade,
  playerSetSoundCheck,
  sharingStatus,
  sharingUpdate,
  visualizerMicDevices,
} from "../api/commands";
import type { MediaSettings, MicDevice, SharingSettings, SharingStatus } from "../api/types";
import { useApp } from "../state/store";
import { AutoUpdateSetting } from "../updates/UpdateSheet";
import { loadBitPerfect, loadCrossfade, loadSoundCheck, saveBitPerfect, saveCrossfade, saveSoundCheck } from "./playbackPrefs";
import type { TextSizes } from "./textSizes";
import "./Preferences.css";

type Tab = "general" | "playback" | "sharing" | "advanced";

const TABS: { id: Tab; label: string; icon: ReactNode }[] = [
  {
    id: "general",
    label: "General",
    icon: <path d="M16 4a12 12 0 1 0 0 24 12 12 0 0 0 0-24zm0 5.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13z" />,
  },
  {
    id: "playback",
    label: "Playback",
    icon: <path d="M10 6v20l17-10z" />,
  },
  {
    id: "sharing",
    label: "Sharing",
    icon: <path d="M11 10a4 4 0 1 1 0 8 4 4 0 0 1 0-8zm10 0a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM3 27c0-4.5 3.6-7 8-7s8 2.5 8 7zm16.5 0c0-2.5-.9-4.5-2.4-5.8.9-.5 2.4-1.2 3.9-1.2 4.4 0 8 2.5 8 7z" />,
  },
  {
    id: "advanced",
    label: "Advanced",
    icon: (
      <path d="M14 3h4l.7 3.4 2.2.9 2.9-1.9 2.8 2.8-1.9 2.9.9 2.2L29 14v4l-3.4.7-.9 2.2 1.9 2.9-2.8 2.8-2.9-1.9-2.2.9L18 29h-4l-.7-3.4-2.2-.9-2.9 1.9-2.8-2.8 1.9-2.9-.9-2.2L3 18v-4l3.4-.7.9-2.2-1.9-2.9 2.8-2.8 2.9 1.9 2.2-.9zm2 8a5 5 0 1 0 0 10 5 5 0 0 0 0-10z" />
    ),
  },
];

/** iTunes' Advanced pane: where the library keeps its files, and how it treats them. */
function AdvancedTab() {
  const [settings, setSettings] = useState<MediaSettings | null>(null);
  const showNotice = useApp((s) => s.showNotice);
  useEffect(() => {
    void libraryMediaSettings().then(setSettings);
  }, []);
  if (!settings) return null;
  const save = (next: MediaSettings) => {
    if (!next.folder) return;
    setSettings(next);
    void librarySetMediaSettings(next.folder, next.organize, next.copyOnAdd).catch((e: unknown) => showNotice(String(e)));
  };
  const change = async () => {
    const folder = await open({ title: "Media Folder Location", directory: true, multiple: false, defaultPath: settings.folder ?? undefined });
    if (typeof folder === "string") save({ ...settings, folder });
  };
  return (
    <div className="prefs-advanced">
      <div className="prefs-box">
        <div className="prefs-box-title">Media folder location</div>
        <div className="prefs-folder">
          <span className="prefs-folder-path">{settings.folder}</span>
          <button className="push-button" onClick={() => void change()}>
            Change…
          </button>
          <button className="push-button" onClick={() => void audioDir().then((music) => join(music, "AntiStreaming")).then((folder) => save({ ...settings, folder }))}>
            Reset
          </button>
        </div>
      </div>
      <label className="prefs-check">
        <input type="checkbox" checked={settings.organize} onChange={(e) => save({ ...settings, organize: e.target.checked })} />
        Keep media folder organized
      </label>
      <p className="prefs-note">
        Places files into album and artist folders, and names the files based on the disc number, track number, and the song title.
      </p>
      <label className="prefs-check">
        <input type="checkbox" checked={settings.copyOnAdd} onChange={(e) => save({ ...settings, copyOnAdd: e.target.checked })} />
        Copy files to media folder when adding to library
      </label>
      <p className="prefs-note">Your originals stay where they are; the library uses the copies.</p>
    </div>
  );
}

function GeneralTab() {
  const sizes = useApp((s) => s.textSizes);
  const update = useApp((s) => s.setTextSizes);
  return (
    <>
      <div className="prefs-grid">
        <span className="prefs-label">Source Text:</span>
        <select className="smart-popup" value={sizes.source} onChange={(e) => update({ ...sizes, source: e.target.value as TextSizes["source"] })}>
          <option value="small">Small</option>
          <option value="large">Large</option>
        </select>
        <span className="prefs-label">List Text:</span>
        <select className="smart-popup" value={sizes.list} onChange={(e) => update({ ...sizes, list: e.target.value as TextSizes["list"] })}>
          <option value="small">Small</option>
          <option value="large">Large</option>
        </select>
      </div>
      <AutoUpdateSetting />
    </>
  );
}

/** The visualizer's two choices; the style is picked from the View menu. */
function VisualizerSettings() {
  const { limitFlashing, microphone, microphoneDevice } = useApp((s) => s.visualizer);
  const setVisualizer = useApp((s) => s.setVisualizer);
  const showNotice = useApp((s) => s.showNotice);
  const [devices, setDevices] = useState<MicDevice[]>([]);
  // Listing opens no microphone, but there is nothing to pick from until it is wanted.
  useEffect(() => {
    if (microphone) void visualizerMicDevices().then(setDevices, (e: unknown) => showNotice(String(e)));
  }, [microphone, showNotice]);
  return (
    <div className="prefs-box prefs-visualizer">
      <div className="prefs-box-title">Visualizer</div>
      <label className="prefs-check">
        <input type="checkbox" checked={limitFlashing} onChange={(e) => setVisualizer({ limitFlashing: e.target.checked })} />
        Limit flashing
      </label>
      <p className="prefs-note">Keeps the picture to no more than three bright flashes a second, for people who are sensitive to flashing lights.</p>
      <label className="prefs-check">
        <input type="checkbox" checked={microphone} onChange={(e) => setVisualizer({ microphone: e.target.checked })} />
        React to sound around you (microphone)
      </label>
      <div className="prefs-device">
        <select
          className="smart-popup"
          aria-label="Microphone"
          disabled={!microphone}
          value={microphoneDevice ?? ""}
          onChange={(e) => setVisualizer({ microphoneDevice: e.target.value || null })}
        >
          <option value="">System default</option>
          {devices.map((device) => (
            <option key={device.id} value={device.id}>
              {device.name}
            </option>
          ))}
        </select>
      </div>
      <p className="prefs-note">
        The visualizer follows the room instead of the music, so it can keep time with a record player or an instrument. macOS
        asks for permission the first time. Nothing is recorded or kept.
      </p>
    </div>
  );
}

function PlaybackTab() {
  const [bitPerfect, setBitPerfect] = useState(loadBitPerfect);
  const [soundCheck, setSoundCheck] = useState(loadSoundCheck);
  const [crossfade, setCrossfade] = useState(loadCrossfade);
  // The slider remembers its length while Crossfade Songs is off.
  const [fadeSeconds, setFadeSeconds] = useState(() => loadCrossfade() ?? 6);
  const applyCrossfade = (seconds: number | null) => {
    setCrossfade(seconds);
    saveCrossfade(seconds);
    void playerSetCrossfade(seconds);
  };
  const outputFormat = useApp((s) => s.playback.outputFormat);
  const active = useApp((s) => s.playback.bitPerfect);
  const toggle = (enabled: boolean) => {
    setBitPerfect(enabled);
    saveBitPerfect(enabled);
    void playerSetBitPerfect(enabled);
  };
  const toggleSoundCheck = (enabled: boolean) => {
    setSoundCheck(enabled);
    saveSoundCheck(enabled);
    void playerSetSoundCheck(enabled);
  };
  return (
    <div>
      <label className="prefs-check">
        <input type="checkbox" checked={crossfade !== null} onChange={(e) => applyCrossfade(e.target.checked ? fadeSeconds : null)} />
        Crossfade Songs
      </label>
      <div className="prefs-crossfade">
        <input
          type="range"
          min={1}
          max={12}
          step={1}
          value={fadeSeconds}
          disabled={crossfade === null}
          aria-label="Crossfade length"
          onChange={(e) => {
            setFadeSeconds(Number(e.target.value));
            applyCrossfade(Number(e.target.value));
          }}
        />
        <span>{fadeSeconds} {fadeSeconds === 1 ? "second" : "seconds"}</span>
      </div>
      <p className="prefs-note">Songs from the same album still play without a break, so live and gapless albums stay intact.</p>
      <label className="prefs-check">
        <input type="checkbox" checked={soundCheck} onChange={(e) => toggleSoundCheck(e.target.checked)} />
        Sound Check
      </label>
      <p className="prefs-note">
        Automatically adjusts songs to play at the same volume. Songs with ReplayGain or Sound Check tags use them; the rest are
        measured in the background, a few seconds each. Not applied with bit-perfect output.
      </p>
      <label className="prefs-check">
        <input type="checkbox" checked={bitPerfect} onChange={(e) => toggle(e.target.checked)} />
        Bit-perfect output
      </label>
      <p className="prefs-note">
        AntiStreaming takes exclusive use of your audio device and switches it to each song's own sample rate and bit depth,
        so the music reaches your DAC untouched. Other apps can't play sound meanwhile, and volume is fixed: use your
        device's or amplifier's volume instead.
      </p>
      {outputFormat && (
        <div className="prefs-status">
          {active ? "Bit-perfect: " : "Output: "}
          <strong>{outputFormat}</strong>
        </div>
      )}
      <VisualizerSettings />
    </div>
  );
}

function SharingTab() {
  const [status, setStatus] = useState<SharingStatus | null>(null);
  const [draft, setDraft] = useState<SharingSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [revealed, setRevealed] = useState(false);

  useEffect(() => {
    void sharingStatus().then((s) => {
      setStatus(s);
      setDraft(s.settings);
    });
  }, []);

  if (!status || !draft) return null;
  const save = (settings: SharingSettings) => {
    setError(null);
    void sharingUpdate(settings).then(
      (s) => {
        setStatus(s);
        setDraft(s.settings);
      },
      (e) => setError(String(e)),
    );
  };
  const dirty = JSON.stringify(draft) !== JSON.stringify(status.settings);

  return (
    <div className="prefs-sharing">
      <label className="prefs-check">
        <input type="checkbox" checked={draft.enabled} onChange={(e) => save({ ...draft, enabled: e.target.checked })} />
        Share my library with my phone and other devices
      </label>
      <p className="prefs-note">
        Phone apps that speak Subsonic (Symfonium, Amperfy, play:Sub, DSub) can browse and stream your music from this Mac. Only
        devices on your network that know the password can connect.
      </p>
      <div className="prefs-grid">
        <span className="prefs-label">Shared name:</span>
        <input className="smart-input" value={draft.sharedName} onChange={(e) => setDraft({ ...draft, sharedName: e.target.value })} />
        <span className="prefs-label">User name:</span>
        <input className="smart-input" value={draft.username} onChange={(e) => setDraft({ ...draft, username: e.target.value })} />
        <span className="prefs-label">Password:</span>
        <span className="prefs-inline">
          <input
            className="smart-input"
            type={revealed ? "text" : "password"}
            value={draft.password}
            onChange={(e) => setDraft({ ...draft, password: e.target.value })}
          />
          <button className="push-button" onClick={() => setRevealed((r) => !r)}>
            {revealed ? "Hide" : "Show"}
          </button>
        </span>
        <span className="prefs-label">Stream at most:</span>
        <select
          className="smart-popup"
          value={draft.maxKbps ?? 0}
          onChange={(e) => save({ ...draft, maxKbps: Number(e.target.value) || null })}
        >
          <option value={0}>Original quality</option>
          {[320, 256, 192, 128].map((kbps) => (
            <option key={kbps} value={kbps}>
              {kbps} kbps
            </option>
          ))}
        </select>
        <span className="prefs-label">Port:</span>
        <input
          className="smart-input smart-input--number"
          type="number"
          min={1024}
          max={65535}
          value={draft.port}
          onChange={(e) => setDraft({ ...draft, port: Number(e.target.value) })}
        />
      </div>
      {dirty && (
        <button className="push-button push-button--default prefs-apply" onClick={() => save(draft)}>
          Apply
        </button>
      )}
      <div className="prefs-status">
        {status.running ? (
          <>
            <strong>Sharing is on.</strong> In your phone app, add a Subsonic server at
            <ul>
              {status.addresses.map((a) => (
                <li key={a}>
                  <code>{a}</code>
                </li>
              ))}
            </ul>
            with the user name and password above. macOS may ask once whether to allow incoming connections.
          </>
        ) : (
          <span>Sharing is off.</span>
        )}
        {(error ?? status.error) && <p className="smart-error">{error ?? status.error}</p>}
      </div>
    </div>
  );
}

/** iTunes 10's Preferences: a toolbar of tabs over each pane, with OK to close. */
export function Preferences() {
  const [tab, setTab] = useState<Tab>("general");
  const setOpen = useApp((s) => s.setPreferencesOpen);
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}>
      <div className="sheet prefs" role="dialog" aria-label="Preferences">
        <div className="prefs-toolbar" role="tablist">
          {TABS.map((t) => (
            <button key={t.id} role="tab" aria-selected={tab === t.id} className="prefs-tab" onClick={() => setTab(t.id)}>
              <svg width={32} height={32} viewBox="0 0 32 32" aria-hidden="true">
                {t.icon}
              </svg>
              {t.label}
            </button>
          ))}
        </div>
        <div className="prefs-pane">
          {tab === "general" && <GeneralTab />}
          {tab === "playback" && <PlaybackTab />}
          {tab === "sharing" && <SharingTab />}
          {tab === "advanced" && <AdvancedTab />}
        </div>
        <div className="sheet-buttons">
          <button className="push-button push-button--default" onClick={() => setOpen(false)}>
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
