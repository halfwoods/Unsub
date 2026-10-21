import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState, type ReactNode } from "react";
import {
  libraryAddPaths,
  libraryMediaSettings,
  librarySetMediaSettings,
  libraryStats,
  updatesSetAutoCheck,
  updatesStatus,
  welcomeDone,
  welcomeFolders,
} from "../api/commands";
import type { MediaSettings, UpdatesStatus, WelcomeFolder } from "../api/types";
import { SourceIcon, type SourceIconName } from "../sidebar/icons";
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

  useEffect(() => {
    void welcomeFolders().then(setFolders);
    void libraryMediaSettings().then((settings) => {
      setMedia(settings);
      setOrganize(settings.organize);
    });
    void updatesStatus().then((status) => {
      setUpdates(status);
      setAutoCheck(status.autoCheck);
    });
  }, []);

  const remember = async () => {
    try {
      await welcomeDone();
    } catch (e) {
      showNotice(String(e));
    }
  };

  const skip = async () => {
    await remember();
    useApp.setState({ welcoming: false });
  };

  /** Applies the Options, remembers the welcome was answered, then carries out what was picked. */
  const answer = async (next?: () => void, startsImport = false) => {
    try {
      // Only a change is written, so leaving the box alone never touches the media folder.
      if (media?.folder && organize !== media.organize) await librarySetMediaSettings(media.folder, organize, media.copyOnAdd);
      if (updates?.configured && autoCheck !== updates.autoCheck) await updatesSetAutoCheck(autoCheck);
    } catch (e) {
      showNotice(String(e));
    }
    await remember();
    useApp.setState((s) => ({ welcoming: false, welcomeTipPending: s.welcomeTipPending || startsImport }));
    next?.();
  };

  const addFolders = (paths: string[]) => void answer(() => void libraryAddPaths(paths), true);

  const chooseFolder = async () => {
    const picked = await open({ title: "Add to Library", directory: true, multiple: true });
    if (Array.isArray(picked) && picked.length > 0) addFolders(picked);
  };

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && void skip()}>
      <div className="sheet welcome" role="dialog" aria-label="Welcome to Unsub">
        <h2 className="welcome-title">Welcome to Unsub</h2>
        <p className="welcome-subtitle">Let's find your music. It stays where it is unless you say otherwise.</p>
        <ul className="welcome-options">
          {IS_MAC && (
            <WelcomeOption
              icon="music"
              title="Import from the Music app"
              text="Brings in the songs you own as files, with their play counts, ratings and playlists."
              hint="Songs you only stream from Apple Music can't play outside Apple's app. They go on your To Own list, so you can see what you'd lose."
            >
              <button className="push-button" onClick={() => void answer(() => void openImportSheet(null), true)}>
                Import…
              </button>
            </WelcomeOption>
          )}
          <WelcomeOption
            icon="folder"
            title="Add a music folder"
            text={folders.length > 0 ? "Add the folder your music lives in. We found these:" : "Pick the folder your music lives in."}
          >
            {folders.map(({ path, label }) => (
              <button key={path} className="push-button" title={path} onClick={() => addFolders([path])}>
                Add {label}
              </button>
            ))}
            <button className="push-button" onClick={() => void chooseFolder()}>
              Choose Folder…
            </button>
          </WelcomeOption>
          <WelcomeOption
            icon="playlist"
            title="Bring your Spotify playlists"
            text="Rebuild your playlists and listening history from Spotify, YouTube Music or ListenBrainz."
          >
            <button className="push-button" onClick={() => void answer(() => useApp.setState({ importingElsewhere: true }), true)}>
              Import…
            </button>
          </WelcomeOption>
          {devices.map((device) => (
            <WelcomeOption key={device.id} icon="ipod" title={`Use ${device.name}`} text="It's plugged in. Its page shows what's on it and syncs it.">
              <button className="push-button" onClick={() => void answer(() => setSource(deviceSource(device.id)))}>
                Open iPod
              </button>
            </WelcomeOption>
          ))}
        </ul>
        {devices.length === 0 && <p className="welcome-hint">Plug in an iPod any time; it shows up under Devices.</p>}
        <div className="welcome-heading">Options</div>
        <label className="prefs-check">
          <input type="checkbox" checked={organize} onChange={(e) => setOrganize(e.target.checked)} />
          Keep my media folder organized
        </label>
        <p className="prefs-note">
          Songs in Unsub's own media folder, like CD rips, are filed by artist and album. Music you add from other folders is never moved.
        </p>
        {updates?.configured && (
          <label className="prefs-check">
            <input type="checkbox" checked={autoCheck} onChange={(e) => setAutoCheck(e.target.checked)} />
            Check for updates automatically
          </label>
        )}
        <div className="sheet-buttons">
          <button className="push-button" title="Fine, we'll be over here." onClick={() => void skip()}>
            Skip
          </button>
          <button className="push-button push-button--default" autoFocus onClick={() => void answer()}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
