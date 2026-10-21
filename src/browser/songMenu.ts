import { Menu, type MenuOptions } from "@tauri-apps/api/menu";
import { libraryAddToPlaylist, libraryEditTracks, librarySetEnabled, type MenuAction } from "../api/commands";
import type { Playlist, Track } from "../api/types";
import { copySongsToLibrary } from "../devices/CopyToLibrary";
import { playlistsByParent } from "../sidebar/PlaylistRows";
import { useApp } from "../state/store";

type MenuItems = NonNullable<MenuOptions["items"]>;

const SEPARATOR = { item: "Separator" } as const;
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

async function popUp(menu: Menu) {
  void lastMenu?.close();
  lastMenu = menu;
  await menu.popup();
}

/** The right-click menu for songs on an iPod, which can't be edited but can be copied into the library. */
export async function showDeviceSongMenu(deviceId: string) {
  const ids = [...useApp.getState().selection];
  await popUp(await Menu.new({ items: [{ text: "Copy to Library", action: () => copySongsToLibrary(deviceId, ids) }] }));
}

/** The right-click menu for the selected songs of the library list; `clicked` is the row under the cursor. */
export async function showSongMenu(clicked: Track, inRegularPlaylist: boolean) {
  const { tracks, selection, playlists, refresh, refreshPlaylists, deleteSelection } = useApp.getState();
  const ids = [...selection];
  const selected = tracks.filter((t) => selection.has(t.id));
  const addToPlaylist = playlistItems(playlistsByParent(playlists), null, (id) => {
    void libraryAddToPlaylist(id, ids).then(() => Promise.all([refresh(), refreshPlaylists()]));
  });

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
      barItem("show-in-finder", "Show in Finder"),
      SEPARATOR,
      ...(addToPlaylist.length > 0 ? [{ text: "Add to Playlist", items: addToPlaylist }] : []),
      barItem("new-playlist-from-selection", "New Playlist from Selection"),
      SEPARATOR,
      ...(inRegularPlaylist ? [{ text: "Remove from Playlist", action: () => void deleteSelection() }] : []),
      { text: "Delete from Library…", action: () => void deleteSelection(true) },
    ],
  });
  await popUp(menu);
}
