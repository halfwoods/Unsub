import type { DateUnit, LimitOrder, LimitUnit, SmartField, SmartOp, SmartRule, SmartValue } from "../api/types";

export type FieldType = "text" | "number" | "date" | "bool" | "playlist";
/** How a number is shown to the user versus stored by the library. */
export type NumberUnit = "plain" | "rating" | "time" | "size";

export interface FieldInfo {
  id: SmartField;
  label: string;
  type: FieldType;
  unit?: NumberUnit;
}

// iTunes 10's field menu, in its order.
export const FIELDS: FieldInfo[] = [
  { id: "album", label: "Album", type: "text" },
  { id: "albumArtist", label: "Album Artist", type: "text" },
  { id: "artist", label: "Artist", type: "text" },
  { id: "bitRate", label: "Bit Rate", type: "number" },
  { id: "checked", label: "Checked", type: "bool" },
  { id: "compilation", label: "Compilation", type: "bool" },
  { id: "composer", label: "Composer", type: "text" },
  { id: "dateAdded", label: "Date Added", type: "date" },
  { id: "discNumber", label: "Disc Number", type: "number" },
  { id: "genre", label: "Genre", type: "text" },
  { id: "kind", label: "Kind", type: "text" },
  { id: "lastPlayed", label: "Last Played", type: "date" },
  { id: "lastSkipped", label: "Last Skipped", type: "date" },
  { id: "name", label: "Name", type: "text" },
  { id: "playlist", label: "Playlist", type: "playlist" },
  { id: "plays", label: "Plays", type: "number" },
  { id: "rating", label: "Rating", type: "number", unit: "rating" },
  { id: "sampleRate", label: "Sample Rate", type: "number" },
  { id: "size", label: "Size", type: "number", unit: "size" },
  { id: "skips", label: "Skips", type: "number" },
  { id: "time", label: "Time", type: "number", unit: "time" },
  { id: "trackNumber", label: "Track Number", type: "number" },
  { id: "year", label: "Year", type: "number" },
];

export const fieldInfo = (id: SmartField) => FIELDS.find((f) => f.id === id) ?? FIELDS[0];

export const OPS: Record<FieldType, SmartOp[]> = {
  text: ["contains", "doesNotContain", "is", "isNot", "startsWith", "endsWith"],
  number: ["is", "isNot", "greaterThan", "lessThan", "inRange"],
  date: ["is", "before", "after", "inRange", "inTheLast", "notInTheLast"],
  bool: ["isTrue", "isFalse"],
  playlist: ["is", "isNot"],
};

export const OP_LABELS: Record<SmartOp, string> = {
  contains: "contains",
  doesNotContain: "does not contain",
  is: "is",
  isNot: "is not",
  startsWith: "starts with",
  endsWith: "ends with",
  greaterThan: "is greater than",
  lessThan: "is less than",
  inRange: "is in the range",
  before: "is before",
  after: "is after",
  inTheLast: "is in the last",
  notInTheLast: "is not in the last",
  isTrue: "is true",
  isFalse: "is false",
};

export const DATE_UNITS: { id: DateUnit; label: string }[] = [
  { id: "days", label: "days" },
  { id: "weeks", label: "weeks" },
  { id: "months", label: "months" },
];

export const LIMIT_UNITS: { id: LimitUnit; label: string }[] = [
  { id: "items", label: "items" },
  { id: "minutes", label: "minutes" },
  { id: "hours", label: "hours" },
  { id: "megabytes", label: "MB" },
  { id: "gigabytes", label: "GB" },
];

export const LIMIT_ORDERS: { id: LimitOrder; label: string }[] = [
  { id: "random", label: "random" },
  { id: "album", label: "album" },
  { id: "artist", label: "artist" },
  { id: "genre", label: "genre" },
  { id: "name", label: "name" },
  { id: "highestRating", label: "highest rating" },
  { id: "lowestRating", label: "lowest rating" },
  { id: "mostRecentlyPlayed", label: "most recently played" },
  { id: "leastRecentlyPlayed", label: "least recently played" },
  { id: "mostOftenPlayed", label: "most often played" },
  { id: "leastOftenPlayed", label: "least often played" },
  { id: "mostRecentlyAdded", label: "most recently added" },
  { id: "leastRecentlyAdded", label: "least recently added" },
];

const DAY = 86_400;
const today = () => Math.floor(Date.now() / 1000 / DAY) * DAY;

/** The value a fresh rule gets when its field or operator changes, as iTunes resets it. */
export function defaultValue(field: FieldInfo, op: SmartOp, playlistIds: number[]): SmartValue {
  if (field.type === "bool") return "empty";
  if (field.type === "text") return { text: "" };
  if (field.type === "playlist") return { playlist: playlistIds[0] ?? 0 };
  if (op === "inTheLast" || op === "notInTheLast") return { relative: { amount: 2, unit: "weeks" } };
  if (field.type === "date") return op === "inRange" ? { range: { from: today() - 30 * DAY, to: today() } } : { number: today() };
  if (op === "inRange") return { range: { from: 0, to: field.unit === "rating" ? 100 : 10 } };
  return { number: field.unit === "rating" ? 60 : 0 };
}

export function newCondition(playlistIds: number[]): SmartRule {
  const field = fieldInfo("artist");
  return { type: "condition", field: field.id, op: "contains", value: defaultValue(field, "contains", playlistIds) };
}

// Storage ↔ display conversions for the number editors.
export const toDisplay = (unit: NumberUnit | undefined, stored: number) => {
  if (unit === "rating") return stored / 20;
  if (unit === "time") return stored / 1000;
  if (unit === "size") return stored / 1_000_000;
  return stored;
};

export const fromDisplay = (unit: NumberUnit | undefined, shown: number) => {
  if (unit === "rating") return Math.round(shown) * 20;
  if (unit === "time") return Math.round(shown * 1000);
  if (unit === "size") return Math.round(shown * 1_000_000);
  return Math.round(shown);
};

export const unitSuffix = (unit: NumberUnit | undefined) =>
  ({ rating: "stars", time: "seconds", size: "MB", plain: "" })[unit ?? "plain"];

export const dateToInput = (unix: number) => new Date(unix * 1000).toISOString().slice(0, 10);
export const inputToDate = (value: string) => Math.floor(Date.parse(`${value}T00:00:00Z`) / 1000);
