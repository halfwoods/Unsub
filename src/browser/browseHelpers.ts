import type { TrackQuery } from "../api/types";

/** Display names never substitute for the backend's exact grouping keys. */
export function browseScrollKey(source: string, group: string, query: TrackQuery): string {
  return JSON.stringify([source, group, query.kind, query.search, query.playlist ?? null, query.ids ?? null,
    query.artistKey ?? null, query.albumKey ?? null, query.genre ?? null, query.composer ?? null]);
}
export function artistInitials(name: string): string {
  const words = name.trim().split(/\s+/u).filter(Boolean);
  return words.length ? (words[0].slice(0, 1) + (words.length > 1 ? words[words.length - 1].slice(0, 1) : "")).toUpperCase() : "?";
}
export function nextGridIndex(index: number, key: string, columns: number, count: number): number {
  if (count === 0) return 0;
  const next = key === "Home" ? 0 : key === "End" ? count - 1 :
    index + ({ ArrowLeft: -1, ArrowRight: 1, ArrowUp: -columns, ArrowDown: columns }[key] ?? 0);
  return Math.max(0, Math.min(count - 1, next));
}
