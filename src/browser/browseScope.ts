import type { TrackQuery } from "../api/types";

export type BrowseFilters = Pick<TrackQuery, "genre" | "artistGroup" | "albumGroup" | "composer">;
export const CLEAR_BROWSE: BrowseFilters = { genre: null, artistGroup: null, albumGroup: null, composer: null };
export const hasBrowseFilters = (query: TrackQuery) => Object.values(filtersOf(query)).some((value) => value != null);
export const filtersOf = (query: TrackQuery): BrowseFilters => ({ genre: query.genre ?? null, artistGroup: query.artistGroup ?? null, albumGroup: query.albumGroup ?? null, composer: query.composer ?? null });
export const scopeKey = (source: string, mode: string, group: string, query: TrackQuery) => JSON.stringify([source, mode, group, query.search, query.ids, filtersOf(query)]);
const scrollOffsets = new Map<string, number>();
export const readViewScroll = (key: string) => scrollOffsets.get(key) ?? 0;
export function saveViewScroll(key: string, offset: number) {
  // Session navigation history is bounded; layouts persist independently.
  if (scrollOffsets.size > 200) scrollOffsets.delete(scrollOffsets.keys().next().value!);
  scrollOffsets.set(key, offset);
}

/** A changed upstream choice invalidates every downstream choice, never the search or playlist. */
export function selectBrowserFilter(query: TrackQuery, field: "genre" | "artistGroup" | "albumGroup", value: string | null): TrackQuery {
  return { ...query, [field]: value, ...(field === "genre" ? { artistGroup: null, albumGroup: null } : field === "artistGroup" ? { albumGroup: null } : {}) };
}
