import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect } from "react";
import {
  libraryAddPaths,
  onCdChanged,
  onCdRip,
  onDownload,
  onDeviceSync,
  onRemoteChanged,
  onOrganizeProgress,
  onDevicesChanged,
  onLibraryChanged,
  onMenuAction,
  onPlayerEvent,
  onScanProgress,
  playerNext,
  playerPrevious,
  playerSetBitPerfect,
  playerSetCrossfade,
  playerSetSoundCheck,
  playerStatus,
  type MenuAction,
} from "./api/commands";
import { Content } from "./browser/Content";
import { BottomBar } from "./chrome/BottomBar";
import { Toolbar } from "./chrome/Toolbar";
import { SetUpPlayer } from "./devices/SetUpPlayer";
import { applyEqualizer, EqualizerPanel } from "./equalizer/EqualizerPanel";
import { LibrarySheets } from "./info/LibrarySheets";
import { ConnectToServer } from "./shared/Shared";
import { Sidebar } from "./sidebar/Sidebar";
import { UpdateSheet } from "./updates/UpdateSheet";
import { newCondition } from "./smart/fields";
import { SmartPlaylistEditor } from "./smart/SmartPlaylistEditor";
import { loadBitPerfect, loadCrossfade, loadEqualizer, loadSoundCheck } from "./preferences/playbackPrefs";
import { Preferences } from "./preferences/Preferences";
import { isLibrarySource, isPlaylistSource, useApp } from "./state/store";
import "./theme/base.css";
import "./App.css";

const VOLUME_STEP = 0.1;

function runMenuAction(action: MenuAction) {
  const { togglePlay, setVolume, playback, createPlaylist, openSmartEditor, playlists, setPreferencesOpen, openLibrarySheet, showDuplicates, showNotice } =
    useApp.getState();
  const { source, selection, tracks } = useApp.getState();
  // Only songs in the library can be edited or shown; a CD's or iPod's are read-only here.
  const inLibrary = isLibrarySource(source) || isPlaylistSource(source);
  switch (action) {
    case "get-info":
      if (inLibrary && selection.size > 0) openLibrarySheet({ type: "info", trackIds: [...selection] });
      return;
    case "show-in-finder": {
      const track = tracks.find((t) => selection.has(t.id));
      if (!inLibrary || !track) return;
      if (track.missing) return showNotice("The file for this song can't be found");
      return void revealItemInDir(track.path);
    }
    case "show-duplicates":
      return void showDuplicates(false);
    case "show-exact-duplicates":
      return void showDuplicates(true);
    case "organize-library":
      return openLibrarySheet({ type: "organize" });
    case "set-up-player":
      return useApp.setState({ settingUpPlayer: true });
    case "connect-to-server":
      return useApp.setState({ connectingToServer: true });
    case "equalizer":
      return useApp.setState((s) => ({ equalizerOpen: !s.equalizerOpen }));
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
    const { refresh, refreshDevices, refreshCds, refreshShared, refreshPlaylists, applyPlayerEvent, setScan, setWindowActive } = useApp.getState();
    const subscriptions = [
      onPlayerEvent(applyPlayerEvent),
      onMenuAction(runMenuAction),
      onDevicesChanged(() => void refreshDevices()),
      onCdChanged(() => void refreshCds()),
      onRemoteChanged(() => void refreshShared()),
      onCdRip((cdRip) => useApp.setState({ cdRip })),
      onDeviceSync((deviceSync) => useApp.setState({ deviceSync })),
      onOrganizeProgress((organize) => useApp.setState({ organize })),
      onScanProgress(setScan),
      onDownload((p) =>
        useApp.setState({ download: p && { trackId: p.trackId, from: p.from, fraction: p.total > 0 ? p.done / p.total : 0 } }),
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
    void refreshCds();
    void refreshShared();
    void refreshPlaylists();
    void playerStatus().then((status) => applyPlayerEvent({ type: "stateChanged", ...status }));
    if (loadBitPerfect()) void playerSetBitPerfect(true);
    if (loadSoundCheck()) void playerSetSoundCheck(true);
    applyEqualizer(loadEqualizer());
    const crossfade = loadCrossfade();
    if (crossfade !== null) void playerSetCrossfade(crossfade);
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
  const settingUpPlayer = useApp((s) => s.settingUpPlayer);
  const connectingToServer = useApp((s) => s.connectingToServer);
  const equalizerOpen = useApp((s) => s.equalizerOpen);
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
      <LibrarySheets />
      {settingUpPlayer && <SetUpPlayer />}
      {connectingToServer && <ConnectToServer />}
      <UpdateSheet />
      {equalizerOpen && <EqualizerPanel />}
    </div>
  );
}
