import type { AlbumSummary, Track } from "../api/types";

/**
 * Tracks and album summaries meet on album artist + album, as the library groups them.
 * Compilations are grouped by title alone, since each track keeps its own artist.
 */
const albumKeyOf = (albumArtist: string, album: string, compilation: boolean) =>
  `${compilation ? "" : albumArtist.toLowerCase()}\u0000${album.toLowerCase()}`;

export const trackAlbumKey = (t: Track) => albumKeyOf(t.albumArtist || t.artist, t.album, t.compilation);
export const summaryAlbumKey = (a: AlbumSummary) => albumKeyOf(a.albumArtist, a.album, a.compilation);
