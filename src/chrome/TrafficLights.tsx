import type { MouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { setMiniPlayer } from "../api/commands";
import { useApp } from "../state/store";
import { useMiniPlayer } from "./miniMode";
import "./TrafficLights.css";

const appWindow = getCurrentWindow();

// iTunes 10 stacked the window buttons vertically to save toolbar width.
export function TrafficLights() {
  const fullscreen = useApp((s) => s.windowFullscreen);
  const mini = useMiniPlayer((s) => s.mini);
  // As in iTunes 10, a plain click shrinks to the mini player and Option-click zooms.
  const zoom = (e: MouseEvent) => {
    if (fullscreen) void appWindow.setFullscreen(false);
    else if (mini) void setMiniPlayer(false);
    else if (e.altKey) void appWindow.toggleMaximize();
    else if (!useMiniPlayer.getState().sheetOpen) void setMiniPlayer(true);
  };
  return (
    <div className="traffic-lights">
      <button className="traffic-light traffic-light--close" aria-label="Close" onClick={() => appWindow.close()}>
        <svg viewBox="0 0 12 12" aria-hidden="true">
          <path d="M3.5 3.5l5 5M8.5 3.5l-5 5" />
        </svg>
      </button>
      <button className="traffic-light traffic-light--minimize" aria-label="Minimize" disabled={fullscreen} onClick={() => appWindow.minimize()}>
        <svg viewBox="0 0 12 12" aria-hidden="true">
          <path d="M3 6h6" />
        </svg>
      </button>
      <button className="traffic-light traffic-light--zoom" aria-label={mini ? "Switch from Mini Player" : "Zoom"} onClick={zoom}>
        <svg viewBox="0 0 12 12" aria-hidden="true">
          <path d="M3 6h6M6 3v6" />
        </svg>
      </button>
    </div>
  );
}
