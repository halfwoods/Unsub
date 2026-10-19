import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";
import {
  libraryAddPaths,
  onDevicesChanged,
  onLibraryChanged,
  onMenuAction,
  onPlayerEvent,
  onScanProgress,
  playerNext,
  playerPrevious,
  playerStatus,
  type MenuAction,
} from "./api/commands";
import { Content } from "./browser/Content";
import { BottomBar } from "./chrome/BottomBar";
import { Toolbar } from "./chrome/Toolbar";
import { Sidebar } from "./sidebar/Sidebar";
import { useApp } from "./state/store";
import "./theme/base.css";
import "./App.css";

const VOLUME_STEP = 0.1;

function runMenuAction(action: MenuAction) {
  const { togglePlay, setVolume, playback } = useApp.getState();
  switch (action) {
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
    const { refresh, refreshDevices, applyPlayerEvent, setScan, setWindowActive } = useApp.getState();
    const subscriptions = [
      onPlayerEvent(applyPlayerEvent),
      onMenuAction(runMenuAction),
      onDevicesChanged(() => void refreshDevices()),
      onScanProgress(setScan),
      onLibraryChanged(() => {
        setScan(null);
        void refresh();
      }),
      getCurrentWindow().onFocusChanged(({ payload }) => setWindowActive(payload)),
      getCurrentWebview().onDragDropEvent(({ payload }) => {
        if (payload.type === "drop" && payload.paths.length > 0) void libraryAddPaths(payload.paths);
      }),
    ];
    void refresh();
    void refreshDevices();
    void playerStatus().then((status) => applyPlayerEvent({ type: "stateChanged", ...status }));
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
    </div>
  );
}
