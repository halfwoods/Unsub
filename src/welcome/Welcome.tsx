import { useEffect, useState, type ReactNode } from "react";
import {
  libraryMediaSettings,
  librarySetMediaSettings,
  libraryStats,
  updatesSetAutoCheck,
  updatesStatus,
  welcomeFolders,
} from "../api/commands";
import type { MediaSettings, UpdatesStatus, WelcomeFolder } from "../api/types";
import { SourceIcon, type SourceIconName } from "../sidebar/icons";
import { addMusicPaths, completeMusicWelcome, pickMusicFolders } from "../import/addMusic";
import { deviceSource, useApp } from "../state/store";
import { IS_MAC } from "../util/platform";
import "./Welcome.css";

const TIPS = [
  "You're all set. Right-click a song for more.",
  "Advanced › Clean Up Song Info fixes messy titles.",
  "The speaker button plays to Chromecast, Sonos and AirPlay.",
];

/** A notice's own four seconds, and a breath before the next. */
const TIP_MS = 4500;

/** Once the import the welcome started has brought in songs, says what to try next, one tip at a time in the LCD. */
export async function showWelcomeTips() {
  if (!useApp.getState().welcomeTipPending) return;
  const { count } = await libraryStats({ kind: null, search: null, sort: "artist", descending: false });
  // Another change event may have got here first while the count was read.
  if (count === 0 || !useApp.getState().welcomeTipPending) return;
  useApp.setState({ welcomeTipPending: false });
  const { notice, showNotice } = useApp.getState();
  // A notice already up is news about the import itself, such as files that couldn't be added, and mustn't be talked over.
  if (notice === null) TIPS.forEach((tip, i) => setTimeout(() => showNotice(tip), i * TIP_MS));
}

function WelcomeOption({
  icon,
  title,
  text,
  hint,
  children,
}: {
  icon: SourceIconName;
  title: string;
  text: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <li className="welcome-option">
      <div className="welcome-icon">
        <SourceIcon name={icon} />
      </div>
      <div className="welcome-body">
        <div className="welcome-option-title">{title}</div>
        <p className="welcome-option-text">{text}</p>
        {hint && <p className="welcome-option-hint">{hint}</p>}
        <div className="welcome-actions">{children}</div>
      </div>
    </li>
  );
}

/**
 * The first-run welcome, in the spirit of iTunes' setup assistant: ways to bring music in, all of them the File menu's own,
 * and the two choices worth making on day one. Picking anything, or Skip, is remembered so it doesn't come back by itself.
 */
export function Welcome() {
  const devices = useApp((s) => s.devices);
  const showNotice = useApp((s) => s.showNotice);
  const setSource = useApp((s) => s.setSource);
  const openImportSheet = useApp((s) => s.openImportSheet);
  const [folders, setFolders] = useState<WelcomeFolder[]>([]);
  const [media, setMedia] = useState<MediaSettings | null>(null);
  const [updates, setUpdates] = useState<UpdatesStatus | null>(null);
  const [organize, setOrganize] = useState(false);
  const [autoCheck, setAutoCheck] = useState(true);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void welcomeFolders().then(setFolders, (e: unknown) => showNotice(String(e)));
    void libraryMediaSettings().then((settings) => {
      setMedia(settings);
      setOrganize(settings.organize);
    }, (e: unknown) => showNotice(String(e)));
    void updatesStatus().then((status) => {
      setUpdates(status);
      setAutoCheck(status.autoCheck);
    }, (e: unknown) => showNotice(String(e)));
  }, []);

  const skip = () => completeMusicWelcome(false);

  /** A failed folder import leaves the welcome open so another folder can be chosen. */
  const answer = async (next?: () => void | boolean | Promise<void | boolean>, startsImport = false) => {
    setBusy(true);
    try {
      // Only a change is written, so leaving the box alone never touches the media folder.
      if (media?.folder && organize !== media.organize) await librarySetMediaSettings(media.folder, organize, media.copyOnAdd);
      if (updates?.configured && autoCheck !== updates.autoCheck) await updatesSetAutoCheck(autoCheck);
      if (next && (await next()) === false) return;
      await completeMusicWelcome(startsImport);
    } catch (e) {
      showNotice(String(e));
    } finally {
      setBusy(false);
    }
  };

  const addFolders = async (paths: string[]) => {
    await answer(() => addMusicPaths(paths), true);
    // A small folder can finish scanning before its enqueue request returns.
    if (!useApp.getState().welcoming) void showWelcomeTips().catch((e: unknown) => showNotice(String(e)));
  };

  const chooseFolder = async () => {
    setBusy(true);
    try {
      const picked = await pickMusicFolders();
      if (picked.length > 0) await addFolders(picked);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && !busy && void skip()}>
      <div className="sheet welcome" role="dialog" aria-label="Welcome to Unsub" aria-busy={busy}>
        <h2 className="welcome-title">Welcome to Unsub</h2>
        <p className="welcome-subtitle">Choose where to add music from. Nothing is added until you choose, and your files stay where they are unless you say otherwise.</p>
        <ul className="welcome-options">
          {IS_MAC && (
            <WelcomeOption
              icon="music"
              title="Import from the Music app"
              text="Brings in the songs you own as files, with their play counts, ratings and playlists."
              hint="Songs you only stream from Apple Music can't play outside Apple's app. They go on your To Own list, so you can see what you'd lose."
            >
              <button className="push-button" disabled={busy} onClick={() => void answer(() => openImportSheet(null), true)}>
                Import…
              </button>
            </WelcomeOption>
          )}
          <WelcomeOption
            icon="folder"
            title="Add a music folder"
            text="Choose any folder on your computer or an external drive. You can add more folders later."
          >
            <button className="push-button push-button--default" autoFocus disabled={busy} onClick={() => void chooseFolder()}>
              Choose Folder…
            </button>
            <p className="welcome-drag-hint">Or drag music files and folders onto this window.</p>
            {folders.length > 0 && (
              <div className="welcome-folder-shortcuts">
                <p>Or add one of these folders:</p>
                {folders.map(({ path, label }) => (
                  <div key={path} className="welcome-folder-shortcut">
                    <button className="push-button" disabled={busy} onClick={() => void addFolders([path])}>
                      Add {label}
                    </button>
                    <span className="welcome-folder-path">{path}</span>
                  </div>
                ))}
              </div>
            )}
          </WelcomeOption>
          <WelcomeOption
            icon="playlist"
            title="Bring your Spotify playlists"
            text="Rebuild your playlists and listening history from Spotify, YouTube Music or ListenBrainz."
          >
            <button className="push-button" disabled={busy} onClick={() => void answer(() => useApp.setState({ importingElsewhere: true }), true)}>
              Import…
            </button>
          </WelcomeOption>
          {devices.map((device) => (
            <WelcomeOption key={device.id} icon="ipod" title={`Use ${device.name}`} text="It's plugged in. Its page shows what's on it and syncs it.">
              <button className="push-button" disabled={busy} onClick={() => void answer(() => setSource(deviceSource(device.id)))}>
                Open iPod
              </button>
            </WelcomeOption>
          ))}
        </ul>
        {devices.length === 0 && <p className="welcome-hint">Plug in an iPod any time; it shows up under Devices.</p>}
        <div className="welcome-heading">Options</div>
        <label className="prefs-check">
          <input type="checkbox" checked={organize} disabled={busy} onChange={(e) => setOrganize(e.target.checked)} />
          Keep my media folder organized
        </label>
        <p className="prefs-note">
          Songs in Unsub's own media folder, like CD rips, are filed by artist and album. Music you add from other folders is never moved.
        </p>
        {updates?.configured && (
          <label className="prefs-check">
            <input type="checkbox" checked={autoCheck} disabled={busy} onChange={(e) => setAutoCheck(e.target.checked)} />
            Check for updates automatically
          </label>
        )}
        <div className="sheet-buttons">
          <button className="push-button" title="Fine, we'll be over here." disabled={busy} onClick={() => void skip()}>
            Skip
          </button>
          <button className="push-button" disabled={busy} onClick={() => void answer()}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
