/** Playback choices are per Mac (its audio device, its speakers), so they live in localStorage. */
function load(key: string): boolean {
  try {
    return localStorage.getItem(key) === "true";
  } catch {
    return false;
  }
}

function save(key: string, enabled: boolean) {
  try {
    localStorage.setItem(key, String(enabled));
  } catch {
    // Storage unavailable: the setting still applies until the app quits.
  }
}

export const loadBitPerfect = () => load("antistreaming.bitPerfect");
export const saveBitPerfect = (enabled: boolean) => save("antistreaming.bitPerfect", enabled);
export const loadSoundCheck = () => load("antistreaming.soundCheck");
export const saveSoundCheck = (enabled: boolean) => save("antistreaming.soundCheck", enabled);
