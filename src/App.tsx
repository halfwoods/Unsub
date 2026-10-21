import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { lazy, Suspense, useEffect } from "react";
import {
  onCastTarget,
  onCdChanged,
  onCdRip,
  onDownload,
  onDeviceSync,
  onRemoteChanged,
  onOrganizeProgress,
  onCleanUpDone,
  onCleanUpProgress,
  onDetailsProgress,
  onDevicesChanged,
  onImportProgress,
  onLibraryChanged,
  onMenuAction,
  onPlayerEvent,
  onRadioLink,
  onRadioNowPlaying,
  onRadioStation,
  onScanProgress,
  playerNext,
  playerPrevious,
  playerSetBitPerfect,
  playerSetCrossfade,
  playerSetSoundCheck,
  playerStatus,
  welcomeDue,
  type MenuAction,
} from "./api/commands";
import { Content } from "./browser/Content";
import { BottomBar } from "./chrome/BottomBar";
import { Toolbar } from "./chrome/Toolbar";
import { SetUpPlayer } from "./devices/SetUpPlayer";
import { applyEqualizer, EqualizerPanel } from "./equalizer/EqualizerPanel";
import { ImportElsewhere } from "./import/ImportElsewhere";
import { ImportSheet } from "./import/ImportSheet";
import { LibrarySheets } from "./info/LibrarySheets";
import { addDropped } from "./radio/importStations";
import { OpenStream } from "./radio/OpenStream";
import { ConnectToServer } from "./shared/Shared";
import { Sidebar } from "./sidebar/Sidebar";
import { VISUALIZER_STYLES } from "./visualizer/styles";
import { UpdateSheet } from "./updates/UpdateSheet";
import { Welcome, showWelcomeTips } from "./welcome/Welcome";
import { newCondition } from "./smart/fields";
import { SmartPlaylistEditor } from "./smart/SmartPlaylistEditor";
import { loadBitPerfect, loadCrossfade, loadEqualizer, loadSoundCheck } from "./preferences/playbackPrefs";
import { Preferences } from "./preferences/Preferences";
import { chapterSeekTarget, isLibrarySource, isPlaylistSource, useApp } from "./state/store";
import { cleanUpNotice } from "./util/format";
import "./theme/base.css";
import "./App.css";

const VOLUME_STEP = 0.1;

// The WebGL styles are most of the app's code and only matter once the visualizer is shown, so they load then.
const VisualizerPane = lazy(() => import("./visualizer/VisualizerPane").then((module) => ({ default: module.VisualizerPane })));

async function importLibraryXml() {
  const path = await open({ title: "Import Library XML", multiple: false, directory: false, filters: [{ name: "Library XML", extensions: ["xml"] }] });
  if (typeof path === "string") await useApp.getState().openImportSheet(path);
}

function runMenuAction(action: MenuAction) {
  const { togglePlay, setVolume, seekTo, playback, createPlaylist, openSmartEditor, playlists, setPreferencesOpen, setVisualizer, openLibrarySheet, openImportSheet, showDuplicates, showNotice, deleteSelection } =
    useApp.getState();
  const { source, selection, tracks, chapters } = useApp.getState();
  // Only songs in the library can be edited or shown; a CD's or iPod's are read-only here.
  const inLibrary = isLibrarySource(source) || isPlaylistSource(source);
  const visualizerStyle = VISUALIZER_STYLES.find((style) => action === `visualizer-${style}`);
  if (visualizerStyle) return setVisualizer({ style: visualizerStyle, open: true });
  switch (action) {
    case "get-info":
      if (inLibrary && selection.size > 0) openLibrarySheet({ type: "info", trackIds: [...selection] });
      return;
    case "clean-up": {
      // With nothing selected, the whole list is cleaned up, as a playlist was by the scripts this replaces.
      const trackIds = selection.size > 0 ? [...selection] : tracks.map((t) => t.id);
      if (inLibrary && trackIds.length > 0) openLibrarySheet({ type: "cleanUp", trackIds });
      return;
    }
    case "get-song-details":
      return openLibrarySheet({ type: "songDetails" });
    case "show-in-finder": {
      const track = tracks.find((t) => selection.has(t.id));
      if (!inLibrary || !track) return;
      if (track.missing) return showNotice("The file for this song can't be found");
      return void revealItemInDir(track.path);
    }
    case "delete": {
      // Delete belongs to a focused text field, and a sheet in front (Get Info with unsaved edits) must not be replaced.
      const typing = document.activeElement?.matches("input, textarea, [contenteditable]:not([contenteditable=false])");
      if (!typing && !document.querySelector(".sheet-backdrop")) void deleteSelection();
      return;
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
    case "open-stream":
      return useApp.setState({ openingStream: true });
    case "import-music-library":
      return void openImportSheet(null);
    case "import-library-xml":
      return void importLibraryXml();
    case "import-elsewhere":
      return useApp.setState({ importingElsewhere: true });
    case "equalizer":
      return useApp.setState((s) => ({ equalizerOpen: !s.equalizerOpen }));
    case "preferences":
      return setPreferencesOpen(true);
    case "welcome":
      return useApp.setState({ welcoming: true });
    case "show-visualizer":
      return setVisualizer({ open: !useApp.getState().visualizer.open });
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
    case "next-chapter":
    case "previous-chapter": {
      const target = chapterSeekTarget(chapters?.list ?? [], playback.positionMs, action === "next-chapter" ? "next" : "previous");
      if (target !== null) seekTo(target);
      return;
    }
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
      onCastTarget((castTarget) => useApp.setState({ castTarget })),
      onMenuAction(runMenuAction),
      onRadioStation((liveStation) => useApp.setState({ liveStation, liveTitle: null })),
      onRadioLink((liveLink) => useApp.setState({ liveLink })),
      onRadioNowPlaying((title) => useApp.setState((s) => (s.liveStation?.id === title.stationId ? { liveTitle: title } : {}))),
      onDevicesChanged(() => void refreshDevices()),
      onCdChanged(() => void refreshCds()),
      onRemoteChanged(() => void refreshShared()),
      onCdRip((cdRip) => useApp.setState({ cdRip })),
      onDeviceSync((deviceSync) => useApp.setState({ deviceSync })),
      onOrganizeProgress((organize) => useApp.setState({ organize })),
      onCleanUpProgress((cleanUp) => useApp.setState({ cleanUp })),
      onCleanUpDone((done) => useApp.getState().showNotice(cleanUpNotice(done))),
      onDetailsProgress((details) => useApp.setState({ details })),
      onImportProgress((importProgress) => useApp.setState({ importProgress })),
      onScanProgress(setScan),
      onDownload((p) =>
        useApp.setState({ download: p && { trackId: p.trackId, from: p.from, fraction: p.total > 0 ? p.done / p.total : 0 } }),
      ),
      onLibraryChanged(() => {
        setScan(null);
        void refresh();
        // New songs change smart playlists' contents and counts.
        void refreshPlaylists();
        void showWelcomeTips();
      }),
      getCurrentWindow().onFocusChanged(({ payload }) => setWindowActive(payload)),
      getCurrentWebview().onDragDropEvent(({ payload }) => {
        if (payload.type === "drop" && payload.paths.length > 0) addDropped(payload.paths);
      }),
    ];
    void refresh();
    void refreshDevices();
    void refreshCds();
    void refreshShared();
    void refreshPlaylists();
    void playerStatus().then((status) => applyPlayerEvent({ type: "stateChanged", ...status }));
    void welcomeDue().then((due) => due && useApp.setState({ welcoming: true }));
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
  const openingStream = useApp((s) => s.openingStream);
  const importSheet = useApp((s) => s.importSheet);
  const importingElsewhere = useApp((s) => s.importingElsewhere);
  const welcoming = useApp((s) => s.welcoming);
  const equalizerOpen = useApp((s) => s.equalizerOpen);
  const visualizerOpen = useApp((s) => s.visualizer.open);
  useBackendEvents();
  useGlobalKeys();

  return (
    <div className="window" data-active={windowActive}>
      <Toolbar />
      <div className="window-body">
        <Sidebar />
        <Content />
        {visualizerOpen && (
          // The pane's own black, so the window doesn't flash its lists while the code arrives.
          <Suspense fallback={<div className="visualizer-pane" />}>
            <VisualizerPane />
          </Suspense>
        )}
      </div>
      <BottomBar />
      {/* Keyed so each opening starts from that playlist's saved rules. */}
      {smartEditor && <SmartPlaylistEditor key={smartEditor.id ?? "new"} />}
      {preferencesOpen && <Preferences />}
      <LibrarySheets />
      {settingUpPlayer && <SetUpPlayer />}
      {connectingToServer && <ConnectToServer />}
      {openingStream && <OpenStream />}
      {importSheet && <ImportSheet preview={importSheet} />}
      {importingElsewhere && <ImportElsewhere />}
      {welcoming && <Welcome />}
      <UpdateSheet />
      {equalizerOpen && <EqualizerPanel />}
    </div>
  );
}
