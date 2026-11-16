/** Show original-file time with millisecond precision. */
export function trimTime(ms: number): string {
  const minutes = Math.floor(ms / 60000);
  const seconds = (ms % 60000) / 1000;
  return `${minutes}:${seconds.toFixed(3).padStart(6, "0")}`;
}

/** Accept seconds, minutes:seconds, or hours:minutes:seconds. */
export function parseTrimTime(value: string): number | null {
  const parts = value.trim().split(":");
  if (parts.length > 3 || parts.some((part) => !/^\d+(?:\.\d{1,3})?$/.test(part))) return null;
  if (parts.slice(0, -1).some((part) => part.includes("."))) return null;
  if (parts.length > 1 && Number(parts[parts.length - 1]) >= 60) return null;
  if (parts.length === 3 && Number(parts[1]) >= 60) return null;
  const seconds = parts.reduce((sum, part) => sum * 60 + Number(part), 0);
  const ms = Math.round(seconds * 1000);
  return Number.isSafeInteger(ms) ? ms : null;
}
