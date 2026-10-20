/** Playback choices are per Mac (its audio device, its speakers), so they live in localStorage. */
function load(key: string): boolean {
  try {
    return localStorage.getItem(key) === "true";
  } catch {
    return false;
  }
}

function save(key: string, value: unknown) {
  try {
    localStorage.setItem(key, typeof value === "string" ? value : JSON.stringify(value));
  } catch {
    // Storage unavailable: the setting still applies until the app quits.
  }
}

function loadJson<T>(key: string, fallback: T): T {
  try {
    const stored = localStorage.getItem(key);
    return stored ? { ...fallback, ...(JSON.parse(stored) as Partial<T>) } : fallback;
  } catch {
    return fallback;
  }
}

export const loadBitPerfect = () => load("antistreaming.bitPerfect");
export const saveBitPerfect = (enabled: boolean) => save("antistreaming.bitPerfect", enabled);
export const loadSoundCheck = () => load("antistreaming.soundCheck");
export const saveSoundCheck = (enabled: boolean) => save("antistreaming.soundCheck", enabled);

export interface EqualizerPrefs {
  on: boolean;
  /** The preset the sliders came from; null once they're moved by hand ("Manual"). */
  preset: string | null;
  preampDb: number;
  bands: number[];
  preventClipping: boolean;
}

const FLAT: EqualizerPrefs = { on: false, preset: "Flat", preampDb: 0, bands: Array(10).fill(0), preventClipping: true };
export const loadEqualizer = () => loadJson("antistreaming.equalizer", FLAT);
export const saveEqualizer = (prefs: EqualizerPrefs) => save("antistreaming.equalizer", prefs);

/** Crossfade Songs' length in seconds, or null when it's off. */
export const loadCrossfade = () => loadJson<{ seconds: number | null }>("antistreaming.crossfade", { seconds: null }).seconds;
export const saveCrossfade = (seconds: number | null) => save("antistreaming.crossfade", { seconds });
