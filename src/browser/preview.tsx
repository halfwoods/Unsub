/** Development-only, isolated UI fixture. pnpm dev → /src/browser/preview.html?rows=100000&width=820
 * Uses the actual App and components. No native database, music files, network, or audio is touched.
 */
import React from "react";
import ReactDOM from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import type { AlbumSummary, ArtistSummary, Playlist, Track, TrackQuery } from "../api/types";

const params = new URLSearchParams(location.search);
const count = Math.min(100000, Math.max(20, Number(params.get("rows")) || 120));
const width = Math.max(640, Math.min(1600, Number(params.get("width")) || 1100));
const fold = (text: string) => text.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase();
type FixtureTrack = Track & { artistGroup: string };
const artists = ["Ann", "Anna", "Beyoncé", "The North", "An exceptionally long artist name that must remain readable in a narrow window", "No Artwork"];
const rows: FixtureTrack[] = Array.from({ length: count }, (_, i) => {
  const artist = artists[Math.floor(i / 20) % artists.length];
  const compilation = i % 37 === 0;
  const artistGroup = compilation ? "compilations" : fold(artist);
  const album = ["First Light", "A Long Way Home", "Late Recordings"][Math.floor(i / 5) % 3];
  return { id: i + 1, path: `/isolated-fixture/${i + 1}.flac`, title: i % 5 === 0 ? `Live song ${i + 1}` : `Song ${i + 1}`,
    artist, albumArtist: i % 2 ? artist : "", artistGroup, album, albumGroup: `${artistGroup}\x1f${fold(album)}`, genre: i % 4 ? "Rock" : "Jazz", composer: i % 3 ? "" : "Composer",
    year: i % 9 === 0 ? null : 1980 + i % 40, trackNumber: i % 9 === 0 ? null : i % 12 + 1, trackCount: 12, discNumber: i % 2 + 1, discCount: 2,
    durationMs: 195000, bitrateKbps: i % 3 ? 320 : null, sampleRate: i % 3 ? 44100 : 96000, bitDepth: 16, channels: 2, codec: i % 3 ? "mp3" : "flac",
    fileSize: 15000000 + i, dateAdded: 1700000000, dateModified: 1700000000, playCount: i % 13, lastPlayed: i % 4 ? 1710000000 : null, skipCount: i % 4,
    rating: (i % 6) * 20, kind: "music", enabled: true, inCloud: false, missing: false, sortTitle: null, sortArtist: null, sortAlbum: null, sortAlbumArtist: null, compilation, rememberPosition: false };
});
const playlists: Playlist[] = [7, 8].map((id) => ({ id, name: id === 7 ? "Evening listening" : "Library review", kind: "regular", parentId: null, position: id - 7, smart: null, trackCount: id === 7 ? 8 : 5 }));
const playlistRows: Record<number, number[]> = { 7: [1, 2, 1, 3, 22, 42, 61, 111], 8: [2, 3, 4, 5, 6] };
const collator = new Intl.Collator(undefined, { numeric: true });
const compare = (a: unknown, b: unknown) => collator.compare(String(a ?? ""), String(b ?? ""));
function tracks(query: TrackQuery): FixtureTrack[] {
  const source = query.playlist ? (playlistRows[query.playlist] ?? []).map((id) => rows[id - 1]) : rows;
  const result = source.filter((t) => t && (!query.kind || t.kind === query.kind) && (!query.ids || query.ids.includes(t.id)) &&
    (query.artistGroup == null || t.artistGroup === query.artistGroup) && (query.albumGroup == null || t.albumGroup === query.albumGroup) &&
    (query.genre == null || fold(t.genre) === fold(query.genre)) && (query.composer == null || fold(t.composer) === fold(query.composer)) &&
    fold(query.search ?? "").split(/\s+/).every((word) => fold(`${t.title} ${t.artist} ${t.albumArtist} ${t.album} ${t.genre} ${t.composer}`).includes(word)));
  if (query.sort === "playlistOrder") return query.descending ? result.reverse() : result;
  const fields: Record<string, keyof Track> = { name: "title", time: "durationMs", plays: "playCount", skips: "skipCount", bitrate: "bitrateKbps", size: "fileSize", kind: "codec" };
  const field = fields[query.sort] ?? query.sort as keyof Track;
  return result.sort((a, b) => {
    if (a[field] == null || a[field] === "") return b[field] == null || b[field] === "" ? 0 : 1;
    if (b[field] == null || b[field] === "") return -1;
    return compare(a[field], b[field]) * (query.descending ? -1 : 1) || compare(a.album, b.album) || a.id - b.id;
  });
}
function albums(query: TrackQuery): AlbumSummary[] {
  const groups = new Map<string, FixtureTrack[]>();
  for (const t of new Map(tracks(query).map((t) => [t.id, t])).values()) {
    const group = groups.get(t.albumGroup);
    if (group) group.push(t); else groups.set(t.albumGroup, [t]);
  }
  return [...groups].map(([key, group]) => ({ key, album: group[0].album, albumArtist: group[0].compilation ? "Compilations" : group[0].albumArtist || group[0].artist,
    year: Math.min(...group.map((t) => t.year ?? 9999)) === 9999 ? null : Math.min(...group.map((t) => t.year ?? 9999)), genre: group[0].genre,
    trackCount: group.length, durationMs: group.length * 195000, artworkTrackId: group[0].id, dateAdded: 1700000000, compilation: group[0].compilation }))
    .sort((a, b) => query.sort === "year" ? (a.year ?? 9999) - (b.year ?? 9999) || compare(a.album, b.album) : compare(a.album, b.album));
}
function artistSummaries(query: TrackQuery): ArtistSummary[] {
  const groups = new Map<string, FixtureTrack[]>();
  for (const t of new Map(tracks(query).map((t) => [t.id, t])).values()) {
    const group = groups.get(t.artistGroup);
    if (group) group.push(t); else groups.set(t.artistGroup, [t]);
  }
  return [...groups].map(([key, group]) => ({ key, name: key === "compilations" ? "Compilations" : group[0].albumArtist || group[0].artist,
    albumCount: new Set(group.map((t) => t.albumGroup)).size, trackCount: group.length,
    artworkTrackIds: [...new Map(group.map((t) => [t.albumGroup, t.id])).values()].slice(0, 4) })).sort((a, b) => compare(a.name, b.name));
}
function genres(query: TrackQuery) {
  return [...new Set(tracks(query).map((t) => t.genre))].map((name) => ({ name, albumCount: albums({ ...query, genre: name }).length, trackCount: tracks({ ...query, genre: name }).length }));
}
const menus = new Map<number, { items?: { text?: string; item?: string; checked?: boolean; enabled?: boolean; id?: string; handler?: Channel<string> }[] }>();
let nextMenu = 1;
let playbackQueue: number[] = [];
let menuPosition = { x: 200, y: 120 };
document.addEventListener("contextmenu", (e) => { menuPosition = { x: e.clientX, y: e.clientY }; }, true);
mockWindows("main");
mockIPC(async (command, payload) => {
  const args = payload as Record<string, unknown>;
  const query = args?.query as TrackQuery;
  if (command === "library_tracks" || command === "remote_tracks" || command === "device_tracks") return tracks(query);
  if (command === "library_stats" || command === "device_stats") { const list = tracks(query); return { count: list.length, totalDurationMs: list.length * 195000, totalBytes: list.reduce((sum, t) => sum + t.fileSize, 0) }; }
  if (command === "library_albums") return albums(query);
  if (command === "library_artists") return artistSummaries(query);
  if (command === "library_artist_albums") return albums({ ...query, artistGroup: String(args.key), sort: "year", descending: false });
  if (command === "library_genres") return genres(query);
  if (command === "library_composers") return [{ name: "Composer", trackCount: tracks({ ...query, composer: "Composer" }).length }];
  if (command === "library_browser_facets") return {
    genres: genres({ ...query, genre: null, artistGroup: null, albumGroup: null }),
    artists: artistSummaries({ ...query, artistGroup: null, albumGroup: null }),
    albums: albums({ ...query, albumGroup: null, sort: "album" }),
  };
  if (command === "library_playlists") return playlists;
  if (command === "library_track") return rows[Number(args.trackId) - 1];
  if (command === "player_play_list") { playbackQueue = args.trackIds as number[]; return; }
  if (command === "player_status") return { state: "stopped", trackId: null, positionMs: 0, durationMs: 0, volume: .75, sampleRate: null, outputSampleRate: null, bitPerfect: false, outputFormat: null, live: false, buffering: false };
  if (command === "updates_status") return { update: null, autoCheck: false, configured: false };
  if (command === "welcome_due" || command === "plugin:window|is_fullscreen") return false;
  if (command === "plugin:menu|new") { const id = nextMenu++; menus.set(id, args.options as never); return [id, `fixture-menu-${id}`]; }
  if (command === "plugin:menu|popup") {
    document.getElementById("fixture-menu")?.remove();
    const menu = document.createElement("div"); menu.id = "fixture-menu"; menu.setAttribute("role", "menu");
    menu.style.cssText = `position:fixed;z-index:200;left:${Math.min(menuPosition.x, innerWidth - 230)}px;top:${Math.min(menuPosition.y, innerHeight - 380)}px;background:#f4f4f4;border:1px solid #999;box-shadow:0 3px 12px #5556;width:220px;max-height:360px;overflow:auto;padding:4px`;
    for (const item of menus.get(Number(args.rid))?.items ?? []) {
      if (item.item) { menu.append(document.createElement("hr")); continue; }
      const button = document.createElement("button"); button.textContent = `${item.checked ? "✓ " : ""}${item.text}`; button.disabled = item.enabled === false; button.setAttribute("role", "menuitem");
      button.style.cssText = "display:block;width:100%;padding:4px;text-align:left";
      button.onclick = () => { item.handler?.onmessage(item.id ?? "fixture"); menu.remove(); };
      menu.append(button);
    }
    document.body.append(menu); menu.querySelector("button")?.focus();
    return;
  }
  if (command === "library_move_in_playlist") {
    const items = playlistRows[Number(args.id)]; const positions = args.positions as number[]; const to = Number(args.to);
    const selected = items.filter((_, index) => positions.includes(index));
    const rest = items.filter((_, index) => !positions.includes(index)); rest.splice(to - positions.filter((p) => p < to).length, 0, ...selected); playlistRows[Number(args.id)] = rest; return;
  }
  if (command === "library_set_rating" || command === "library_set_enabled" || command === "library_edit_tracks" || command === "library_remove_from_playlist") throw new Error("Music/library mutation is disabled in this UI fixture");
  return [];
}, { shouldMockEvents: true });
const internals = (window as unknown as { __TAURI_INTERNALS__: { convertFileSrc: (path: string, protocol?: string) => string } }).__TAURI_INTERNALS__;
internals.convertFileSrc = (path) => rows[Number(path) - 1]?.artist === "No Artwork" ? "/src/browser/test/fixtures/missing.svg" : `/src/browser/test/fixtures/cover-${Number(path) % 4}.svg`;
const { useApp } = await import("../state/store");
const { default: App } = await import("../App");
document.getElementById("root")!.style.cssText = `height:calc(100% - 32px);width:min(100%, ${width}px);margin:auto`;
document.getElementById("fixture-tools")!.style.cssText = "height:32px;background:#eee;color:#333;font:11px sans-serif;display:flex;align-items:center;justify-content:center;gap:12px;";
const tools = document.getElementById("fixture-tools")!;
const button = document.createElement("button"); button.textContent = "Fixture View Options"; button.onclick = () => useApp.setState({ viewOptionsOpen: true }); tools.append(button);
const diagnostics = document.createElement("output"); diagnostics.setAttribute("aria-label", "Fixture diagnostics"); tools.append(diagnostics);
const update = () => {
  const header = document.querySelector(".song-list-header");
  const row = document.querySelector(".song-row");
  const delta = header && row ? Math.round(header.getBoundingClientRect().left - row.getBoundingClientRect().left) : 0;
  const scroller = document.querySelector(".song-list-body") ?? document.querySelector(".album-grid-scroll");
  diagnostics.textContent = `${count.toLocaleString()} fixture songs · ${document.querySelectorAll(".song-row").length} mounted rows · header Δ ${delta}px · scroll ${Math.round(scroller?.scrollLeft ?? 0)},${Math.round(scroller?.scrollTop ?? 0)} · queue ${playbackQueue.length} [${playbackQueue.slice(0, 5).join(",")}]`;
};
ReactDOM.createRoot(document.getElementById("root")!).render(<React.StrictMode><App /></React.StrictMode>);
new MutationObserver(update).observe(document.getElementById("root")!, { childList: true, subtree: true, attributes: true });
document.addEventListener("scroll", update, true); document.addEventListener("pointerup", () => requestAnimationFrame(update));
