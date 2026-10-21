import { Menu, type MenuOptions } from "@tauri-apps/api/menu";
import { ask, save } from "@tauri-apps/plugin-dialog";
import { useState, type MouseEvent } from "react";
import {
  cdEject,
  cdStopImport,
  deviceEject,
  deviceSync,
  deviceSyncPreview,
  deviceSyncSettings,
  libraryDeletePlaylist,
  libraryDuplicatePlaylist,
  libraryExportPlaylist,
  libraryTracks,
  playerEject,
  podcastsRefresh,
  remoteForget,
} from "../api/commands";
import type { AudioCd, Device, Player, Playlist, SharedLibrary } from "../api/types";
import { popUp, SEPARATOR } from "../browser/songMenu";
import { importCd } from "../cd/Cd";
import { deviceSource, playlistIdOf, playlistSource, useApp } from "../state/store";
import { IS_LINUX } from "../util/platform";

type MenuItems = NonNullable<MenuOptions["items"]>;

const showError = (e: unknown) => useApp.getState().showNotice(String(e));

/** A row's right-click menu. `open` is true while it is up, to outline the row it is for: right-clicking doesn't select the row, so the page being looked at stays. */
export function useRowMenu(show: (() => Promise<void>) | undefined) {
  const [open, setOpen] = useState(false);
  const onContextMenu = (e: MouseEvent) => {
    if (!show) return;
    e.preventDefault();
    setOpen(true);
    const shown = show().catch(showError);
    if (!IS_LINUX) return void shown.finally(() => setOpen(false));
    // GTK's popup returns as soon as it is up, so the outline stays until the user does something else.
    const done = new AbortController();
    const clear = () => {
      setOpen(false);
      done.abort();
    };
    for (const type of ["pointerdown", "keydown", "blur"]) window.addEventListener(type, clear, { capture: true, signal: done.signal });
  };
  return { open, onContextMenu };
}

const inPlaylistOrder = (playlist: Playlist) =>
  libraryTracks({ kind: null, search: null, sort: "playlistOrder", descending: false, playlist: playlist.id });

async function playPlaylist(playlist: Playlist, shuffle: boolean) {
  // Like double-clicking a song: unchecked songs are skipped, and a station can't queue behind songs.
  const ids = (await inPlaylistOrder(playlist)).filter((t) => t.enabled && t.kind !== "radio").map((t) => t.id);
  await useApp.getState().playTrackIds(ids, shuffle);
}

async function duplicatePlaylist(playlist: Playlist) {
  const copy = await libraryDuplicatePlaylist(playlist.id);
  await useApp.getState().refreshPlaylists();
  useApp.getState().setSource(playlistSource(copy));
}

async function exportPlaylist(playlist: Playlist) {
  const path = await save({
    title: "Export Playlist",
    // A slash in a name would be taken for a folder.
    defaultPath: `${playlist.name.replace(/[\\/:*?"<>|]/g, "-")}.m3u8`,
    filters: [{ name: "M3U8 Playlist", extensions: ["m3u8"] }],
  });
  if (path !== null) await libraryExportPlaylist(playlist.id, path);
}

const playlistsInside = (playlists: Playlist[], folder: number): Playlist[] =>
  playlists.filter((p) => p.parentId === folder).flatMap((p) => [p, ...playlistsInside(playlists, p.id)]);

/** Asks first, as deleting is not undoable; a folder takes the playlists inside it along, which the question says. */
export async function deletePlaylist(playlist: Playlist) {
  const { playlists, refreshPlaylists, setSource } = useApp.getState();
  const folder = playlist.kind === "folder";
  const inside = folder ? playlistsInside(playlists, playlist.id).filter((p) => p.kind !== "folder").length : 0;
  const alsoDeleted = inside > 0 ? ` The ${inside} ${inside === 1 ? "playlist" : "playlists"} inside it will be deleted too.` : "";
  const confirmed = await ask(`Are you sure you want to delete the ${folder ? "folder" : "playlist"} “${playlist.name}”?${alsoDeleted}`, {
    title: "Unsub",
    kind: "warning",
  });
  if (!confirmed) return;
  await libraryDeletePlaylist(playlist.id).catch(showError);
  await refreshPlaylists();
  // The menu can delete a row other than the page on screen, which then stays, unless it went with a folder.
  const viewed = playlistIdOf(useApp.getState().source);
  if (viewed !== null && !useApp.getState().playlists.some((p) => p.id === viewed)) setSource("music");
}

function folderItems(folder: Playlist): MenuItems {
  const { createPlaylist, setRenamingPlaylist } = useApp.getState();
  return [
    { text: "New Playlist in Folder", action: () => void createPlaylist("regular", false, folder.id) },
    { text: "New Folder in Folder", action: () => void createPlaylist("folder", false, folder.id) },
    SEPARATOR,
    { text: "Rename", action: () => setRenamingPlaylist(folder.id) },
    SEPARATOR,
    { text: "Delete Folder…", action: () => void deletePlaylist(folder) },
  ];
}

function songPlaylistItems(playlist: Playlist): MenuItems {
  const { openSmartEditor, setRenamingPlaylist } = useApp.getState();
  const rules = playlist.smart;
  const hasSongs = playlist.trackCount > 0;
  return [
    { text: "Play", enabled: hasSongs, action: () => void playPlaylist(playlist, false) },
    { text: "Shuffle", enabled: hasSongs, action: () => void playPlaylist(playlist, true) },
    SEPARATOR,
    ...(rules ? [{ text: "Edit Smart Playlist…", action: () => openSmartEditor({ id: playlist.id, name: playlist.name, rules }) }] : []),
    { text: "Rename", action: () => setRenamingPlaylist(playlist.id) },
    { text: "Duplicate", action: () => void duplicatePlaylist(playlist).catch(showError) },
    { text: "Export Playlist…", enabled: hasSongs, action: () => void exportPlaylist(playlist).catch(showError) },
    SEPARATOR,
    { text: "Delete Playlist…", action: () => void deletePlaylist(playlist) },
  ];
}

export async function showPlaylistMenu(playlist: Playlist) {
  await popUp(await Menu.new({ items: playlist.kind === "folder" ? folderItems(playlist) : songPlaylistItems(playlist) }));
}

/**
 * Starts a sync when nothing needs saying first. The first sync to an iPod, one that won't fit and one that
 * is refused are explained on the iPod's Summary page, whose Sync button asks and checks; this takes you there.
 */
async function syncDevice(device: Device) {
  const ready = await Promise.all([deviceSyncSettings(device.id), deviceSyncPreview(device.id)]).then(
    ([settings, preview]) => settings.confirmed && preview.fits,
    () => false,
  );
  if (ready) await deviceSync(device.id).catch(showError);
  else useApp.getState().setSource(deviceSource(device.id));
}

export async function showDeviceMenu(device: Device) {
  const { deviceSync: running } = useApp.getState();
  // A copy takes a sync's place, so neither starts while the other runs; the row swaps its eject button for the sync arrows.
  const busy = running !== null;
  const syncing = running?.deviceId === device.id;
  await popUp(
    await Menu.new({
      items: [
        { text: `Sync “${device.name}”`, enabled: !busy, action: () => void syncDevice(device) },
        { text: "Copy Music to Library…", enabled: !busy, action: () => useApp.setState({ copyFromDevice: device.id }) },
        SEPARATOR,
        { text: `Eject “${device.name}”`, enabled: !syncing, action: () => void deviceEject(device.id).catch(showError) },
      ],
    }),
  );
}

export async function showPlayerMenu(player: Player) {
  const syncing = useApp.getState().deviceSync?.deviceId === player.id;
  await popUp(await Menu.new({ items: [{ text: "Eject", enabled: !syncing, action: () => void playerEject(player.id).catch(showError) }] }));
}

export async function showCdMenu(cd: AudioCd) {
  await popUp(
    await Menu.new({
      items: [
        cd.importing
          ? { text: "Stop Importing", action: () => void cdStopImport() }
          : { text: "Import CD", enabled: useApp.getState().cdRip === null, action: () => importCd(cd.id) },
        SEPARATOR,
        { text: "Eject Disc", action: () => void cdEject(cd.id).catch(showError) },
      ],
    }),
  );
}

export async function showSharedMenu(library: SharedLibrary) {
  await popUp(await Menu.new({ items: [{ text: "Disconnect", enabled: library.hasLogin, action: () => void remoteForget(library.id) }] }));
}

export async function showPodcastsMenu() {
  await popUp(await Menu.new({ items: [{ text: "Update All Podcasts", action: () => void podcastsRefresh(null).catch(showError) }] }));
}
