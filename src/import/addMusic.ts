import { open } from "@tauri-apps/plugin-dialog";
import { libraryAddPaths, welcomeDone } from "../api/commands";
import { useApp } from "../state/store";

/** Dialogs can return one path or several; cancellation must never start an import. */
export function normalizePickedPaths(picked: string | string[] | null): string[] {
  return typeof picked === "string" ? [picked] : (picked ?? []);
}

/** Queues only the paths the user chose, reporting a failed request instead of silently losing it. */
export async function addMusicPaths(paths: string[]): Promise<boolean> {
  if (paths.length === 0) return false;
  try {
    await libraryAddPaths(paths);
    return true;
  } catch (e) {
    useApp.getState().showNotice(`Couldn't add music: ${String(e)}`);
    return false;
  }
}

/** An accepted music import is a first-run choice, including from the native menu or a file drop. */
export async function completeMusicWelcome(): Promise<void> {
  useApp.setState((state) => ({ welcoming: false, welcomeTipPending: state.welcomeTipPending || state.welcoming }));
  try {
    await welcomeDone();
  } catch (e) {
    useApp.getState().showNotice(`Couldn't save the welcome choice: ${String(e)}`);
  }
}

export async function pickMusicFolders(): Promise<string[]> {
  try {
    return normalizePickedPaths(await open({ title: "Add Music Folder", directory: true, multiple: true }));
  } catch (e) {
    useApp.getState().showNotice(`Couldn't open the folder chooser: ${String(e)}`);
    return [];
  }
}

/** The same folder action is available from the sidebar and the empty library. */
export async function chooseMusicFolders(): Promise<boolean> {
  return addMusicPaths(await pickMusicFolders());
}
