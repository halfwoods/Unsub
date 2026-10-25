/** Development-only entry point. No real library, player, files or native commands are used. */
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { Track, TrackQuery, AlbumSummary, ArtistSummary, GenreSummary, ComposerSummary } from "../../api/types";

const fold = (value: string) => value.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase();
const artistKey = (track: Track) => track.compilation ? "compilations" : fold(track.albumArtist || track.artist);
const tracks: Track[] = Array.from({ length: 12000 }, (_, index) => {
  const artist = index < 24 ? "Ann" : index < 48 ? "Annabelle" : index < 72 ? "Beyoncé" : `Artist ${String(Math.floor(index / 48)).padStart(3, "0")}`;
  const album = `Album ${String(Math.floor(index / 12)).padStart(4, "0")}`;
  const compilation = index >= 72 && index < 96;
  return { id: index + 1, path: `/fixture/${index}.flac`, title: index === 0 ? "A very long song title to check column clipping and autosizing" : `Song ${String(index + 1).padStart(5, "0")}`,
    artist, albumArtist: index % 3 === 0 ? "" : artist, album, albumGroup: `${compilation ? "compilations" : fold(artist)}\u001f${fold(album)}`,
    genre: index < 48 ? "Jazz" : index % 2 === 0 ? "Rock" : "Pop", composer: index % 3 === 0 ? "Ada" : "",
    year: index % 13 === 0 ? null : 1970 + Math.floor(index / 12) % 50, trackNumber: index % 12 + 1, trackCount: 12, discNumber: 1, discCount: 1,
    durationMs: 180000, bitrateKbps: index % 5 === 0 ? null : 900, sampleRate: 44100, bitDepth: 16, channels: 2, codec: "FLAC", fileSize: 22000000,
    dateAdded: 1700000000 + index * 60, dateModified: 1700000000, playCount: index % 10, lastPlayed: index % 3 === 0 ? null : 1701000000,
    skipCount: index % 4, rating: (index % 6) * 20, kind: "music", enabled: true, inCloud: false, missing: false,
    sortTitle: null, sortArtist: null, sortAlbum: null, sortAlbumArtist: null, compilation, rememberPosition: false };
});
const playlistIds = [1, 2, 1, 13, 25, 49, 73, 74];

function selected(query: TrackQuery): Track[] {
  let rows = query.playlist != null ? playlistIds.map((id) => tracks[id - 1]) : [...tracks];
  rows = rows.filter((t) => (!query.kind || t.kind === query.kind) && (!query.ids || query.ids.includes(t.id))
    && (query.artistKey == null || artistKey(t) === query.artistKey) && (query.albumKey == null || t.albumGroup === query.albumKey)
    && (query.genre == null || fold(t.genre) === fold(query.genre)) && (query.composer == null || fold(t.composer) === fold(query.composer))
    && (!query.search || fold(query.search).split(/\s+/).every((word) => fold(`${t.title} ${t.artist} ${t.albumArtist} ${t.album} ${t.genre} ${t.composer}`).includes(word))));
  if (query.sort === "playlistOrder") return query.descending ? rows.reverse() : rows;
  const property: Partial<Record<TrackQuery["sort"], keyof Track>> = { name: "title", time: "durationMs", plays: "playCount", bitrate: "bitrateKbps", size: "fileSize", kind: "codec" };
  const key = property[query.sort] ?? query.sort as keyof Track;
  return rows.sort((a, b) => {
    const av = a[key], bv = b[key];
    if (av == null) return bv == null ? a.id - b.id : 1;
    if (bv == null) return -1;
    const compared = typeof av === "number" && typeof bv === "number" ? av - bv : String(av).localeCompare(String(bv));
    return compared * (query.descending ? -1 : 1) || a.id - b.id;
  });
}
function grouped<T>(rows: Track[], key: (t: Track) => string, make: (key: string, list: Track[]) => T): T[] {
  const groups = new Map<string, Track[]>();
  for (const row of new Map(rows.map((t) => [t.id, t])).values()) { const k = key(row); groups.set(k, [...(groups.get(k) ?? []), row]); }
  return Array.from(groups, ([k, list]) => make(k, list));
}
function albums(rows: Track[]): AlbumSummary[] {
  return grouped(rows, (t) => t.albumGroup, (key, list) => ({ key, album: list[0].album, albumArtist: list[0].compilation ? "Compilations" : list[0].albumArtist || list[0].artist,
    year: list.find((t) => t.year != null)?.year ?? null, genre: list[0].genre, trackCount: list.length, durationMs: list.length * 180000, artworkTrackId: list[0].id,
    dateAdded: list[0].dateAdded, compilation: list[0].compilation }));
}
const stopped = { state: "stopped", trackId: null, positionMs: 0, durationMs: 0, sampleRate: null, outputSampleRate: null, volume: 0.75, bitPerfect: false, outputFormat: null, live: false, buffering: false };
const calls: { command: string; payload: unknown }[] = [];
mockIPC((command, raw) => {
  const payload = (raw ?? {}) as Record<string, unknown>;
  const query = (payload.query ?? { kind: "music", search: null, sort: "artist", descending: false }) as TrackQuery;
  const rows = () => selected(query);
  switch (command) {
    case "library_tracks": return rows();
    case "library_stats": return { count: rows().length, totalDurationMs: rows().length * 180000, totalBytes: rows().length * 22000000 };
    case "library_albums": return albums(rows());
    case "library_artist_albums": return albums(selected({ ...query, artistKey: String(payload.key) })).sort((a, b) => (a.year ?? Infinity) - (b.year ?? Infinity) || a.album.localeCompare(b.album));
    case "library_album_tracks": return selected({ ...query, albumKey: String(payload.key), sort: "trackNumber" });
    case "library_artists": return grouped<ArtistSummary>(rows(), artistKey, (key, list) => ({ key, name: key === "compilations" ? "Compilations" : list[0].albumArtist || list[0].artist,
      albumCount: new Set(list.map((t) => t.albumGroup)).size, trackCount: list.length, artworkTrackIds: albums(list).slice(0, 4).map((a) => a.artworkTrackId) }));
    case "library_genres": return grouped<GenreSummary>(rows(), (t) => fold(t.genre), (key, list) => ({ key, name: list[0].genre, albumCount: new Set(list.map((t) => t.albumGroup)).size, trackCount: list.length }));
    case "library_composers": return grouped<ComposerSummary>(rows().filter((t) => t.composer), (t) => t.composer, (name, list) => ({ name, trackCount: list.length }));
    case "library_playlists": return [{ id: 1, name: "Fixture Favorites", kind: "regular", parentId: null, position: 0, trackCount: 8 }, { id: 2, name: "Second Playlist", kind: "regular", parentId: null, position: 1, trackCount: 8 }];
    case "player_status": return stopped;
    case "devices_list": case "players_list": case "remote_libraries": case "cd_list": return [];
    case "welcome_due": return false;
    case "updates_status": return { configured: false, autoCheck: false, update: null };
    default:
      if (command.startsWith("player_") || command === "library_move_in_playlist") calls.push({ command, payload });
      return null;
  }
}, { shouldMockEvents: true });
mockWindows("main");
// Stand in for the local artwork protocol with generated, offline cover fixtures.
const internals = (window as unknown as { __TAURI_INTERNALS__: { convertFileSrc: (path: string, protocol?: string) => string } }).__TAURI_INTERNALS__;
internals.convertFileSrc = (path) => {
  const id = Number(path);
  if (id >= 49 && id <= 72) return "data:image/png;base64,broken";
  return `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256"><rect width="256" height="256" fill="hsl(${id * 37 % 360},35%,36%)"/><circle cx="128" cy="110" r="70" fill="none" stroke="#ffffff88" stroke-width="3"/><text x="128" y="220" fill="white" text-anchor="middle" font-size="20">ALBUM ${id}</text></svg>`)}#`;
};
(window as unknown as { fixtureCalls: typeof calls }).fixtureCalls = calls;
async function start() {
  const { default: App } = await import("../../App");
  createRoot(document.getElementById("root")!).render(<App />);
}
void start();
