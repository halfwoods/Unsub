import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { create } from "zustand";
import { onMiniPlayer, windowMiniPlayer } from "../api/commands";
import { loadMiniPlayerOnTop, saveMiniPlayerOnTop } from "../preferences/playbackPrefs";

interface MiniPlayerState {
  /** Whether the one window is the mini player; the backend decides, since the menu and the Dock icon switch it too. */
  mini: boolean;
  onTop: boolean;
}

export const useMiniPlayer = create<MiniPlayerState>(() => ({ mini: false, onTop: loadMiniPlayerOnTop() }));

export function setMiniPlayerOnTop(onTop: boolean) {
  saveMiniPlayerOnTop(onTop);
  useMiniPlayer.setState({ onTop });
}

/** Follows the backend's mode, and floats the window only while it is the mini player. */
export function useMiniPlayerSync() {
  useEffect(() => {
    const subscription = onMiniPlayer((mini) => useMiniPlayer.setState({ mini }));
    void windowMiniPlayer().then((mini) => useMiniPlayer.setState({ mini }));
    return () => void subscription.then((unlisten) => unlisten());
  }, []);
  const floating = useMiniPlayer((s) => s.mini && s.onTop);
  useEffect(() => {
    void getCurrentWindow().setAlwaysOnTop(floating);
  }, [floating]);
}
