import type { AlbumSummary, Track } from "../api/types";

/**
 * Tracks and album summaries meet on album artist + album, as the library groups them.
 * Compilations are grouped by title alone, since each track keeps its own artist.
 */
const albumKeyOf = (albumArtist: string, album: string, compilation: boolean) =>
  `${compilation ? "" : albumArtist.toLowerCase()}\u0000${album.toLowerCase()}`;

export const trackAlbumKey = (t: Track) => albumKeyOf(t.albumArtist || t.artist, t.album, t.compilation);
export const summaryAlbumKey = (a: AlbumSummary) => albumKeyOf(a.albumArtist, a.album, a.compilation);

/**
 * The songs of `album` that the view lists. Not the library's whole album: a playlist or a search narrows what the
 * album tiles stand for, and a menu that deletes or plays "the album" must not reach the songs the view leaves out.
 * Matched on the library's own album group rather than the tags as written, which it folds (accents, compilations).
 */
export const shownTracksOf = (tracks: Track[], album: AlbumSummary) =>
  tracks.filter((t) => t.albumGroup === album.key && t.kind !== "radio");
