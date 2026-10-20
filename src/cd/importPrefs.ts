import type { ImportFormat } from "../api/types";

/** iTunes kept Import Settings per computer; so does this, in localStorage. */
const KEY = "unsub.importSettings";

export interface ImportPrefs {
  format: ImportFormat;
  verify: boolean;
}

// Lossless by default: a CD is the one chance to get a perfect copy, and smaller copies for
// players can always be made from it later.
const DEFAULT_PREFS: ImportPrefs = { format: { codec: "flac" }, verify: true };

export function loadImportPrefs(): ImportPrefs {
  try {
    const stored = localStorage.getItem(KEY);
    return stored ? { ...DEFAULT_PREFS, ...(JSON.parse(stored) as Partial<ImportPrefs>) } : DEFAULT_PREFS;
  } catch {
    return DEFAULT_PREFS;
  }
}

export function saveImportPrefs(prefs: ImportPrefs) {
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    // Storage unavailable: the settings still apply until the app quits.
  }
}
