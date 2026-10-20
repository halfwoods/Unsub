import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";
import {
  libraryAddPaths,
  onCloudDownload,
  onDevicesChanged,
  onLibraryChanged,
  onMenuAction,
  onPlayerEvent,
  onScanProgress,
  playerNext,
  playerPrevious,
  playerSetBitPerfect,
  playerStatus,
  type MenuAction,
} from "./api/commands";
import { Content } from "./browser/Content";
import { BottomBar } from "./chrome/BottomBar";
import { Toolbar } from "./chrome/Toolbar";
import { Sidebar } from "./sidebar/Sidebar";
import { newCondition } from "./smart/fields";
import { SmartPlaylistEditor } from "./smart/SmartPlaylistEditor";
import { loadBitPerfect } from "./preferences/playbackPrefs";
import { Preferences } from "./preferences/Preferences";
import { useApp } from "./state/store";
import "./theme/base.css";
import "./App.css";

const VOLUME_STEP = 0.1;

function runMenuAction(action: MenuAction) {
  const { togglePlay, setVolume, playback, createPlaylist, openSmartEditor, playlists, setPreferencesOpen } = useApp.getState();
  switch (action) {
    case "preferences":
      return setPreferencesOpen(true);
    case "new-playlist":
      return void createPlaylist("regular");
    case "new-playlist-from-selection":
      return void createPlaylist("regular", true);
    case "new-playlist-folder":
      return void createPlaylist("folder");
    // A new smart playlist starts like iTunes': one empty "Artist contains" rule, live updating.
    case "new-smart-playlist":
      return openSmartEditor({
        id: null,
        name: "untitled playlist",
        rules: { matchAll: true, rules: [newCondition(playlists.map((p) => p.id))], limit: null, onlyChecked: false, live: true },
      });
    case "play-pause":
      return void togglePlay();
    case "next":
      return void playerNext();
    case "previous":
      return void playerPrevious();
    case "volume-up":
      return setVolume(Math.min(1, playback.volume + VOLUME_STEP));
    case "volume-down":
      return setVolume(Math.max(0, playback.volume - VOLUME_STEP));
  }
}

/** Subscribes to backend events for the lifetime of the window. */
function useBackendEvents() {
  useEffect(() => {
    const { refresh, refreshDevices, refreshPlaylists, applyPlayerEvent, setScan, setWindowActive } = useApp.getState();
    const subscriptions = [
      onPlayerEvent(applyPlayerEvent),
      onMenuAction(runMenuAction),
      onDevicesChanged(() => void refreshDevices()),
      onScanProgress(setScan),
      onCloudDownload((p) =>
        useApp.setState({ cloudDownload: p && { trackId: p.trackId, fraction: p.total > 0 ? p.done / p.total : 0 } }),
      ),
      onLibraryChanged(() => {
        setScan(null);
        void refresh();
        // New songs change smart playlists' contents and counts.
        void refreshPlaylists();
      }),
      getCurrentWindow().onFocusChanged(({ payload }) => setWindowActive(payload)),
      getCurrentWebview().onDragDropEvent(({ payload }) => {
        if (payload.type === "drop" && payload.paths.length > 0) void libraryAddPaths(payload.paths);
      }),
    ];
    void refresh();
    void refreshDevices();
    void refreshPlaylists();
    void playerStatus().then((status) => applyPlayerEvent({ type: "stateChanged", ...status }));
    if (loadBitPerfect()) void playerSetBitPerfect(true);
    return () => subscriptions.forEach((s) => void s.then((unlisten) => unlisten()));
  }, []);
}

/** Space toggles playback anywhere except while typing, as in iTunes. */
function useGlobalKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const typing = e.target instanceof HTMLInputElement;
      if (e.key === " " && !typing) {
        e.preventDefault();
        void useApp.getState().togglePlay();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

export default function App() {
  const windowActive = useApp((s) => s.windowActive);
  const smartEditor = useApp((s) => s.smartEditor);
  const preferencesOpen = useApp((s) => s.preferencesOpen);
  useBackendEvents();
  useGlobalKeys();

  return (
    <div className="window" data-active={windowActive}>
      <Toolbar />
      <div className="window-body">
        <Sidebar />
        <Content />
      </div>
      <BottomBar />
      {/* Keyed so each opening starts from that playlist's saved rules. */}
      {smartEditor && <SmartPlaylistEditor key={smartEditor.id ?? "new"} />}
      {preferencesOpen && <Preferences />}
    </div>
  );
}
