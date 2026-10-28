import type { ReactNode } from "react";
import type { SortKey, Track } from "../api/types";
import { COLUMN_SPECS } from "../preferences/libraryViews";
import { formatAudioKind, formatBitRate, formatBytes, formatClock, formatMetadataDate, formatMetadataNumber, formatSampleRate } from "../util/format";
import { Rating } from "./Rating";
import { SongCheckbox } from "./SongCheckbox";

export interface Column {
  id: string;
  title: string;
  width: number;
  align?: "left" | "right" | "center";
  sort?: SortKey;
  render: (track: Track, selected: boolean, index: number) => ReactNode;
}

const numbered = (number: number | null, total: number | null) => number ? total ? `${number} of ${total}` : String(number) : "";
/** Plain cell text also powers bounded autosizing and the full-value tooltip. */
export function columnText(id: string, t: Track): string {
  switch (id) {
    case "name": return t.title;
    case "time": return t.durationMs > 0 ? formatClock(t.durationMs) : "";
    case "artist": return t.artist;
    case "album": return t.album;
    case "genre": return t.genre;
    case "albumArtist": return t.albumArtist;
    case "composer": return t.composer;
    case "year": return t.year ? String(t.year) : "";
    case "plays": return t.playCount > 0 ? formatMetadataNumber(t.playCount) : "";
    case "skips": return formatMetadataNumber(t.skipCount);
    case "trackNumber": return numbered(t.trackNumber, t.trackCount);
    case "discNumber": return numbered(t.discNumber, t.discCount);
    case "dateAdded": return formatMetadataDate(t.dateAdded);
    case "lastPlayed": return formatMetadataDate(t.lastPlayed);
    case "kind": return formatAudioKind(t.codec);
    case "bitrate": return formatBitRate(t.bitrateKbps);
    case "sampleRate": return formatSampleRate(t.sampleRate);
    case "size": return t.fileSize > 0 ? formatBytes(t.fileSize) : "";
    default: return "";
  }
}
const NUMERIC = new Set(["time", "plays", "year", "skips", "trackNumber", "discNumber", "bitrate", "sampleRate", "size"]);
export const ALL_COLUMNS: Column[] = COLUMN_SPECS.map((spec) => ({
  ...spec,
  title: spec.id === "enabled" ? "" : spec.title,
  align: spec.id === "enabled" ? "center" : NUMERIC.has(spec.id) ? "right" : "left",
  render: (t, selected) => spec.id === "enabled" ? <SongCheckbox track={t} selected={selected} /> : spec.id === "rating" ? <Rating trackId={t.id} rating={t.rating} selected={selected} /> : columnText(spec.id, t),
}));
export const COLUMNS = ALL_COLUMNS.slice(0, 8);
export const STATUS_COLUMN_WIDTH = 20;
export const POSITION_COLUMN: Column = { id: "position", title: "#", width: 34, align: "right", sort: "playlistOrder", render: (_track, _selected, index) => index + 1 };
export const CD_COLUMNS: Column[] = [
  { ...POSITION_COLUMN, sort: undefined, render: (track) => track.trackNumber ?? "" },
  ...COLUMNS.filter((c) => c.id !== "rating" && c.id !== "plays").map((c) => ({ ...c, sort: undefined })),
];
export const SHARED_COLUMNS = COLUMNS.filter((c) => c.id !== "enabled" && c.id !== "rating");
export const gridTemplate = (columns: Column[]) => [
  `${STATUS_COLUMN_WIDTH}px`, ...columns.map((c) => `${c.width}px`),
].join(" ");
