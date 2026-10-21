import { Menu, type MenuOptions } from "@tauri-apps/api/menu";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { cdStopImport, libraryAddToPlaylist, libraryEditTracks, librarySetEnabled, partyAdd, type MenuAction } from "../api/commands";
import type { Playlist, Track } from "../api/types";
import { importCd } from "../cd/Cd";
import { copySongsToLibrary } from "../devices/CopyToLibrary";
import { startMixFrom } from "../mixes/startMix";
import { playlistsByParent } from "../sidebar/PlaylistRows";
import { useApp } from "../state/store";
import { IS_MAC, IS_WINDOWS } from "../util/platform";

export type MenuItems = NonNullable<MenuOptions["items"]>;

export const SEPARATOR = { item: "Separator" } as const;
/** menu.rs words the bar's item the same way for each platform. */
const REVEAL_LABEL = IS_MAC ? "Show in Finder" : IS_WINDOWS ? "Show in Explorer" : "Show in File Manager";
const RATING_LABELS = ["None", "★", "★★", "★★★", "★★★★", "★★★★★"];

/** Kept until the next right-click: closing a menu as it is dismissed could drop the event of the item just chosen. */
let lastMenu: Menu | null = null;

/** An item with a menu bar action's id, which menu.rs forwards to the same handler as the bar's own item. */
const barItem = (id: MenuAction, text: string) => ({ id, text });

/** Regular playlists nested in their folders as the source list nests them; folders holding none are left out. */
function playlistItems(byParent: Map<number | null, Playlist[]>, parent: number | null, add: (id: number) => void): MenuItems {
  return (byParent.get(parent) ?? []).flatMap<MenuItems[number]>((playlist) => {
    if (playlist.kind === "regular") return [{ text: playlist.name, action: () => add(playlist.id) }];
    const items = playlist.kind === "folder" ? playlistItems(byParent, playlist.id, add) : [];
    return items.length > 0 ? [{ text: playlist.name, items }] : [];
  });
}

export async function popUp(menu: Menu) {
  void lastMenu?.close();
  lastMenu = menu;
  await menu.popup();
}

/** Pops up a menu of `items` at the cursor. */
export async function showMenu(items: MenuItems) {
  await popUp(await Menu.new({ items }));
}

/** Shows a song's file in the system's file manager, or says why it can't. */
export function revealTrack(track: Track) {
  if (track.missing) return useApp.getState().showNotice("The file for this song can't be found");
  void revealItemInDir(track.path);
}

/** "Add to Playlist" with the playlists as the source list nests them; nothing when there are none to add to. */
function addToPlaylistItems(ids: number[]): MenuItems {
  const { playlists, refresh, refreshPlaylists } = useApp.getState();
  const items = playlistItems(playlistsByParent(playlists), null, (id) => {
    void libraryAddToPlaylist(id, ids).then(() => Promise.all([refresh(), refreshPlaylists()]));
  });
  return items.length > 0 ? [{ text: "Add to Playlist", items }] : [];
}

/** At 0 the songs come right after the current one, as "Play Next" does in iTunes' Party Shuffle. */
const partyItems = (ids: number[]): MenuItems => {
  const { showNotice } = useApp.getState();
  const add = (at: number | null) => void partyAdd(ids, at).catch((e: unknown) => showNotice(String(e)));
  return [
    { text: "Play Next in Party Shuffle", action: () => add(0) },
    { text: "Add to Party Shuffle", action: () => add(null) },
  ];
};

/** The right-click menu for a station of the Radio view; `keep` adds it to My Stations or removes it. */
export async function showStationMenu(play: () => void, keep: { label: string; run: () => void }) {
  await popUp(await Menu.new({ items: [{ text: "Play", action: play }, SEPARATOR, { text: keep.label, action: keep.run }] }));
}

/** The right-click menu for songs on an iPod, which can't be edited but can be copied into the library. */
export async function showDeviceSongMenu(deviceId: string) {
  const ids = [...useApp.getState().selection];
  await popUp(await Menu.new({ items: [{ text: "Copy to Library", action: () => copySongsToLibrary(deviceId, ids) }] }));
}

/** The right-click menu for the selected songs of the library list; `clicked` is the row under the cursor. */
export async function showSongMenu(clicked: Track, inRegularPlaylist: boolean) {
  const { tracks, selection, refresh, deleteSelection } = useApp.getState();
  const ids = [...selection];
  const selected = tracks.filter((t) => selection.has(t.id));

  const menu = await Menu.new({
    items: [
      barItem("get-info", "Get Info"),
      barItem("clean-up", "Clean Up Song Info…"),
      {
        text: "Rating",
        items: RATING_LABELS.map((text, stars) => ({
          text,
          // Stars are rounded the way the rating column draws them.
          checked: selected.every((t) => Math.round(t.rating / 20) === stars),
          action: () => void libraryEditTracks(ids, { rating: stars * 20 }, null),
        })),
      },
      {
        text: clicked.enabled ? "Uncheck Selection" : "Check Selection",
        action: () => void librarySetEnabled(ids, !clicked.enabled).then(refresh),
      },
      barItem("show-in-finder", REVEAL_LABEL),
      SEPARATOR,
      ...addToPlaylistItems(ids),
      barItem("new-playlist-from-selection", "New Playlist from Selection"),
      SEPARATOR,
      ...partyItems(ids),
      { text: "Start a Mix from This Song", action: () => void startMixFrom(clicked, { followsPlayback: false }) },
      SEPARATOR,
      ...(inRegularPlaylist ? [{ text: "Remove from Playlist", action: () => void deleteSelection() }] : []),
      { text: "Delete from Library…", action: () => void deleteSelection(true) },
    ],
  });
  await popUp(menu);
}

/** The right-click menu for albums, given their songs: one album's in the grid and Cover Flow, or every selected album's in the Album List. */
export async function showAlbumMenu(tracks: Track[]) {
  if (tracks.length === 0) return;
  const { playTrackIds, openLibrarySheet, showNotice } = useApp.getState();
  const ids = tracks.map((t) => t.id);
  const play = (shuffled: boolean) => void playTrackIds(ids, shuffled).catch((e: unknown) => showNotice(String(e)));
  await showMenu([
    { text: "Play", action: () => play(false) },
    { text: "Shuffle", action: () => play(true) },
    SEPARATOR,
    { text: "Get Info", action: () => openLibrarySheet({ type: "info", trackIds: ids }) },
    ...addToPlaylistItems(ids),
    ...partyItems(ids),
    { text: REVEAL_LABEL, action: () => revealTrack(tracks[0]) },
    SEPARATOR,
    { text: "Delete from Library…", action: () => openLibrarySheet({ type: "delete", trackIds: ids }) },
  ]);
}

/** The right-click menu for the songs of a CD, which can be played and imported but not edited. */
export async function showCdSongMenu(discId: string, play: () => void) {
  const { cds, cdRip } = useApp.getState();
  const importing = cds.find((c) => c.id === discId)?.importing ?? false;
  // Only one CD imports at a time, so Import CD waits for another's import to end.
  const importItem = importing
    ? { text: "Stop Importing", action: () => void cdStopImport() }
    : { text: "Import CD", enabled: cdRip === null, action: () => importCd(discId) };
  await showMenu([{ text: "Play", action: play }, SEPARATOR, importItem]);
}
