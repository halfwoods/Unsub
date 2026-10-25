import type { Track } from "../api/types";

const number = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });
const date = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
export function formatMetadataDate(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds <= 0) return "";
  const value = new Date(seconds * 1000);
  return Number.isFinite(value.getTime()) ? date.format(value) : "";
}
export function formatColumnText(id: string, t: Track): string {
  const positive = (value: number | null) => value !== null && Number.isFinite(value) && value > 0 ? number.format(value) : "";
  switch (id) {
    case "name": return t.title;
    case "artist": return t.artist;
    case "album": return t.album;
    case "albumArtist": return t.albumArtist;
    case "composer": return t.composer;
    case "genre": return t.genre;
    case "year": return t.year && t.year > 0 ? String(t.year) : "";
    case "trackNumber": return positive(t.trackNumber);
    case "discNumber": return positive(t.discNumber);
    case "dateAdded": return formatMetadataDate(t.dateAdded);
    case "lastPlayed": return formatMetadataDate(t.lastPlayed);
    case "plays": return positive(t.playCount);
    case "skipCount": return positive(t.skipCount);
    case "bitrate": return t.bitrateKbps && t.bitrateKbps > 0 ? `${positive(t.bitrateKbps)} kbps` : "";
    case "sampleRate": return t.sampleRate && t.sampleRate > 0 ? `${number.format(t.sampleRate / 1000)} kHz` : "";
    case "size": return t.fileSize > 0 ? (t.fileSize < 1073741824 ? `${number.format(t.fileSize / 1048576)} MB` : `${number.format(t.fileSize / 1073741824)} GB`) : "";
    case "kind": return t.codec ? `${t.codec.toUpperCase()} audio` : "";
    case "rating": return "★".repeat(Math.max(0, Math.min(5, Math.round(t.rating / 20))));
    case "time": {
      if (t.durationMs <= 0) return "";
      const total = Math.floor(t.durationMs / 1000), s = String(total % 60).padStart(2, "0"), m = Math.floor(total / 60) % 60, h = Math.floor(total / 3600);
      return h ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
    }
    default: return "";
  }
}
