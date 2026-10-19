/** Bit-perfect is a per-Mac choice about this machine's audio device, so it lives in localStorage. */
const KEY = "antistreaming.bitPerfect";

export function loadBitPerfect(): boolean {
  try {
    return localStorage.getItem(KEY) === "true";
  } catch {
    return false;
  }
}

export function saveBitPerfect(enabled: boolean) {
  try {
    localStorage.setItem(KEY, String(enabled));
  } catch {
    // Storage unavailable: the setting still applies until the app quits.
  }
}
