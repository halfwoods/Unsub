import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { create } from "zustand";
import { onMiniPlayer, setMiniPlayer, setMiniPlayerAllowed, windowMiniPlayer } from "../api/commands";
import { loadMiniPlayerOnTop, saveMiniPlayerOnTop } from "../preferences/playbackPrefs";
import { useApp } from "../state/store";

/** Every sheet and dialog renders with one of these, including those whose open state is local to a component. */
const SHEET = '.sheet-backdrop, [role="dialog"], [role="alertdialog"]';

interface MiniPlayerState {
  /** Whether the one window is the mini player; the backend decides, since the menu and the Dock icon switch it too. */
  mini: boolean;
  onTop: boolean;
  /** The player has no room for a sheet, and hiding one would throw away what is typed into it. */
  sheetOpen: boolean;
}

export const useMiniPlayer = create<MiniPlayerState>(() => ({ mini: false, onTop: loadMiniPlayerOnTop(), sheetOpen: false }));

export function setMiniPlayerOnTop(onTop: boolean) {
  saveMiniPlayerOnTop(onTop);
  useMiniPlayer.setState({ onTop });
}

const touchesSheet = (nodes: NodeList) => [...nodes].some((node) => node instanceof Element && (node.matches(SHEET) || node.querySelector(SHEET) !== null));

/** Follows the backend's mode, keeps it from entering over a sheet, and floats the window only while it is the mini player. */
export function useMiniPlayerSync() {
  const visualizerOpen = useApp((s) => s.visualizer.open);
  useEffect(() => {
    const subscription = onMiniPlayer((mini) => useMiniPlayer.setState({ mini }));
    void windowMiniPlayer().then((mini) => useMiniPlayer.setState({ mini }));
    return () => void subscription.then((unlisten) => unlisten());
  }, []);
  useEffect(() => {
    const update = () => useMiniPlayer.setState({ sheetOpen: visualizerOpen || document.querySelector(SHEET) !== null });
    update();
    const observer = new MutationObserver((records) => {
      if (records.some((record) => touchesSheet(record.addedNodes) || touchesSheet(record.removedNodes))) update();
    });
    observer.observe(document.body, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, [visualizerOpen]);
  const mini = useMiniPlayer((s) => s.mini);
  const sheetOpen = useMiniPlayer((s) => s.sheetOpen);
  useEffect(() => {
    void setMiniPlayerAllowed(!sheetOpen);
    // A sheet that opens over the player takes the window back to full size, where it fits.
    if (mini && sheetOpen) void setMiniPlayer(false);
  }, [mini, sheetOpen]);
  const floating = useMiniPlayer((s) => s.mini && s.onTop);
  useEffect(() => {
    void getCurrentWindow().setAlwaysOnTop(floating);
  }, [floating]);
}
