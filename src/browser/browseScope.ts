import type { TrackQuery } from "../api/types";

export interface BrowseStep {
  kind: "artist" | "album" | "genre" | "composer";
  key: string;
  label: string;
}

export interface BrowserFilters {
  genre: string | null;
  artistKey: string | null;
  albumKey: string | null;
}

export const EMPTY_BROWSER: BrowserFilters = { genre: null, artistKey: null, albumKey: null };

/** Retains search, media kind, playlist membership and duplicate-finder IDs. */
export function withoutBrowse(query: TrackQuery): TrackQuery {
  return { ...query, genre: null, artistKey: null, albumKey: null, composer: null };
}

export function browseQuery(query: TrackQuery, filters: BrowserFilters, path: readonly BrowseStep[]): TrackQuery {
  const next = { ...withoutBrowse(query), ...filters };
  for (const step of path) {
    if (step.kind === "artist") next.artistKey = step.key;
    else if (step.kind === "album") next.albumKey = step.key;
    else if (step.kind === "genre") next.genre = step.key;
    else next.composer = step.key;
  }
  return next;
}

/** Upstream choices narrow downstream lists; a facet must never filter itself. */
export function browserQueries(query: TrackQuery) {
  const base = { ...query, sort: "artist" as const, descending: false, albumKey: null };
  return {
    genres: { ...base, genre: null, artistKey: null },
    artists: { ...base, artistKey: null },
    albums: { ...base, sort: "album" as const },
  };
}

export function narrowBrowser(filters: BrowserFilters, field: keyof BrowserFilters, value: string | null): BrowserFilters {
  if (field === "genre") return { genre: value, artistKey: null, albumKey: null };
  if (field === "artistKey") return { ...filters, artistKey: value, albumKey: null };
  return { ...filters, albumKey: value };
}

export const hasBrowseFilters = (query: TrackQuery) =>
  query.artistKey != null || query.albumKey != null || query.genre != null || query.composer != null;

/** Stored positions must be retrieved from the whole playlist, not a filtered projection. */
export const playlistPositionQuery = (query: TrackQuery): TrackQuery => ({
  ...withoutBrowse(query), kind: null, search: null, ids: null, sort: "playlistOrder", descending: false,
});
