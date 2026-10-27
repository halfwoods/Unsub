import type { AlbumSummary, Track } from "../api/types";

/** Use the library's normalized identity, including accents and compilation rules. */
export const trackAlbumKey = (t: Track) => t.albumGroup;
export const summaryAlbumKey = (a: AlbumSummary) => a.key;

/** Keep all occurrences of an album together without losing duplicate playlist entries.
 * Album order follows its first occurrence in the requested sort; rows retain their relative order.
 */
export function orderAlbumTracks(tracks: Track[]): Track[] {
  const groups = new Map<string, Track[]>();
  for (const track of tracks) {
    const key = trackAlbumKey(track);
    const group = groups.get(key);
    if (group) group.push(track); else groups.set(key, [track]);
  }
  return Array.from(groups.values()).flat();
}

export interface AlbumBlock { key: string; start: number; tracks: Track[] }
export function albumBlocks(tracks: Track[]): AlbumBlock[] {
  const groups: AlbumBlock[] = [];
  tracks.forEach((track, index) => {
    const key = trackAlbumKey(track), last = groups[groups.length - 1];
    if (last?.key === key) last.tracks.push(track);
    else groups.push({ key, start: index, tracks: [track] });
  });
  return groups;
}

/**
 * The songs of `album` that the view lists. Not the library's whole album: a playlist or a search narrows what the
 * album tiles stand for, and a menu that deletes or plays "the album" must not reach the songs the view leaves out.
 * Matched on the library's own album group rather than the tags as written, which it folds (accents, compilations).
 */
export const shownTracksOf = (tracks: Track[], album: AlbumSummary) =>
  tracks.filter((t) => t.albumGroup === album.key && t.kind !== "radio");

/** Bound the DOM even when a compilation contains thousands of songs. */
export function albumRowWindow(rows: number, groupTop: number, scrollTop: number, viewportHeight: number, rowHeight: number) {
  const start = Math.max(0, Math.min(rows, Math.floor((scrollTop - groupTop) / rowHeight) - 5));
  const end = Math.max(start, Math.min(rows, Math.ceil((scrollTop + viewportHeight - groupTop) / rowHeight) + 5));
  return { start, end };
}

/** A playlist can contain the same track more than once; preserve the active occurrence. */
export function remapTrackAnchor(previous: Track[], next: Track[], anchor: number | null): number | null {
  if (anchor === null || !previous[anchor]) return null;
  const id = previous[anchor].id;
  let occurrence = 0;
  for (let index = 0; index < anchor; index++) if (previous[index].id === id) occurrence++;
  for (let index = 0; index < next.length; index++) {
    if (next[index].id === id && occurrence-- === 0) return index;
  }
  return null;
}
