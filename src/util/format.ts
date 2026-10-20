import type { CleanUpDone, EditReport } from "../api/types";

/** "3:07", or "1:02:07" past an hour, the way the LCD and Time column show it. */
export function formatClock(ms: number): string {
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** Status-bar totals use one unit with one decimal: "40.9 minutes", "3.2 hours", "1.5 days". */
export function formatTotalDuration(ms: number): string {
  const minutes = ms / 60000;
  if (minutes < 60) return `${minutes.toFixed(1)} minutes`;
  const hours = minutes / 60;
  if (hours < 24) return `${hours.toFixed(1)} hours`;
  return `${(hours / 24).toFixed(1)} days`;
}

export function formatBytes(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  if (mb < 1024) return `${mb.toFixed(1)} MB`;
  return `${(mb / 1024).toFixed(2)} GB`;
}

/** What to tell the user about the songs whose files could not take an edit, or null when every file did. */
export function dbOnlyNotice(report: EditReport): string | null {
  const files = report.dbOnly.length;
  if (files === 0) return null;
  return `${files === 1 ? "One file" : `${files.toLocaleString()} files`} couldn't be changed; the library has the new info`;
}

/** What a finished clean-up tells the user. */
export function cleanUpNotice({ report, stopped, error }: CleanUpDone): string {
  if (error) return `Couldn't finish cleaning up song info: ${error}`;
  const songs = pluralize(report.written.length + report.dbOnly.length, "song", "songs");
  const done = stopped ? `Stopped after updating ${songs}` : `Updated ${songs}`;
  const libraryOnly = dbOnlyNotice(report);
  return libraryOnly ? `${done}. ${libraryOnly}` : done;
}

export function pluralize(count: number, singular: string, plural: string): string {
  return `${count.toLocaleString()} ${count === 1 ? singular : plural}`;
}
